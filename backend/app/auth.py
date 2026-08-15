import asyncio
import base64
import hashlib
import os
import secrets
from urllib.parse import urlencode, urlparse

from .config import get_settings
from .http import client, raise_for_status

_oauth_endpoints: dict = {}
_discovery_lock = asyncio.Lock()

# Endpoints must live on the configured mail server. Without this check a
# compromised or misconfigured discovery document could redirect the
# authorization code — and the client secret — to an attacker's host.
_REQUIRED_ENDPOINTS = ("authorization_endpoint", "token_endpoint")


def _same_origin(url: str, base: str) -> bool:
    a, b = urlparse(url), urlparse(base)
    return (a.scheme, a.hostname, a.port) == (b.scheme, b.hostname, b.port)


async def _discover_endpoints() -> dict:
    if _oauth_endpoints:
        return _oauth_endpoints

    settings = get_settings()
    async with _discovery_lock:
        if _oauth_endpoints:       # filled in while we waited
            return _oauth_endpoints

        response = await client().get(settings.openid_config_url)
        raise_for_status(response, "OAuth discovery")
        doc = response.json()

        discovered = {}
        for name in _REQUIRED_ENDPOINTS:
            url = doc.get(name)
            if not url or not isinstance(url, str):
                raise ValueError(f"OAuth discovery document is missing {name}")
            if not _same_origin(url, settings.stalwart_url):
                raise ValueError(f"OAuth {name} points outside STALWART_URL: {url}")
            discovered[name] = url

        _oauth_endpoints.update(discovered)
    return _oauth_endpoints


def _pkce_pair() -> tuple[str, str]:
    """Return (code_verifier, code_challenge) using S256."""
    code_verifier = base64.urlsafe_b64encode(os.urandom(32)).rstrip(b"=").decode()
    digest = hashlib.sha256(code_verifier.encode()).digest()
    code_challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode()
    return code_verifier, code_challenge


async def get_authorization_url(session: dict) -> str:
    endpoints = await _discover_endpoints()
    settings = get_settings()

    state = secrets.token_urlsafe(32)
    code_verifier, code_challenge = _pkce_pair()

    session["oauth_state"] = state
    session["code_verifier"] = code_verifier

    params = {
        "response_type": "code",
        "client_id": settings.oauth_client_id,
        "redirect_uri": settings.oauth_redirect_uri,
        "scope": settings.oauth_scope,
        "state": state,
        "code_challenge": code_challenge,
        "code_challenge_method": "S256",
    }
    return f"{endpoints['authorization_endpoint']}?{urlencode(params)}"


async def exchange_code(code: str, state: str, session: dict) -> dict:
    expected_state = session.get("oauth_state")
    # Constant-time compare: the state is a CSRF token, so don't leak its prefix
    # through timing on a byte-by-byte string comparison.
    if not expected_state or not secrets.compare_digest(expected_state, state):
        raise ValueError("Invalid OAuth state parameter")

    endpoints = await _discover_endpoints()
    settings = get_settings()

    code_verifier = session.get("code_verifier")
    if not code_verifier:
        raise ValueError("Missing PKCE code verifier in session")

    # Single-use: whether or not the exchange succeeds, this verifier is spent.
    session.pop("code_verifier", None)
    session.pop("oauth_state", None)

    response = await client().post(
        endpoints["token_endpoint"],
        data={
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": settings.oauth_redirect_uri,
            "client_id": settings.oauth_client_id,
            "client_secret": settings.oauth_client_secret,
            "code_verifier": code_verifier,
        },
    )
    raise_for_status(response, "OAuth token exchange")
    return response.json()


async def refresh_access_token(refresh_token: str) -> dict:
    endpoints = await _discover_endpoints()
    settings = get_settings()

    response = await client().post(
        endpoints["token_endpoint"],
        data={
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": settings.oauth_client_id,
            "client_secret": settings.oauth_client_secret,
        },
    )
    raise_for_status(response, "OAuth token refresh")
    return response.json()
