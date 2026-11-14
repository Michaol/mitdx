from __future__ import annotations

import datetime
import logging
from typing import List, Optional, Tuple
from mitdx._core import TdxClient, ping_servers
from mitdx.utils import to_df
from mitdx.consts import (
    HQ_HOSTS,
    MARKET_SH,
    MARKET_SZ,
    MAX_BARS_PER_REQUEST,
    MAX_K_BATCHES,
    QUOTES_BATCH_SIZE,
    SECURITY_LIST_BATCH_SIZE,
)

logger = logging.getLogger(__name__)

# Valid TDX market codes for security listing (0=Shenzhen, 1=Shanghai).
# Note: market=2 (北交所) exists in mootdx but TDX protocol does not support
# GetSecurityList/GetSecurityCount for it.
_VALID_SECURITY_MARKETS = (MARKET_SZ, MARKET_SH)


def _normalize_code(symbol: str) -> str:
    """Normalize a security code before sending it to the TDX server.

    Lowercases the symbol and strips ONE leading market prefix ('sh'/'sz'),
    e.g. 'SH600036' -> '600036'. The Rust core truncates codes to 6 bytes,
    so a raw prefixed code ('sh600036' -> b'sh6000') would silently query
    the wrong security. Market inference lives in _market_code() and
    _index_market_code().
    """
    s = str(symbol).strip().lower()
    for prefix in ("sh", "sz"):
        if s.startswith(prefix):
            return s.removeprefix(prefix)
    return s


def _market_code(symbol: str) -> int:
    """Infer TDX market code from symbol string.
    Shanghai (market=1): starts with 5, 6, 9, or 7
    Shenzhen (market=0): everything else (0, 1, 2, 3)
    Accepts prefixed symbols ('sh600036', 'SZ000001').
    """
    s = _normalize_code(symbol)
    if s.startswith(("5", "6", "9", "7")):
        return MARKET_SH
    return MARKET_SZ


def _index_market_code(symbol: str) -> int:
    """Infer TDX market code for index/指数 symbols.
    Shanghai indices (market=1): 000xxx (上证指数), 88xxxx, 99xxxx
    Shenzhen indices (market=0): 399xxx (深证指数)
    Accepts prefixed symbols ('sh000001', 'SZ399001').
    """
    s = _normalize_code(symbol)
    if s.startswith("000") or s.startswith(("88", "99")):
        return MARKET_SH
    return MARKET_SZ


def _validate_market(market: int) -> None:
    """Validate that market code is supported for security listing."""
    if market not in _VALID_SECURITY_MARKETS:
        raise ValueError(
            f"market must be {MARKET_SZ} (Shenzhen) or {MARKET_SH} (Shanghai), "
            f"got {market!r}"
        )


def _parse_date(s):
    """Parse 'YYYY-MM-DD' or 'YYYYMMDD' into a date; None passes through.

    Raises ValueError on malformed input (callers validate before connecting).
    """
    if s is None:
        return None
    s = str(s).replace('-', '')
    return datetime.date(int(s[:4]), int(s[4:6]), int(s[6:8]))


def _bar_date(bar):
    """Date portion of a bar's datetime, or None if unparseable."""
    dt_str = bar.get("datetime", "")
    if len(dt_str) < 10:
        return None
    try:
        return _parse_date(dt_str[:10])
    except ValueError:
        return None


def _fetch_k_batches(client, frequency: int, market: int, code: str, begin_dt) -> List[dict]:
    """Paginate backwards from the most recent bar in MAX_BARS_PER_REQUEST batches.

    Stops on an empty batch, when a batch's earliest bar predates begin_dt,
    or after MAX_K_BATCHES batches. The cap also keeps every `start` offset
    within the protocol's u16 range (see consts.MAX_K_BATCHES).
    """
    bars: List[dict] = []
    for batch_idx in range(MAX_K_BATCHES):
        batch = client.get_security_bars(
            category=frequency, market=market, code=code,
            start=batch_idx * MAX_BARS_PER_REQUEST, count=MAX_BARS_PER_REQUEST,
        )
        if not batch:
            break
        bars.extend(batch)
        # Batches arrive oldest-first; once a batch reaches back before
        # begin, older batches cannot contain wanted bars.
        if begin_dt is not None:
            earliest = _bar_date(batch[0])
            if earliest is not None and earliest < begin_dt:
                break
    return bars


def _filter_bars_by_date(bars: List[dict], begin_dt, end_dt) -> List[dict]:
    """Keep bars within [begin_dt, end_dt] inclusive; unparseable dates pass."""
    if begin_dt is None and end_dt is None:
        return bars
    filtered = []
    for bar in bars:
        bar_dt = _bar_date(bar)
        if bar_dt is not None:
            if begin_dt is not None and bar_dt < begin_dt:
                continue
            if end_dt is not None and bar_dt > end_dt:
                continue
        filtered.append(bar)
    return filtered


def _parse_block_data(data: bytes) -> List[dict]:
    """Parse TDX block file binary data into a list of dicts.

    Block file format (from pytdx BlockReader):
    - 384 bytes header (skip)
    - u16 num_blocks
    - For each block:
      - 9 bytes blockname (GBK, null-terminated)
      - u16 stock_count
      - u16 block_type
      - For each stock: 7 bytes code (UTF-8, null-terminated)
      - Padding to 2800 bytes total per block entry

    Returns list of dicts with keys: blockname, block_type, code_index, code.
    """
    import struct

    if len(data) < 386:
        return []

    pos = 384
    (num,) = struct.unpack("<H", data[pos:pos + 2])
    pos += 2

    results = []
    for _ in range(num):
        if pos + 9 > len(data):
            break

        blockname_raw = data[pos:pos + 9]
        pos += 9
        blockname = blockname_raw.decode("gbk", "ignore").rstrip("\x00")

        if pos + 4 > len(data):
            break
        stock_count, block_type = struct.unpack("<HH", data[pos:pos + 4])
        pos += 4
        block_stock_begin = pos

        for code_index in range(stock_count):
            if pos + 7 > len(data):
                break
            one_code = data[pos:pos + 7].decode("utf-8", "ignore").rstrip("\x00")
            pos += 7
            results.append({
                "blockname": blockname,
                "block_type": block_type,
                "code_index": code_index,
                "code": one_code,
            })

        # Each block entry is padded to 2800 bytes from the stock data start
        pos = block_stock_begin + 2800

    return results


class Quotes:
    """
    Python wrapper for TDX Quotes API using the fast Rust core.
    Provides backward compatibility with mootdx API where sensible.
    """

    def __init__(self, market: str = 'std'):
        self.market = market
        self.client = TdxClient()
        self.connected = False

    @staticmethod
    def factory(market='std', **kwargs):
        """Factory method to maintain backward compatibility with mootdx."""
        if market != 'std':
            raise NotImplementedError("Currently only 'std' market is supported in mitdx Phase 2.")
        return Quotes(market=market)

    def connect(self, ip: Optional[str] = None, port: int = 7709):
        if not ip:
            # simple fallback discovery for now
            ip = HQ_HOSTS[0][1]
            port = HQ_HOSTS[0][2]

        self.connected = self.client.connect(ip, port)
        return self.connected

    def disconnect(self):
        if self.connected:
            self.client.disconnect()
            self.connected = False

    def _ensure_connected(self):
        if not self.connected:
            # Ping all known servers using Rust multi-threading and connect to the fastest one
            ips = [(ip, port) for name, ip, port in HQ_HOSTS]
            fastest_servers = ping_servers(ips)

            # Try top 3 lowest-latency servers. A failed attempt (exception
            # or False) must not prevent trying the remaining candidates.
            for ip, port, latency in fastest_servers[:3]:
                if latency < 9999:  # Not completely failed
                    try:
                        if self.connect(ip, port):
                            return
                    except Exception:
                        logger.debug(
                            "Connect attempt to %s:%s failed", ip, port, exc_info=True
                        )
                        continue

            raise ConnectionError("Failed to connect to any TDX HQ server. All pings failed.")

    # ---------------------------------------------------------------
    #  _fetch_security_batch  —  内部：单市场分页拉取证券列表
    # ---------------------------------------------------------------
    def _fetch_security_batch(self, market: int) -> List[dict]:
        """Fetch the full security list for a single market via pagination.

        Shared by stocks() and stock_all() to avoid DRY duplication.
        Returns a list of dicts with keys: code, volunit, name, decimal_point, pre_close.
        """
        _validate_market(market)
        count = self.client.get_security_count(market=market)
        all_stocks: List[dict] = []
        total_batches = (count + SECURITY_LIST_BATCH_SIZE - 1) // SECURITY_LIST_BATCH_SIZE

        for batch_idx, start in enumerate(range(0, count, SECURITY_LIST_BATCH_SIZE), start=1):
            batch = self.client.get_security_list(market=market, start=start)
            all_stocks.extend(batch)
            logger.info(
                "Fetching market %d securities: batch %d/%d (fetched %d/%d)",
                market, batch_idx, total_batches, len(all_stocks), count,
            )

        return all_stocks

    # ---------------------------------------------------------------
    #  stock_count  —  市场证券数量
    # ---------------------------------------------------------------
    def stock_count(self, market: int = MARKET_SH) -> int:
        """
        Get the total number of securities for a given market.
        market: 0 = Shenzhen (深圳), 1 = Shanghai (上海)
        """
        self._ensure_connected()
        _validate_market(market)
        try:
            return self.client.get_security_count(market=market)
        except Exception:
            logger.exception("Failed to fetch security count for market=%d", market)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  stocks  —  市场证券列表
    # ---------------------------------------------------------------
    def stocks(self, market: int = MARKET_SH, backend: str = 'pandas', **kwargs):
        """
        Get the full list of securities for a given market.
        Returns DataFrame with columns: code, name, volunit, decimal_point, pre_close.
        market: 0 = Shenzhen (深圳), 1 = Shanghai (上海)
        """
        self._ensure_connected()
        # Validate user input before the network try-block so an invalid
        # market raises ValueError without tearing down the connection.
        _validate_market(market)
        try:
            all_stocks = self._fetch_security_batch(market)
            return to_df(all_stocks, backend=backend)
        except Exception:
            logger.exception("Failed to fetch security list for market=%d", market)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  stock_all  —  全市场证券列表 (沪深)
    # ---------------------------------------------------------------
    def stock_all(self, backend: str = 'pandas', **kwargs):
        """
        Get the full list of securities from both Shanghai and Shenzhen markets.
        Returns DataFrame with columns: market, code, name, volunit, decimal_point, pre_close.
        """
        self._ensure_connected()
        try:
            all_stocks: List[dict] = []
            for market in _VALID_SECURITY_MARKETS:
                batch = self._fetch_security_batch(market)
                for item in batch:
                    item['market'] = market
                all_stocks.extend(batch)
            return to_df(all_stocks, backend=backend)
        except Exception:
            logger.exception("Failed to fetch all securities")
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  bars  —  K线数据
    # ---------------------------------------------------------------
    def bars(self, symbol: str = '600036', frequency: int = 9, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch K-line bars.
        Supports standard parameters: frequency (0=5min, ..., 9=day)
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_security_bars(category=frequency, market=market, code=code, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch bars for symbol=%s", symbol)
            self.disconnect()  # Force reconnect next time
            raise

    # ---------------------------------------------------------------
    #  minute  —  分钟K线 (bars 别名)
    # ---------------------------------------------------------------
    def minute(self, symbol: str = '600036', frequency: int = 0, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch minute-level K-line data. Alias for bars() with minute-level frequencies.
        frequency: 0=5min, 1=15min, 2=30min, 3=1hour
        """
        return self.bars(symbol=symbol, frequency=frequency, start=start, count=count, backend=backend, **kwargs)

    # ---------------------------------------------------------------
    #  index_bars  —  指数K线 (含涨跌家数)
    # ---------------------------------------------------------------
    def index_bars(self, symbol: str = '000001', frequency: int = 9, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch index K-line bars. Same as bars() but includes up_count/down_count.
        Returns DataFrame with columns: datetime, open, high, low, close, vol, amount, up_count, down_count.
        Auto-detects index market: 000xxx/88xxxx/99xxxx -> SH, 399xxx -> SZ.
        """
        self._ensure_connected()
        market = _index_market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_index_bars(category=frequency, market=market, code=code, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch index bars for symbol=%s", symbol)
            self.disconnect()
            raise

    # alias for backward compatibility
    index = index_bars

    # ---------------------------------------------------------------
    #  xdxr  —  除权除息
    # ---------------------------------------------------------------
    def xdxr(self, symbol: str = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch XDXR (ex-dividend / ex-rights) information for a stock.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_xdxr_info(market=market, code=code)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch xdxr for symbol=%s", symbol)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  transactions  —  分笔成交
    # ---------------------------------------------------------------
    def transactions(self, symbol: str = '600036', start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch tick-level transaction data (分笔成交).
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_transaction_data(market=market, code=code, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch transactions for symbol=%s", symbol)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  quotes  —  实时行情快照 (五档盘口)
    # ---------------------------------------------------------------
    def quotes(self, symbols: str | List[str] | List[Tuple[int, str]] = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch real-time quote snapshots (5-level order book).
        symbols can be:
          - a single stock code string: '600036' (prefixed 'sh600036' also works)
          - a list of stock code strings: ['600036', '000001']
          - a list of (market, code) tuples: [(1, '600036'), (0, '000001')]
        Codes are normalized (lowercased, one leading sh/sz prefix stripped).
        The request is split into batches of at most QUOTES_BATCH_SIZE (80)
        symbols — the TDX server convention — and results are concatenated
        in input order. An empty list returns an empty DataFrame without
        contacting the server.
        """
        # Normalize to list of (market, code) tuples
        stock_list: List[Tuple[int, str]]
        if isinstance(symbols, str):
            stock_list = [(_market_code(symbols), _normalize_code(symbols))]
        elif isinstance(symbols, list) and len(symbols) > 0:
            if isinstance(symbols[0], str):
                stock_list = [(_market_code(s), _normalize_code(s)) for s in symbols]
            else:
                stock_list = [(int(m), _normalize_code(c)) for m, c in symbols]
        else:
            stock_list = []

        # Empty input: return immediately without pinging/connecting.
        if not stock_list:
            return to_df([], backend=backend)

        self._ensure_connected()

        try:
            res: List[dict] = []
            for i in range(0, len(stock_list), QUOTES_BATCH_SIZE):
                batch = stock_list[i:i + QUOTES_BATCH_SIZE]
                res.extend(self.client.get_security_quotes(stock_list=batch))
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch quotes for symbols=%s", symbols)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  finance  —  财务数据 (network API)
    # ---------------------------------------------------------------
    def finance(self, symbol: str = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch financial summary for a stock (流通股本, 总资产, 净利润 etc).
        Returns a single-row DataFrame with 35 financial fields.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_finance_info(market=market, code=code)
            return to_df([res], backend=backend)
        except Exception:
            logger.exception("Failed to fetch finance for symbol=%s", symbol)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  f10  —  F10 公司资料
    # ---------------------------------------------------------------
    def f10(self, symbol: str = '600036', **kwargs):
        """
        Fetch F10 company information.
        Returns a dict with 'category' (list of sections) and 'content' (full text).
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            categories = self.client.get_company_info_category(market=market, code=code)
            result = {"category": categories, "content": ""}
            # Fetch all content sections
            parts = []
            for cat in categories:
                fname = cat.get("filename", "")
                start = cat.get("start", 0)
                length = cat.get("length", 0)
                if fname and length > 0:
                    text = self.client.get_company_info_content(
                        market=market, code=code, filename=fname, start=start, length=length
                    )
                    parts.append(text)
            result["content"] = "\n".join(parts)
            return result
        except Exception:
            logger.exception("Failed to fetch F10 for symbol=%s", symbol)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  minute_data  —  当日分时数据
    # ---------------------------------------------------------------
    def minute_data(self, symbol: str = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch intraday minute-by-minute price/volume data for today.
        Returns DataFrame with columns: price, vol.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_minute_time_data(market=market, code=code)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch minute data for symbol=%s", symbol)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  history_minute_data  —  历史分时数据
    # ---------------------------------------------------------------
    def history_minute_data(self, symbol: str = '600036', date: int = 20260101, backend: str = 'pandas', **kwargs):
        """
        Fetch minute-by-minute price/volume data for a historical date.
        date: YYYYMMDD format integer, e.g. 20260520
        Returns DataFrame with columns: price, vol.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_history_minute_time_data(market=market, code=code, date=date)
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch history minute data for symbol=%s date=%d", symbol, date)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  history_transactions  —  历史分笔成交
    # ---------------------------------------------------------------
    def history_transactions(self, symbol: str = '600036', date: int = 20260101, start: int = 0, count: int = 30, backend: str = 'pandas', **kwargs):
        """
        Fetch tick-level transaction data for a historical date.
        date: YYYYMMDD format integer, e.g. 20260520
        Returns DataFrame with columns: time, price, vol, buyorsell.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)
        try:
            res = self.client.get_history_transaction_data(
                market=market, code=code, start=start, count=count, date=date
            )
            return to_df(res, backend=backend)
        except Exception:
            logger.exception("Failed to fetch history transactions for symbol=%s date=%d", symbol, date)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  block_meta  —  板块元数据
    # ---------------------------------------------------------------
    def block_meta(self, block_file: str = 'block_zs.dat') -> dict:
        """
        Get block file metadata (size, hash).
        block_file: one of 'block_zs.dat' (指数), 'block_gn.dat' (概念),
                    'block_fg.dat' (风格), 'block_hy.dat' (行业), 'block.dat' (通用)
        Returns dict with keys: size, hash_value.
        """
        self._ensure_connected()
        try:
            return self.client.get_block_info_meta(block_file=block_file)
        except Exception:
            logger.exception("Failed to fetch block meta for %s", block_file)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  block_info  —  板块详情 (成分股列表)
    # ---------------------------------------------------------------
    def block_info(self, block_file: str = 'block_zs.dat', backend: str = 'pandas', **kwargs):
        """
        Get block sector data (板块成分股).
        Downloads the block file from TDX server in chunks and parses it.
        Returns DataFrame with columns: blockname, block_type, code_index, code.
        """
        self._ensure_connected()
        try:
            meta = self.client.get_block_info_meta(block_file=block_file)
            if not meta:
                return to_df([], backend=backend)

            file_size = meta['size']
            one_chunk = 0x7530  # 30000 bytes
            file_content = bytearray()

            chunks = (file_size + one_chunk - 1) // one_chunk
            for i in range(chunks):
                offset = i * one_chunk
                logger.info("Downloading block %s: chunk %d/%d", block_file, i + 1, chunks)
                chunk = self.client.get_block_info(block_file=block_file, start=offset, size=file_size)
                file_content.extend(chunk)

            # Parse block file (same format as pytdx BlockReader)
            records = _parse_block_data(file_content)
            return to_df(records, backend=backend)
        except Exception:
            logger.exception("Failed to fetch block info for %s", block_file)
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  k  —  按日期范围查询K线
    # ---------------------------------------------------------------
    def k(self, symbol: str = '600036', begin: Optional[str] = None, end: Optional[str] = None,
          frequency: int = 9, backend: str = 'pandas', **kwargs):
        """
        Fetch K-line bars within a date range.

        begin/end: date strings like '2026-01-01' or '20260101' (inclusive).
        Invalid dates raise ValueError before any network activity.

        Bars are paginated backwards from the most recent, in batches of up
        to MAX_BARS_PER_REQUEST (800) bars, stopping when a batch is empty,
        when a batch's earliest bar predates begin, or after MAX_K_BATCHES
        (82) batches — a safety cap of 65,600 bars, bounded by the protocol's
        u16 `start` field (max 65535). If begin is None the cap bounds the
        reachable history; for minute-level frequencies 65,600 bars only
        cover a few months of trading, so older minute data may be
        unreachable.

        Returns DataFrame with the same columns as bars(), sorted by
        datetime ascending and filtered to [begin, end].
        """
        # Validate user-supplied dates BEFORE connecting / entering the
        # network try-block, so a bad date raises without disconnecting.
        begin_dt = _parse_date(begin)
        end_dt = _parse_date(end)

        self._ensure_connected()
        market = _market_code(symbol)
        code = _normalize_code(symbol)

        try:
            bars = _fetch_k_batches(self.client, frequency, market, code, begin_dt)
            bars = _filter_bars_by_date(bars, begin_dt, end_dt)
            # Batches were accumulated newest-first; restore chronological order.
            bars.sort(key=lambda b: b.get("datetime", ""))
            return to_df(bars, backend=backend)
        except Exception:
            logger.exception("Failed to fetch k data for symbol=%s", symbol)
            self.disconnect()
            raise

    def __enter__(self):
        self._ensure_connected()
        return self

    def __exit__(self, _exc_type, _exc_val, _exc_tb):
        self.disconnect()
