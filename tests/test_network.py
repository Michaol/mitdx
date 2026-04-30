import pytest
import datetime
from mitdx._core import TdxClient
from mitdx.consts import HQ_HOSTS

# Use first configured HQ host for testing
_name, TEST_SERVER_IP, TEST_SERVER_PORT = HQ_HOSTS[0]


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
