"""Offline regression tests for mitdx.quotes (G1-quotes).

Covers findings F2 (k pagination), F5 (symbol normalization), F6 (quotes
batching), F11 (connect fallback), F17 (index market routing), F21 (user
input errors must not disconnect) and F16 (helper unit matrices, block
parsing). Everything network-shaped is mocked; no sockets are opened.
"""

import datetime as dt
import struct
from unittest.mock import MagicMock, call, patch

import pytest

from mitdx import quotes as q
from mitdx.consts import (
    MARKET_SH,
    MARKET_SZ,
    MAX_BARS_PER_REQUEST,
    MAX_K_BATCHES,
    QUOTES_BATCH_SIZE,
)


# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------

@pytest.fixture
def quotes_obj():
    """Quotes instance with a fully mocked Rust client, already connected."""
    with patch.object(q, "TdxClient"):
        obj = q.Quotes()
    obj.client = MagicMock()
    obj.connected = True
    return obj


@pytest.fixture
def disconnected_obj():
    """Quotes instance with a mocked client, NOT connected."""
    with patch.object(q, "TdxClient"):
        obj = q.Quotes()
    obj.client = MagicMock()
    obj.connected = False
    return obj


def make_bars(n, end_date=dt.date(2026, 1, 10)):
    """n synthetic daily bars, oldest-first, on consecutive calendar days."""
    bars = []
    for i in range(n):
        d = end_date - dt.timedelta(days=n - 1 - i)
        bars.append({
            "datetime": f"{d.isoformat()} 15:00",
            "open": 10.0 + i,
            "high": 11.0 + i,
            "low": 9.0 + i,
            "close": 10.5 + i,
            "vol": 100.0,
            "amount": 1000.0,
        })
    return bars


def serve_bars(bars):
    """Mock get_security_bars: start is the offset from the newest bar,
    batches are returned oldest-first (TDX wire order)."""
    def _get(category, market, code, start, count):
        end = len(bars) - start
        if end <= 0:
            return []
        return bars[max(0, end - count):end]
    return _get


def bar_starts(mock):
    return [c.kwargs["start"] for c in mock.get_security_bars.call_args_list]


def build_block_file(blocks):
    """Synthetic TDX block file mirroring pytdx BlockReader's format.

    blocks: list of (name, block_type, codes). Layout: 384-byte header,
    u16 block count, then per block a 9s GBK name, u16 stock_count,
    u16 block_type, 7s codes, zero-padded to 2800 bytes from code start.
    """
    out = bytearray(b"\x00" * 384)
    out += struct.pack("<H", len(blocks))
    for name, btype, codes in blocks:
        out += name.encode("gbk").ljust(9, b"\x00")[:9]
        out += struct.pack("<HH", len(codes), btype)
        stock_begin = len(out)
        for c in codes:
            out += c.encode("utf-8").ljust(7, b"\x00")[:7]
        out += b"\x00" * (2800 - (len(out) - stock_begin))
    return bytes(out)


# ---------------------------------------------------------------------------
# Constants (F2 / F6)
# ---------------------------------------------------------------------------

class TestConstants:
    def test_protocol_constants(self):
        assert MAX_BARS_PER_REQUEST == 800
        assert MAX_K_BATCHES == 82
        # u16 wire limit: the last batch start must fit in u16 (max 65535).
        assert (MAX_K_BATCHES - 1) * MAX_BARS_PER_REQUEST <= 65535
        assert QUOTES_BATCH_SIZE == 80


# ---------------------------------------------------------------------------
# _normalize_code (F5)
# ---------------------------------------------------------------------------

class TestNormalizeCode:
    @pytest.mark.parametrize("symbol,expected", [
        ("600036", "600036"),
        ("000001", "000001"),
        ("sh600036", "600036"),
        ("SH600036", "600036"),
        ("Sh600036", "600036"),
        ("sz000001", "000001"),
        ("SZ000001", "000001"),
        ("sZ399001", "399001"),
        # exactly ONE leading prefix is stripped
        ("shsh600036", "sh600036"),
        ("szsh600036", "sh600036"),
        ("shsz600036", "sz600036"),
        # whitespace tolerated
        ("  sh600036  ", "600036"),
        # non-string input coerced
        (600036, "600036"),
        # degenerate inputs
        ("sh", ""),
        ("", ""),
        ("bj430047", "bj430047"),  # unknown prefixes untouched
    ])
    def test_normalize(self, symbol, expected):
        assert q._normalize_code(symbol) == expected


# ---------------------------------------------------------------------------
# _market_code (F5 / F16)
# ---------------------------------------------------------------------------

class TestMarketCode:
    @pytest.mark.parametrize("symbol,expected", [
        ("600036", MARKET_SH),
        ("510050", MARKET_SH),
        ("900901", MARKET_SH),
        ("730901", MARKET_SH),
        ("000001", MARKET_SZ),
        ("300750", MARKET_SZ),
        ("159915", MARKET_SZ),
        ("200001", MARKET_SZ),
        ("sh600036", MARKET_SH),
        ("SH600036", MARKET_SH),
        ("SZ000001", MARKET_SZ),
        ("sz399001", MARKET_SZ),
        ("sh510050", MARKET_SH),
        ("", MARKET_SZ),
        ("sh", MARKET_SZ),
    ])
    def test_market(self, symbol, expected):
        assert q._market_code(symbol) == expected


# ---------------------------------------------------------------------------
# _index_market_code (F17)
# ---------------------------------------------------------------------------

class TestIndexMarketCode:
    @pytest.mark.parametrize("symbol,expected", [
        ("000001", MARKET_SH),   # 上证指数
        ("000300", MARKET_SH),
        ("880001", MARKET_SH),
        ("888001", MARKET_SH),   # '88' covers former '888' check
        ("999999", MARKET_SH),   # '99' covers former '999' check
        ("990001", MARKET_SH),
        ("399001", MARKET_SZ),   # 深证指数
        ("399006", MARKET_SZ),
        # F17: prefixed symbols must route correctly (used to misroute to SZ)
        ("sh000001", MARKET_SH),
        ("SH000001", MARKET_SH),
        ("sz399001", MARKET_SZ),
        ("SZ399001", MARKET_SZ),
        ("sh880001", MARKET_SH),
        ("  sh000001  ", MARKET_SH),
        ("", MARKET_SZ),
    ])
    def test_index_market(self, symbol, expected):
        assert q._index_market_code(symbol) == expected


# ---------------------------------------------------------------------------
# F5: every client call site receives a normalized code
# ---------------------------------------------------------------------------

class TestSymbolNormalizationAtCallSites:
    @pytest.mark.parametrize("method,client_method,extra", [
        ("bars", "get_security_bars", {}),
        # index_bars uses the INDEX router (000/88/99 -> SH); covered by
        # test_index_bars_prefixed_* below with real index symbols.
        ("xdxr", "get_xdxr_info", {}),
        ("transactions", "get_transaction_data", {}),
        ("minute_data", "get_minute_time_data", {}),
        ("history_minute_data", "get_history_minute_time_data", {"date": 20260105}),
        ("history_transactions", "get_history_transaction_data", {"date": 20260105}),
    ])
    def test_sh_prefix_stripped(self, quotes_obj, method, client_method, extra):
        getattr(quotes_obj.client, client_method).return_value = []
        getattr(quotes_obj, method)(symbol="SH600036", **extra)
        kwargs = getattr(quotes_obj.client, client_method).call_args.kwargs
        assert kwargs["code"] == "600036"
        assert kwargs["market"] == MARKET_SH

    def test_finance_normalized(self, quotes_obj):
        quotes_obj.client.get_finance_info.return_value = {"liutongguben": 1.0}
        df = quotes_obj.finance(symbol="sh600036")
        kwargs = quotes_obj.client.get_finance_info.call_args.kwargs
        assert kwargs["code"] == "600036"
        assert kwargs["market"] == MARKET_SH
        assert len(df) == 1

    def test_sz_prefix_stripped(self, quotes_obj):
        quotes_obj.client.get_security_bars.return_value = []
        quotes_obj.bars(symbol="sz000001")
        kwargs = quotes_obj.client.get_security_bars.call_args.kwargs
        assert kwargs["code"] == "000001"
        assert kwargs["market"] == MARKET_SZ

    def test_index_bars_prefixed_sh_index(self, quotes_obj):
        quotes_obj.client.get_index_bars.return_value = []
        quotes_obj.index_bars(symbol="sh000001")
        kwargs = quotes_obj.client.get_index_bars.call_args.kwargs
        assert kwargs["code"] == "000001"
        assert kwargs["market"] == MARKET_SH

    def test_index_bars_prefixed_sz_index(self, quotes_obj):
        quotes_obj.client.get_index_bars.return_value = []
        quotes_obj.index_bars(symbol="SZ399001")
        kwargs = quotes_obj.client.get_index_bars.call_args.kwargs
        assert kwargs["code"] == "399001"
        assert kwargs["market"] == MARKET_SZ

    def test_f10_both_calls_normalized(self, quotes_obj):
        quotes_obj.client.get_company_info_category.return_value = [
            {"filename": "600036.txt", "start": 0, "length": 4},
        ]
        quotes_obj.client.get_company_info_content.return_value = "text"
        result = quotes_obj.f10(symbol="sh600036")
        cat_kwargs = quotes_obj.client.get_company_info_category.call_args.kwargs
        content_kwargs = quotes_obj.client.get_company_info_content.call_args.kwargs
        assert cat_kwargs["code"] == "600036"
        assert cat_kwargs["market"] == MARKET_SH
        assert content_kwargs["code"] == "600036"
        assert content_kwargs["market"] == MARKET_SH
        assert result["content"] == "text"

    def test_minute_alias_normalized(self, quotes_obj):
        quotes_obj.client.get_security_bars.return_value = []
        quotes_obj.minute(symbol="SH600036")
        kwargs = quotes_obj.client.get_security_bars.call_args.kwargs
        assert kwargs["code"] == "600036"
        assert kwargs["market"] == MARKET_SH

    def test_k_normalized(self, quotes_obj):
        quotes_obj.client.get_security_bars.side_effect = serve_bars(make_bars(10))
        quotes_obj.k(symbol="SH600036")
        kwargs = quotes_obj.client.get_security_bars.call_args.kwargs
        assert kwargs["code"] == "600036"
        assert kwargs["market"] == MARKET_SH


# ---------------------------------------------------------------------------
# F6: quotes() batching
# ---------------------------------------------------------------------------

class TestQuotesBatching:
    @staticmethod
    def _capture(quotes_obj):
        calls = []

        def _get(stock_list):
            calls.append(list(stock_list))
            return [{"code": c, "price": 1.0} for _, c in stock_list]

        quotes_obj.client.get_security_quotes.side_effect = _get
        return calls

    def test_200_symbols_split_80_80_40(self, quotes_obj):
        symbols = [str(600000 + i) for i in range(200)]
        calls = self._capture(quotes_obj)
        df = quotes_obj.quotes(symbols)
        assert [len(c) for c in calls] == [80, 80, 40]
        assert all(len(c) <= QUOTES_BATCH_SIZE for c in calls)
        assert len(df) == 200
        # input order preserved across batches
        assert df["code"].tolist() == symbols
        assert all(m == MARKET_SH for batch in calls for m, _ in batch)

    def test_9400_symbols_never_exceed_batch_size(self, quotes_obj):
        # F6 regression: a single request with >=9361 symbols overflowed
        # the Rust u16 packet length. Every batch must stay <= 80.
        symbols = [str(600000 + i) for i in range(9400)]
        calls = self._capture(quotes_obj)
        df = quotes_obj.quotes(symbols)
        assert len(calls) == 118
        assert all(len(c) <= QUOTES_BATCH_SIZE for c in calls)
        assert sum(len(c) for c in calls) == 9400
        assert len(df) == 9400

    @pytest.mark.parametrize("n,expected_sizes", [
        (1, [1]),
        (80, [80]),
        (81, [80, 1]),
        (160, [80, 80]),
    ])
    def test_batch_boundaries(self, quotes_obj, n, expected_sizes):
        calls = self._capture(quotes_obj)
        quotes_obj.quotes([str(600000 + i) for i in range(n)])
        assert [len(c) for c in calls] == expected_sizes

    def test_single_prefixed_string(self, quotes_obj):
        calls = self._capture(quotes_obj)
        quotes_obj.quotes("sh600036")
        assert calls == [[(MARKET_SH, "600036")]]

    def test_prefixed_string_list(self, quotes_obj):
        calls = self._capture(quotes_obj)
        quotes_obj.quotes(["SH600036", "sz000001"])
        assert calls == [[(MARKET_SH, "600036"), (MARKET_SZ, "000001")]]

    def test_tuple_list_keeps_explicit_market(self, quotes_obj):
        calls = self._capture(quotes_obj)
        quotes_obj.quotes([(1, "sh600036"), (0, "SZ000001")])
        assert calls == [[(1, "600036"), (0, "000001")]]

    def test_empty_list_no_network(self, quotes_obj):
        df = quotes_obj.quotes([])
        quotes_obj.client.get_security_quotes.assert_not_called()
        assert len(df) == 0

    def test_empty_list_disconnected_no_ping(self, disconnected_obj, monkeypatch):
        # Final-review fix: quotes([]) must not ping/connect when disconnected.
        ping = MagicMock(side_effect=AssertionError("ping_servers must not be called"))
        monkeypatch.setattr(q, "ping_servers", ping)
        df = disconnected_obj.quotes([])
        assert len(df) == 0
        disconnected_obj.client.connect.assert_not_called()
        ping.assert_not_called()

    def test_network_error_disconnects(self, quotes_obj):
        quotes_obj.client.get_security_quotes.side_effect = OSError("boom")
        with pytest.raises(OSError):
            quotes_obj.quotes(["600036"])
        quotes_obj.client.disconnect.assert_called_once()
        assert quotes_obj.connected is False


# ---------------------------------------------------------------------------
# F2 / F21: k() pagination, filtering, stop conditions, date validation
# ---------------------------------------------------------------------------

class TestKPagination:
    def test_paginates_backwards_until_begin(self, quotes_obj):
        bars = make_bars(2000)
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        begin = bars[100]["datetime"][:10]
        df = quotes_obj.k("600036", begin=begin)

        # batches at offsets 0, 800, 1600; third batch reaches before begin
        assert bar_starts(quotes_obj.client) == [0, 800, 1600]
        for c in quotes_obj.client.get_security_bars.call_args_list:
            assert c.kwargs["count"] == MAX_BARS_PER_REQUEST
            assert c.kwargs["category"] == 9
        assert len(df) == 1900
        assert df["datetime"].iloc[0] == bars[100]["datetime"]
        assert df["datetime"].iloc[-1] == bars[-1]["datetime"]
        # chronological order restored across batches
        assert df["datetime"].tolist() == sorted(df["datetime"].tolist())

    def test_stops_on_empty_batch(self, quotes_obj):
        bars = make_bars(500)
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k("600036")
        assert bar_starts(quotes_obj.client) == [0, 800]
        assert len(df) == 500

    def test_end_only_paginates_to_exhaustion(self, quotes_obj):
        bars = make_bars(1700)
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        end = bars[1000]["datetime"][:10]
        df = quotes_obj.k("600036", end=end)
        # no begin -> no early stop; fetch until the server runs out
        assert bar_starts(quotes_obj.client) == [0, 800, 1600, 2400]
        assert len(df) == 1001
        assert df["datetime"].iloc[-1] == bars[1000]["datetime"]

    def test_safety_cap_limits_batches(self, quotes_obj, monkeypatch):
        monkeypatch.setattr(q, "MAX_K_BATCHES", 3)
        # server with "infinite" history: always a full batch
        quotes_obj.client.get_security_bars.side_effect = (
            lambda **kw: make_bars(MAX_BARS_PER_REQUEST)
        )
        df = quotes_obj.k("600036")
        assert quotes_obj.client.get_security_bars.call_count == 3
        assert len(df) == 3 * MAX_BARS_PER_REQUEST

    def test_real_cap_keeps_start_within_u16(self, quotes_obj):
        # Final-review regression: start is a u16 wire field. With the real
        # MAX_K_BATCHES, infinite history must stop before start > 65535.
        quotes_obj.client.get_security_bars.side_effect = (
            lambda **kw: make_bars(MAX_BARS_PER_REQUEST)
        )
        quotes_obj.k("600036")
        starts = bar_starts(quotes_obj.client)
        assert len(starts) == MAX_K_BATCHES
        assert max(starts) <= 65535
        assert starts == [i * MAX_BARS_PER_REQUEST for i in range(MAX_K_BATCHES)]

    def test_cap_bounds_unreachable_begin(self, quotes_obj, monkeypatch):
        # minute-level history: begin far older than the cap can reach
        monkeypatch.setattr(q, "MAX_K_BATCHES", 2)
        minute_bars = [
            {"datetime": "2026-01-05 09:35", "close": 1.0, "vol": 10.0}
            for _ in range(MAX_BARS_PER_REQUEST)
        ]
        quotes_obj.client.get_security_bars.side_effect = lambda **kw: minute_bars
        df = quotes_obj.k("600036", begin="2020-01-01", frequency=0)
        assert quotes_obj.client.get_security_bars.call_count == 2
        assert len(df) == 2 * MAX_BARS_PER_REQUEST

    def test_date_filter_inclusive_both_ends(self, quotes_obj):
        bars = make_bars(100)
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k(
            "600036",
            begin=bars[10]["datetime"][:10],
            end=bars[19]["datetime"][:10],
        )
        assert len(df) == 10
        assert df["datetime"].iloc[0] == bars[10]["datetime"]
        assert df["datetime"].iloc[-1] == bars[19]["datetime"]

    def test_compact_and_dashed_dates(self, quotes_obj):
        bars = make_bars(30, end_date=dt.date(2026, 1, 30))
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k("600036", begin="20260110", end="2026-01-15")
        assert len(df) == 6
        assert df["datetime"].iloc[0].startswith("2026-01-10")
        assert df["datetime"].iloc[-1].startswith("2026-01-15")

    def test_minute_frequency_filters_by_date_part(self, quotes_obj):
        bars = []
        for day in ("2026-01-05", "2026-01-06"):
            for hm in ("09:35", "09:40", "09:45", "09:50"):
                bars.append({"datetime": f"{day} {hm}", "close": 1.0, "vol": 10.0})
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k("600036", begin="2026-01-05", end="2026-01-05", frequency=0)
        assert len(df) == 4
        assert all(s.startswith("2026-01-05") for s in df["datetime"])
        assert quotes_obj.client.get_security_bars.call_args.kwargs["category"] == 0

    def test_unparseable_bar_datetime_kept(self, quotes_obj):
        bars = [
            {"datetime": "", "close": 1.0},
            {"datetime": "2026-01-05 15:00", "close": 2.0},
        ]
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k("600036", begin="2026-01-01", end="2026-01-31")
        assert len(df) == 2

    def test_network_error_disconnects(self, quotes_obj):
        quotes_obj.client.get_security_bars.side_effect = OSError("boom")
        with pytest.raises(OSError):
            quotes_obj.k("600036")
        quotes_obj.client.disconnect.assert_called_once()
        assert quotes_obj.connected is False

    def test_polars_backend(self, quotes_obj):
        import polars as pl
        bars = make_bars(50)
        quotes_obj.client.get_security_bars.side_effect = serve_bars(bars)
        df = quotes_obj.k("600036", backend="polars")
        assert isinstance(df, pl.DataFrame)
        assert df.height == 50


class TestKDateValidation:
    """F21: invalid user dates must raise BEFORE any network activity and
    must NOT tear down the connection."""

    @pytest.mark.parametrize("bad", [
        "not-a-date",
        "2026-13-40",
        "2026-02-30",
        "2026010",
        "abcdefghij",
    ])
    @pytest.mark.parametrize("kwarg", ["begin", "end"])
    def test_invalid_date_no_network_no_disconnect(self, disconnected_obj, bad, kwarg):
        with patch.object(q, "ping_servers", side_effect=AssertionError("must not ping")) as ping:
            with pytest.raises(ValueError):
                disconnected_obj.k("600036", **{kwarg: bad})
        ping.assert_not_called()
        disconnected_obj.client.get_security_bars.assert_not_called()
        disconnected_obj.client.disconnect.assert_not_called()
        assert disconnected_obj.connected is False


# ---------------------------------------------------------------------------
# F11: _ensure_connected fallback
# ---------------------------------------------------------------------------

class TestEnsureConnected:
    SERVERS = [
        ("1.1.1.1", 7709, 5),
        ("2.2.2.2", 7709, 10),
        ("3.3.3.3", 7709, 20),
        ("4.4.4.4", 7709, 30),
    ]

    def test_exception_tries_next_server(self, disconnected_obj):
        with patch.object(q, "ping_servers", return_value=self.SERVERS):
            disconnected_obj.client.connect.side_effect = [OSError("refused"), True]
            disconnected_obj._ensure_connected()
        assert disconnected_obj.connected is True
        assert disconnected_obj.client.connect.call_args_list == [
            call("1.1.1.1", 7709),
            call("2.2.2.2", 7709),
        ]

    def test_all_raise_connection_error(self, disconnected_obj):
        with patch.object(q, "ping_servers", return_value=self.SERVERS):
            disconnected_obj.client.connect.side_effect = OSError("refused")
            with pytest.raises(ConnectionError):
                disconnected_obj._ensure_connected()
        # only the top 3 candidates are tried
        assert disconnected_obj.client.connect.call_count == 3
        assert disconnected_obj.connected is False

    def test_false_returns_continue(self, disconnected_obj):
        with patch.object(q, "ping_servers", return_value=self.SERVERS):
            disconnected_obj.client.connect.side_effect = [False, False, True]
            disconnected_obj._ensure_connected()
        assert disconnected_obj.connected is True
        assert disconnected_obj.client.connect.call_count == 3

    def test_failed_pings_skipped(self, disconnected_obj):
        servers = [("1.1.1.1", 7709, 9999), ("2.2.2.2", 7709, 10)]
        with patch.object(q, "ping_servers", return_value=servers):
            disconnected_obj.client.connect.return_value = True
            disconnected_obj._ensure_connected()
        disconnected_obj.client.connect.assert_called_once_with("2.2.2.2", 7709)

    def test_already_connected_skips_ping(self, quotes_obj):
        with patch.object(q, "ping_servers", side_effect=AssertionError("must not ping")) as ping:
            quotes_obj._ensure_connected()
        ping.assert_not_called()
        quotes_obj.client.connect.assert_not_called()


# ---------------------------------------------------------------------------
# F21: stocks() market validation must not disconnect
# ---------------------------------------------------------------------------

class TestUserInputErrors:
    def test_stocks_invalid_market_no_disconnect(self, quotes_obj):
        with pytest.raises(ValueError):
            quotes_obj.stocks(market=2)
        quotes_obj.client.get_security_count.assert_not_called()
        quotes_obj.client.disconnect.assert_not_called()
        assert quotes_obj.connected is True

    def test_stock_count_invalid_market_no_disconnect(self, quotes_obj):
        with pytest.raises(ValueError):
            quotes_obj.stock_count(market=99)
        quotes_obj.client.get_security_count.assert_not_called()
        quotes_obj.client.disconnect.assert_not_called()
        assert quotes_obj.connected is True


# ---------------------------------------------------------------------------
# F16: _parse_block_data + block_info chunked download
# ---------------------------------------------------------------------------

class TestParseBlockData:
    def test_two_blocks_flat(self):
        data = build_block_file([
            ("测试板块", 2, ["600036", "000001"]),
            ("BLOCK2", 0, ["300750"]),
        ])
        records = q._parse_block_data(data)
        assert records == [
            {"blockname": "测试板块", "block_type": 2, "code_index": 0, "code": "600036"},
            {"blockname": "测试板块", "block_type": 2, "code_index": 1, "code": "000001"},
            {"blockname": "BLOCK2", "block_type": 0, "code_index": 0, "code": "300750"},
        ]

    def test_differential_against_pytdx(self):
        # pytdx is an optional reference implementation (dev environments);
        # the synthetic-byte tests above pin the same behavior without it.
        block_reader = pytest.importorskip("pytdx.reader.block_reader")
        BlockReader = block_reader.BlockReader
        data = build_block_file([
            ("银行", 2, ["600036", "601398", "000001"]),
            ("概念A", 1, ["300750", "002594"]),
            ("空板块", 0, []),
            ("风格", 3, ["510050"]),
        ])
        expected = [dict(r) for r in BlockReader().get_data(bytearray(data))]
        assert q._parse_block_data(data) == expected

    def test_empty_block_advances_padding(self):
        data = build_block_file([
            ("EMPTY", 0, []),
            ("AFTER", 1, ["600036"]),
        ])
        records = q._parse_block_data(data)
        assert records == [
            {"blockname": "AFTER", "block_type": 1, "code_index": 0, "code": "600036"},
        ]

    @pytest.mark.parametrize("data", [b"", b"\x00" * 100, b"\x00" * 385])
    def test_short_data_returns_empty(self, data):
        assert q._parse_block_data(data) == []

    def test_zero_blocks(self):
        data = b"\x00" * 384 + struct.pack("<H", 0)
        assert q._parse_block_data(data) == []

    def test_truncated_block_no_exception(self):
        data = build_block_file([("板块", 2, ["600036", "000001"])])
        # cut in the middle of the second code: partial results, no crash
        cut = 384 + 2 + 9 + 4 + 7 + 3
        records = q._parse_block_data(data[:cut])
        assert records == [
            {"blockname": "板块", "block_type": 2, "code_index": 0, "code": "600036"},
        ]

    def test_truncated_header_no_exception(self):
        data = build_block_file([("板块", 2, ["600036"])])
        assert q._parse_block_data(data[:400]) == []


class TestBlockInfo:
    def test_chunked_download_and_parse(self, quotes_obj):
        data = build_block_file([("板块A", 2, ["600036", "000001"])])
        data += b"\x00" * (30001 - len(data))  # force two 30000-byte chunks
        quotes_obj.client.get_block_info_meta.return_value = {
            "size": len(data), "hash_value": 0,
        }
        quotes_obj.client.get_block_info.side_effect = (
            lambda block_file, start, size: data[start:start + 30000]
        )
        df = quotes_obj.block_info("block_gn.dat")
        assert len(df) == 2
        assert df["code"].tolist() == ["600036", "000001"]
        assert quotes_obj.client.get_block_info.call_count == 2
        starts = [c.kwargs["start"] for c in quotes_obj.client.get_block_info.call_args_list]
        assert starts == [0, 30000]

    def test_empty_meta_returns_empty_df(self, quotes_obj):
        quotes_obj.client.get_block_info_meta.return_value = {}
        df = quotes_obj.block_info("block_zs.dat")
        assert len(df) == 0
        quotes_obj.client.get_block_info.assert_not_called()


# ---------------------------------------------------------------------------
# Cross-group note: finance() now documents 35 fields (G2 adds baoliu2)
# ---------------------------------------------------------------------------

class TestFinanceDocstring:
    def test_documents_35_fields(self):
        assert "35" in q.Quotes.finance.__doc__
        assert "34" not in q.Quotes.finance.__doc__