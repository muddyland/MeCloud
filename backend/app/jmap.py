import asyncio
import hashlib
import time
from collections import OrderedDict

from .config import get_settings
from .http import UpstreamError, client, raise_for_status, stream_client

# JMAP session documents keyed by a hash of the access token, never the token
# itself: the raw bearer credential should not be sitting around as a dict key
# in a process that also renders logs and tracebacks.
_JMAP_CACHE_MAXSIZE = 512
_JMAP_CACHE_TTL = 600          # seconds; the session doc changes rarely but not never

_jmap_session_cache: "OrderedDict[str, tuple[dict, float]]" = OrderedDict()

# One in-flight fetch per token — a cold cache used to let N concurrent requests
# each fetch the session document independently.
_session_locks: dict[str, asyncio.Lock] = {}

# Bounds concurrent SSE streams process-wide.
_open_streams = 0


def _token_key(access_token: str) -> str:
    return hashlib.sha256(access_token.encode()).hexdigest()


def _cache_get(access_token: str) -> dict | None:
    key = _token_key(access_token)
    entry = _jmap_session_cache.get(key)
    if entry is None:
        return None
    session, expires_at = entry
    if time.monotonic() > expires_at:
        _jmap_session_cache.pop(key, None)
        return None
    _jmap_session_cache.move_to_end(key)      # LRU: least recently *used* is evicted
    return session


def _cache_set(access_token: str, session: dict) -> None:
    key = _token_key(access_token)
    _jmap_session_cache[key] = (session, time.monotonic() + _JMAP_CACHE_TTL)
    _jmap_session_cache.move_to_end(key)
    while len(_jmap_session_cache) > _JMAP_CACHE_MAXSIZE:
        _jmap_session_cache.popitem(last=False)


def invalidate_session(access_token: str) -> None:
    """Drop the cached JMAP session for a token (used when a token is refreshed)."""
    key = _token_key(access_token)
    _jmap_session_cache.pop(key, None)
    _session_locks.pop(key, None)


def _auth(credential: str) -> dict[str, str]:
    """The Authorization header for an upstream call.

    Takes either a bare OAuth access token, or a complete credential that
    already names its scheme. A paired desktop client authenticates with an app
    password over Basic, not a bearer token, and this is the single place the
    header is built — so accepting both here is what keeps that from spreading
    through every caller.
    """
    if credential.startswith(("Bearer ", "Basic ")):
        return {"Authorization": credential}
    return {"Authorization": f"Bearer {credential}"}


async def get_jmap_session(access_token: str) -> dict:
    settings = get_settings()
    response = await client().get(settings.jmap_session_url, headers=_auth(access_token))
    raise_for_status(response, "JMAP session fetch")
    session = response.json()
    if not isinstance(session, dict) or "apiUrl" not in session:
        raise UpstreamError("JMAP session document is malformed")
    _cache_set(access_token, session)
    return session


async def _session_for(access_token: str) -> dict:
    """Cached JMAP session document, fetching at most once per token concurrently."""
    cached = _cache_get(access_token)
    if cached is not None:
        return cached

    key = _token_key(access_token)
    lock = _session_locks.setdefault(key, asyncio.Lock())
    async with lock:
        cached = _cache_get(access_token)          # another waiter may have filled it
        if cached is not None:
            return cached
        try:
            return await get_jmap_session(access_token)
        finally:
            _session_locks.pop(key, None)


def _account_id(session: dict) -> str:
    accounts = session.get("accounts") or {}
    if not accounts:
        raise UpstreamError("JMAP session has no accounts")
    return next(iter(accounts))


async def jmap_request(access_token: str, payload: dict) -> dict:
    session = await _session_for(access_token)

    response = await client().post(
        session["apiUrl"],
        json=payload,
        headers={**_auth(access_token), "Content-Type": "application/json"},
    )
    if response.status_code in (401, 403):
        # The token was revoked or expired upstream. Drop the cached session so
        # a subsequent login does not reuse it, and let the caller turn this
        # into a 401 rather than a confusing 502.
        invalidate_session(access_token)
    raise_for_status(response, "JMAP request")
    return response.json()


async def create_app_password(access_token: str, description: str) -> dict:
    """Create an app password, and return {id, secret}.

    A paired device gets one of these rather than a copy of the browser's OAuth
    refresh token. Two reasons, and the second is the bug that made this
    necessary: a refresh token is shared with the session it came from, so the
    browser refreshing or signing out invalidates the device with it; and an app
    password is listed and revocable in the app's own App Passwords screen, so
    "disconnect that computer" becomes something the user can actually do.
    """
    session = await _session_for(access_token)
    account_id = _account_id(session)
    # Every advertised capability, rather than guessing the URI for Stalwart's
    # x:AppPassword extension — the same approach the web client takes.
    using = list(session.get("capabilities", {}).keys()) or ["urn:ietf:params:jmap:core"]

    data = await jmap_request(access_token, {
        "using": using,
        "methodCalls": [[
            "x:AppPassword/set",
            {"accountId": account_id,
             "create": {"new": {"description": description[:120], "expiresAt": None}}},
            "0",
        ]],
    })
    response = (data.get("methodResponses") or [[None, {}]])[0][1]
    if response.get("notCreated", {}).get("new"):
        raise UpstreamError("The mail server would not create a password for this device")
    created = (response.get("created") or {}).get("new") or {}
    if not created.get("secret"):
        raise UpstreamError("The mail server created a device password but did not return it")

    # The account name to pair the password with. It comes from the JMAP session
    # document rather than our own session, which never stored one — leaving the
    # username empty produced a credential of ":secret" that the mail server
    # rejected on the very first call.
    username = session.get("username")
    if not username:
        raise UpstreamError("The mail server did not say which account this session belongs to")
    return {**created, "username": username}


async def delete_app_password(access_token: str, password_id: str) -> None:
    """Remove a device's app password, so disconnecting actually disconnects."""
    session = await _session_for(access_token)
    account_id = _account_id(session)
    using = list(session.get("capabilities", {}).keys()) or ["urn:ietf:params:jmap:core"]
    await jmap_request(access_token, {
        "using": using,
        "methodCalls": [[
            "x:AppPassword/set",
            {"accountId": account_id, "destroy": [password_id]},
            "0",
        ]],
    })


async def get_sieve_script(access_token: str) -> dict | None:
    """Return {id, name, content, isActive} for the active/first Sieve script."""
    session = await _session_for(access_token)
    account_id = _account_id(session)

    if "urn:ietf:params:jmap:sieve" not in session.get("capabilities", {}):
        return None

    result = await jmap_request(access_token, {
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve"],
        "methodCalls": [["SieveScript/get", {"accountId": account_id, "ids": None}, "s"]],
    })
    scripts = result.get("methodResponses", [[]])[0][1].get("list", [])

    if not scripts:
        return {"id": None, "name": "My Rules", "content": "", "isActive": False}

    script = next((s for s in scripts if s.get("isActive")), scripts[0])
    blob_id = script.get("blobId")
    content = ""

    if blob_id:
        download_url = (
            session.get("downloadUrl", "")
            .replace("{accountId}", account_id)
            .replace("{blobId}", blob_id)
            .replace("{name}", "script.sieve")
            .replace("{type}", "application%2Fsieve")
        )
        resp = await client().get(download_url, headers=_auth(access_token))
        raise_for_status(resp, "Sieve script download")
        content = resp.text

    return {
        "id": script["id"],
        "name": script.get("name", "My Rules"),
        "content": content,
        "isActive": script.get("isActive", False),
    }


async def save_sieve_script(
    access_token: str,
    script_id: str | None,
    name: str,
    content: str,
    make_active: bool = True,
) -> dict:
    """Upload Sieve content and create/update the SieveScript object."""
    session = await _session_for(access_token)
    account_id = _account_id(session)

    upload_url = session.get("uploadUrl", "").replace("{accountId}", account_id)
    up = await client().post(
        upload_url,
        content=content.encode(),
        headers={**_auth(access_token), "Content-Type": "application/sieve"},
    )
    raise_for_status(up, "Sieve blob upload")
    blob_id = up.json()["blobId"]

    if script_id:
        method_call = ["SieveScript/set", {
            "accountId": account_id,
            "update": {script_id: {"blobId": blob_id}},
        }, "s"]
    else:
        method_call = ["SieveScript/set", {
            "accountId": account_id,
            "create": {"new": {"name": name, "blobId": blob_id}},
        }, "s"]

    result = await jmap_request(access_token, {
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve"],
        "methodCalls": [method_call],
    })
    created = result.get("methodResponses", [[]])[0][1].get("created", {}) or {}
    actual_id = script_id or (created.get("new") or {}).get("id")

    if make_active and actual_id:
        await jmap_request(access_token, {
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve"],
            "methodCalls": [["SieveScript/set", {
                "accountId": account_id,
                "onSuccessActivateScript": actual_id,
            }, "sa"]],
        })

    return {"id": actual_id, "name": name}


async def jmap_event_stream(access_token: str):
    global _open_streams

    session = await _session_for(access_token)
    event_url = session.get("eventSourceUrl", "")
    if not event_url:
        return

    # Replace placeholders in the EventSource URL template
    event_url = event_url.replace("{types}", "*").replace("{closeafter}", "no").replace("{ping}", "30")

    # Refuse rather than queue: a client waiting on a stream slot looks like a
    # hung connection, and every queued waiter is memory we never get back.
    if _open_streams >= get_settings().max_event_streams:
        raise UpstreamError("Too many concurrent event streams", 503)

    _open_streams += 1
    try:
        async with stream_client().stream(
            "GET",
            event_url,
            headers={**_auth(access_token), "Accept": "text/event-stream"},
        ) as response:
            if response.status_code in (401, 403):
                invalidate_session(access_token)
            raise_for_status(response, "JMAP event stream")
            async for line in response.aiter_lines():
                yield line
    finally:
        _open_streams -= 1
