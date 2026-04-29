"""通达信离线数据读取接口

使用 Rust 核心解析 VIPDOC 二进制文件，通过 Python 包装层对外暴露  backward-compatible API。

Usage:
    from mitdx.reader import Reader

    reader = Reader.factory(market='std', tdxdir='C:/new_tdx')
    df = reader.daily(symbol='600036')
"""

from __future__ import annotations

from pathlib import Path
from typing import Optional

import pandas as pd


# Security type classification (ported from mootdx/contrib/compat.py)
SECURITY_COEFFICIENT: dict[str, list[float]] = {
    "SH_A_STOCK": [0.01, 0.01],
    "SH_B_STOCK": [0.001, 0.01],
    "SH_STAR_STOCK": [0.01, 0.01],
    "SH_INDEX": [0.01, 1.0],
    "SH_FUND": [0.001, 1.0],
    "SH_BOND": [0.001, 1.0],
    "SZ_A_STOCK": [0.01, 0.01],
    "SZ_B_STOCK": [0.01, 0.01],
    "SZ_INDEX": [0.01, 1.0],
    "SZ_FUND": [0.001, 0.01],
    "SZ_BOND": [0.001, 0.01],
}


def _get_market(symbol: str) -> str:
    """Determine market (sh/sz) from symbol code prefix."""
    code = symbol.lstrip("shSHszSZ#")[:2]

    if symbol.lower().startswith("sh") or symbol.lower().startswith("sz"):
        return symbol[:2].lower()

    # Infer from code prefix
    if code in ("60", "68", "90", "00", "11", "50", "51", "58", "88", "99"):
        return "sh"

    return "sz"


def _get_security_type(filepath: str) -> str:
    """Detect security type from filepath for coefficient lookup."""
    fname = Path(filepath).stem.lower()

    # Extract exchange prefix and code head
    if fname.startswith("sh"):
        exchange, code_head = "sh", fname[2:4]
    elif fname.startswith("sz"):
        exchange, code_head = "sz", fname[2:4]
    else:
        return "SZ_A_STOCK"  # default

    if exchange == "sz":
        if code_head in ("00", "30"):
            return "SZ_A_STOCK"
        if code_head == "20":
            return "SZ_B_STOCK"
        if code_head == "39":
            return "SZ_INDEX"
        if code_head in ("15", "16", "18"):
            return "SZ_FUND"
        if code_head in ("10", "11", "12", "13", "14"):
            return "SZ_BOND"
        return "SZ_A_STOCK"

    if exchange == "sh":
        if code_head == "60":
            return "SH_A_STOCK"
        if code_head == "90":
            return "SH_B_STOCK"
        if code_head == "68":
            return "SH_STAR_STOCK"
        if code_head in ("00", "88", "99"):
            return "SH_INDEX"
        if code_head in ("50", "51", "58"):
            return "SH_FUND"
        if code_head in ("01", "02", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20"):
            return "SH_BOND"
        return "SH_A_STOCK"

    return "SZ_A_STOCK"


class Reader:
    """Factory for creating market-specific readers."""

    @staticmethod
    def factory(market: str = "std", **kwargs) -> "StdReader | ExtReader":
        """Create a reader instance.

        Args:
            market: 'std' for standard market (A-shares), 'ext' for extended market
            tdxdir: Path to TDX installation directory

        Returns:
            StdReader or ExtReader instance
        """
        if market == "ext":
            return ExtReader(**kwargs)
        return StdReader(**kwargs)


class ReaderBase:
    """Base class for TDX data readers."""

    def __init__(self, tdxdir: str = "C:/new_tdx"):
        tdxdir_path = Path(tdxdir)
        if not tdxdir_path.is_dir():
            raise FileNotFoundError(f"TDX directory not found: {tdxdir}")
        self.tdxdir = tdxdir

    def _find_path(
        self, symbol: str, subdir: str = "lday", suffix: str | list[str] = "day"
    ) -> Optional[Path]:
        """Locate VIPDOC data file for a given symbol."""
        symbol = Path(symbol).stem

        if symbol.startswith("88"):
            market = "sh"
        else:
            market = _get_market(symbol)

        # Ensure symbol has market prefix
        if not symbol.lower().startswith(("sh", "sz")):
            symbol = f"{market}{symbol}"

        suffixes = [suffix] if isinstance(suffix, str) else suffix

        for ext in suffixes:
            ext = ext.lstrip(".")
            filepath = Path(self.tdxdir) / "vipdoc" / market / subdir / f"{symbol}.{ext}"
            if filepath.exists():
                return filepath

        return None


class StdReader(ReaderBase):
    """Standard market (A-shares) reader."""

    def daily(self, symbol: str, **kwargs) -> Optional[pd.DataFrame]:
        """Read daily bar data.

        Args:
            symbol: Stock code (e.g. '600036')

        Returns:
            DataFrame with columns: date, open, high, low, close, amount, volume
        """
        from mitdx._core import read_daily_bars

        vipdoc = self._find_path(symbol=symbol, subdir="lday", suffix="day")
        if vipdoc is None:
            return None

        sec_type = _get_security_type(str(vipdoc))
        coeff = SECURITY_COEFFICIENT.get(sec_type, [0.01, 0.01])

        records = read_daily_bars(str(vipdoc), price_coeff=coeff[0], vol_coeff=coeff[1])
        if not records:
            return None

        df = pd.DataFrame(records)
        df["date"] = pd.to_datetime(df["date"])
        df.set_index("date", inplace=True)
        return df

    def minute(self, symbol: str, suffix: int = 1, **kwargs) -> Optional[pd.DataFrame]:
        """Read minute bar data (1-min or 5-min).

        Args:
            symbol: Stock code
            suffix: 1 for 1-minute, 5 for 5-minute

        Returns:
            DataFrame with columns: datetime, open, high, low, close, amount, volume
        """
        from mitdx._core import read_minute_bars

        subdir = "fzline" if str(suffix) == "5" else "minline"
        file_suffix = ["lc5", "5"] if str(suffix) == "5" else ["lc1", "1"]

        vipdoc = self._find_path(symbol=symbol, subdir=subdir, suffix=file_suffix)
        if vipdoc is None:
            return None

        records = read_minute_bars(str(vipdoc))
        if not records:
            return None

        df = pd.DataFrame(records)
        df["datetime"] = pd.to_datetime(df["datetime"])
        df.set_index("datetime", inplace=True)
        return df

    def fzline(self, symbol: str) -> Optional[pd.DataFrame]:
        """Read 5-minute bar data."""
        return self.minute(symbol, suffix=5)


class ExtReader(ReaderBase):
    """Extended market reader (futures, options, etc.)."""

    def daily(self, symbol: str, **kwargs) -> Optional[pd.DataFrame]:
        """Read extended market daily bar data."""
        from mitdx._core import read_daily_bars

        vipdoc = self._find_path(symbol=symbol, subdir="lday", suffix="day")
        if vipdoc is None:
            return None

        records = read_daily_bars(str(vipdoc), price_coeff=0.01, vol_coeff=1.0)
        if not records:
            return None

        df = pd.DataFrame(records)
        df["date"] = pd.to_datetime(df["date"])
        df.set_index("date", inplace=True)
        return df

    def minute(self, symbol: str, **kwargs) -> Optional[pd.DataFrame]:
        """Read extended market minute data."""
        from mitdx._core import read_minute_bars

        vipdoc = self._find_path(symbol=symbol, subdir="minline", suffix=["lc1", "1"])
        if vipdoc is None:
            return None

        records = read_minute_bars(str(vipdoc))
        if not records:
            return None

        df = pd.DataFrame(records)
        df["datetime"] = pd.to_datetime(df["datetime"])
        df.set_index("datetime", inplace=True)
        return df

    def fzline(self, symbol: str) -> Optional[pd.DataFrame]:
        """Read 5-minute bar data."""
        from mitdx._core import read_minute_bars

        vipdoc = self._find_path(symbol=symbol, subdir="fzline", suffix="lc5")
        if vipdoc is None:
            return None

        records = read_minute_bars(str(vipdoc))
        if not records:
            return None

        df = pd.DataFrame(records)
        df["datetime"] = pd.to_datetime(df["datetime"])
        df.set_index("datetime", inplace=True)
        return df
