import base64
import hashlib
import os
import secrets
from urllib.parse import urlencode

import httpx

from .config import get_settings

_oauth_endpoints: dict = {}


async def _discover_endpoints() -> dict:
    if _oauth_endpoints:
        return _oauth_endpoints

    settings = get_settings()
    async with httpx.AsyncClient() as client:
        response = await client.get(settings.openid_config_url)
        response.raise_for_status()
        doc = response.json()

    _oauth_endpoints["authorization_endpoint"] = doc["authorization_endpoint"]
    _oauth_endpoints["token_endpoint"] = doc["token_endpoint"]
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
    endpoints = await _discover_endpoints()
    settings = get_settings()

    if session.get("oauth_state") != state:
        raise ValueError("Invalid OAuth state parameter")

    code_verifier = session.get("code_verifier")
    if not code_verifier:
        raise ValueError("Missing PKCE code verifier in session")

    async with httpx.AsyncClient() as client:
        response = await client.post(
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
        response.raise_for_status()
        return response.json()


async def refresh_access_token(refresh_token: str) -> dict:
    endpoints = await _discover_endpoints()
    settings = get_settings()

    async with httpx.AsyncClient() as client:
        response = await client.post(
            endpoints["token_endpoint"],
            data={
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": settings.oauth_client_id,
                "client_secret": settings.oauth_client_secret,
            },
        )
        response.raise_for_status()
        return response.json()
