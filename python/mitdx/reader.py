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

from mitdx._core import read_daily_bars, read_minute_bars


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
    # BJ (Beijing Stock Exchange) coefficients are UNVERIFIED against live
    # data. Values deliberately equal the SZ_A_STOCK default that bj files
    # previously fell through to, so numeric output is unchanged. (mootdx
    # itself raises NotImplementedError for non-sh/sz exchanges.)
    "BJ_A_STOCK": [0.01, 0.01],
}


def _get_market(symbol: str) -> str:
    """Determine market (sh/sz/bj) from symbol code prefix.

    Faithful port of mootdx.utils.get_stock_market (source of truth).
    Rule order matters: the longer SH prefixes in rule 2 ('110', '113',
    '132', '204') must be tested before the shorter SZ prefixes in rule 3
    ('13', '20', ...) that they overlap with, otherwise SH bonds/repos
    would misroute to SZ. Unmatched symbols fall back to 'sh' (the mootdx
    default). Prefix tuples are verbatim from upstream, including the
    duplicated '18' and the redundant '1318' (already covered by '13').
    """
    # Legacy mitdx preprocessing: strip a leading '#' extended-market marker
    # before inference (ds routing itself is intentionally out of scope).
    s = symbol.lower().removeprefix("#")

    market = "sh"  # mootdx default/fallback

    # Rule 1 is case-insensitive here; mootdx only matches sh/sz/SH/SZ and
    # lets mixed case ('Sh', 'sZ') fall through to the numeric rules.
    if s.startswith(("sh", "sz")):  # rule 1: explicit prefix wins
        market = s[:2]
    elif s.startswith(("50", "51", "60", "68", "90", "110", "113", "132", "204")):  # rule 2
        market = "sh"
    elif s.startswith(("00", "12", "13", "18", "15", "16", "18", "20", "30", "39", "115", "1318")):  # rule 3
        market = "sz"
    elif s.startswith(("5", "6", "9", "7")):  # rule 4
        market = "sh"
    elif s.startswith(("4", "8")):  # rule 5
        market = "bj"

    return market


SZ_MAP = {
    "00": "SZ_A_STOCK", "30": "SZ_A_STOCK", "20": "SZ_B_STOCK",
    "39": "SZ_INDEX", "15": "SZ_FUND", "16": "SZ_FUND", "18": "SZ_FUND",
    "10": "SZ_BOND", "11": "SZ_BOND", "12": "SZ_BOND", "13": "SZ_BOND", "14": "SZ_BOND"
}

SH_MAP = {
    "60": "SH_A_STOCK", "90": "SH_B_STOCK", "68": "SH_STAR_STOCK",
    "00": "SH_INDEX", "88": "SH_INDEX", "99": "SH_INDEX",
    "50": "SH_FUND", "51": "SH_FUND", "58": "SH_FUND",
    "01": "SH_BOND", "02": "SH_BOND", "10": "SH_BOND", "11": "SH_BOND", "12": "SH_BOND",
    "13": "SH_BOND", "14": "SH_BOND", "15": "SH_BOND", "16": "SH_BOND", "17": "SH_BOND",
    "18": "SH_BOND", "19": "SH_BOND", "20": "SH_BOND"
}

BJ_MAP = {
    "43": "BJ_A_STOCK", "83": "BJ_A_STOCK", "87": "BJ_A_STOCK", "92": "BJ_A_STOCK"
}

def _get_security_type(filepath: str) -> str:
    """Detect security type from filepath for coefficient lookup."""
    fname = Path(filepath).stem.lower()

    if fname.startswith("sz"):
        return SZ_MAP.get(fname[2:4], "SZ_A_STOCK")
    elif fname.startswith("sh"):
        return SH_MAP.get(fname[2:4], "SH_A_STOCK")
    elif fname.startswith("bj"):
        return BJ_MAP.get(fname[2:4], "BJ_A_STOCK")

    return "SZ_A_STOCK"


class Reader:
    """Factory for creating market-specific readers."""

    @staticmethod
    def factory(market: str = "std", **kwargs) -> "StdReader | ExtReader":
        """Create a reader instance.

        Args:
            market: 'std' for standard market (A-shares); 'ext' returns an
                ExtReader, which currently behaves identically to StdReader
                (extended-market ds routing is not implemented).
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
        # Lowercase for lookup so 'SH600036' resolves on case-sensitive
        # filesystems (mootdx find_path normalizes case as well).
        symbol = Path(symbol).stem.lower()

        # TDX sector indices (88****) live under sh, overriding the rules.
        if symbol.startswith("88"):
            market = "sh"
        else:
            market = _get_market(symbol)

        # Re-attach the resolved market prefix: strip a leading sh/sz once
        # (removeprefix-style, NOT mootdx's replace-all-occurrences quirk),
        # then prepend the lowercase market.
        if symbol.startswith("sh"):
            symbol = symbol.removeprefix("sh")
        elif symbol.startswith("sz"):
            symbol = symbol.removeprefix("sz")
        symbol = f"{market}{symbol}"

        suffixes = [suffix] if isinstance(suffix, str) else suffix

        for ext in suffixes:
            ext = ext.lstrip(".")
            filepath = Path(self.tdxdir) / "vipdoc" / market / subdir / f"{symbol}.{ext}"
            if filepath.exists():
                return filepath

        return None

    def daily(self, symbol: str, **kwargs) -> Optional[pd.DataFrame]:
        """Read daily bar data.

        Args:
            symbol: Stock code (e.g. '600036')

        Returns:
            DataFrame with columns: date, open, high, low, close, amount, volume
        """
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


class StdReader(ReaderBase):
    """Standard market (A-shares) reader."""
    pass


class ExtReader(ReaderBase):
    """Extended-market reader placeholder (mootdx API compatibility only).

    Extended-market (ds) routing is NOT implemented: this class currently
    behaves identically to StdReader (same vipdoc sh/sz/bj file lookup and
    parsing). It exists so ``Reader.factory(market='ext')`` keeps returning
    an ExtReader instance for code written against the mootdx API. The CLI
    deliberately does not expose 'ext' as a --market choice.
    """
    pass
