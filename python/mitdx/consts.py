# Market codes
MARKET_SZ = 0  # 深圳
MARKET_SH = 1  # 上海

# Security list batch size (TDX protocol limit per request)
SECURITY_LIST_BATCH_SIZE = 1000

# Maximum number of bars a single get_security_bars request may return
# (TDX protocol limit). k() paginates backwards in batches of this size.
MAX_BARS_PER_REQUEST = 800

# Safety cap on the number of batches k() fetches when paginating backwards
# through history: at most MAX_K_BATCHES * MAX_BARS_PER_REQUEST bars.
# The wire protocol's `start` field is u16 (max 65535), so the last batch
# start, (MAX_K_BATCHES - 1) * MAX_BARS_PER_REQUEST, must stay <= 65535:
# 82 batches -> last start 64800, at most 65600 bars.
MAX_K_BATCHES = 82

# Maximum number of symbols per get_security_quotes request (TDX server
# convention). Larger lists are split into batches to stay well below the
# u16 packet-length limit of the wire protocol.
QUOTES_BATCH_SIZE = 80

# Static list of HQ (行情) hosts for real-time quotes
HQ_HOSTS = [  # NOSONAR - server address configuration, not credentials
    # (name, ip, port)
    ('深圳双线主站1', '110.41.147.114', 7709),
    ('深圳双线主站2', '8.129.13.54', 7709),
    ('深圳双线主站3', '120.24.149.49', 7709),
    ('上海双线主站1', '124.70.176.52', 7709),
    ('上海双线主站2', '124.71.187.122', 7709),
    ('北京双线主站1', '114.115.234.36', 7709),
    ('华南双线主站1', '119.147.212.81', 7709),
]

# Dedicated GP (General Purpose) hosts for financial data downloads
GP_HOSTS = [
    ('TDX Financial 1', '120.76.152.87', 7709),
    ('TDX Financial 2', '119.147.212.81', 7709),
]
