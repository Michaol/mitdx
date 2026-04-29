import datetime
from mitdx.calendar import is_trading_day, is_holiday

def test_is_trading_day():
    # 2026-01-01 is 元旦 (New Year's Day), which is a holiday
    d1 = datetime.date(2026, 1, 1)
    assert is_holiday(d1) is True
    assert is_trading_day(d1) is False
    
    # 2026-05-01 is Labor Day (holiday)
    d2 = datetime.date(2026, 5, 1)
    assert is_holiday(d2) is True
    assert is_trading_day(d2) is False

def test_is_trading_day_weekend():
    # 2026-01-03 is Saturday
    d3 = datetime.date(2026, 1, 3)
    assert is_holiday(d3) is True
    assert is_trading_day(d3) is False

def test_is_trading_day_normal():
    # 2026-04-20 is a Monday without a holiday (presumably)
    # Actually wait, let's test a known past date to be totally sure
    d4 = datetime.date(2024, 1, 2) # Jan 2 2024 was Tuesday, normal day
    assert is_holiday(d4) is False
    assert is_trading_day(d4) is True
