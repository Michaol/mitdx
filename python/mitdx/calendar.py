import logging
import urllib.request
import re
import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

# Provide a simple local cache function
CACHE_DIR = Path.home() / '.mitdx'
CACHE_FILE = CACHE_DIR / 'holiday.csv'

def _fetch_holiday_data():
    req = urllib.request.Request('https://www.tdx.com.cn/url/holiday/', headers={'User-Agent': 'Mozilla/5.0 mitdx/0.1.0'})
    with urllib.request.urlopen(req, timeout=5) as response:
        text = response.read().decode('gbk')
        ret = re.findall(r'<textarea id="data" style="display:none;">([\s\S]+?)</textarea>', text, re.M)
        if ret:
            return ret[0].strip()
        return ""

def _get_holidays_from_cache():
    try:
        if not CACHE_DIR.exists():
            CACHE_DIR.mkdir(parents=True, exist_ok=True)
    except OSError:
        pass  # Handle read-only file systems gracefully

    # Refresh cache if older than 7 days or not exists
    try:
        if CACHE_FILE.exists():
            mtime = CACHE_FILE.stat().st_mtime
            if (datetime.datetime.now().timestamp() - mtime) < 7 * 24 * 3600:
                return CACHE_FILE.read_text(encoding='utf-8')
    except OSError:
        pass

    # Fetch new
    try:
        data = _fetch_holiday_data()
    except Exception as e:
        logger.warning("Failed to fetch holiday data from TDX server: %s", e)
        data = ""

    if data:
        try:
            CACHE_FILE.write_text(data, encoding='utf-8')
        except OSError:
            pass
    else:
        try:
            if CACHE_FILE.exists():
                # Fallback to old cache if network failed
                data = CACHE_FILE.read_text(encoding='utf-8')
        except OSError:
            pass

    return data

# Module level cached set of Chinese holiday date strings "YYYY-MM-DD"
_CN_HOLIDAYS = None

def _load_holidays():
    global _CN_HOLIDAYS
    if _CN_HOLIDAYS is not None:
        return _CN_HOLIDAYS
        
    _CN_HOLIDAYS = set()
    csv_data = _get_holidays_from_cache()
    if not csv_data:
        return _CN_HOLIDAYS
        
    for line in csv_data.split('\n'):
        parts = line.strip().split('|')
        if len(parts) >= 3:
            date_str = parts[0] # YYYYMMDD
            country = parts[2]
            if country == '中国' and len(date_str) == 8:
                formatted = f"{date_str[:4]}-{date_str[4:6]}-{date_str[6:8]}"
                _CN_HOLIDAYS.add(formatted)
                
    return _CN_HOLIDAYS

def is_holiday(date: Optional[datetime.date] = None) -> bool:
    """
    Check if a given date is a holiday (festival or weekend) in China.
    """
    if date is None:
        date = datetime.datetime.now().date()
        
    # Weekends
    if date.weekday() >= 5:
        return True
        
    holidays = _load_holidays()
    return date.strftime('%Y-%m-%d') in holidays

def is_trading_day(date: Optional[datetime.date] = None) -> bool:
    """
    Check if a given date is a trading day in the Chinese stock market.
    """
    return not is_holiday(date)
