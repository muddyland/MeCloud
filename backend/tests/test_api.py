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


async def test_auth_logout_clears_session_and_redirects(client):
    r = await client.get("/auth/logout", follow_redirects=False)
    assert r.status_code in (302, 307)
    assert r.headers["location"] == "/"


async def test_callback_rejects_invalid_state(client):
    # Provide a state that doesn't match anything in the session
    r = await client.get("/auth/callback?code=abc123&state=bad-state")
    assert r.status_code == 400


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
