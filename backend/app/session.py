"""Encrypted cookie sessions.

Starlette's stock ``SessionMiddleware`` *signs* the session cookie but does not
encrypt it: the payload is plain base64 and anyone holding the cookie can read
it. This session holds OAuth access and refresh tokens, so the payload is
sealed with Fernet (AES-128-CBC + HMAC-SHA256) instead. A stolen cookie is
still a stolen session — but it is no longer a stolen *bearer token* that can be
replayed straight against the mail server, and the tokens never appear in
plaintext in browser storage, proxy logs, or a disk image.

The API is deliberately identical to Starlette's: ``request.session`` is a dict.
"""
import base64
import json
import logging
import time
import typing

from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from starlette.datastructures import MutableHeaders
from starlette.requests import HTTPConnection
from starlette.types import ASGIApp, Message, Receive, Scope, Send

logger = logging.getLogger(__name__)

# Browsers drop cookies over ~4 KB. Warn well before we get there so a large
# upstream token doesn't silently log everyone out.
_COOKIE_WARN_BYTES = 3_500


def _derive_key(secret: str) -> bytes:
    """Stretch an arbitrary secret into a Fernet key."""
    raw = HKDF(
        algorithm=hashes.SHA256(),
        length=32,
        salt=None,
        # Domain separation for the HKDF, not branding: this string is an
        # input to the key derivation, so renaming it re-keys every session
        # cookie in existence and signs everyone out. It stays as it is.
        info=b"jmap-webmail.session.v1",
    ).derive(secret.encode())
    return base64.urlsafe_b64encode(raw)


class EncryptedSessionMiddleware:
    def __init__(
        self,
        app: ASGIApp,
        secret_key: str,
        session_cookie: str = "session",
        max_age: int = 60 * 60 * 24 * 7,
        path: str = "/",
        same_site: str = "lax",
        https_only: bool = False,
    ) -> None:
        self.app = app
        self.fernet = Fernet(_derive_key(secret_key))
        self.max_age = max_age
        self.path = path
        self.security_flags = f"httponly; samesite={same_site}"
        if https_only:
            self.security_flags += "; secure"
            # __Host- pins the cookie to this exact origin: no Domain attribute,
            # Path=/, Secure. It cannot be set by a sibling subdomain.
            session_cookie = f"__Host-{session_cookie}"
        self.session_cookie = session_cookie

    def _decode(self, token: str) -> tuple[dict, int | None]:
        """Return (session, issued_at). An unreadable cookie yields an empty session."""
        try:
            raw = token.encode()
            data = json.loads(self.fernet.decrypt(raw, ttl=self.max_age))
            if not isinstance(data, dict):
                return {}, None
            return data, self.fernet.extract_timestamp(raw)
        except (InvalidToken, ValueError, UnicodeDecodeError):
            # Expired, tampered with, or encrypted under a rotated secret.
            return {}, None

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] not in ("http", "websocket"):
            await self.app(scope, receive, send)
            return

        connection = HTTPConnection(scope)
        issued_at: int | None = None

        if self.session_cookie in connection.cookies:
            scope["session"], issued_at = self._decode(connection.cookies[self.session_cookie])
            initial_empty = not scope["session"]
        else:
            scope["session"] = {}
            initial_empty = True

        initial_snapshot = json.dumps(scope["session"], sort_keys=True)

        async def send_wrapper(message: Message) -> None:
            if message["type"] == "http.response.start":
                session = scope["session"]
                if session:
                    changed = json.dumps(session, sort_keys=True) != initial_snapshot
                    # Refresh the cookie once it is past half its life so an
                    # active user never gets logged out mid-session, without
                    # emitting Set-Cookie on every single response.
                    stale = issued_at is not None and (
                        time.time() - issued_at > self.max_age // 2
                    )
                    if changed or stale or issued_at is None:
                        self._set_cookie(message, session)
                elif not initial_empty:
                    self._clear_cookie(message)
            await send(message)

        await self.app(scope, receive, send_wrapper)

    def _set_cookie(self, message: Message, session: dict) -> None:
        token = self.fernet.encrypt(json.dumps(session).encode()).decode()
        if len(token) > _COOKIE_WARN_BYTES:
            logger.warning(
                "Session cookie is %d bytes — approaching the ~4 KB browser limit.", len(token)
            )
        header_value = (
            f"{self.session_cookie}={token}; path={self.path}; "
            f"Max-Age={self.max_age}; {self.security_flags}"
        )
        MutableHeaders(scope=message).append("Set-Cookie", header_value)

    def _clear_cookie(self, message: Message) -> None:
        header_value = (
            f"{self.session_cookie}=null; path={self.path}; "
            f"expires=Thu, 01 Jan 1970 00:00:00 GMT; {self.security_flags}"
        )
        MutableHeaders(scope=message).append("Set-Cookie", header_value)


__all__: typing.Final = ["EncryptedSessionMiddleware"]
