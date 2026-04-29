from typing import Optional, List, Dict, Any, Union

def to_df(data: List[Union[Dict[str, Any], tuple]], columns: Optional[List[str]] = None, backend: str = 'pandas') -> Any:
    """
    Convert a list of dicts or tuples to a DataFrame using the specified backend.
    
    :param data: The raw data produced by the Rust pyo3 module.
    :param columns: Explicit column names if data is a list of tuples.
    :param backend: Result format, either 'pandas' or 'polars'.
    :return: pd.DataFrame or pl.DataFrame
    """
    if not data:
        if backend == 'polars':
            import polars as pl
            return pl.DataFrame(schema=columns)
        else:
            import pandas as pd
            return pd.DataFrame(columns=columns)
            
    if backend == 'polars':
        import polars as pl
        if columns and isinstance(data[0], tuple):
            return pl.DataFrame(data, schema=columns, orient="row")
        else:
            return pl.DataFrame(data)
    else:
        import pandas as pd
        if columns and isinstance(data[0], tuple):
            return pd.DataFrame(data, columns=columns)
        else:
            return pd.DataFrame(data)
