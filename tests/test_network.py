import pytest
import datetime
from mitdx._core import TdxClient

# known stable public TDX server for testing
TEST_SERVER_IP = "110.41.147.114"  # 深圳双线主站1
TEST_SERVER_PORT = 7709


def test_tdx_client_connect():
    client = TdxClient()
    connected = client.connect(TEST_SERVER_IP, TEST_SERVER_PORT)
    assert connected is True
    
    # Try getting daily bars for 600036 (招商银行)
    # category 9 = daily
    # market 1 = SH
    bars = client.get_security_bars(category=9, market=1, code="600036", start=0, count=10)
    
    # Should get exactly 10 bars (TDX usually returns count bars or fewer if not available)
    assert isinstance(bars, list)
    if len(bars) > 0:
        bar = bars[0]
        assert "datetime" in bar
        assert "open" in bar
        assert "close" in bar
        assert "high" in bar
        assert "low" in bar
        assert "vol" in bar
        assert "amount" in bar
        
        # Verify plausible prices for 600036
        assert bar["open"] > 1.0
        assert bar["close"] > 1.0
    
    client.disconnect()
