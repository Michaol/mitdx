"""mitdx v1.1.0 — Full API Integration Test Suite"""
import pytest
from mitdx.quotes import Quotes


@pytest.fixture(scope="module")
def quotes_client():
    q = Quotes()
    q._ensure_connected()
    yield q
    q.disconnect()


@pytest.mark.network
def test_bars(quotes_client):
    df = quotes_client.bars(symbol="600036", frequency=9, count=5)
    assert not df.empty
    assert len(df) == 5


@pytest.mark.network
def test_minute(quotes_client):
    df = quotes_client.minute(symbol="600036", frequency=0, count=5)
    assert not df.empty


@pytest.mark.network
def test_index(quotes_client):
    df = quotes_client.index(symbol="999999", frequency=9, count=5)
    assert not df.empty


@pytest.mark.network
def test_xdxr(quotes_client):
    df = quotes_client.xdxr(symbol="600036")
    assert not df.empty


@pytest.mark.network
def test_transactions(quotes_client):
    df = quotes_client.transactions(symbol="600036", start=0, count=10)
    assert not df.empty


@pytest.mark.network
def test_quotes(quotes_client):
    df = quotes_client.quotes(symbols=["600036", "000001"])
    assert not df.empty
    assert "price" in df.columns


@pytest.mark.network
def test_f10(quotes_client):
    result = quotes_client.f10(symbol="600036")
    assert "category" in result
    assert "content" in result
    assert len(result["category"]) > 0


@pytest.mark.network
def test_finance(quotes_client):
    df = quotes_client.finance(symbol="000001")
    assert not df.empty
