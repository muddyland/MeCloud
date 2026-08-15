"""
Unit tests for the JMAP Mail API.

All upstream HTTP calls (Stalwart JMAP server) are mocked so these tests run
without any external dependencies.
"""
import pytest
from unittest.mock import AsyncMock, patch, MagicMock
from pydantic import ValidationError

from app.models import JMAPRequest
from app.main import app, require_auth


# ---------------------------------------------------------------------------
# Health / config
# ---------------------------------------------------------------------------

async def test_health_returns_ok(client):
    r = await client.get("/health")
    assert r.status_code == 200
    assert r.json() == {"status": "ok"}


async def test_public_config_returns_expected_shape(client):
    r = await client.get("/api/config")
    assert r.status_code == 200
    data = r.json()
    assert "appName" in data
    assert "stalwartUrl" in data
    assert data["appName"] == "Test Mail"


# ---------------------------------------------------------------------------
# Auth — unauthenticated access
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("method,path", [
    ("GET",  "/api/jmap/session"),
    ("POST", "/api/jmap"),
    ("GET",  "/api/jmap/events"),
    ("GET",  "/api/sieve"),
    ("PUT",  "/api/sieve"),
])
async def test_authenticated_endpoints_reject_unauthenticated(client, method, path):
    r = await client.request(method, path, json={} if method == "POST" else None)
    assert r.status_code == 401, f"{method} {path} should return 401"


async def test_me_unauthenticated_returns_false(client):
    r = await client.get("/auth/me")
    assert r.status_code == 200
    assert r.json()["authenticated"] is False


# ---------------------------------------------------------------------------
# Auth — OAuth flow
# ---------------------------------------------------------------------------

async def test_auth_login_redirects_to_authorization_url(client):
    fake_url = "http://auth.test/authorize?response_type=code&state=xyz"
    with patch("app.main.get_authorization_url", new_callable=AsyncMock) as m:
        m.return_value = fake_url
        r = await client.get("/auth/login", follow_redirects=False)
    assert r.status_code in (302, 307)
    assert r.headers["location"] == fake_url


async def test_auth_logout_accepts_post(client):
    r = await client.post("/auth/logout")
    assert r.status_code == 200
    assert r.json() == {"ok": True}


async def test_auth_logout_rejects_get(client):
    # A GET logout can be triggered cross-site by any <img> tag, so it is gone.
    r = await client.get("/auth/logout", follow_redirects=False)
    assert r.status_code in (404, 405)
    assert r.json() != {"ok": True}


# ---------------------------------------------------------------------------
# SPA fallback boundaries
#
# These tests exist because the assertion above once passed for the wrong
# reason: the suite runs with no static directory, so the catch-all route was
# never registered. In the real image it *is* registered, and it answered
# `GET /auth/logout` with 200 + index.html — a GET to a POST-only route is only
# a partial route match, so Starlette kept looking and the catch-all won.
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("path", [
    "api/jmap",
    "api/anything",
    "/api/jmap",
    "auth/logout",
    "auth/callback",
    "/auth/login",
])
def test_server_paths_are_never_served_by_the_spa(path):
    from app.main import is_spa_path
    assert is_spa_path(path) is False


@pytest.mark.parametrize("path", [
    "",
    "calendar",
    "contacts",
    "_app/immutable/entry/start.js",
    "icons/icon-192.png",
    # Not a reserved prefix — just a route that happens to start with the letters.
    "apis-and-things",
    "authors",
])
def test_app_routes_are_served_by_the_spa(path):
    from app.main import is_spa_path
    assert is_spa_path(path) is True


async def test_callback_rejects_invalid_state(client):
    # State mismatch redirects back to login rather than exposing an error page
    r = await client.get("/auth/callback?code=abc123&state=bad-state", follow_redirects=False)
    assert r.status_code in (302, 307)
    assert r.headers["location"] == "/auth/login"


# ---------------------------------------------------------------------------
# JMAP proxy — payload validation
# ---------------------------------------------------------------------------

async def test_jmap_proxy_rejects_missing_fields(auth_client):
    r = await auth_client.post("/api/jmap", json={"using": []})
    assert r.status_code == 422


async def test_jmap_proxy_rejects_too_many_method_calls(auth_client):
    r = await auth_client.post("/api/jmap", json={
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Mailbox/get", {}, "q"]] * 51,  # exceeds limit of 50
    })
    assert r.status_code == 422


async def test_jmap_proxy_rejects_too_many_capabilities(auth_client):
    r = await auth_client.post("/api/jmap", json={
        "using": [f"urn:test:{i}" for i in range(21)],  # exceeds limit of 20
        "methodCalls": [["Mailbox/get", {}, "q"]],
    })
    assert r.status_code == 422


async def test_jmap_proxy_forwards_valid_request(auth_client):
    expected = {"methodResponses": [["Mailbox/get", {"list": []}, "mb"]]}
    with patch("app.main.jmap_request", new_callable=AsyncMock) as m:
        m.return_value = expected
        r = await auth_client.post("/api/jmap", json={
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [["Mailbox/get", {"accountId": "acc1", "ids": None}, "mb"]],
        })
    assert r.status_code == 200
    assert r.json() == expected


# ---------------------------------------------------------------------------
# Sieve
# ---------------------------------------------------------------------------

async def test_sieve_get_returns_script(auth_client):
    mock_script = {"id": "s1", "name": "My Rules", "content": "# empty", "isActive": True}
    with patch("app.main.get_sieve_script", new_callable=AsyncMock) as m:
        m.return_value = mock_script
        r = await auth_client.get("/api/sieve")
    assert r.status_code == 200
    assert r.json()["id"] == "s1"


async def test_sieve_get_returns_503_when_unsupported(auth_client):
    with patch("app.main.get_sieve_script", new_callable=AsyncMock) as m:
        m.return_value = None
        r = await auth_client.get("/api/sieve")
    assert r.status_code == 503


async def test_sieve_put_saves_script(auth_client):
    with patch("app.main.save_sieve_script", new_callable=AsyncMock) as m:
        m.return_value = {"id": "s1", "name": "My Rules"}
        r = await auth_client.put("/api/sieve", json={
            "content": "require []; # empty script",
            "name": "My Rules",
        })
    assert r.status_code == 200
    assert r.json()["id"] == "s1"


async def test_sieve_put_rejects_oversized_content(auth_client):
    r = await auth_client.put("/api/sieve", json={
        "content": "x" * 70_000,  # exceeds 64 KB ceiling
        "name": "Big Script",
    })
    assert r.status_code == 422


async def test_sieve_put_rejects_oversized_name(auth_client):
    r = await auth_client.put("/api/sieve", json={
        "content": "require [];",
        "name": "n" * 300,  # exceeds 255 char limit
    })
    assert r.status_code == 422


# ---------------------------------------------------------------------------
# Pydantic models (unit tests — no HTTP)
# ---------------------------------------------------------------------------

def test_jmap_request_valid():
    req = JMAPRequest(
        using=["urn:ietf:params:jmap:core"],
        methodCalls=[["Mailbox/get", {"accountId": "1"}, "m"]],
    )
    assert len(req.methodCalls) == 1


def test_jmap_request_rejects_too_many_method_calls():
    with pytest.raises(ValidationError):
        JMAPRequest(
            using=["urn:ietf:params:jmap:core"],
            methodCalls=[["x", {}, "q"]] * 51,
        )


def test_jmap_request_rejects_missing_method_calls():
    with pytest.raises(ValidationError):
        JMAPRequest(using=["urn:ietf:params:jmap:core"])  # type: ignore


# ---------------------------------------------------------------------------
# Security headers
# ---------------------------------------------------------------------------

async def test_security_headers_present(client):
    r = await client.get("/health")
    assert r.headers.get("x-content-type-options") == "nosniff"
    assert r.headers.get("x-frame-options") == "DENY"
    assert "content-security-policy" in r.headers
    csp = r.headers["content-security-policy"]
    assert "object-src 'none'" in csp
    assert "frame-src 'none'" in csp
    assert "base-uri 'self'" in csp
    assert "frame-ancestors 'none'" in csp


async def test_api_responses_are_not_cacheable(auth_client):
    """Mail content must never be written to a shared or on-disk cache."""
    r = await auth_client.get("/api/config")
    assert "no-store" in r.headers.get("cache-control", "")


async def test_health_is_not_marked_no_store(client):
    # Only /api and /auth get the no-store treatment; the healthcheck does not.
    r = await client.get("/health")
    assert "no-store" not in r.headers.get("cache-control", "")


# ---------------------------------------------------------------------------
# Request size limits
# ---------------------------------------------------------------------------

async def test_oversized_body_is_rejected(auth_client):
    from app.config import get_settings

    oversized = "x" * (get_settings().max_request_bytes + 1_000)
    r = await auth_client.post("/api/jmap", json={
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Mailbox/get", {"filter": oversized}, "q"]],
    })
    assert r.status_code == 413


# ---------------------------------------------------------------------------
# Upstream error mapping
# ---------------------------------------------------------------------------

async def test_upstream_auth_failure_becomes_401(auth_client):
    """A revoked token must log the user out, not loop them through 502s."""
    from app.http import UpstreamError

    with patch("app.main.jmap_request", new_callable=AsyncMock) as m:
        m.side_effect = UpstreamError("JMAP request failed with HTTP 401", 401)
        r = await auth_client.post("/api/jmap", json={
            "using": ["urn:ietf:params:jmap:core"],
            "methodCalls": [["Mailbox/get", {}, "q"]],
        })
    assert r.status_code == 401


async def test_upstream_server_error_becomes_502(auth_client):
    from app.http import UpstreamError

    with patch("app.main.jmap_request", new_callable=AsyncMock) as m:
        m.side_effect = UpstreamError("JMAP request failed with HTTP 500", 500)
        r = await auth_client.post("/api/jmap", json={
            "using": ["urn:ietf:params:jmap:core"],
            "methodCalls": [["Mailbox/get", {}, "q"]],
        })
    assert r.status_code == 502


async def test_upstream_timeout_becomes_504(auth_client):
    import httpx

    with patch("app.main.jmap_request", new_callable=AsyncMock) as m:
        m.side_effect = httpx.ConnectTimeout("timed out")
        r = await auth_client.post("/api/jmap", json={
            "using": ["urn:ietf:params:jmap:core"],
            "methodCalls": [["Mailbox/get", {}, "q"]],
        })
    assert r.status_code == 504


# ---------------------------------------------------------------------------
# Unknown API paths
# ---------------------------------------------------------------------------

async def test_unknown_api_path_returns_json_404(client):
    r = await client.get("/api/does-not-exist")
    assert r.status_code == 404
    assert r.json()["detail"] == "Not found"
