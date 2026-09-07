"""Unit tests for the desktop download tokens."""
import time

import pytest

from app import downloads

SECRET = "a" * 64
OTHER = "b" * 64


def test_a_fresh_token_verifies_for_its_own_artifact():
    token = downloads.mint(SECRET, "linux-x86_64")
    assert downloads.verify(SECRET, token, "linux-x86_64")


def test_a_token_is_bound_to_one_artifact():
    # Otherwise a link to the source zip would also fetch the binary.
    token = downloads.mint(SECRET, "source")
    assert not downloads.verify(SECRET, token, "linux-x86_64")


def test_a_token_is_bound_to_the_signing_secret():
    token = downloads.mint(SECRET, "source")
    assert not downloads.verify(OTHER, token, "source")


def test_a_token_expires():
    past = time.time() - downloads.TOKEN_TTL_SECONDS - 1
    token = downloads.mint(SECRET, "source", now=past)
    assert not downloads.verify(SECRET, token, "source")
    # ...and was valid when it was minted.
    assert downloads.verify(SECRET, token, "source", now=past + 1)


def test_expiry_is_the_advertised_ttl():
    now = 1_000_000.0
    token = downloads.mint(SECRET, "source", now=now)
    assert downloads.verify(SECRET, token, "source", now=now + downloads.TOKEN_TTL_SECONDS - 1)
    assert not downloads.verify(SECRET, token, "source", now=now + downloads.TOKEN_TTL_SECONDS + 1)


@pytest.mark.parametrize("token", ["", "nonsense", "no-dot", "a.b", "....", "%%%.%%%"])
def test_malformed_tokens_are_refused_rather_than_raising(token):
    assert downloads.verify(SECRET, token, "source") is False


def test_a_tampered_payload_is_refused():
    token = downloads.mint(SECRET, "source")
    payload, _, signature = token.partition(".")
    forged = downloads.mint(SECRET, "linux-x86_64").partition(".")[0]
    assert not downloads.verify(SECRET, f"{forged}.{signature}", "linux-x86_64")


def test_the_download_key_is_not_the_session_key():
    # Separate HKDF info; a download token must never be usable as a session,
    # and the two keys must not be derivable from one another.
    from app.session import _derive_key as session_key
    assert downloads._key(SECRET) != session_key(SECRET)


def test_artifact_lookup_is_a_fixed_table():
    # The key from the URL indexes this table rather than reaching the
    # filesystem, so traversal has nothing to traverse.
    assert downloads.artifact("linux-x86_64") is not None
    assert downloads.artifact("source") is not None
    for bad in ["../../etc/passwd", "linux-x86_64/../..", "", "LINUX-X86_64"]:
        assert downloads.artifact(bad) is None


def test_available_reports_only_what_is_on_disk(tmp_path, monkeypatch):
    monkeypatch.setattr(downloads, "DOWNLOAD_DIR", tmp_path)
    assert downloads.available() == []

    (tmp_path / "mecloud-desktop-source.zip").write_bytes(b"zip")
    listed = downloads.available()
    assert [a["key"] for a in listed] == ["source"]
    assert listed[0]["size"] == 3
