"""mitdx v1.1.0 — Full API Integration Test Suite"""
import mitdx

print(f"=== mitdx v{mitdx.__version__} full API integration test ===\n")

from mitdx.quotes import Quotes

q = Quotes()
q._ensure_connected()
print("[OK] Auto-connected to fastest TDX server\n")

# 1. bars (existing)
print("--- TEST 1: bars (daily K-line, 600036) ---")
df = q.bars(symbol="600036", frequency=9, count=5)
print(df.to_string())
print()

# 2. minute (alias)
print("--- TEST 2: minute (5-min K-line, 600036) ---")
df = q.minute(symbol="600036", frequency=0, count=5)
print(df.to_string())
print()

# 3. index (alias)
print("--- TEST 3: index (shanghai composite 999999) ---")
df = q.index(symbol="999999", frequency=9, count=5)
print(df.to_string())
print()

# 4. xdxr
print("--- TEST 4: xdxr (ex-dividend/rights, 600036) ---")
df = q.xdxr(symbol="600036")
print(df.head(5).to_string() if len(df) > 0 else "(empty)")
print()

# 5. transactions
print("--- TEST 5: transactions (tick data, 600036) ---")
df = q.transactions(symbol="600036", start=0, count=10)
print(df.to_string())
print()

# 6. quotes (snapshot)
print("--- TEST 6: quotes (realtime snapshot, 600036 + 000001) ---")
df = q.quotes(symbols=["600036", "000001"])
cols_to_show = ["market", "code", "price", "open", "high", "low", "vol", "amount"]
available = [c for c in cols_to_show if c in df.columns]
print(df[available].to_string())
print()

# 7. f10
print("--- TEST 7: f10 (company info, 600036) ---")
result = q.f10(symbol="600036")
cats = result.get("category", [])
print(f"F10 categories: {len(cats)}")
for c in cats:
    name = c.get("name", "?")
    fname = c.get("filename", "?")
    start = c.get("start", 0)
    length = c.get("length", 0)
    print(f"  - {name} [{fname}] start={start} len={length}")
content = result.get("content", "")
print(f"Content length: {len(content)} chars")
if content:
    print(content[:300])
print()

# 8. finance (alias)
print("--- TEST 8: finance (xdxr alias, 000001) ---")
df = q.finance(symbol="000001")
print(df.head(3).to_string() if len(df) > 0 else "(empty)")
print()

q.disconnect()
print("[OK] All 8 API tests completed successfully!")
