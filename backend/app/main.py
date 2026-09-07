import html
import logging
import mimetypes
import os
import time

mimetypes.add_type('application/manifest+json', '.webmanifest')
from contextlib import asynccontextmanager
from urllib.parse import parse_qs, quote

import httpx
from fastapi import FastAPI, Request, HTTPException, Depends
from pydantic import BaseModel, Field
from fastapi.responses import (
    JSONResponse, RedirectResponse, StreamingResponse, FileResponse, HTMLResponse,
)
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.middleware.trustedhost import TrustedHostMiddleware
from starlette.types import ASGIApp, Message, Receive, Scope, Send
from slowapi import Limiter
from slowapi.util import get_remote_address
from slowapi.errors import RateLimitExceeded

from . import http as upstream
from .config import get_settings
from .auth import (
    get_authorization_url, exchange_code, refresh_access_token, _discover_endpoints,
    safe_local_path,
)
from .http import UpstreamError
from .jmap import (
    get_jmap_session, jmap_request, jmap_event_stream,
    get_sieve_script, save_sieve_script, invalidate_session,
)
from .blobs import (
    BlobStream, blob_csp, content_disposition, open_blob, safe_content_type,
    upload_blob,
)
from .models import JMAPRequest
from . import downloads, devices
from .session import EncryptedSessionMiddleware

logger = logging.getLogger(__name__)

settings = get_settings()


# 'unsafe-inline' in script-src is required by SvelteKit's hydration bootstrap,
# which is emitted as an inline <script>. It is not the app's XSS boundary:
# untrusted email HTML is rendered in a sandboxed iframe with no allow-scripts,
# so it cannot execute script at all regardless of this directive. Removing it
# means switching SvelteKit to `csp: { mode: 'hash' }` in svelte.config.js.
_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline'; "
    "style-src 'self' 'unsafe-inline'; "
    "img-src 'self' data: blob: https:; "    # emails may contain external images
    "font-src 'self' data:; "
    "connect-src 'self'; "
    # 'self', not 'none': the file preview frames same-origin blob responses.
    # Cross-origin framing stays impossible, and the framed response carries its
    # own restrictive policy plus nosniff (see blobs.blob_csp).
    "frame-src 'self'; "
    "worker-src 'self'; "                    # service worker
    "manifest-src 'self'; "
    "media-src 'self' data:; "
    "object-src 'none'; "
    "base-uri 'self'; "
    "form-action 'self';"
)


def _csp() -> str:
    """The app policy, with the configurable framing directive appended."""
    return f"{_CSP} frame-ancestors {settings.frame_ancestors_value};"


class SecurityHeadersMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request, call_next):
        response = await call_next(request)
        response.headers["X-Content-Type-Options"]  = "nosniff"
        xfo = settings.x_frame_options
        if xfo:
            response.headers["X-Frame-Options"] = xfo
        response.headers["Referrer-Policy"]          = "strict-origin-when-cross-origin"
        response.headers["Permissions-Policy"]       = "camera=(), microphone=(), geolocation=()"
        # setdefault, not assignment: the blob download route sets its own
        # `sandbox` policy to neutralise user-supplied content, and this
        # middleware must not overwrite it with the permissive app policy.
        response.headers.setdefault("Content-Security-Policy", _csp())
        response.headers["Cross-Origin-Opener-Policy"]   = "same-origin"
        response.headers["Cross-Origin-Resource-Policy"] = "same-origin"

        path = request.url.path
        if path.startswith("/api/") or path.startswith("/auth/"):
            # Mail content and auth responses must never land in a shared or
            # on-disk cache, and must not be replayed from history after logout.
            response.headers["Cache-Control"] = "no-store, no-cache, must-revalidate, private"
            response.headers["Pragma"]        = "no-cache"

        if settings.is_production:
            response.headers["Strict-Transport-Security"] = "max-age=63072000; includeSubDomains; preload"
        return response


# Declared here because the body-size middleware registration below needs it.
UPLOAD_PATH = "/api/files/blob"

_BODY_METHODS = frozenset({"POST", "PUT", "PATCH"})


class BodySizeLimitMiddleware:
    """Reject request bodies larger than ``max_request_bytes``.

    A declared Content-Length is checked up front and the request then streams
    through untouched. A body-bearing request that declines to declare its
    length (chunked encoding) is buffered up to the limit instead, so it cannot
    simply omit the header to bypass the check.
    """

    def __init__(self, app: ASGIApp, max_bytes: int, upload_max_bytes: int,
                 upload_paths: tuple[str, ...] = ()) -> None:
        self.app = app
        self.max_bytes = max_bytes
        self.upload_max_bytes = upload_max_bytes
        self.upload_paths = upload_paths

    def limit_for(self, path: str) -> int:
        """Uploads get their own ceiling; everything else keeps the tight one."""
        return self.upload_max_bytes if path.startswith(self.upload_paths) else self.max_bytes

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        max_bytes = self.limit_for(scope.get("path", ""))
        headers = dict(scope.get("headers") or [])
        declared = headers.get(b"content-length")

        if declared is not None:
            try:
                too_big = int(declared) > max_bytes
            except ValueError:
                too_big = True
            if too_big:
                await self._too_large(scope, send)
                return
            await self.app(scope, receive, send)
            return

        if scope.get("method", "GET").upper() not in _BODY_METHODS:
            await self.app(scope, receive, send)
            return

        body = bytearray()
        while True:
            message = await receive()
            if message["type"] != "http.request":
                break
            body.extend(message.get("body", b""))
            if len(body) > max_bytes:
                await self._too_large(scope, send)
                return
            if not message.get("more_body", False):
                break

        buffered = bytes(body)
        sent = False

        async def replay() -> Message:
            nonlocal sent
            if sent:
                return {"type": "http.disconnect"}
            sent = True
            return {"type": "http.request", "body": buffered, "more_body": False}

        await self.app(scope, replay, send)

    async def _too_large(self, scope: Scope, send: Send) -> None:
        response = JSONResponse({"detail": "Request body too large"}, status_code=413)
        await response(scope, _empty_receive, send)


async def _empty_receive() -> Message:
    return {"type": "http.disconnect"}


limiter = Limiter(key_func=get_remote_address)


@asynccontextmanager
async def lifespan(app: FastAPI):
    await upstream.startup()
    # Warm the OAuth2 discovery cache so the first login has no extra latency
    try:
        await _discover_endpoints()
    except Exception as e:
        logger.warning("OAuth discovery warm-up failed (will retry on first login): %s", e)
    try:
        yield
    finally:
        await upstream.shutdown()


app = FastAPI(title="MeCloud", lifespan=lifespan, docs_url=None, redoc_url=None)

app.state.limiter = limiter


@app.exception_handler(RateLimitExceeded)
async def _rate_limit_handler(request: Request, exc: RateLimitExceeded):
    """429 with a Retry-After the client can actually act on.

    slowapi's stock handler sends none, which leaves a caller guessing. That
    matters for file uploads: dropping a folder fires one request per file, so
    hitting a per-minute limit is an ordinary consequence of ordinary use, and
    the client waits the advised time rather than failing the upload.

    The window length is a ceiling, not the exact reset, so the client may wait
    slightly longer than strictly required — which is the right direction to err.
    """
    retry_after = 60
    try:
        retry_after = int(exc.limit.limit.get_expiry())
    except Exception:      # noqa: BLE001 — never let telemetry break the response
        logger.debug("Could not derive Retry-After from %r", exc.limit)

    return JSONResponse(
        {"detail": "Too many requests — please slow down."},
        status_code=429,
        headers={"Retry-After": str(retry_after)},
    )


@app.exception_handler(UpstreamError)
async def _upstream_error_handler(request: Request, exc: UpstreamError):
    """Turn an upstream failure into something the client can act on.

    A revoked or expired token used to surface as a 502, which the frontend
    retried forever. Mapping it to 401 makes the client log the user back in.
    """
    if exc.is_auth_failure:
        request.session.clear()
        return JSONResponse({"detail": "Session expired, please log in again"}, status_code=401)
    # Pass through statuses the client can act on; collapse the rest to 502 so
    # upstream internals are not echoed back.
    if exc.status in (400, 404, 413, 503):
        return JSONResponse({"detail": str(exc)}, status_code=exc.status)
    logger.error("Upstream error on %s: %s", request.url.path, exc)
    return JSONResponse({"detail": "Upstream service error"}, status_code=502)


@app.exception_handler(httpx.HTTPError)
async def _upstream_transport_error_handler(request: Request, exc: httpx.HTTPError):
    """Connect/read timeouts and DNS failures are a bad gateway, not a bug."""
    logger.error("Upstream transport error on %s: %s: %s",
                 request.url.path, type(exc).__name__, exc)
    return JSONResponse({"detail": "Upstream service unavailable"}, status_code=504)


# Middleware added last runs outermost, so this list reads inside-out:
# session -> CORS -> body limit -> security headers -> host check.
app.add_middleware(
    EncryptedSessionMiddleware,
    secret_key=settings.session_secret,
    https_only=settings.is_production,
    same_site="lax",
    max_age=settings.session_max_age,
)
origins = (
    [settings.app_url]
    if settings.is_production
    else ["http://localhost:5173", "http://localhost:8000"]
)
app.add_middleware(
    CORSMiddleware,
    allow_origins=origins,
    allow_credentials=True,
    allow_methods=["GET", "POST", "PUT", "DELETE", "OPTIONS"],
    allow_headers=["Content-Type"],
)
app.add_middleware(
    BodySizeLimitMiddleware,
    max_bytes=settings.max_request_bytes,
    upload_max_bytes=settings.max_upload_bytes,
    upload_paths=(UPLOAD_PATH,),
)
app.add_middleware(SecurityHeadersMiddleware)
app.add_middleware(TrustedHostMiddleware, allowed_hosts=settings.allowed_hosts)


async def _device_access_token(request: Request) -> str | None:
    """Trade a device token for a live Stalwart access token, or None.

    The device token carries an OAuth *refresh* token, so every call refreshes.
    That is one extra upstream request per API call and is why the sync client
    is expected to batch its work rather than chatter — but it means a device
    holds nothing that grants access on its own for longer than the mail server
    allows, and revoking upstream takes effect immediately.
    """
    presented = devices.bearer_token(request.headers.get("authorization"))
    if not presented:
        return None
    device = devices.read(settings.session_secret, presented)
    if device is None:
        raise HTTPException(status_code=401, detail="This device is no longer paired")
    try:
        tokens = await refresh_access_token(device.refresh_token)
    except Exception:
        raise HTTPException(status_code=401, detail="This device is no longer paired")
    return tokens["access_token"]


async def require_auth(request: Request) -> str:
    # A paired desktop client authenticates with a bearer token rather than the
    # session cookie: it syncs with no window open, so there is no browser
    # session to borrow.
    from_device = await _device_access_token(request)
    if from_device:
        return from_device

    token = request.session.get("access_token")
    if not token:
        raise HTTPException(status_code=401, detail="Not authenticated")

    expires_at = request.session.get("token_expires_at", 0)
    if expires_at and time.time() > expires_at - 300:   # refresh 5 min before expiry
        rt = request.session.get("refresh_token")
        if not rt:
            request.session.clear()
            raise HTTPException(status_code=401, detail="Session expired, please log in again")
        try:
            old_token = token
            new_tokens = await refresh_access_token(rt)
            token = new_tokens["access_token"]
            request.session["access_token"] = token
            if "refresh_token" in new_tokens:
                request.session["refresh_token"] = new_tokens["refresh_token"]
            request.session["token_expires_at"] = time.time() + new_tokens.get("expires_in", 3600)
            invalidate_session(old_token)
        except Exception:
            request.session.clear()
            raise HTTPException(status_code=401, detail="Session expired, please log in again")

    return token


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.get("/api/config")
async def public_config():
    return {"appName": settings.app_name, "stalwartUrl": settings.stalwart_url}


def _pairing_page(*, name: str, sealed: str, username: str) -> HTMLResponse:
    """The approval screen.

    Deliberately plain server-rendered HTML with a form POST. There is no
    JavaScript, nothing to fetch, and the only action is a button the user
    presses — so what is approved is exactly what is displayed.
    """
    safe_name = html.escape(name)
    safe_user = html.escape(username or "this account")
    body = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect a device</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
        background:#f6f7f9; color:#111827;
        font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
 @media (prefers-color-scheme: dark) {{ body {{ background:#0b1220; color:#f3f4f6; }}
   .card {{ background:#111827 !important; border-color:#1f2937 !important; }}
   .muted {{ color:#9ca3af !important; }} li {{ color:#d1d5db; }} }}
 .card {{ max-width:26rem; width:100%; margin:1rem; padding:1.75rem;
          background:#fff; border:1px solid #e5e7eb; border-radius:14px; }}
 h1 {{ font-size:1.05rem; margin:0 0 .35rem; }}
 .muted {{ color:#6b7280; font-size:.85rem; margin:0 0 1.1rem; }}
 ul {{ margin:0 0 1.3rem; padding-left:1.1rem; font-size:.9rem; }}
 li {{ margin-bottom:.3rem; }}
 .row {{ display:flex; gap:.6rem; align-items:center; }}
 button {{ padding:.6rem 1.15rem; border:0; border-radius:9px; font-size:.95rem;
           font-weight:600; background:#3b82f6; color:#fff; cursor:pointer; }}
 a.cancel {{ color:#6b7280; font-size:.9rem; text-decoration:none; padding:.6rem; }}
</style></head><body>
<div class="card">
  <h1>Connect {safe_name}?</h1>
  <p class="muted">Signed in as {safe_user}</p>
  <ul>
    <li>Read and change the files in your drive</li>
    <li>Stay connected in the background, until you disconnect it on the mail server</li>
  </ul>
  <form method="post" action="/auth/device/approve" class="row">
    <input type="hidden" name="request_token" value="{html.escape(sealed)}">
    <button type="submit">Connect</button>
    <a class="cancel" href="/">Cancel</a>
  </form>
</div></body></html>"""
    return HTMLResponse(body, headers={"Cache-Control": "no-store"})


def _handoff_page(target: str) -> HTMLResponse:
    """Send the browser on to the waiting client.

    Deliberately not a redirect. The response to a form POST is still part of
    that form submission as far as `form-action` is concerned, and WebKit and
    Firefox check redirects against it — so a 303 to the loopback port is
    blocked outright by our own `form-action 'self'`:

        Sending form data to '…/auth/device/approve' violates the following
        Content Security Policy directive: "form-action 'self'"

    Widening the directive to allow loopback would fix the symptom by making
    the policy weaker everywhere, for one page. A meta refresh is an ordinary
    top-level navigation instead: no CSP directive governs it, no script is
    involved, and the visible link means it still works if a browser has
    refreshes turned off.
    """
    safe = html.escape(target, quote=True)
    body = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="0;url={safe}">
<title>Connecting…</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
        background:#f6f7f9; color:#111827;
        font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; text-align:center; }}
 @media (prefers-color-scheme: dark) {{ body {{ background:#0b1220; color:#f3f4f6; }} }}
 p {{ margin:.4rem; }} .muted {{ color:#6b7280; font-size:.88rem; }}
</style></head><body><div>
  <p>Connecting to MeCloud on this computer…</p>
  <p class="muted">If nothing happens, <a href="{safe}">finish connecting</a>.</p>
</div></body></html>"""
    return HTMLResponse(body, headers={"Cache-Control": "no-store"})


@app.get("/auth/device")
@limiter.limit("20/minute")
async def device_pairing(request: Request, redirect: str = "", name: str = "MeCloud Desktop"):
    """Ask the signed-in user to approve a device.

    Reached by the desktop client opening a browser here. Requiring the session
    is the whole point: it proves a person with an account is present, rather
    than a process that happens to run on the machine.
    """
    if not devices.is_loopback_redirect(redirect):
        raise HTTPException(
            status_code=400,
            detail="A device can only be paired to an address on this computer.",
        )
    if not request.session.get("access_token"):
        # Sign in first, then come back to this exact request. Path and query
        # only: the callback refuses anything absolute, since that value is the
        # one an open redirect would abuse.
        query = request.url.query
        request.session["after_login"] = f"/auth/device?{query}" if query else "/auth/device"
        return RedirectResponse(url="/auth/login", status_code=302)

    sealed = devices.seal_pairing(settings.session_secret, redirect=redirect, name=name)
    return _pairing_page(name=name, sealed=sealed, username=request.session.get("username", ""))


@app.post("/auth/device/approve")
@limiter.limit("20/minute")
async def device_approve(request: Request):
    """Mint the device token and hand it back over the loopback redirect."""
    if not request.session.get("access_token"):
        raise HTTPException(status_code=401, detail="Not authenticated")

    # Parsed from the raw body rather than through request.form(): Starlette
    # requires python-multipart for form parsing even when the body is plain
    # url-encoded, and one hidden field does not justify the dependency. The
    # body-size middleware has already capped what can arrive here.
    raw = (await request.body()).decode("utf-8", "replace")
    fields = parse_qs(raw, keep_blank_values=True)
    request_token = (fields.get("request_token") or [""])[0]

    opened = devices.open_pairing(settings.session_secret, request_token)
    if opened is None:
        raise HTTPException(status_code=400, detail="This pairing request has expired. Try again.")
    redirect, name = opened
    # Checked again after unsealing: the value acted on must be the checked one.
    if not devices.is_loopback_redirect(redirect):
        raise HTTPException(status_code=400, detail="Invalid pairing address.")

    refresh = request.session.get("refresh_token")
    if not refresh:
        raise HTTPException(
            status_code=400,
            detail="This session has no refresh token, so a device cannot be paired from it.",
        )

    token = devices.issue(
        settings.session_secret,
        refresh_token=refresh,
        username=request.session.get("username", ""),
        name=name,
    )
    joiner = "&" if "?" in redirect else "?"
    return _handoff_page(f"{redirect}{joiner}token={quote(token)}")


@app.get("/api/desktop/releases")
@limiter.limit("60/minute")
async def desktop_releases(request: Request, _: str = Depends(require_auth)):
    """What this build shipped, so the UI can offer only what exists.

    The version is what the desktop client compares against its own to decide
    whether an update is available, so this endpoint answers both questions.
    """
    return {"version": downloads.version(), "artifacts": downloads.available()}


@app.post("/api/desktop/token")
@limiter.limit("30/minute")
async def desktop_token(request: Request, artifact: str, _: str = Depends(require_auth)):
    """Trade the session for a short-lived link to one artefact.

    The session is checked here, once. The link it returns then works on its
    own, which is what makes the download survive leaving the app — a browser
    download manager, or a curl on another machine.
    """
    art = downloads.artifact(artifact)
    if art is None or not downloads.path_for(art).is_file():
        raise HTTPException(status_code=404, detail="No such download")
    return {
        "token": downloads.mint(settings.session_secret, art.key),
        "url": f"/api/desktop/download/{art.key}",
        "expires_in": downloads.TOKEN_TTL_SECONDS,
        "filename": art.filename,
    }


@app.get("/api/desktop/download/{artifact}")
@limiter.limit(settings.rate_limit_download)
async def desktop_download(request: Request, artifact: str, token: str = ""):
    """Serve an artefact against a signed token.

    Deliberately no session dependency: the token *is* the authorisation, and
    requiring both would break the case the token exists for.
    """
    art = downloads.artifact(artifact)
    if art is None:
        raise HTTPException(status_code=404, detail="No such download")
    if not downloads.verify(settings.session_secret, token, art.key):
        # One message for missing, malformed and expired alike.
        raise HTTPException(status_code=403, detail="This download link is not valid or has expired.")

    path = downloads.path_for(art)
    if not path.is_file():
        raise HTTPException(status_code=404, detail="No such download")

    return FileResponse(
        path,
        media_type=art.media_type,
        filename=art.filename,
        headers={"Cache-Control": "no-store"},
    )


@app.get("/manifest.webmanifest", response_class=JSONResponse)
async def web_manifest():
    name = settings.app_name
    short = name.split()[0] if name else "MeCloud"
    return {
        "name": name,
        "short_name": short,
        "description": f"{name} — mail, calendar, contacts, notes and files",
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "orientation": "portrait-primary",
        "background_color": "#ffffff",
        "theme_color": "#2563eb",
        "categories": ["productivity", "utilities"],
        "icons": [
            {"src": "/icons/icon-192.png",          "sizes": "192x192", "type": "image/png", "purpose": "any"},
            {"src": "/icons/icon-512.png",           "sizes": "512x512", "type": "image/png", "purpose": "any"},
            {"src": "/icons/icon-maskable-192.png",  "sizes": "192x192", "type": "image/png", "purpose": "maskable"},
            {"src": "/icons/icon-maskable-512.png",  "sizes": "512x512", "type": "image/png", "purpose": "maskable"},
        ],
    }


@app.get("/auth/login")
@limiter.limit("20/minute")
async def login(request: Request):
    url = await get_authorization_url(request.session)
    return RedirectResponse(url)


@app.get("/auth/callback")
@limiter.limit("20/minute")
async def callback(request: Request, code: str, state: str):
    try:
        tokens = await exchange_code(code, state, request.session)
    except ValueError as e:
        # State mismatch or missing PKCE — log what the session actually held so
        # we can tell whether the cookie arrived at all, then send the user back
        # to re-start the login (most common after a container restart).
        logger.warning(
            "OAuth callback rejected (%s). session_keys=%s  url_state=%.8s…",
            e,
            list(request.session.keys()),
            state,
        )
        request.session.clear()
        return RedirectResponse("/auth/login")
    except Exception as e:
        logger.error("OAuth callback error: %s", e)
        raise HTTPException(status_code=502, detail="Authentication failed")

    if not tokens.get("access_token"):
        logger.error("OAuth token endpoint returned no access_token")
        raise HTTPException(status_code=502, detail="Authentication failed")

    # Where to go once signed in. Read before the rotation below clears it,
    # and validated, because it came in on a query parameter.
    destination = safe_local_path(request.session.get("after_login")) or "/"

    # Rotate the session on privilege change so a pre-login cookie handed to the
    # browser by an attacker cannot be upgraded into an authenticated one.
    request.session.clear()
    request.session["access_token"]     = tokens["access_token"]
    request.session["token_expires_at"] = time.time() + tokens.get("expires_in", 3600)
    if "refresh_token" in tokens:
        request.session["refresh_token"] = tokens["refresh_token"]

    return RedirectResponse(destination)


@app.post("/auth/logout")
async def logout(request: Request):
    # POST-only: a GET logout is trivially triggered cross-site by an <img> tag.
    request.session.clear()
    return JSONResponse({"ok": True})


@app.get("/auth/me")
async def me(request: Request):
    if not request.session.get("access_token"):
        return {"authenticated": False, "email": None}
    return {"authenticated": True, "email": request.session.get("email")}


@app.get("/api/jmap/session")
@limiter.limit("60/minute")
async def jmap_session(request: Request, access_token: str = Depends(require_auth)):
    return await get_jmap_session(access_token)


@app.post("/api/jmap")
@limiter.limit(settings.rate_limit_jmap)
async def jmap_proxy(request: Request, payload: JMAPRequest, access_token: str = Depends(require_auth)):
    return await jmap_request(access_token, payload.model_dump())


@app.get("/api/jmap/events")
@limiter.limit("30/minute")
async def jmap_events(request: Request, access_token: str = Depends(require_auth)):
    async def event_generator():
        try:
            async for line in jmap_event_stream(access_token):
                yield f"{line}\n"
        except UpstreamError as e:
            logger.info("JMAP event stream ended: %s", e)
        except Exception as e:
            logger.error("JMAP event stream error: %s", e)

    return StreamingResponse(
        event_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-store",
            "X-Accel-Buffering": "no",
            "Connection": "keep-alive",
        },
    )


class SieveSaveRequest(BaseModel):
    id: str | None = Field(default=None, max_length=128)
    name: str = Field(default="My Rules", max_length=255)
    content: str = Field(max_length=65_536)   # 64 KB ceiling
    makeActive: bool = True


@app.get("/api/sieve")
@limiter.limit("60/minute")
async def sieve_get(request: Request, access_token: str = Depends(require_auth)):
    result = await get_sieve_script(access_token)
    if result is None:
        raise HTTPException(status_code=503, detail="Sieve not supported by this server")
    return result


@app.put("/api/sieve")
@limiter.limit("30/minute")
async def sieve_put(request: Request, payload: SieveSaveRequest,
                    access_token: str = Depends(require_auth)):
    return await save_sieve_script(
        access_token, payload.id, payload.name, payload.content, payload.makeActive
    )


# ── File storage blobs (JMAP for File Storage, urn:ietf:params:jmap:filenode) ──
#
# FileNode metadata rides the ordinary /api/jmap proxy. Only the bytes need
# these routes, because the bearer token lives in the server-side session and
# the browser therefore cannot reach the JMAP upload/download endpoints itself.

@app.post(UPLOAD_PATH)
@limiter.limit(settings.rate_limit_upload)
async def upload_file_blob(request: Request, access_token: str = Depends(require_auth)):
    """Stream a request body up to the JMAP upload endpoint."""
    result = await upload_blob(
        access_token,
        request.headers.get("content-type", "application/octet-stream"),
        request.stream(),
    )
    return {
        "blobId": result.get("blobId"),
        "type": result.get("type"),
        "size": result.get("size"),
    }


@app.get(UPLOAD_PATH + "/{blob_id}")
@limiter.limit(settings.rate_limit_download)
async def download_file_blob(
    request: Request,
    blob_id: str,
    name: str = "",
    type: str = "",
    inline: bool = False,
    access_token: str = Depends(require_auth),
):
    """Stream a blob back to the browser, defanged.

    Serving user-supplied bytes from our own origin is the sharp edge here: an
    HTML or SVG file rendered inline would execute as first-party script, with
    access to everything on this origin. So the response is neutralised three
    ways — an allow-list of types that may render inline (everything else is
    forced to application/octet-stream), an attachment disposition unless the
    type is on that list, and a per-response `sandbox` CSP that strips the
    document of script and same-origin privileges even if the first two are
    somehow wrong.
    """
    stream: BlobStream = await open_blob(access_token, blob_id, name, type)

    resolved_type = safe_content_type(type or stream.response.headers.get("content-type"),
                                      inline=inline)
    render_inline = inline and resolved_type != "application/octet-stream"

    headers = {
        "Content-Disposition": content_disposition(name, inline=render_inline),
        # Overrides the app-wide policy via setdefault in SecurityHeadersMiddleware.
        "Content-Security-Policy": blob_csp(inline=render_inline),
        "X-Content-Type-Options": "nosniff",
    }
    if stream.upstream_size:
        headers["Content-Length"] = stream.upstream_size

    return StreamingResponse(stream.chunks(), media_type=resolved_type, headers=headers)


# Paths the single-page-app fallback must never answer for. Anything under these
# prefixes is server API surface: an unmatched route there is a 404, not a
# request for the app shell.
_RESERVED_PREFIXES = ("api/", "auth/")


def is_spa_path(path: str) -> bool:
    """True when the SPA fallback may serve `path`.

    Route ordering alone is not enough to get this right. A GET to a POST-only
    endpoint (``/auth/logout``) is only a *partial* route match, so Starlette
    keeps looking and the catch-all wins — answering 200 with index.html where
    it should refuse. Deciding here rather than relying on registration order
    makes that impossible.
    """
    return not path.lstrip("/").startswith(_RESERVED_PREFIXES)


@app.api_route("/api/{rest:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH"],
               include_in_schema=False)
async def api_not_found(rest: str):
    # Without this the SPA catch-all below would answer unknown /api paths with
    # index.html, so a typo in a fetch URL looked like a JSON parse error.
    raise HTTPException(status_code=404, detail="Not found")


# Serve the SvelteKit static build — SPA-aware catch-all must be last
_static_dir = settings.frontend_static_dir
if os.path.isdir(_static_dir):
    _static_real = os.path.realpath(_static_dir)
    _index_html = os.path.join(_static_dir, "index.html")

    @app.get("/{full_path:path}", include_in_schema=False)
    async def serve_frontend(full_path: str = ""):
        if not is_spa_path(full_path):
            raise HTTPException(status_code=404, detail="Not found")
        if full_path:
            candidate = os.path.realpath(os.path.join(_static_dir, full_path))
            if candidate.startswith(_static_real + os.sep) and os.path.isfile(candidate):
                # Vite emits content-hashed filenames under _app/immutable, so
                # those can be cached hard. Everything else revalidates.
                immutable = "/_app/immutable/" in f"/{full_path}"
                cache = ("public, max-age=31536000, immutable" if immutable
                         else "public, max-age=0, must-revalidate")
                return FileResponse(candidate, headers={"Cache-Control": cache})
        # The SPA shell must always revalidate or users get stranded on an old
        # build that references deleted asset hashes.
        return FileResponse(_index_html, headers={"Cache-Control": "no-cache"})
