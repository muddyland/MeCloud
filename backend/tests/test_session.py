"""Tests for the encrypted session cookie.

The point of this middleware is that the OAuth tokens it carries are not
readable from the cookie, so that is what these assert.
"""
import json

import pytest
from httpx import ASGITransport, AsyncClient
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from app.session import EncryptedSessionMiddleware

SECRET = "unit-test-secret-that-is-long-enough-to-be-realistic"
TOKEN = "super-secret-access-token-value"


def _build_app(**kwargs):
    async def write(request):
        request.session["access_token"] = TOKEN
        return JSONResponse({"ok": True})

    async def read(request):
        return JSONResponse({"token": request.session.get("access_token")})

    async def clear(request):
        request.session.clear()
        return JSONResponse({"ok": True})

    app = Starlette(routes=[
        Route("/write", write),
        Route("/read", read),
        Route("/clear", clear),
    ])
    app.add_middleware(EncryptedSessionMiddleware, secret_key=SECRET, **kwargs)
    return app


@pytest.fixture
async def session_client():
    async with AsyncClient(
        transport=ASGITransport(app=_build_app()), base_url="http://test"
    ) as c:
        yield c


async def test_session_round_trips(session_client):
    await session_client.get("/write")
    r = await session_client.get("/read")
    assert r.json()["token"] == TOKEN


async def test_cookie_does_not_leak_the_token(session_client):
    r = await session_client.get("/write")
    cookie = r.headers["set-cookie"]
    # The whole point: the bearer token must not be recoverable by eye, by
    # base64, or by JSON-decoding the cookie the way a signed-only session allows.
    assert TOKEN not in cookie
    assert "access_token" not in cookie
    with pytest.raises(Exception):
        json.loads(cookie.split("=", 1)[1].split(";", 1)[0])


async def test_tampered_cookie_yields_an_empty_session(session_client):
    await session_client.get("/write")
    name = next(iter(session_client.cookies.keys()))
    session_client.cookies.set(name, "not-a-valid-fernet-token")
    r = await session_client.get("/read")
    assert r.json()["token"] is None


async def test_cookie_from_a_different_secret_is_rejected():
    """Rotating SESSION_SECRET must invalidate every outstanding session."""
    async with AsyncClient(
        transport=ASGITransport(app=_build_app()), base_url="http://test"
    ) as first:
        r = await first.get("/write")
        cookie = r.cookies

    other = _build_app()
    other.user_middleware.clear()
    other.middleware_stack = None
    other.add_middleware(EncryptedSessionMiddleware, secret_key="a-completely-different-secret")

    async with AsyncClient(
        transport=ASGITransport(app=other), base_url="http://test", cookies=cookie
    ) as second:
        r = await second.get("/read")
    assert r.json()["token"] is None


async def test_clearing_the_session_expires_the_cookie(session_client):
    await session_client.get("/write")
    r = await session_client.get("/clear")
    assert "expires=Thu, 01 Jan 1970" in r.headers["set-cookie"]


async def test_https_only_sets_secure_and_host_prefix():
    app = _build_app(https_only=True)
    async with AsyncClient(transport=ASGITransport(app=app), base_url="https://test") as c:
        r = await c.get("/write")
    cookie = r.headers["set-cookie"]
    assert cookie.startswith("__Host-session=")
    assert "secure" in cookie.lower()
    assert "httponly" in cookie.lower()


async def test_no_cookie_is_set_when_nothing_is_stored():
    async def noop(request):
        return JSONResponse({"ok": True})

    app = Starlette(routes=[Route("/", noop)])
    app.add_middleware(EncryptedSessionMiddleware, secret_key=SECRET)
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as c:
        r = await c.get("/")
    assert "set-cookie" not in r.headers
