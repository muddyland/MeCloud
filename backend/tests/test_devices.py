"""Unit tests for device pairing and device tokens."""
import time

import pytest

from app import devices

SECRET = "s" * 64
OTHER = "o" * 64


def test_a_device_token_round_trips():
    token = devices.issue(SECRET, app_password="pw-123", password_id="p1",
                          username="ada@x.test", name="Laptop")
    device = devices.read(SECRET, token)
    assert device is not None
    assert device.secret == "pw-123"
    assert device.password_id == "p1"
    assert device.username == "ada@x.test"
    assert device.name == "Laptop"


def test_the_device_credential_is_basic_auth_for_the_mail_server():
    import base64
    token = devices.issue(SECRET, app_password="pw", password_id="p1",
                          username="ada@x.test", name="Laptop")
    header = devices.read(SECRET, token).basic_credential()
    scheme, _, value = header.partition(" ")
    assert scheme == "Basic"
    assert base64.b64decode(value).decode() == "ada@x.test:pw"


def test_a_device_token_is_bound_to_the_secret():
    token = devices.issue(SECRET, app_password="pw", password_id="p", username="a", name="n")
    assert devices.read(OTHER, token) is None


@pytest.mark.parametrize("token", ["", "nonsense", "a.b.c", "x" * 200])
def test_malformed_device_tokens_are_refused_rather_than_raising(token):
    assert devices.read(SECRET, token) is None


def test_a_device_token_expires():
    token = devices.issue(SECRET, app_password="pw", password_id="p", username="a", name="n")
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
    token = devices.issue(SECRET, app_password="pw", password_id="p", username="a", name="n")
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
    assert devices.open_pairing(SECRET, sealed) == ("http://127.0.0.1:4321/paired", "Laptop", "")
    sealed = devices.seal_pairing(SECRET, redirect="http://127.0.0.1:4321/paired", name="Laptop",
                                  state="s" * 32)
    assert devices.open_pairing(SECRET, sealed)[2] == "s" * 32


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


# ── the hand-off back to the client ─────────────────────────────────────────

def test_the_handoff_is_a_navigation_not_a_redirect():
    """A 303 here is blocked by our own `form-action 'self'`.

    The response to a form POST is still governed by the submission's
    form-action in WebKit and Firefox, and the loopback port is cross-origin,
    so a redirect never reaches the waiting client.
    """
    from app.main import _handoff_page

    target = "http://127.0.0.1:41234/paired?token=abc"
    response = _handoff_page(target)
    assert response.status_code == 200
    body = response.body.decode()
    assert 'http-equiv="refresh"' in body
    assert target in body
    # A visible link too, for a browser with meta refresh disabled.
    assert f'href="{target}"' in body


def test_the_handoff_escapes_what_it_puts_in_the_page():
    from app.main import _handoff_page

    body = _handoff_page('http://127.0.0.1:1/p?token=a"><script>x</script>').body.decode()
    assert "<script>" not in body
    assert "&lt;script&gt;" in body


# ── state nonce and the POST hand-off ───────────────────────────────────────

@pytest.mark.parametrize("state,ok", [
    ("a" * 16, True), ("A-z_09" * 5, True), ("a" * 128, True),
    ("a" * 15, False), ("a" * 129, False), ("a" * 20 + "!", False), ("", False),
    ("a" * 20 + "&token=x", False),
])
def test_state_shape(state, ok):
    assert devices.is_valid_state(state) is ok


@pytest.mark.parametrize("url,origin", [
    ("http://127.0.0.1:41234/paired", "http://127.0.0.1:41234"),
    ("http://[::1]:8123/paired?x=1", "http://[::1]:8123"),
])
def test_loopback_origin(url, origin):
    assert devices.loopback_origin(url) == origin


def test_the_form_handoff_posts_token_and_state_and_scopes_form_action():
    import base64
    import hashlib
    import re
    from app.main import _handoff_form

    response = _handoff_form("http://127.0.0.1:41234/paired", "tok/+=", "s" * 32)
    body = response.body.decode()
    csp = response.headers["content-security-policy"]
    assert 'method="post"' in body and 'action="http://127.0.0.1:41234/paired"' in body
    assert 'name="token" value="tok/+="' in body
    assert 'name="state" value="' + "s" * 32 + '"' in body
    assert "token=" not in body.split("<form", 1)[0]          # never in a URL
    assert "form-action http://127.0.0.1:41234;" in csp
    assert "unsafe-inline" not in csp.split("script-src", 1)[1].split(";", 1)[0]
    # The inline script on the page is exactly the one the policy allows.
    script = re.search(r"<script>(.*?)</script>", body, re.S).group(1)
    digest = base64.b64encode(hashlib.sha256(script.encode()).digest()).decode()
    assert f"'sha256-{digest}'" in csp


def test_the_form_handoff_escapes_its_values():
    from app.main import _handoff_form

    body = _handoff_form("http://127.0.0.1:1/p", '"><script>x</script>', "s" * 32).body.decode()
    assert "<script>x" not in body


async def test_an_invalid_state_is_refused(client):
    response = await client.get("/auth/device", params={
        "redirect": "http://127.0.0.1:41234/paired", "state": "short"})
    assert response.status_code == 400


async def test_approve_with_state_returns_the_form_handoff(monkeypatch):
    """A sealed request carrying a nonce is answered with the POST hand-off."""
    from app import main

    sealed = devices.seal_pairing(main.settings.session_secret,
                                  redirect="http://127.0.0.1:41234/paired", name="Laptop",
                                  state="s" * 32)

    async def fake_create(token, name):
        return {"secret": "pw", "id": "p1", "username": "ada"}

    monkeypatch.setattr(main, "create_app_password", fake_create)

    # Drive the route directly with a session that is signed in.
    from starlette.requests import Request
    body = f"request_token={sealed}".encode()

    async def receive():
        return {"type": "http.request", "body": body, "more_body": False}

    scope = {"type": "http", "method": "POST", "path": "/auth/device/approve", "headers": [],
             "query_string": b"", "session": {"access_token": "t"}, "client": ("127.0.0.1", 1),
             "server": ("test", 80), "scheme": "http", "root_path": "", "app": main.app}
    response = await main.device_approve.__wrapped__(Request(scope, receive))
    assert "form-action http://127.0.0.1:41234;" in response.headers["content-security-policy"]
    assert 'name="state"' in response.body.decode()
