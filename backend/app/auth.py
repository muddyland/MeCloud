import asyncio
import base64
import hashlib
import os
import secrets
import time
from urllib.parse import urlencode, urlparse

from .config import get_settings
from .http import UpstreamError, client, raise_for_status

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


async def _post_refresh(refresh_token: str) -> dict:
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


# ── Refresh, once per refresh token ─────────────────────────────────────────
#
# A page load fires several API calls at once, and every one of them carries the
# same cookie, hence the same refresh token. Without coordination each call
# refreshes independently; with refresh-token rotation (Stalwart rotates) the
# first one spends the token and every other one fails with invalid_grant —
# which used to log the user out at random, a few minutes before expiry.
#
# So one refresh runs per refresh token, and its result is kept briefly so the
# requests that were already in flight with the old cookie get the same new
# tokens instead of trying to spend the old one again.

_REFRESH_RESULT_TTL = 120          # seconds; long enough for in-flight requests
_refresh_locks: dict[str, asyncio.Lock] = {}
_refresh_results: dict[str, tuple[dict, float]] = {}


class RefreshRejected(Exception):
    """The authorisation server refused the refresh token: sign in again."""


def _rt_key(refresh_token: str) -> str:
    return hashlib.sha256(refresh_token.encode()).hexdigest()


def _prune_refresh_results(now: float) -> None:
    for key, (_, expires) in list(_refresh_results.items()):
        if expires < now:
            _refresh_results.pop(key, None)


async def refresh_access_token(refresh_token: str) -> dict:
    """Exchange a refresh token, at most once however many callers ask.

    Raises :class:`RefreshRejected` when the server says the token is no good
    (an OAuth error response: 400 invalid_grant, 401). Anything else — a
    timeout, a 5xx, a DNS failure — propagates unchanged, because a mail server
    restarting is not a reason to sign the user out.
    """
    key = _rt_key(refresh_token)
    now = time.monotonic()
    _prune_refresh_results(now)
    cached = _refresh_results.get(key)
    if cached:
        return cached[0]

    lock = _refresh_locks.setdefault(key, asyncio.Lock())
    try:
        async with lock:
            cached = _refresh_results.get(key)
            if cached:
                return cached[0]
            try:
                tokens = await _post_refresh(refresh_token)
            except UpstreamError as e:
                if e.status in (400, 401, 403):
                    raise RefreshRejected(str(e)) from e
                raise
            if not isinstance(tokens, dict) or not tokens.get("access_token"):
                raise RefreshRejected("Token endpoint returned no access_token")
            _refresh_results[key] = (tokens, time.monotonic() + _REFRESH_RESULT_TTL)
            return tokens
    finally:
        if not lock.locked():
            _refresh_locks.pop(key, None)


def safe_local_path(value: str | None) -> str | None:
    """A post-login destination that cannot leave this site.

    The value arrives from a query parameter the client chose, so it is
    attacker-controlled in the general case. Anything absolute, scheme-relative
    ("//evil.example"), backslash-smuggled, or not starting with a single "/"
    is refused — an open redirect on a login flow hands attackers a credible
    phishing link on the real domain.
    """
    if not value or not isinstance(value, str):
        return None
    candidate = value.strip()
    if not candidate.startswith("/"):
        return None
    if candidate.startswith("//") or candidate.startswith("/\\"):
        return None
    if "\\" in candidate or "\n" in candidate or "\r" in candidate:
        return None
    # A scheme cannot appear in a path-only URL.
    if urlparse(candidate).scheme or urlparse(candidate).netloc:
        return None
    return candidate
