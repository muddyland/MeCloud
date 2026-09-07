"""Credentials for the desktop client's background work.

The embedded web UI signs in through the ordinary session cookie and needs
nothing from this module. The **sync engine** is different: it runs when no
window is open, so it needs a credential of its own.

The shape mirrors the session cookie rather than inventing a second scheme.
`session.py` hands the browser an encrypted blob holding the OAuth tokens and
trusts nothing the browser sends back until it decrypts; a device token is the
same idea with a longer life and its own key. Nothing new is stored on the
server, which matters because this app deliberately has no database — a device
registry would be the first thing to need one, to back up, and to leak.

What that costs: an individual token cannot be revoked from here. Revocation is
the mail server's job, where it belongs — dropping the OAuth refresh token, or
the app password behind it, stops the device. Rotating SESSION_SECRET stops all
of them, and every browser session with it.
"""
from __future__ import annotations

import json
import time
from dataclasses import dataclass

from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
import base64

# A paired device is expected to keep working for a long time; the OAuth
# refresh token inside is the real limit, and the mail server governs that.
DEVICE_TOKEN_MAX_AGE = 60 * 60 * 24 * 365

# How long a pairing request stays open once the client has asked for it. The
# user has to read a consent screen and click, not make a cup of tea.
PAIRING_MAX_AGE = 300


@dataclass(frozen=True)
class Device:
    """What a decrypted device token tells us."""
    refresh_token: str
    username: str
    name: str
    issued_at: int

    @property
    def age(self) -> int:
        return int(time.time()) - self.issued_at


def _fernet(secret: str) -> Fernet:
    """A key derived from SESSION_SECRET, separated from every other use.

    The `info` string is what keeps this key unrelated to the session cookie's
    and the download token's. A device token must never decrypt as a session,
    and a session must never be usable as a device.
    """
    raw = HKDF(
        algorithm=hashes.SHA256(),
        length=32,
        salt=None,
        info=b"mecloud.device.v1",
    ).derive(secret.encode())
    return Fernet(base64.urlsafe_b64encode(raw))


def issue(secret: str, *, refresh_token: str, username: str, name: str) -> str:
    """Mint a token for one paired device."""
    payload = json.dumps({
        "rt": refresh_token,
        "u": username,
        "n": name[:120],
        "iat": int(time.time()),
    }).encode()
    return _fernet(secret).encrypt(payload).decode()


def read(secret: str, token: str, *, max_age: int = DEVICE_TOKEN_MAX_AGE) -> Device | None:
    """Decrypt a device token, or None if it is not one we issued.

    Every failure — wrong key, tampering, expiry, malformed contents — returns
    None. The caller has one response to all of them, and telling them apart
    only helps someone probing.
    """
    if not token:
        return None
    try:
        raw = _fernet(secret).decrypt(token.encode(), ttl=max_age)
        data = json.loads(raw)
        return Device(
            refresh_token=str(data["rt"]),
            username=str(data.get("u", "")),
            name=str(data.get("n", "")),
            issued_at=int(data.get("iat", 0)),
        )
    except (InvalidToken, ValueError, KeyError, TypeError):
        return None


def bearer_token(authorization: str | None) -> str | None:
    """Pull the token out of an Authorization header, if it is a bearer one."""
    if not authorization:
        return None
    scheme, _, value = authorization.partition(" ")
    if scheme.lower() != "bearer" or not value.strip():
        return None
    return value.strip()


# ── Pairing handshake ───────────────────────────────────────────────────────
#
# The client cannot simply ask for a token: anything running on the machine
# could. So it opens a page in the user's signed-in session, which asks them to
# approve, and the answer goes back to a loopback address the client is already
# listening on.

def _pairing_fernet(secret: str) -> Fernet:
    raw = HKDF(
        algorithm=hashes.SHA256(),
        length=32,
        salt=None,
        info=b"mecloud.pairing.v1",
    ).derive(secret.encode())
    return Fernet(base64.urlsafe_b64encode(raw))


def seal_pairing(secret: str, *, redirect: str, name: str) -> str:
    """Wrap a pairing request so the consent page cannot be pointed elsewhere.

    The redirect travels through the browser, and an unsigned one would let any
    page hand the user a consent screen that posts the token to a host of its
    choosing. Sealing it means the value the approval acts on is the value this
    server produced after checking it.
    """
    payload = json.dumps({"r": redirect, "n": name[:120]}).encode()
    return _pairing_fernet(secret).encrypt(payload).decode()


def open_pairing(secret: str, sealed: str) -> tuple[str, str] | None:
    """Unwrap a pairing request. Returns (redirect, name) or None."""
    try:
        raw = _pairing_fernet(secret).decrypt(sealed.encode(), ttl=PAIRING_MAX_AGE)
        data = json.loads(raw)
        return str(data["r"]), str(data.get("n", ""))
    except (InvalidToken, ValueError, KeyError, TypeError):
        return None


def is_loopback_redirect(url: str) -> bool:
    """Whether a pairing redirect is somewhere only this machine can receive.

    The token is handed to whatever answers this URL, so it must be a port on
    the user's own computer. Anything else — a hostname that resolves off-box,
    a scheme that is not plain HTTP, a name that merely *looks* local — is
    refused. `localhost` is excluded deliberately: it is a name, and a name can
    be made to resolve anywhere.
    """
    from urllib.parse import urlparse

    parsed = urlparse(url)
    if parsed.scheme != "http":
        return False
    if parsed.hostname not in ("127.0.0.1", "::1"):
        return False
    if parsed.port is None or not (1024 <= parsed.port <= 65535):
        return False
    return True
