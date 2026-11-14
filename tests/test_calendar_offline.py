"""Offline, deterministic tests for mitdx.calendar (findings F8 / F16-calendar).

No network access and no real user cache: every test stubs
``mitdx.calendar._fetch_holiday_data``, points CACHE_DIR/CACHE_FILE at
``tmp_path``, and starts with the in-process ``_CN_HOLIDAYS`` cache reset.
"""
import datetime
import logging
import os
import time
from unittest.mock import Mock

import pytest

from mitdx import calendar

# Deterministic reference dates (weekday assumptions pinned by a test below).
NEW_YEAR = datetime.date(2026, 1, 1)        # Thursday, statutory holiday
SATURDAY = datetime.date(2026, 1, 3)        # weekend
NORMAL_MONDAY = datetime.date(2026, 4, 20)  # ordinary trading day
LABOR_DAY = datetime.date(2026, 5, 1)       # Friday, statutory holiday

SYNTHETIC_DATA = "\n".join([
    "20260101|元旦|中国",
    "20260501|劳动节|中国",
    "20261225|Christmas|美国",  # foreign holiday: must be ignored
    "20260601|儿童节",           # too few fields: must be ignored
    "2026010|短日期|中国",       # malformed date: must be ignored
    "garbage",                   # junk line: must be ignored
])


@pytest.fixture(autouse=True)
def offline_calendar(tmp_path, monkeypatch):
    """Isolate cache paths + in-process state; forbid real network by default."""
    monkeypatch.setattr(calendar, "CACHE_DIR", tmp_path / ".mitdx")
    monkeypatch.setattr(calendar, "CACHE_FILE", tmp_path / ".mitdx" / "holiday.csv")
    monkeypatch.setattr(calendar, "_CN_HOLIDAYS", None)
    monkeypatch.setattr(calendar, "_last_failed_fetch", None)

    def _no_network():
        raise AssertionError("unexpected network fetch in offline test")

    monkeypatch.setattr(calendar, "_fetch_holiday_data", _no_network)
    # No teardown needed: monkeypatch restores all attributes automatically.


def _write_cache(text, age_days=0.0):
    """Create the file cache with a controlled mtime (age_days=0 -> fresh)."""
    calendar.CACHE_DIR.mkdir(parents=True, exist_ok=True)
    calendar.CACHE_FILE.write_text(text, encoding="utf-8")
    if age_days:
        stamp = time.time() - age_days * 24 * 3600
        os.utime(calendar.CACHE_FILE, (stamp, stamp))


def test_reference_dates_weekdays():
    # Guard the weekday assumptions the other tests rely on.
    assert NEW_YEAR.weekday() == 3     # Thursday
    assert SATURDAY.weekday() == 5     # Saturday
    assert NORMAL_MONDAY.weekday() == 0  # Monday
    assert LABOR_DAY.weekday() == 4    # Friday


def test_fetch_failure_no_cache_weekend_only_and_retries(monkeypatch, caplog):
    """F8 regression: fetch failure without cache must not poison the process."""
    fetch = Mock(side_effect=OSError("simulated network failure"))
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)
    # Disable the retry cooldown so this test exercises immediate retries.
    monkeypatch.setattr(calendar, "FETCH_RETRY_COOLDOWN_SECONDS", 0)

    with caplog.at_level(logging.WARNING, logger="mitdx.calendar"):
        # Without holiday data only weekends can be detected (bool API kept).
        assert calendar.is_holiday(NEW_YEAR) is False
        assert fetch.call_count == 1
        assert calendar.is_trading_day(NEW_YEAR) is True
        assert calendar.is_holiday(SATURDAY) is True
        assert calendar.is_trading_day(SATURDAY) is False
        # Weekend checks short-circuit before any fetch; the second weekday
        # call already retried (nothing was cached after the failure).
        assert fetch.call_count == 2

        # F8 regression: the empty result must NOT be cached — the next call
        # retries the fetch instead of silently staying weekend-only for the
        # whole process lifetime.
        assert calendar.is_holiday(LABOR_DAY) is False
        assert fetch.call_count >= 2
        assert calendar._CN_HOLIDAYS is None

    # Operating without holiday data must be surfaced via logger.warning.
    assert any(
        r.levelno == logging.WARNING and "weekend-only" in r.getMessage()
        for r in caplog.records
    )


def test_empty_fetch_response_not_cached(monkeypatch):
    """F8 variant: a successful fetch returning '' must not be cached either."""
    fetch = Mock(return_value="")
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)
    monkeypatch.setattr(calendar, "FETCH_RETRY_COOLDOWN_SECONDS", 0)

    assert calendar.is_holiday(NEW_YEAR) is False
    assert calendar.is_holiday(LABOR_DAY) is False  # weekday call -> retry
    assert fetch.call_count == 2  # retried, not poisoned by an empty set
    assert calendar._CN_HOLIDAYS is None
    # Weekends are still detected without data, and short-circuit the fetch.
    assert calendar.is_holiday(SATURDAY) is True
    assert fetch.call_count == 2


def test_retry_after_failure_recovers(monkeypatch):
    """Once the network recovers, the retry actually loads holiday data."""
    failing = Mock(side_effect=OSError("simulated network failure"))
    monkeypatch.setattr(calendar, "_fetch_holiday_data", failing)
    monkeypatch.setattr(calendar, "FETCH_RETRY_COOLDOWN_SECONDS", 0)
    assert calendar.is_holiday(NEW_YEAR) is False
    assert calendar._CN_HOLIDAYS is None

    monkeypatch.setattr(
        calendar, "_fetch_holiday_data", Mock(return_value="20260101|元旦|中国\n")
    )
    assert calendar.is_holiday(NEW_YEAR) is True
    assert calendar.is_trading_day(NEW_YEAR) is False


def test_fetch_success_holidays_recognized_and_cached_in_process(monkeypatch):
    fetch = Mock(return_value=SYNTHETIC_DATA)
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)

    assert calendar.is_holiday(NEW_YEAR) is True
    assert calendar.is_trading_day(NEW_YEAR) is False
    assert calendar.is_holiday(LABOR_DAY) is True
    assert calendar.is_trading_day(LABOR_DAY) is False
    assert calendar.is_holiday(NORMAL_MONDAY) is False
    assert calendar.is_trading_day(NORMAL_MONDAY) is True
    # Foreign / malformed lines are filtered out.
    assert calendar.is_holiday(datetime.date(2026, 12, 25)) is False
    assert calendar.is_holiday(datetime.date(2026, 6, 1)) is False

    # In-process cache: subsequent calls do not refetch.
    assert fetch.call_count == 1
    assert calendar._CN_HOLIDAYS == {"2026-01-01", "2026-05-01"}
    # Data persisted to the file cache.
    assert calendar.CACHE_FILE.read_text(encoding="utf-8") == SYNTHETIC_DATA


def test_stale_cache_triggers_refetch(monkeypatch):
    _write_cache("20260101|元旦|中国\n", age_days=8)  # older than 7 days
    fetch = Mock(return_value="20260501|劳动节|中国\n")
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)

    assert calendar.is_holiday(LABOR_DAY) is True
    assert fetch.call_count == 1
    # Fresh data replaces the stale cache (not merged with it).
    assert calendar.is_holiday(NEW_YEAR) is False
    assert calendar.CACHE_FILE.read_text(encoding="utf-8") == "20260501|劳动节|中国\n"


def test_fresh_cache_used_without_network(monkeypatch):
    _write_cache("20260101|元旦|中国\n")  # mtime = now -> fresh
    fetch = Mock(return_value="")
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)

    assert calendar.is_holiday(NEW_YEAR) is True
    assert calendar.is_trading_day(NEW_YEAR) is False
    assert calendar.is_holiday(NORMAL_MONDAY) is False
    assert fetch.call_count == 0


def test_network_failure_with_old_cache_falls_back(monkeypatch):
    _write_cache("20260101|元旦|中国\n", age_days=8)  # stale, but usable
    fetch = Mock(side_effect=OSError("simulated network failure"))
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)

    assert calendar.is_holiday(NEW_YEAR) is True
    assert calendar.is_trading_day(NEW_YEAR) is False
    assert fetch.call_count == 1
    # The old-cache fallback is cached in-process: no further fetch attempts.
    assert calendar.is_holiday(NEW_YEAR) is True
    assert fetch.call_count == 1


def test_cooldown_throttles_retries(monkeypatch):
    """Final-review fix: while data is unavailable, retries are throttled."""
    fetch = Mock(side_effect=OSError("simulated network failure"))
    monkeypatch.setattr(calendar, "_fetch_holiday_data", fetch)

    assert calendar.is_holiday(NEW_YEAR) is False
    assert calendar.is_holiday(LABOR_DAY) is False  # within cooldown: no refetch
    assert fetch.call_count == 1
    assert calendar._CN_HOLIDAYS is None

    # Once the cooldown elapses, the next weekday call retries (F8 intact).
    base = time.monotonic()
    monkeypatch.setattr(
        calendar.time, "monotonic",
        lambda: base + calendar.FETCH_RETRY_COOLDOWN_SECONDS + 1,
    )
    assert calendar.is_holiday(NEW_YEAR) is False
    assert fetch.call_count == 2