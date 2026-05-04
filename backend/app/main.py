import logging
import mimetypes
import os
import time

mimetypes.add_type('application/manifest+json', '.webmanifest')
from contextlib import asynccontextmanager
from fastapi import FastAPI, Request, HTTPException, Depends
from pydantic import BaseModel, Field
from fastapi.responses import JSONResponse, RedirectResponse, StreamingResponse, FileResponse
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.middleware.sessions import SessionMiddleware
from slowapi import Limiter, _rate_limit_exceeded_handler
from slowapi.util import get_remote_address
from slowapi.errors import RateLimitExceeded

from .config import get_settings
from .auth import get_authorization_url, exchange_code, refresh_access_token, _discover_endpoints
from .jmap import get_jmap_session, jmap_request, jmap_event_stream, get_sieve_script, save_sieve_script
from .models import JMAPRequest

logger = logging.getLogger(__name__)

settings = get_settings()


_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline'; "    # unsafe-inline needed for SvelteKit hydration
    "style-src 'self' 'unsafe-inline'; "
    "img-src 'self' data: blob: https:; "    # emails may contain external images
    "font-src 'self' data:; "
    "connect-src 'self'; "
    "frame-src 'none'; "
    "object-src 'none'; "
    "base-uri 'self'; "
    "form-action 'self';"
)


class SecurityHeadersMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request, call_next):
        response = await call_next(request)
        response.headers["X-Content-Type-Options"]  = "nosniff"
        response.headers["X-Frame-Options"]          = "DENY"
        response.headers["Referrer-Policy"]          = "strict-origin-when-cross-origin"
        response.headers["Permissions-Policy"]       = "camera=(), microphone=(), geolocation=()"
        response.headers["Content-Security-Policy"]  = _CSP
        if settings.is_production:
            response.headers["Strict-Transport-Security"] = "max-age=63072000; includeSubDomains; preload"
        return response


limiter = Limiter(key_func=get_remote_address)


@asynccontextmanager
async def lifespan(app: FastAPI):
    # Warm the OAuth2 discovery cache so the first login has no extra latency
    try:
        await _discover_endpoints()
    except Exception:
        pass  # Non-fatal — discovery will retry on first login
    yield


app = FastAPI(title="JMAP Mail", lifespan=lifespan, docs_url=None, redoc_url=None)

app.state.limiter = limiter
app.add_exception_handler(RateLimitExceeded, _rate_limit_exceeded_handler)

app.add_middleware(SecurityHeadersMiddleware)
app.add_middleware(
    SessionMiddleware,
    secret_key=settings.session_secret,
    https_only=settings.is_production,
    same_site="lax",
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
    allow_methods=["*"],
    allow_headers=["*"],
)


async def require_auth(request: Request) -> str:
    token = request.session.get("access_token")
    if not token:
        raise HTTPException(status_code=401, detail="Not authenticated")

    expires_at = request.session.get("token_expires_at", 0)
    if expires_at and time.time() > expires_at - 300:   # refresh 5 min before expiry
        rt = request.session.get("refresh_token")
        if rt:
            try:
                old_token = token
                new_tokens = await refresh_access_token(rt)
                token = new_tokens["access_token"]
                request.session["access_token"] = token
                if "refresh_token" in new_tokens:
                    request.session["refresh_token"] = new_tokens["refresh_token"]
                request.session["token_expires_at"] = time.time() + new_tokens.get("expires_in", 3600)
                # Invalidate the JMAP session cache for the old token
                from .jmap import _jmap_session_cache
                _jmap_session_cache.pop(old_token, None)
            except Exception:
                raise HTTPException(status_code=401, detail="Session expired, please log in again")

    return token


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.get("/api/config")
async def public_config():
    return {"appName": settings.app_name, "stalwartUrl": settings.stalwart_url}


@app.get("/manifest.webmanifest", response_class=JSONResponse)
async def web_manifest():
    name = settings.app_name
    short = name.split()[0] if name else "Mail"
    return {
        "name": name,
        "short_name": short,
        "description": f"{name} webmail client",
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
            "OAuth callback rejected (%s). session_keys=%s  url_state=%.8s…  "
            "session_state=%.8s…",
            e,
            list(request.session.keys()),
            state,
            request.session.get("oauth_state", "<missing>"),
        )
        request.session.clear()
        return RedirectResponse("/auth/login")
    except Exception as e:
        logger.error("OAuth callback error: %s", e)
        raise HTTPException(status_code=502, detail="Authentication failed")

    request.session["access_token"]     = tokens["access_token"]
    request.session["token_expires_at"] = time.time() + tokens.get("expires_in", 3600)
    if "refresh_token" in tokens:
        request.session["refresh_token"] = tokens["refresh_token"]
    request.session.pop("oauth_state", None)

    return RedirectResponse("/")


@app.get("/auth/logout")
async def logout(request: Request):
    request.session.clear()
    return RedirectResponse("/")


@app.get("/auth/me")
async def me(request: Request):
    token = request.session.get("access_token")
    if not token:
        return {"authenticated": False, "email": None}
    # Email is not stored separately yet; return authenticated status
    return {"authenticated": True, "email": request.session.get("email")}


@app.get("/api/jmap/session")
async def jmap_session(access_token: str = Depends(require_auth)):
    try:
        session = await get_jmap_session(access_token)
        return session
    except Exception as e:
        logger.error("JMAP session error: %s", e)
        raise HTTPException(status_code=502, detail="Upstream service error")


@app.post("/api/jmap")
async def jmap_proxy(payload: JMAPRequest, access_token: str = Depends(require_auth)):
    try:
        result = await jmap_request(access_token, payload.model_dump())
        return result
    except Exception as e:
        logger.error("JMAP request error: %s", e)
        raise HTTPException(status_code=502, detail="Upstream service error")


@app.get("/api/jmap/events")
async def jmap_events(access_token: str = Depends(require_auth)):
    async def event_generator():
        try:
            async for line in jmap_event_stream(access_token):
                yield f"{line}\n"
        except Exception as e:
            logger.error("JMAP event stream error: %s", e)

    return StreamingResponse(
        event_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "X-Accel-Buffering": "no",
        },
    )


class SieveSaveRequest(BaseModel):
    id: str | None = None
    name: str = Field(default="My Rules", max_length=255)
    content: str = Field(max_length=65_536)   # 64 KB ceiling
    makeActive: bool = True


@app.get("/api/sieve")
async def sieve_get(access_token: str = Depends(require_auth)):
    try:
        result = await get_sieve_script(access_token)
        if result is None:
            raise HTTPException(status_code=503, detail="Sieve not supported by this server")
        return result
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Sieve get error: %s", e)
        raise HTTPException(status_code=502, detail="Upstream service error")


@app.put("/api/sieve")
async def sieve_put(payload: SieveSaveRequest, access_token: str = Depends(require_auth)):
    try:
        result = await save_sieve_script(
            access_token, payload.id, payload.name, payload.content, payload.makeActive
        )
        return result
    except Exception as e:
        logger.error("Sieve put error: %s", e)
        raise HTTPException(status_code=502, detail="Upstream service error")


# Serve the SvelteKit static build — SPA-aware catch-all must be last
_static_dir = settings.frontend_static_dir
if os.path.isdir(_static_dir):
    _static_real = os.path.realpath(_static_dir)

    @app.get("/{full_path:path}")
    async def serve_frontend(full_path: str = ""):
        if full_path:
            candidate = os.path.realpath(os.path.join(_static_dir, full_path))
            if candidate.startswith(_static_real + os.sep) and os.path.isfile(candidate):
                return FileResponse(candidate)
        return FileResponse(os.path.join(_static_dir, "index.html"))
