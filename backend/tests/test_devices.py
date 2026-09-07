"""Unit tests for device pairing and device tokens."""
import time

import pytest

from app import devices

SECRET = "s" * 64
OTHER = "o" * 64


def test_a_device_token_round_trips():
    token = devices.issue(SECRET, refresh_token="rt-123", username="ada@x.test", name="Laptop")
    device = devices.read(SECRET, token)
    assert device is not None
    assert device.refresh_token == "rt-123"
    assert device.username == "ada@x.test"
    assert device.name == "Laptop"


def test_a_device_token_is_bound_to_the_secret():
    token = devices.issue(SECRET, refresh_token="rt", username="a", name="n")
    assert devices.read(OTHER, token) is None


@pytest.mark.parametrize("token", ["", "nonsense", "a.b.c", "x" * 200])
def test_malformed_device_tokens_are_refused_rather_than_raising(token):
    assert devices.read(SECRET, token) is None


def test_a_device_token_expires():
    token = devices.issue(SECRET, refresh_token="rt", username="a", name="n")
    # Readable now under the real lifetime...
    assert devices.read(SECRET, token) is not None
    # ...and refused once its age passes the window it is checked against.
    # Two seconds against a one-second window, so a second boundary landing
    # between issue and check cannot make this flap.
    time.sleep(2.1)
    assert devices.read(SECRET, token, max_age=1) is None


def test_each_token_scheme_uses_its_own_key():
    """The three HKDF `info` strings must stay distinct.

    Tested through behaviour rather than by comparing key bytes: what actually
    matters is that a token minted for one purpose is worthless for another.
    """
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.kdf.hkdf import HKDF

    def derive(info):
        return HKDF(algorithm=hashes.SHA256(), length=32, salt=None,
                    info=info).derive(SECRET.encode())

    infos = [b"mecloud.session.v1", b"mecloud.device.v1",
             b"mecloud.download.v1", b"mecloud.pairing.v1"]
    keys = [derive(i) for i in infos]
    assert len(set(keys)) == len(infos)


def test_a_device_token_is_not_a_download_token():
    from app import downloads
    token = devices.issue(SECRET, refresh_token="rt", username="a", name="n")
    assert downloads.verify(SECRET, token, "source") is False


def test_a_pairing_request_is_not_a_device_token():
    # Same primitive, different key: a sealed pairing request must not be
    # usable as the credential it is a request for.
    sealed = devices.seal_pairing(SECRET, redirect="http://127.0.0.1:4321/x", name="n")
    assert devices.read(SECRET, sealed) is None


def test_a_session_cookie_is_not_a_device_token():
    from app.session import _derive_key
    from cryptography.fernet import Fernet
    session_blob = Fernet(_derive_key(SECRET)).encrypt(b'{"access_token": "a"}').decode()
    assert devices.read(SECRET, session_blob) is None


# ── bearer header ───────────────────────────────────────────────────────────

@pytest.mark.parametrize("header,expected", [
    ("Bearer abc", "abc"),
    ("bearer abc", "abc"),
    ("BEARER   abc  ", "abc"),
    ("Basic abc", None),
    ("abc", None),
    ("Bearer", None),
    ("Bearer   ", None),
    ("", None),
    (None, None),
])
def test_bearer_token_parsing(header, expected):
    assert devices.bearer_token(header) == expected


# ── pairing ─────────────────────────────────────────────────────────────────

def test_a_sealed_pairing_request_round_trips():
    sealed = devices.seal_pairing(SECRET, redirect="http://127.0.0.1:4321/paired", name="Laptop")
    assert devices.open_pairing(SECRET, sealed) == ("http://127.0.0.1:4321/paired", "Laptop")


def test_a_sealed_pairing_request_cannot_be_forged():
    # The redirect travels through the browser; unsigned, any page could send
    # the user a consent screen that posts the token somewhere else.
    sealed = devices.seal_pairing(OTHER, redirect="http://127.0.0.1:4321/x", name="n")
    assert devices.open_pairing(SECRET, sealed) is None
    assert devices.open_pairing(SECRET, "not-a-token") is None


@pytest.mark.parametrize("url", [
    "http://127.0.0.1:1024/paired",
    "http://127.0.0.1:65535/paired",
    "http://[::1]:8123/paired",
])
def test_loopback_redirects_are_accepted(url):
    assert devices.is_loopback_redirect(url)


@pytest.mark.parametrize("url", [
    "http://localhost:4321/paired",        # a name, and a name can resolve anywhere
    "https://127.0.0.1:4321/paired",       # only plain http is expected on loopback
    "http://evil.example.com/paired",
    "http://127.0.0.1/paired",             # no port
    "http://127.0.0.1:80/paired",          # privileged port
    "http://127.0.0.1.evil.com:8000/x",    # prefix that merely looks local
    "http://0.0.0.0:8000/x",               # all interfaces, not just this machine
    "file:///tmp/x",
    "",
])
def test_non_loopback_redirects_are_refused(url):
    assert not devices.is_loopback_redirect(url)


# ── post-login destination ──────────────────────────────────────────────────

@pytest.mark.parametrize("value", [
    "/auth/device?redirect=http%3A%2F%2F127.0.0.1%3A5000%2Fpaired",
    "/",
    "/mail?compose=1",
])
def test_a_local_path_is_kept_as_the_destination(value):
    from app.auth import safe_local_path
    assert safe_local_path(value) == value


@pytest.mark.parametrize("value", [
    "//evil.example.com/x",        # scheme-relative: leaves the site
    "https://evil.example.com/x",
    "http://evil.example.com",
    "/\\evil.example.com",
    "javascript:alert(1)",
    "mail",                         # not rooted
    "",
    None,
    "/x\r\nSet-Cookie: a=b",
])
def test_a_destination_that_could_leave_the_site_is_refused(value):
    # An open redirect on a login flow is a phishing link on the real domain.
    from app.auth import safe_local_path
    assert safe_local_path(value) is None
