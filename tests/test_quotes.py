import pytest
from mitdx.quotes import Quotes

def test_quotes_bars():
    client = Quotes.factory(market='std')
    
    # Use '110.41.147.114' explicitly for fast test if needed, or let it connect automatically
    connected = client.connect(ip='110.41.147.114', port=7709)
    assert connected is True
    
    # 600036 招银
    df = client.bars(symbol='600036', frequency=9, start=0, count=20)
    
    assert not df.empty
    assert len(df) == 20
    assert 'open' in df.columns
    assert 'close' in df.columns
    assert 'datetime' in df.columns
    
    row = df.iloc[-1]
    assert row['open'] > 1.0
    
    client.disconnect()
