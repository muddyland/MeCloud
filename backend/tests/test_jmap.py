"""Tests for the JMAP session cache and request-envelope validation."""
import pytest
from pydantic import ValidationError

from app import jmap
from app.models import JMAPRequest


@pytest.fixture(autouse=True)
def clean_cache():
    jmap._jmap_session_cache.clear()
    jmap._session_locks.clear()
    yield
    jmap._jmap_session_cache.clear()
    jmap._session_locks.clear()


# ---------------------------------------------------------------------------
# Session cache
# ---------------------------------------------------------------------------

def test_cache_round_trips():
    jmap._cache_set("token-a", {"apiUrl": "http://x/api"})
    assert jmap._cache_get("token-a") == {"apiUrl": "http://x/api"}


def test_cache_is_not_keyed_by_the_raw_token():
    """The bearer token should never sit in memory as a plain dict key."""
    jmap._cache_set("token-a", {"apiUrl": "http://x/api"})
    assert "token-a" not in jmap._jmap_session_cache
    assert len(next(iter(jmap._jmap_session_cache))) == 64   # sha256 hex


def test_cache_entries_expire():
    jmap._cache_set("token-a", {"apiUrl": "http://x/api"})
    # Reach in and backdate the expiry rather than sleeping for ten minutes.
    key = jmap._token_key("token-a")
    session, _ = jmap._jmap_session_cache[key]
    jmap._jmap_session_cache[key] = (session, 0.0)
    assert jmap._cache_get("token-a") is None


def test_cache_evicts_least_recently_used():
    for i in range(jmap._JMAP_CACHE_MAXSIZE):
        jmap._cache_set(f"token-{i}", {"apiUrl": f"http://x/{i}"})

    # Touch the oldest entry so it is no longer the least *recently used*.
    assert jmap._cache_get("token-0") is not None

    jmap._cache_set("token-new", {"apiUrl": "http://x/new"})

    assert len(jmap._jmap_session_cache) == jmap._JMAP_CACHE_MAXSIZE
    assert jmap._cache_get("token-0") is not None      # kept: recently used
    assert jmap._cache_get("token-1") is None          # evicted: oldest untouched


def test_invalidate_session_drops_the_entry():
    jmap._cache_set("token-a", {"apiUrl": "http://x/api"})
    jmap.invalidate_session("token-a")
    assert jmap._cache_get("token-a") is None


# ---------------------------------------------------------------------------
# Request envelope validation
#
# The proxy forwards these to the mail server with the session's credentials
# attached, so the shape is checked rather than trusted.
# ---------------------------------------------------------------------------

def test_valid_envelope_is_accepted():
    req = JMAPRequest(
        using=["urn:ietf:params:jmap:core"],
        methodCalls=[["Mailbox/get", {"accountId": "1"}, "m"]],
    )
    assert len(req.methodCalls) == 1


@pytest.mark.parametrize("bad_call", [
    ["Mailbox/get", {}],                       # too few elements
    ["Mailbox/get", {}, "m", "extra"],         # too many
    [123, {}, "m"],                            # name is not a string
    ["Mailbox/get", "not-an-object", "m"],     # arguments are not an object
    ["Mailbox/get", {}, 5],                    # callId is not a string
    ["", {}, "m"],                             # empty method name
    ["Mailbox/get", {}, ""],                   # empty callId
    "not-a-list",                              # not a call at all
])
def test_malformed_method_calls_are_rejected(bad_call):
    with pytest.raises(ValidationError):
        JMAPRequest(using=["urn:ietf:params:jmap:core"], methodCalls=[bad_call])


def test_empty_method_calls_are_rejected():
    with pytest.raises(ValidationError):
        JMAPRequest(using=["urn:ietf:params:jmap:core"], methodCalls=[])


def test_empty_using_is_rejected():
    with pytest.raises(ValidationError):
        JMAPRequest(using=[], methodCalls=[["Mailbox/get", {}, "m"]])


def test_overlong_capability_urn_is_rejected():
    with pytest.raises(ValidationError):
        JMAPRequest(using=["u" * 300], methodCalls=[["Mailbox/get", {}, "m"]])
