import httpx
from .config import get_settings

_jmap_session_cache: dict[str, dict] = {}
_JMAP_CACHE_MAXSIZE = 512


def _cache_set(token: str, session: dict) -> None:
    if len(_jmap_session_cache) >= _JMAP_CACHE_MAXSIZE:
        _jmap_session_cache.pop(next(iter(_jmap_session_cache)))
    _jmap_session_cache[token] = session


async def get_jmap_session(access_token: str) -> dict:
    settings = get_settings()
    async with httpx.AsyncClient(follow_redirects=True) as client:
        response = await client.get(
            settings.jmap_session_url,
            headers={"Authorization": f"Bearer {access_token}"},
        )
        response.raise_for_status()
        session = response.json()
        _cache_set(access_token, session)
        return session


async def jmap_request(access_token: str, payload: dict) -> dict:
    if access_token not in _jmap_session_cache:
        await get_jmap_session(access_token)

    session = _jmap_session_cache[access_token]
    api_url = session["apiUrl"]

    async with httpx.AsyncClient() as client:
        response = await client.post(
            api_url,
            json=payload,
            headers={
                "Authorization": f"Bearer {access_token}",
                "Content-Type": "application/json",
            },
        )
        response.raise_for_status()
        return response.json()


async def get_sieve_script(access_token: str) -> dict | None:
    """Return {id, name, content, isActive} for the active/first Sieve script."""
    if access_token not in _jmap_session_cache:
        await get_jmap_session(access_token)
    session = _jmap_session_cache[access_token]
    account_id = next(iter(session["accounts"]))

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
        async with httpx.AsyncClient(follow_redirects=True) as client:
            resp = await client.get(
                download_url,
                headers={"Authorization": f"Bearer {access_token}"},
            )
            resp.raise_for_status()
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
    if access_token not in _jmap_session_cache:
        await get_jmap_session(access_token)
    session = _jmap_session_cache[access_token]
    account_id = next(iter(session["accounts"]))

    upload_url = session.get("uploadUrl", "").replace("{accountId}", account_id)
    async with httpx.AsyncClient() as client:
        up = await client.post(
            upload_url,
            content=content.encode(),
            headers={
                "Authorization": f"Bearer {access_token}",
                "Content-Type": "application/sieve",
            },
        )
        up.raise_for_status()
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
    if access_token not in _jmap_session_cache:
        await get_jmap_session(access_token)

    session = _jmap_session_cache[access_token]
    event_url = session.get("eventSourceUrl", "")
    if not event_url:
        return

    # Replace placeholders in the EventSource URL template
    event_url = event_url.replace("{types}", "*").replace("{closeafter}", "no").replace("{ping}", "30")

    async with httpx.AsyncClient(timeout=None) as client:
        async with client.stream(
            "GET",
            event_url,
            headers={
                "Authorization": f"Bearer {access_token}",
                "Accept": "text/event-stream",
            },
        ) as response:
            async for line in response.aiter_lines():
                yield line
