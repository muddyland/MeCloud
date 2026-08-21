"""Tests for blob upload/download proxying.

The interesting risk is not "does the file arrive" — it is that we are serving
user-supplied bytes from our own origin. A stored HTML or SVG file rendered
inline would run as first-party script with access to the whole app, so most of
what follows is about making that impossible.
"""
import pytest
from unittest.mock import AsyncMock, patch

from app.blobs import (
    INLINE_SAFE_TYPES, content_disposition, safe_content_type, validate_blob_id,
)
from app.http import UpstreamError
from app.main import BodySizeLimitMiddleware, UPLOAD_PATH


# ---------------------------------------------------------------------------
# Blob id validation — these are interpolated into the download URL template
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("blob_id", ["abc123", "G-a_b.c", "n:1234", "x" * 256])
def test_plausible_blob_ids_are_accepted(blob_id):
    assert validate_blob_id(blob_id) == blob_id


@pytest.mark.parametrize("blob_id", [
    "",
    "../../etc/passwd",          # path traversal
    "a/b",                       # extra path segment
    "a?x=1",                     # query injection
    "a#frag",
    "a b",                       # whitespace
    "http://evil.example/x",     # absolute URL
    "x" * 257,                   # unbounded length
    "a%2f..%2f",                 # pre-encoded traversal
])
def test_hostile_blob_ids_are_rejected(blob_id):
    with pytest.raises(UpstreamError) as exc:
        validate_blob_id(blob_id)
    assert exc.value.status == 400


# ---------------------------------------------------------------------------
# Content type — the allow-list is the XSS boundary
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("declared", [
    "text/html",
    "image/svg+xml",             # SVG carries <script>
    "application/xhtml+xml",
    "text/html; charset=utf-8",
    "application/javascript",
    "text/xml",
    None,
    "",
    "nonsense",
])
def test_script_capable_types_never_render_inline(declared):
    assert safe_content_type(declared, inline=True) == "application/octet-stream"


@pytest.mark.parametrize("declared", sorted(INLINE_SAFE_TYPES))
def test_allow_listed_types_may_render_inline(declared):
    assert safe_content_type(declared, inline=True) == declared


@pytest.mark.parametrize("declared", sorted(INLINE_SAFE_TYPES))
def test_nothing_renders_inline_when_downloading(declared):
    assert safe_content_type(declared, inline=False) == "application/octet-stream"


def test_type_parameters_and_case_are_normalised():
    assert safe_content_type("IMAGE/PNG; charset=binary", inline=True) == "image/png"


# ---------------------------------------------------------------------------
# Content-Disposition — a filename is attacker-controlled header material
# ---------------------------------------------------------------------------

def test_disposition_defaults_to_attachment():
    assert content_disposition("notes.txt", inline=False).startswith("attachment;")


def test_disposition_can_be_inline():
    assert content_disposition("photo.png", inline=True).startswith("inline;")


def test_header_injection_via_filename_is_neutralised():
    evil = 'a";\r\nSet-Cookie: stolen=1\r\n\r\n<script>.txt'
    header = content_disposition(evil, inline=False)
    assert "\r" not in header and "\n" not in header
    # The quoted ASCII fallback must not contain a bare quote that ends it early.
    ascii_part = header.split('filename="', 1)[1].split('"', 1)[0]
    assert '"' not in ascii_part


def test_unicode_filename_survives_via_rfc6266():
    header = content_disposition("réunion été.pdf", inline=False)
    assert "filename*=UTF-8''" in header
    assert "%C3%A9" in header          # é percent-encoded in the UTF-8 form


def test_empty_filename_falls_back():
    assert 'filename="download"' in content_disposition("", inline=False)
    assert 'filename="download"' in content_disposition(None, inline=False)


# ---------------------------------------------------------------------------
# Body-size limits — uploads get a bigger ceiling, everything else must not
# ---------------------------------------------------------------------------

def test_uploads_get_the_large_limit_and_nothing_else_does():
    mw = BodySizeLimitMiddleware(
        app=None, max_bytes=1_000, upload_max_bytes=99_000, upload_paths=(UPLOAD_PATH,),
    )
    assert mw.limit_for(UPLOAD_PATH) == 99_000
    assert mw.limit_for(UPLOAD_PATH + "/blob-1") == 99_000
    # The tight cap is what protects the JMAP proxy; raising it there would
    # undo the point of having two limits.
    assert mw.limit_for("/api/jmap") == 1_000
    assert mw.limit_for("/api/sieve") == 1_000
    assert mw.limit_for("/") == 1_000


# ---------------------------------------------------------------------------
# Routes
# ---------------------------------------------------------------------------

async def test_upload_requires_authentication(client):
    r = await client.post(UPLOAD_PATH, content=b"x")
    assert r.status_code == 401


async def test_download_requires_authentication(client):
    r = await client.get(f"{UPLOAD_PATH}/abc123")
    assert r.status_code == 401


async def test_upload_returns_the_blob_id(auth_client):
    with patch("app.main.upload_blob", new_callable=AsyncMock) as m:
        m.return_value = {"blobId": "G123", "type": "image/png", "size": 42}
        r = await auth_client.post(UPLOAD_PATH, content=b"fakepng",
                                   headers={"Content-Type": "image/png"})
    assert r.status_code == 200
    assert r.json() == {"blobId": "G123", "type": "image/png", "size": 42}


async def test_malformed_blob_id_is_a_400_not_a_502(auth_client):
    with patch("app.main.open_blob", new_callable=AsyncMock) as m:
        m.side_effect = UpstreamError("Malformed blob id", 400)
        r = await auth_client.get(f"{UPLOAD_PATH}/whatever")
    assert r.status_code == 400


async def test_missing_blob_is_a_404_not_a_502(auth_client):
    with patch("app.main.open_blob", new_callable=AsyncMock) as m:
        m.side_effect = UpstreamError("Blob not found", 404)
        r = await auth_client.get(f"{UPLOAD_PATH}/abc123")
    assert r.status_code == 404


class _FakeStream:
    """Stands in for an already-opened upstream download."""

    def __init__(self, payload=b"hello", content_type="text/html"):
        self._payload = payload
        self.response = type("R", (), {"headers": {"content-type": content_type}})()
        self.upstream_size = str(len(payload))

    async def chunks(self):
        yield self._payload


async def test_download_defangs_html_and_forces_attachment(auth_client):
    with patch("app.main.open_blob", new_callable=AsyncMock) as m:
        m.return_value = _FakeStream(b"<script>alert(1)</script>", "text/html")
        r = await auth_client.get(
            f"{UPLOAD_PATH}/abc123", params={"name": "evil.html", "type": "text/html",
                                             "inline": "true"},
        )
    assert r.status_code == 200
    # Asked for inline HTML; got an opaque attachment instead.
    assert r.headers["content-type"].startswith("application/octet-stream")
    assert r.headers["content-disposition"].startswith("attachment;")
    assert r.headers["x-content-type-options"] == "nosniff"
    # And even then the document is sandboxed, so nothing could run.
    assert "sandbox" in r.headers["content-security-policy"]


async def test_download_allows_inline_for_an_image(auth_client):
    with patch("app.main.open_blob", new_callable=AsyncMock) as m:
        m.return_value = _FakeStream(b"\x89PNG", "image/png")
        r = await auth_client.get(
            f"{UPLOAD_PATH}/abc123", params={"name": "cat.png", "type": "image/png",
                                             "inline": "true"},
        )
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("image/png")
    assert r.headers["content-disposition"].startswith("inline;")
    assert "sandbox" in r.headers["content-security-policy"]


async def test_download_is_never_cached(auth_client):
    with patch("app.main.open_blob", new_callable=AsyncMock) as m:
        m.return_value = _FakeStream(b"secret", "text/plain")
        r = await auth_client.get(f"{UPLOAD_PATH}/abc123", params={"name": "s.txt"})
    assert "no-store" in r.headers.get("cache-control", "")
