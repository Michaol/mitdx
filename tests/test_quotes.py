import pytest
from mitdx.quotes import Quotes

@pytest.mark.network
def test_quotes_bars():
    with Quotes.factory(market='std') as client:
        # 600036 招银
        df = client.bars(symbol='600036', frequency=9, start=0, count=20)

        assert not df.empty
        assert len(df) == 20
        assert 'open' in df.columns
        assert 'close' in df.columns
        assert 'datetime' in df.columns

        row = df.iloc[-1]
        assert row['open'] > 1.0
