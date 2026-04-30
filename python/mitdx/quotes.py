import logging
from typing import List, Optional, Tuple, Union
from mitdx._core import TdxClient, ping_servers
from mitdx.utils import to_df
from mitdx.consts import HQ_HOSTS

logger = logging.getLogger(__name__)


def _market_code(symbol: str) -> int:
    """Infer TDX market code from symbol string.
    Shanghai (market=1): starts with 5, 6, 9, or 7
    Shenzhen (market=0): everything else (0, 1, 2, 3)
    """
    s = str(symbol).lower().removeprefix("sh").removeprefix("sz")
    if s and s[0] in ("5", "6", "9", "7"):
        return 1
    return 0


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

            for ip, port, latency in fastest_servers:
                if latency < 9999:  # Not completely failed
                    if self.connect(ip, port):
                        return

            raise ConnectionError("Failed to connect to any TDX HQ server. All pings failed.")

    # ---------------------------------------------------------------
    #  bars  —  K线数据 (已有)
    # ---------------------------------------------------------------
    def bars(self, symbol: str = '600036', frequency: int = 9, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch K-line bars.
        Supports standard parameters: frequency (0=5min, ..., 9=day)
        """
        self._ensure_connected()
        market = _market_code(symbol)
        try:
            res = self.client.get_security_bars(category=frequency, market=market, code=symbol, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch bars: {e}")
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
    #  index  —  指数数据 (bars 别名)
    # ---------------------------------------------------------------
    def index(self, symbol: str = '000001', frequency: int = 9, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch index K-line data. Alias for bars() that auto-detects index market code.
        """
        return self.bars(symbol=symbol, frequency=frequency, start=start, count=count, backend=backend, **kwargs)

    # ---------------------------------------------------------------
    #  xdxr  —  除权除息
    # ---------------------------------------------------------------
    def xdxr(self, symbol: str = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch XDXR (ex-dividend / ex-rights) information for a stock.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        try:
            res = self.client.get_xdxr_info(market=market, code=symbol)
            return to_df(res, backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch xdxr: {e}")
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
        try:
            res = self.client.get_transaction_data(market=market, code=symbol, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch transactions: {e}")
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  quotes  —  实时行情快照 (五档盘口)
    # ---------------------------------------------------------------
    def quotes(self, symbols: Union[str, List[str], List[Tuple[int, str]]] = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch real-time quote snapshots (5-level order book).
        symbols can be:
          - a single stock code string: '600036'
          - a list of stock code strings: ['600036', '000001']
          - a list of (market, code) tuples: [(1, '600036'), (0, '000001')]
        """
        self._ensure_connected()

        # Normalize to list of (market, code) tuples
        if isinstance(symbols, str):
            stock_list = [(_market_code(symbols), symbols)]
        elif isinstance(symbols, list) and len(symbols) > 0:
            if isinstance(symbols[0], str):
                stock_list = [(_market_code(s), s) for s in symbols]
            else:
                stock_list = [(int(m), str(c)) for m, c in symbols]
        else:
            stock_list = []

        try:
            res = self.client.get_security_quotes(stock_list=stock_list)
            return to_df(res, backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch quotes: {e}")
            self.disconnect()
            raise

    # ---------------------------------------------------------------
    #  finance  —  财务数据 (network API)
    # ---------------------------------------------------------------
    def finance(self, symbol: str = '600036', backend: str = 'pandas', **kwargs):
        """
        Fetch financial summary for a stock (流通股本, 总资产, 净利润 etc).
        Returns a single-row DataFrame with 34 financial fields.
        """
        self._ensure_connected()
        market = _market_code(symbol)
        try:
            res = self.client.get_finance_info(market=market, code=symbol)
            return to_df([res], backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch finance: {e}")
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
        try:
            categories = self.client.get_company_info_category(market=market, code=symbol)
            result = {"category": categories, "content": ""}
            # Fetch all content sections
            parts = []
            for cat in categories:
                fname = cat.get("filename", "")
                start = cat.get("start", 0)
                length = cat.get("length", 0)
                if fname and length > 0:
                    text = self.client.get_company_info_content(
                        market=market, code=symbol, filename=fname, start=start, length=length
                    )
                    parts.append(text)
            result["content"] = "\n".join(parts)
            return result
        except Exception as e:
            logger.error(f"Failed to fetch F10: {e}")
            self.disconnect()
            raise

    def __enter__(self):
        self._ensure_connected()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.disconnect()

