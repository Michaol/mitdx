"""Offline tests for mitdx.utils.to_df (F16-python).

Exercises the empty / tuple / dict paths on BOTH the pandas and polars
backends. No network, no Rust client involved.
"""

import pandas as pd
import polars as pl

from mitdx.utils import to_df


class TestEmpty:
    def test_empty_pandas_default(self):
        df = to_df([])
        assert isinstance(df, pd.DataFrame)
        assert df.empty
        assert len(df.columns) == 0

    def test_empty_pandas_with_columns(self):
        df = to_df([], columns=["code", "name"])
        assert isinstance(df, pd.DataFrame)
        assert list(df.columns) == ["code", "name"]
        assert len(df) == 0

    def test_empty_polars_default(self):
        df = to_df([], backend="polars")
        assert isinstance(df, pl.DataFrame)
        assert df.height == 0
        assert df.width == 0

    def test_empty_polars_with_columns(self):
        df = to_df([], columns=["code", "name"], backend="polars")
        assert isinstance(df, pl.DataFrame)
        assert df.columns == ["code", "name"]
        assert df.height == 0


class TestDictRows:
    DATA = [
        {"code": "600036", "close": 10.5, "vol": 500.0},
        {"code": "000001", "close": 11.0, "vol": 600.0},
    ]

    def test_pandas(self):
        df = to_df(self.DATA)
        assert isinstance(df, pd.DataFrame)
        assert list(df.columns) == ["code", "close", "vol"]
        assert len(df) == 2
        assert df["code"].tolist() == ["600036", "000001"]
        assert df["close"].tolist() == [10.5, 11.0]

    def test_polars(self):
        df = to_df(self.DATA, backend="polars")
        assert isinstance(df, pl.DataFrame)
        assert df.columns == ["code", "close", "vol"]
        assert df.height == 2
        assert df["code"].to_list() == ["600036", "000001"]
        assert df["close"].to_list() == [10.5, 11.0]

    def test_single_dict_finance_shape(self):
        # finance() wraps its single dict result as [res]
        row = {"liutongguben": 1.5, "zongzichan": 100.0}
        pd_df = to_df([row])
        pl_df = to_df([row], backend="polars")
        assert len(pd_df) == 1
        assert pl_df.height == 1
        assert pd_df["liutongguben"].iloc[0] == 1.5
        assert pl_df["liutongguben"][0] == 1.5

    def test_column_order_follows_first_dict(self):
        data = [{"b": 1, "a": 2}]
        assert list(to_df(data).columns) == ["b", "a"]
        assert to_df(data, backend="polars").columns == ["b", "a"]


class TestTupleRows:
    DATA = [(1, "a"), (2, "b")]
    COLUMNS = ["n", "s"]

    def test_pandas_with_columns(self):
        df = to_df(self.DATA, columns=self.COLUMNS)
        assert isinstance(df, pd.DataFrame)
        assert list(df.columns) == ["n", "s"]
        assert df.values.tolist() == [[1, "a"], [2, "b"]]

    def test_polars_with_columns(self):
        df = to_df(self.DATA, columns=self.COLUMNS, backend="polars")
        assert isinstance(df, pl.DataFrame)
        assert df.columns == ["n", "s"]
        assert df.rows() == [(1, "a"), (2, "b")]

    def test_pandas_without_columns_gets_integer_names(self):
        df = to_df(self.DATA)
        assert list(df.columns) == [0, 1]
        assert df.values.tolist() == [[1, "a"], [2, "b"]]


class TestBackendSelection:
    def test_default_backend_is_pandas(self):
        assert isinstance(to_df([{"a": 1}]), pd.DataFrame)

    def test_explicit_pandas(self):
        assert isinstance(to_df([{"a": 1}], backend="pandas"), pd.DataFrame)

    def test_explicit_polars(self):
        assert isinstance(to_df([{"a": 1}], backend="polars"), pl.DataFrame)


class TestTupleRowsWithoutColumns:
    def test_polars_without_columns(self):
        # Final-review fix: tuple rows must work on polars without columns
        # (previously TypeError inferring mixed-type columns).
        df = to_df([(1, "a"), (2, "b")], backend="polars")
        assert df.shape == (2, 2)
        assert df.rows() == [(1, "a"), (2, "b")]