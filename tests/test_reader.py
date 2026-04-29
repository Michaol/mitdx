"""Basic tests for mitdx._core Rust module."""

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
        assert abs(df.iloc[0]["open"] - 10.0) < 0.01  # 1000 * 0.01 = 10.0
        assert abs(df.iloc[0]["close"] - 10.5) < 0.01
