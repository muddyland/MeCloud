"""Serving the desktop client.

The binary and the source archive are built into the image (see the Dockerfile's
`desktop-builder` stage) and served from a fixed directory. Two things matter
here: only signed-in users may download, and the download itself has to work
outside the app's own fetch — a browser download, an `Invoke-WebRequest`, a
`curl` on a headless box — where the session cookie is not necessarily present
and definitely should not be relied upon.

So the session buys a **short-lived, signed URL** rather than the bytes. The
session is checked once, at the moment the token is minted; the download link
then stands on its own for a few minutes.

Why a signed token rather than a database of one-time links: this has no state
to lose, nothing to clean up, and nothing to go stale if the app is restarted or
runs as more than one process. The cost is that a token cannot be revoked before
it expires, which is why the lifetime is minutes rather than hours and why the
token names exactly one artefact — a leaked link buys a copy of a public-ish
binary for five minutes and nothing else.
"""
from __future__ import annotations

import base64
import hashlib
import hmac
import os
import time
from dataclasses import dataclass
from pathlib import Path

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

# How long a minted link stays good. Long enough to click, start a slow
# download, or paste into a terminal on another machine; short enough that a
# link left in a shell history is worthless.
TOKEN_TTL_SECONDS = 300

# Where the Dockerfile leaves what it built.
DOWNLOAD_DIR = Path(os.environ.get("DESKTOP_DOWNLOAD_DIR", "/app/downloads"))


@dataclass(frozen=True)
class Artifact:
    """One downloadable file."""
    key: str            # stable id used in URLs and tokens
    filename: str       # what the file is called on disk and on the way out
    label: str          # shown in the UI
    description: str
    media_type: str
    platform: str       # "linux" | "source"


# Deliberately a fixed list rather than a directory listing: the key that
# reaches the filesystem is chosen from this table, so a crafted key cannot
# reach a path that was never meant to be downloadable.
ARTIFACTS: tuple[Artifact, ...] = (
    Artifact(
        key="linux-x86_64",
        filename="mecloud-desktop-linux-x86_64.tar.gz",
        label="Linux (x86-64)",
        description="The client, its icon and a desktop entry. Extract and run install.sh.",
        media_type="application/gzip",
        platform="linux",
    ),
    Artifact(
        key="source",
        filename="mecloud-desktop-source.zip",
        label="Source code",
        description="Build it yourself — required for Windows and macOS.",
        media_type="application/zip",
        platform="source",
    ),
)

_BY_KEY = {a.key: a for a in ARTIFACTS}


def artifact(key: str) -> Artifact | None:
    return _BY_KEY.get(key)


def path_for(art: Artifact) -> Path:
    return DOWNLOAD_DIR / art.filename


def available() -> list[dict]:
    """The artefacts this build actually shipped, with their sizes.

    A build that skipped the desktop stage has none, and saying so beats
    offering a link that 404s.
    """
    out = []
    for art in ARTIFACTS:
        path = path_for(art)
        if not path.is_file():
            continue
        out.append({
            "key": art.key,
            "label": art.label,
            "description": art.description,
            "platform": art.platform,
            "filename": art.filename,
            "size": path.stat().st_size,
        })
    return out


def _key(secret: str) -> bytes:
    """A signing key derived from SESSION_SECRET.

    Separate `info` from the session cookie's own derivation, so the two keys
    are unrelated: a download token can never be mistaken for, or turned into,
    a session.
    """
    return HKDF(
        algorithm=hashes.SHA256(),
        length=32,
        salt=None,
        info=b"mecloud.download.v1",
    ).derive(secret.encode())


def _b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


def _unb64(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def mint(secret: str, artifact_key: str, *, now: float | None = None) -> str:
    """Sign a token good for one artefact, for TOKEN_TTL_SECONDS."""
    expires = int((now if now is not None else time.time()) + TOKEN_TTL_SECONDS)
    payload = f"{artifact_key}:{expires}".encode()
    signature = hmac.new(_key(secret), payload, hashlib.sha256).digest()
    return f"{_b64(payload)}.{_b64(signature)}"


def verify(secret: str, token: str, artifact_key: str, *, now: float | None = None) -> bool:
    """Whether `token` authorises `artifact_key` right now.

    Every failure returns False rather than raising: the caller has exactly one
    thing to do about any of them, and distinguishing "malformed" from "expired"
    in a response only helps someone probing.
    """
    if not token or "." not in token:
        return False
    encoded_payload, _, encoded_signature = token.partition(".")
    try:
        payload = _unb64(encoded_payload)
        signature = _unb64(encoded_signature)
    except Exception:
        return False

    expected = hmac.new(_key(secret), payload, hashlib.sha256).digest()
    # Constant time: the comparison is against a value the caller supplied.
    if not hmac.compare_digest(signature, expected):
        return False

    try:
        key, _, expires = payload.decode().rpartition(":")
        expires_at = int(expires)
    except Exception:
        return False

    if key != artifact_key:
        return False
    return (now if now is not None else time.time()) < expires_at
