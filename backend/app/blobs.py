"""Blob upload and download proxying.

JMAP keeps file *metadata* (FileNode objects) in the regular method-call API,
but the bytes move over separate upload/download endpoints authenticated with
the same bearer token. That token lives in the encrypted server-side session
and deliberately never reaches the browser, so the browser cannot talk to those
endpoints directly — these two functions stand in for it.

Everything here streams. A 25 MB upload buffered in memory per concurrent
request is an easy way to lose a small container.
"""
import re
from urllib.parse import quote

from .http import UpstreamError, client, raise_for_status, stream_client
from .jmap import _account_id, _auth, _session_for

# Media types we are willing to let a browser render inline. Everything else is
# forced to download. This list is deliberately short: anything that can carry
# script (text/html, image/svg+xml, application/xhtml+xml) is NOT here, because
# a rendered document inherits *our* origin and would run as first-party script.
INLINE_SAFE_TYPES = frozenset({
    "image/png", "image/jpeg", "image/gif", "image/webp", "image/avif", "image/bmp",
    "application/pdf",
    "text/plain",
    "audio/mpeg", "audio/ogg", "audio/wav", "audio/flac", "audio/mp4",
    "video/mp4", "video/webm", "video/ogg",
})

# A blob id is an opaque server token. Pin it to a conservative shape so it can
# never inject path segments or query parameters into the download URL template.
_BLOB_ID_RE = re.compile(r"^[A-Za-z0-9_\-=.:~]{1,256}$")

# Header-unsafe characters in a user-supplied filename. RFC 6266 gives us
# filename* for the real name; this is the ASCII fallback.
_UNSAFE_NAME_RE = re.compile(r'[^\w.\-+ ]')


def validate_blob_id(blob_id: str) -> str:
    if not blob_id or not _BLOB_ID_RE.match(blob_id):
        raise UpstreamError("Malformed blob id", 400)
    return blob_id


def safe_content_type(declared: str | None, *, inline: bool) -> str:
    """The Content-Type we are prepared to put our own origin behind."""
    base = (declared or "").split(";")[0].strip().lower()
    if inline and base in INLINE_SAFE_TYPES:
        return base
    # Not on the allow-list, or being downloaded: hand it over as opaque bytes
    # so no sniffing or rendering can happen.
    return "application/octet-stream"


def content_disposition(name: str | None, *, inline: bool) -> str:
    """RFC 6266 disposition with an ASCII fallback and a UTF-8 filename*."""
    raw = (name or "download").strip() or "download"
    raw = raw.replace("\r", "").replace("\n", "")
    ascii_name = _UNSAFE_NAME_RE.sub("_", raw)[:120] or "download"
    disposition = "inline" if inline else "attachment"
    return f"{disposition}; filename=\"{ascii_name}\"; filename*=UTF-8''{quote(raw, safe='')}"


def _download_url(session: dict, account_id: str, blob_id: str, name: str, type_: str) -> str:
    template = session.get("downloadUrl", "")
    if not template:
        raise UpstreamError("Server does not advertise a download endpoint", 503)
    # Every substitution is percent-encoded: these values reach us from the
    # client, and the template is interpolated into a URL we then fetch.
    return (
        template
        .replace("{accountId}", quote(account_id, safe=""))
        .replace("{blobId}", quote(blob_id, safe=""))
        .replace("{name}", quote(name or "download", safe=""))
        .replace("{type}", quote(type_ or "application/octet-stream", safe=""))
    )


async def upload_blob(access_token: str, content_type: str, body_stream) -> dict:
    """Stream an upload to the JMAP upload endpoint, return {blobId, type, size}."""
    session = await _session_for(access_token)
    account_id = _account_id(session)

    template = session.get("uploadUrl", "")
    if not template:
        raise UpstreamError("Server does not advertise an upload endpoint", 503)
    url = template.replace("{accountId}", quote(account_id, safe=""))

    response = await client().post(
        url,
        content=body_stream,          # async iterator — never fully in memory
        headers={
            **_auth(access_token),
            # Upstream decides what it will accept; we pass the claim through
            # but never trust it when serving the bytes back (see safe_content_type).
            "Content-Type": (content_type or "application/octet-stream").split(";")[0].strip(),
        },
    )
    raise_for_status(response, "Blob upload")
    result = response.json()
    if not isinstance(result, dict) or "blobId" not in result:
        raise UpstreamError("Upload endpoint returned no blobId")
    return result


class BlobStream:
    """An open download whose status is already known.

    The context manager has to be entered *before* the response is returned to
    the client. Discovering a 404 halfway through an async generator is too
    late — the 200 status line has already gone out, and the browser sees a
    truncated file instead of an error.
    """

    def __init__(self, ctx, response) -> None:
        self._ctx = ctx
        self.response = response

    @property
    def upstream_size(self) -> str | None:
        return self.response.headers.get("content-length")

    async def chunks(self):
        try:
            async for chunk in self.response.aiter_bytes():
                yield chunk
        finally:
            await self._ctx.__aexit__(None, None, None)

    async def aclose(self) -> None:
        await self._ctx.__aexit__(None, None, None)


async def open_blob(access_token: str, blob_id: str, name: str, type_: str) -> BlobStream:
    """Open a blob download and validate the upstream status before returning."""
    validate_blob_id(blob_id)
    session = await _session_for(access_token)
    account_id = _account_id(session)
    url = _download_url(session, account_id, blob_id, name, type_)

    ctx = stream_client().stream("GET", url, headers=_auth(access_token))
    response = await ctx.__aenter__()
    try:
        if response.status_code == 404:
            raise UpstreamError("Blob not found", 404)
        raise_for_status(response, "Blob download")
    except BaseException:
        await ctx.__aexit__(None, None, None)
        raise
    return BlobStream(ctx, response)
