"""App passwords on Stalwart 1.0.3, where OAuth tokens are scoped."""
import re
from pathlib import Path

import pytest

from app import jmap, stalwart_permissions as sp

REFUSAL = {
    "type": "forbidden",
    "description": "You are not authorized to grant permissions: interactAi, sysAccountSettingsUpdate",
}


@pytest.fixture
def server(monkeypatch):
    sent = []
    answers = []

    async def fake_session(token):
        return {"primaryAccounts": {"urn:ietf:params:jmap:mail": "a"}, "accounts": {"a": {}},
                "capabilities": {"urn:ietf:params:jmap:core": {}}, "username": "alice@example.com"}

    async def fake_request(token, body):
        props = body["methodCalls"][0][1]["create"]["new"]
        sent.append(props)
        return {"methodResponses": [["x:AppPassword/set", answers[len(sent) - 1], "0"]]}

    monkeypatch.setattr(jmap, "_session_for", fake_session)
    monkeypatch.setattr(jmap, "jmap_request", fake_request)
    return sent, answers


CREATED = {"created": {"new": {"id": "p1", "secret": "s3cret"}}}


async def test_the_account_permissions_are_asked_for_first(server):
    sent, answers = server
    answers += [CREATED]
    result = await jmap.create_app_password("tok", "MeCloud on laptop")
    assert result["secret"] == "s3cret" and result["username"] == "alice@example.com"
    assert sent == [{"description": "MeCloud on laptop", "expiresAt": None}]


async def test_a_scoped_token_lists_the_interoperable_permissions(server):
    sent, answers = server
    answers += [{"notCreated": {"new": REFUSAL}}, CREATED]
    await jmap.create_app_password("tok", "MeCloud on laptop")
    assert sent[1]["permissions"]["@type"] == "Replace"
    assert list(sent[1]["permissions"]["permissions"]) == list(sp.INTEROPERABLE_PERMISSIONS)


async def test_permissions_the_account_lacks_are_dropped(server):
    sent, answers = server
    answers += [
        {"notCreated": {"new": REFUSAL}},
        {"notCreated": {"new": {"type": "forbidden", "description": "You are not authorized to grant permissions: davCalAcl."}}},
        CREATED,
    ]
    await jmap.create_app_password("tok", "x")
    assert "davCalAcl" not in sent[2]["permissions"]["permissions"]
    assert sent[2]["permissions"]["permissions"]["jmapFileNodeGet"] is True


async def test_an_unexplained_refusal_is_reported(server):
    sent, answers = server
    answers += [{"notCreated": {"new": REFUSAL}}, {"notCreated": {"new": {"type": "forbidden"}}}]
    with pytest.raises(jmap.UpstreamError):
        await jmap.create_app_password("tok", "x")
    assert len(sent) == 2


def test_the_device_password_cannot_manage_the_account():
    for p in ("sysAppPasswordCreate", "sysAccountPasswordUpdate", "sysAccountSettingsUpdate", "sysApiKeyCreate"):
        assert p not in sp.INTEROPERABLE_PERMISSIONS


def test_the_frontend_list_matches():
    js = Path(__file__).resolve().parents[2] / "frontend" / "src" / "lib" / "stalwartPermissions.js"
    if not js.exists():
        pytest.skip("frontend sources not present")
    block = js.read_text().split("Object.freeze([", 1)[1].split("]);", 1)[0]
    assert tuple(re.findall(r"'([A-Za-z0-9]+)'", block)) == sp.INTEROPERABLE_PERMISSIONS
