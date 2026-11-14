"""Basic tests for mitdx._core Rust module."""

import struct
from pathlib import Path

import pytest


class TestRustCore:
    """Test that the Rust _core module loads and functions exist."""

    def test_import_core(self):
        from mitdx import _core
        assert hasattr(_core, "read_daily_bars")
        assert hasattr(_core, "read_minute_bars")

    def test_read_daily_file_not_found(self):
        from mitdx._core import read_daily_bars
        with pytest.raises(FileNotFoundError):
            read_daily_bars("/nonexistent/path.day")

    def test_read_minute_file_not_found(self):
        from mitdx._core import read_minute_bars
        with pytest.raises(FileNotFoundError):
            read_minute_bars("/nonexistent/path.lc1")


class TestReader:
    """Test the Python Reader wrapper."""

    def test_factory_std(self, tmp_path):
        from mitdx.reader import Reader

        # Create a minimal vipdoc structure
        vipdoc = tmp_path / "vipdoc" / "sh" / "lday"
        vipdoc.mkdir(parents=True)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        assert reader is not None

    def test_factory_ext(self, tmp_path):
        from mitdx.reader import Reader

        vipdoc = tmp_path / "vipdoc"
        vipdoc.mkdir(parents=True)

        reader = Reader.factory(market="ext", tdxdir=str(tmp_path))
        assert reader is not None

    def test_daily_missing_file(self, tmp_path):
        """daily() should return None for missing data files."""
        from mitdx.reader import Reader

        vipdoc = tmp_path / "vipdoc" / "sh" / "lday"
        vipdoc.mkdir(parents=True)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        result = reader.daily(symbol="600036")
        assert result is None

    def test_daily_valid_file(self, tmp_path):
        """daily() should parse a synthetic .day file correctly."""
        import struct
        from mitdx.reader import Reader

        # Create vipdoc directory structure
        vipdoc = tmp_path / "vipdoc" / "sh" / "lday"
        vipdoc.mkdir(parents=True)

        # Create a synthetic .day file (1 record, 32 bytes)
        # date=20250101, open=1000, high=1100, low=900, close=1050, amount=1000000.0, volume=50000, reserved=0
        record = struct.pack("<IIIIIfII", 20250101, 1000, 1100, 900, 1050, 1000000.0, 50000, 0)
        day_file = vipdoc / "sh600036.day"
        day_file.write_bytes(record)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="600036")

        assert df is not None
        assert len(df) == 1
        assert abs(df.iloc[0]["open"] - 10.0) < 0.01  # open: 1000 fen scaled by 0.01 gives 10.0 yuan
        assert abs(df.iloc[0]["close"] - 10.5) < 0.01


# ---------------------------------------------------------------------------
# F1 regression tests: market routing must follow the reference rule chain in
# mootdx.utils.get_stock_market (first match wins, default 'sh'):
#   1. explicit sh/sz prefix             -> that market
#   2. 50/51/60/68/90/110/113/132/204    -> sh
#   3. 00/12/13/18/15/16/20/30/39/115/1318 -> sz
#   4. 5/6/9/7                           -> sh
#   5. 4/8                               -> bj
# Expected values below are hand-derived from that reference, not from mitdx.
# ---------------------------------------------------------------------------


def _write_day_file(directory: Path, filename: str, close: int = 1050, volume: int = 50000) -> None:
    """Write one synthetic 32-byte TDX daily record (test utility)."""
    directory.mkdir(parents=True, exist_ok=True)
    record = struct.pack("<IIIIIfII", 20250101, 1000, 1100, 900, close, 1000000.0, volume, 0)
    (directory / filename).write_bytes(record)


class TestMarketRouting:
    """daily()/_find_path() must pick the market directory mootdx would pick."""

    def test_daily_000001_reads_sz_stock_not_sh_index(self, tmp_path):
        """F1 PoC: '000001' is the SZ stock, not the SH index.

        Breaks if '00' routes to sh: close would be 3000.0 and volume 50000
        (SH_INDEX coefficients) instead of 10.5 / 500 (SZ_A_STOCK).
        """
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh000001.day", close=300000)
        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", "sz000001.day", close=1050)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="000001")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(500.0)  # 50000 * 0.01 (SZ_A_STOCK)

    def test_daily_002415_finds_sz_sme_file(self, tmp_path):
        """002xxx SME codes live under sz; breaks if routed to sh (returns None)."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", "sz002415.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="002415")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)

    @pytest.mark.parametrize("code", ["600036", "688981"])
    def test_daily_sh_codes_still_route_sh(self, tmp_path, code):
        """Guard: 60xxxx/68xxxx must keep resolving under sh."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", f"sh{code}.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol=code)

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(500.0)

    @pytest.mark.parametrize("code, expected_volume", [("300750", 500.0), ("399001", 50000.0)])
    def test_daily_sz_codes_route_sz(self, tmp_path, code, expected_volume):
        """Guard: 30xxxx (ChiNext) and 39xxxx (SZ index) resolve under sz."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", f"sz{code}.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol=code)

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(expected_volume)

    def test_daily_880001_sector_index_routes_sh(self, tmp_path):
        """TDX sector indices 88xxxx live under sh (find_path special case)."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh880001.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="880001")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(50000.0)  # SH_INDEX vol_coeff 1.0

    @pytest.mark.parametrize("code", ["110001", "113001", "132001", "204001"])
    def test_daily_sh_bond_repo_prefixes_route_sh(self, tmp_path, code):
        """Rule 2 before rule 3: 110/113/132/204 are SH, not SZ via '13'/'20'."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", f"sh{code}.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol=code)

        assert df is not None

    @pytest.mark.parametrize("code", ["115001", "120001", "130001"])
    def test_daily_sz_bond_prefixes_route_sz(self, tmp_path, code):
        """115/12/13 (non-132) are SZ; breaks if '11' blanket-routes to sh."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", f"sz{code}.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol=code)

        assert df is not None

    def test_daily_7x_prefix_routes_sh(self, tmp_path):
        """Rule 4 includes '7' (SH subscription codes); breaks if missing."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh700001.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="700001")

        assert df is not None

    def test_daily_explicit_sh_prefix_reads_index(self, tmp_path):
        """'sh000001' must stay the SH index even though '00' alone is SZ."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh000001.day", close=300000)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="sh000001")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(3000.0)
        assert df.iloc[0]["volume"] == pytest.approx(50000.0)

    def test_daily_explicit_sz_prefix_reads_stock(self, tmp_path):
        """'sz000001' must read the SZ stock even with the SH index present."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh000001.day", close=300000)
        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", "sz000001.day", close=1050)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="sz000001")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(500.0)  # SZ_A_STOCK, not SH_INDEX

    def test_daily_explicit_sz_prefix_overrides_code_digits(self, tmp_path):
        """'sz600036' must read the sz file even though '60' alone is SH."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh600036.day", close=2000)
        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", "sz600036.day", close=1050)

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="sz600036")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)

    def test_daily_uppercase_prefix_finds_lowercase_file(self, tmp_path):
        """'SH600036' must resolve to sh600036.day (case normalization)."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh600036.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="SH600036")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)

    @pytest.mark.parametrize("symbol, expected_name", [
        ("SH600036", "sh600036.day"),
        ("Sh600036", "sh600036.day"),
        ("SZ000001", "sz000001.day"),
    ])
    def test_find_path_lowercases_symbol(self, tmp_path, symbol, expected_name):
        """Lookup filename must be lowercased, even on case-insensitive filesystems.

        Breaks if original case is kept: lookup fails on case-sensitive FS.
        """
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh600036.day")
        _write_day_file(tmp_path / "vipdoc" / "sz" / "lday", "sz000001.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        path = reader._find_path(symbol)

        assert path is not None
        assert path.name == expected_name

    @pytest.mark.parametrize("code", ["430047", "830001"])
    def test_daily_bj_codes_route_bj(self, tmp_path, code):
        """Rule 5: 4xxxxx/8xxxxx (BJ) resolve under vipdoc/bj.

        Note: mitdx prepends the market prefix for bj too (existing _find_path
        semantics); mootdx find_path leaves bj symbols unprefixed.
        """
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "bj" / "lday", f"bj{code}.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol=code)

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)

    def test_daily_unknown_prefix_defaults_sh(self, tmp_path):
        """mootdx default is 'sh': '220001' matches no rule."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh220001.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="220001")

        assert df is not None

    def test_find_path_minute_routes_sz(self, tmp_path):
        """minute() shares _find_path: '000001' lc1 must resolve under sz."""
        from mitdx.reader import Reader

        minline = tmp_path / "vipdoc" / "sz" / "minline"
        minline.mkdir(parents=True)
        (minline / "sz000001.lc1").write_bytes(b"")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        path = reader._find_path("000001", subdir="minline", suffix=["lc1", "1"])

        assert path is not None
        assert path == minline / "sz000001.lc1"


class TestGetMarketRules:
    """_get_market matches mootdx.utils.get_stock_market(symbol, string=True),
    with two documented intentional divergences (see inline comments below):
    case-insensitive rule-1 prefixes, and legacy leading-'#' stripping.
    """

    @pytest.mark.parametrize("symbol, expected", [
        # rule 3 -> sz
        ("000001", "sz"), ("002415", "sz"), ("300750", "sz"), ("399001", "sz"),
        ("120001", "sz"), ("130001", "sz"), ("150001", "sz"), ("160001", "sz"),
        ("180001", "sz"), ("200001", "sz"), ("115001", "sz"), ("131801", "sz"),
        # rule 2 -> sh (longer prefixes must win over rule 3 '13'/'20')
        ("600036", "sh"), ("688981", "sh"), ("900901", "sh"),
        ("500001", "sh"), ("510050", "sh"),
        ("110001", "sh"), ("113001", "sh"), ("132001", "sh"), ("204001", "sh"),
        # rule 4 -> sh
        ("700001", "sh"), ("999999", "sh"),
        # rule 5 -> bj (88xxxx is bj here; _find_path overrides it to sh)
        ("430047", "bj"), ("830001", "bj"), ("880001", "bj"),
        # default fallback -> sh
        ("", "sh"), ("220001", "sh"),
        # rule 1: explicit prefix, case-insensitive
        ("sh000001", "sh"), ("sz600036", "sz"),
        ("SH600036", "sh"), ("SZ000001", "sz"),
        ("Sh600036", "sh"), ("sZ000001", "sz"),
        # legacy '#' preprocessing kept (ds routing out of scope)
        ("#510050", "sh"), ("#300750", "sz"),
    ])
    def test_get_market_matches_mootdx(self, symbol, expected):
        from mitdx.reader import _get_market

        assert _get_market(symbol) == expected


# ---------------------------------------------------------------------------
# F24 regression tests: bj-resolved files (bj430047.day etc.) must get an
# explicit BJ_A_STOCK classification instead of silently falling through to
# the SZ_A_STOCK default. mootdx raises NotImplementedError for non-sh/sz
# exchanges; mitdx intentionally keeps serving data with coefficients
# identical to the old accidental default ([0.01, 0.01]), so numeric output
# is unchanged (values unverified against live BJ data).
# ---------------------------------------------------------------------------


class TestSecurityTypeBJ:
    """_get_security_type must classify bj files explicitly (F24)."""

    @pytest.mark.parametrize("code", ["430047", "830001", "870001", "920001"])
    def test_bj_prefixes_map_to_bj_a_stock(self, code):
        from mitdx.reader import _get_security_type

        assert _get_security_type(f"/vipdoc/bj/lday/bj{code}.day") == "BJ_A_STOCK"

    def test_bj_unknown_prefix_falls_back_to_bj_a_stock(self):
        """Unlisted bj prefixes keep the A-stock default (mirrors sh/sz branches)."""
        from mitdx.reader import _get_security_type

        assert _get_security_type("/vipdoc/bj/lday/bj990001.day") == "BJ_A_STOCK"

    def test_bj_coefficients_match_legacy_default(self):
        """Values must stay identical to the old accidental SZ_A_STOCK default."""
        from mitdx.reader import SECURITY_COEFFICIENT

        assert SECURITY_COEFFICIENT["BJ_A_STOCK"] == [0.01, 0.01]
        assert SECURITY_COEFFICIENT["BJ_A_STOCK"] == SECURITY_COEFFICIENT["SZ_A_STOCK"]

    def test_daily_bj_file_yields_stock_coefficients(self, tmp_path):
        """daily() on a synthetic bj file: close 10.5 / volume 500 (0.01/0.01)."""
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "bj" / "lday", "bj430047.day")

        reader = Reader.factory(market="std", tdxdir=str(tmp_path))
        df = reader.daily(symbol="430047")

        assert df is not None
        assert df.iloc[0]["close"] == pytest.approx(10.5)
        assert df.iloc[0]["volume"] == pytest.approx(500.0)

    @pytest.mark.parametrize("filename, expected", [
        ("sh000001.day", "SH_INDEX"),
        ("sh600036.day", "SH_A_STOCK"),
        ("sz000001.day", "SZ_A_STOCK"),
        ("sz399001.day", "SZ_INDEX"),
    ])
    def test_sh_sz_classification_unchanged(self, filename, expected):
        """Guard: the new bj branch must not disturb sh/sz lookups."""
        from mitdx.reader import _get_security_type

        assert _get_security_type(f"/vipdoc/x/lday/{filename}") == expected


# ---------------------------------------------------------------------------
# F4 regression tests: ExtReader is kept for mootdx API compatibility but has
# NO extended-market (ds) routing — it behaves identically to StdReader. The
# CLI (mitdx's own surface) must not advertise 'ext' as a --market choice.
# ---------------------------------------------------------------------------


class TestExtReaderCompat:
    """factory('ext') stays compatible; CLI drops the false 'ext' option (F4)."""

    def test_factory_ext_returns_ext_reader_instance(self, tmp_path):
        from mitdx.reader import ExtReader, Reader

        reader = Reader.factory(market="ext", tdxdir=str(tmp_path))
        assert isinstance(reader, ExtReader)

    def test_ext_reader_behaves_identically_to_std_reader(self, tmp_path):
        """Pin the docstring claim: no ds routing, same parsing as StdReader."""
        import pandas as pd
        from mitdx.reader import Reader

        _write_day_file(tmp_path / "vipdoc" / "sh" / "lday", "sh600036.day")

        std = Reader.factory(market="std", tdxdir=str(tmp_path))
        ext = Reader.factory(market="ext", tdxdir=str(tmp_path))
        pd.testing.assert_frame_equal(std.daily("600036"), ext.daily("600036"))

    def test_cli_parser_rejects_market_ext(self):
        from mitdx.cli import _init_parser

        parser = _init_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(
                ["reader", "daily", "--symbol", "600036", "--market", "ext"]
            )
        assert exc_info.value.code == 2  # argparse invalid-choice exit code

    def test_cli_parser_accepts_market_std(self):
        from mitdx.cli import _init_parser

        parser = _init_parser()
        args = parser.parse_args(
            ["reader", "daily", "--symbol", "600036", "--market", "std"]
        )
        assert args.market == "std"
