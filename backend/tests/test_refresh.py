"""Token refresh: once per refresh token, and never a sign-out for a hiccup."""
import asyncio
import time

import httpx
import pytest
from fastapi import HTTPException

from app import auth, main
from app.http import UpstreamError


@pytest.fixture(autouse=True)
def _clean_refresh_state():
    auth._refresh_locks.clear()
    auth._refresh_results.clear()
    yield
    auth._refresh_locks.clear()
    auth._refresh_results.clear()


class _Req:
    """Just enough of a Request for require_auth."""

    def __init__(self, session):
        self.session = session
        self.headers = {}


def _expiring_session(**extra):
    return {"access_token": "old", "refresh_token": "rt-1",
            "token_expires_at": time.time() + 60, **extra}


async def test_concurrent_callers_share_one_refresh(monkeypatch):
    calls = 0

    async def fake_post(rt):
        nonlocal calls
        calls += 1
        await asyncio.sleep(0.01)
        return {"access_token": f"new-{calls}", "refresh_token": "rt-2", "expires_in": 3600}

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    results = await asyncio.gather(*(auth.refresh_access_token("rt-1") for _ in range(10)))
    assert calls == 1
    assert {r["access_token"] for r in results} == {"new-1"}


async def test_a_late_request_with_the_old_cookie_reuses_the_result(monkeypatch):
    calls = 0

    async def fake_post(rt):
        nonlocal calls
        calls += 1
        if calls > 1:
            # A rotating server refuses a spent refresh token.
            raise UpstreamError("OAuth token refresh failed with HTTP 400", 400)
        return {"access_token": "new", "refresh_token": "rt-2", "expires_in": 3600}

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    first = await auth.refresh_access_token("rt-1")
    second = await auth.refresh_access_token("rt-1")
    assert first == second and calls == 1


@pytest.mark.parametrize("status", [400, 401])
async def test_a_rejected_refresh_token_signs_out(monkeypatch, status):
    async def fake_post(rt):
        raise UpstreamError("nope", status)

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    session = _expiring_session()
    with pytest.raises(HTTPException) as exc:
        await main.require_auth(_Req(session))
    assert exc.value.status_code == 401
    assert session == {}


@pytest.mark.parametrize("error", [
    UpstreamError("Upstream down", 502),
    httpx.ConnectTimeout("timed out"),
])
async def test_a_transient_failure_keeps_the_session_and_the_live_token(monkeypatch, error):
    async def fake_post(rt):
        raise error

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    session = _expiring_session()
    token = await main.require_auth(_Req(session))
    assert token == "old"
    assert session["refresh_token"] == "rt-1"


async def test_a_transient_failure_after_expiry_is_503_not_a_sign_out(monkeypatch):
    async def fake_post(rt):
        raise UpstreamError("Upstream down", 502)

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    session = _expiring_session(token_expires_at=time.time() - 10)
    with pytest.raises(HTTPException) as exc:
        await main.require_auth(_Req(session))
    assert exc.value.status_code == 503
    assert session["refresh_token"] == "rt-1"


async def test_a_successful_refresh_updates_the_session(monkeypatch):
    async def fake_post(rt):
        return {"access_token": "new", "refresh_token": "rt-2", "expires_in": 3600}

    monkeypatch.setattr(auth, "_post_refresh", fake_post)
    session = _expiring_session()
    assert await main.require_auth(_Req(session)) == "new"
    assert session["refresh_token"] == "rt-2"
    assert session["token_expires_at"] > time.time() + 3000
