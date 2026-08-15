"""Shared upstream HTTP clients.

Every request used to build its own ``httpx.AsyncClient``, which meant a fresh
TCP + TLS handshake per JMAP call and no bound on how long a hung upstream could
hold a worker. These are process-wide pooled clients with explicit timeouts,
opened and closed by the app lifespan.
"""
import httpx

from .config import get_settings

_client: httpx.AsyncClient | None = None
_stream_client: httpx.AsyncClient | None = None


class UpstreamError(Exception):
    """An upstream (Stalwart) call failed.

    ``status`` is the upstream HTTP status when there was one, so callers can
    distinguish "your token is dead, log in again" (401/403) from "the mail
    server is having a bad day" (5xx).
    """

    def __init__(self, message: str, status: int | None = None) -> None:
        super().__init__(message)
        self.status = status

    @property
    def is_auth_failure(self) -> bool:
        return self.status in (401, 403)


def _timeout() -> httpx.Timeout:
    settings = get_settings()
    return httpx.Timeout(
        connect=settings.upstream_connect_timeout,
        read=settings.upstream_read_timeout,
        write=settings.upstream_read_timeout,
        pool=settings.upstream_connect_timeout,
    )


async def startup() -> None:
    global _client, _stream_client
    limits = httpx.Limits(max_connections=100, max_keepalive_connections=20)
    _client = httpx.AsyncClient(timeout=_timeout(), limits=limits, follow_redirects=True)
    # Event streams stay open indefinitely by design, so only the connect phase
    # is bounded. They get their own pool so a burst of idle streams cannot
    # starve ordinary JMAP requests of connections.
    _stream_client = httpx.AsyncClient(
        timeout=httpx.Timeout(connect=get_settings().upstream_connect_timeout, read=None,
                              write=None, pool=None),
        limits=httpx.Limits(max_connections=get_settings().max_event_streams),
        follow_redirects=True,
    )


async def shutdown() -> None:
    global _client, _stream_client
    for client in (_client, _stream_client):
        if client is not None:
            await client.aclose()
    _client = _stream_client = None


def client() -> httpx.AsyncClient:
    """The pooled client for ordinary request/response calls."""
    global _client
    if _client is None:                       # e.g. unit tests that skip lifespan
        _client = httpx.AsyncClient(timeout=_timeout(), follow_redirects=True)
    return _client


def stream_client() -> httpx.AsyncClient:
    """The pooled client for long-lived Server-Sent Event connections."""
    global _stream_client
    if _stream_client is None:
        _stream_client = httpx.AsyncClient(
            timeout=httpx.Timeout(connect=get_settings().upstream_connect_timeout, read=None,
                                  write=None, pool=None),
            follow_redirects=True,
        )
    return _stream_client


def raise_for_status(response: httpx.Response, what: str) -> None:
    """Translate an upstream error response into an :class:`UpstreamError`."""
    if response.is_success:
        return
    raise UpstreamError(f"{what} failed with HTTP {response.status_code}", response.status_code)
