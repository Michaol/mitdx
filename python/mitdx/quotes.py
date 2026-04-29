import logging
from typing import Optional
from mitdx._core import TdxClient, ping_servers
from mitdx.utils import to_df
from mitdx.consts import HQ_HOSTS

logger = logging.getLogger(__name__)

class Quotes:
    """
    Python wrapper for TDX Quotes API using the fast Rust core.
    Provides backward compatibility with mootdx API where sensible.
    """
    
    def __init__(self, market: str = 'std'):
        self.market = market
        self.client = TdxClient()
        self.connected = False
        
    @staticmethod
    def factory(market='std', **kwargs):
        """Factory method to maintain backward compatibility with mootdx."""
        if market != 'std':
            raise NotImplementedError("Currently only 'std' market is supported in mitdx Phase 2.")
        return Quotes(market=market)
        
    def connect(self, ip: Optional[str] = None, port: int = 7709):
        if not ip:
            # simple fallback discovery for now
            ip = HQ_HOSTS[0][1]
            port = HQ_HOSTS[0][2]
            
        self.connected = self.client.connect(ip, port)
        return self.connected
        
    def disconnect(self):
        if self.connected:
            self.client.disconnect()
            self.connected = False
            
    def _ensure_connected(self):
        if not self.connected:
            # Ping all known servers using Rust multi-threading and connect to the fastest one
            ips = [(ip, port) for name, ip, port in HQ_HOSTS]
            fastest_servers = ping_servers(ips)
            
            for ip, port, latency in fastest_servers:
                if latency < 9999: # Not completely failed
                    if self.connect(ip, port):
                        return
                        
            raise ConnectionError("Failed to connect to any TDX HQ server. All pings failed.")
            
    def bars(self, symbol: str = '600036', frequency: int = 9, start: int = 0, count: int = 10, backend: str = 'pandas', **kwargs):
        """
        Fetch K-line bars.
        Supports standard parameters: frequency (0=5min, ..., 9=day)
        """
        self._ensure_connected()
        market_code = 1 if str(symbol).startswith('6') or str(symbol).startswith('5') else 0
        try:
            res = self.client.get_security_bars(category=frequency, market=market_code, code=symbol, start=start, count=count)
            return to_df(res, backend=backend)
        except Exception as e:
            logger.error(f"Failed to fetch bars: {e}")
            self.disconnect() # Force reconnect next time
            raise

    def __enter__(self):
        self._ensure_connected()
        return self
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.disconnect()
