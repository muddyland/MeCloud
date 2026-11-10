//! The JMAP session cache, the request proxy, app passwords, Sieve and the
//! event stream.

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::crypto::sha256_hex;
use crate::error::{AppError, Result};
use crate::permissions::{is_forbidden, refused_permissions, replace_permissions};
use crate::upstream::{authorization, check};
use crate::AppState;

const CACHE_MAXSIZE: usize = 512;
/// The session document changes rarely, but not never.
const CACHE_TTL: Duration = Duration::from_secs(600);

/// JMAP session documents, keyed by a hash of the credential and never the
/// credential itself, least recently used evicted first.
#[derive(Default)]
pub struct SessionCache {
    entries: Mutex<IndexMap<String, (Arc<Value>, Instant)>>,
    /// One in-flight fetch per credential: a cold cache used to let N
    /// concurrent requests each fetch the document.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub fn token_key(credential: &str) -> String {
    sha256_hex(credential.as_bytes())
}

impl SessionCache {
    pub fn get(&self, credential: &str) -> Option<Arc<Value>> {
        self.get_at(credential, Instant::now())
    }

    fn get_at(&self, credential: &str, now: Instant) -> Option<Arc<Value>> {
        let key = token_key(credential);
        let mut entries = self.entries.lock().unwrap();
        let index = entries.get_index_of(&key)?;
        if now > entries[index].1 {
            entries.shift_remove_index(index);
            return None;
        }
        let last = entries.len() - 1;
        entries.move_index(index, last);
        Some(entries[last].0.clone())
    }

    pub fn set(&self, credential: &str, session: Arc<Value>) {
        let key = token_key(credential);
        let mut entries = self.entries.lock().unwrap();
        entries.shift_remove(&key);
        entries.insert(key, (session, Instant::now() + CACHE_TTL));
        while entries.len() > CACHE_MAXSIZE {
            entries.shift_remove_index(0);
        }
    }

    /// Drop the document for a credential: refreshed, revoked or expired.
    pub fn invalidate(&self, credential: &str) {
        let key = token_key(credential);
        self.entries.lock().unwrap().shift_remove(&key);
        self.locks.lock().unwrap().remove(&key);
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Fetch the session document afresh, and cache it.
pub async fn get_jmap_session(st: &AppState, credential: &str) -> Result<Arc<Value>> {
    let response = st
        .upstream
        .client
        .get(st.settings.jmap_session_url())
        .header("Authorization", authorization(credential))
        .send()
        .await?;
    let session: Value = check(response, "JMAP session fetch")?.json().await?;
    if !session.get("apiUrl").is_some_and(|_| session.is_object()) {
        return Err(AppError::upstream(
            "JMAP session document is malformed",
            None,
        ));
    }
    let session = Arc::new(session);
    st.jmap_sessions.set(credential, session.clone());
    Ok(session)
}

/// The cached session document, fetched at most once per credential at a time.
pub async fn session_for(st: &AppState, credential: &str) -> Result<Arc<Value>> {
    if let Some(cached) = st.jmap_sessions.get(credential) {
        return Ok(cached);
    }
    let key = token_key(credential);
    let lock = st
        .jmap_sessions
        .locks
        .lock()
        .unwrap()
        .entry(key.clone())
        .or_default()
        .clone();
    let _guard = lock.lock().await;
    if let Some(cached) = st.jmap_sessions.get(credential) {
        return Ok(cached);
    }
    let result = get_jmap_session(st, credential).await;
    let mut locks = st.jmap_sessions.locks.lock().unwrap();
    if locks.get(&key).is_some_and(|l| Arc::ptr_eq(l, &lock)) {
        locks.remove(&key);
    }
    result
}

pub fn account_id(session: &Value) -> Result<String> {
    session
        .get("accounts")
        .and_then(Value::as_object)
        .and_then(|accounts| accounts.keys().next().cloned())
        .ok_or_else(|| AppError::upstream("JMAP session has no accounts", None))
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Every advertised capability, rather than guessing the URI of Stalwart's
/// x:AppPassword extension — the same approach the web client takes.
fn all_capabilities(session: &Value) -> Vec<Value> {
    let caps: Vec<Value> = session
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|c| c.keys().map(|k| Value::String(k.clone())).collect())
        .unwrap_or_default();
    if caps.is_empty() {
        vec![json!("urn:ietf:params:jmap:core")]
    } else {
        caps
    }
}

pub async fn jmap_request(st: &AppState, credential: &str, payload: &Value) -> Result<Value> {
    let session = session_for(st, credential).await?;
    let api_url = session
        .get("apiUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response = st
        .upstream
        .client
        .post(api_url)
        .header("Authorization", authorization(credential))
        .json(payload)
        .send()
        .await?;
    if matches!(response.status().as_u16(), 401 | 403) {
        // Revoked or expired upstream: do not reuse the document after a
        // fresh sign-in, and let the caller turn this into a 401.
        st.jmap_sessions.invalidate(credential);
    }
    Ok(check(response, "JMAP request")?.json().await?)
}

/// The first method response's arguments, or an empty object.
fn first_response(data: &Value) -> Value {
    data.get("methodResponses")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get(1))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// Create an app password for a device; returns the created object with the
/// account's `username` added.
///
/// A device gets one of these rather than a copy of the browser's OAuth
/// refresh token: that token belongs to the session it came from, so a
/// refresh or sign-out there killed the device, and an app password is listed
/// and revocable in the App Passwords screen.
pub async fn create_app_password(
    st: &AppState,
    credential: &str,
    description: &str,
) -> Result<Map<String, Value>> {
    let session = session_for(st, credential).await?;
    let account = account_id(&session)?;
    let using = all_capabilities(&session);
    let mut props = Map::new();
    props.insert(
        "description".into(),
        json!(crate::devices::truncate_chars(description, 120)),
    );
    props.insert("expiresAt".into(), Value::Null);

    let response = create_app_password_call(st, credential, &using, &account, props).await?;
    if response.pointer("/notCreated/new").is_some_and(truthy) {
        return Err(AppError::upstream(
            "The mail server would not create a password for this device",
            None,
        ));
    }
    let mut created = response
        .pointer("/created/new")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !created.get("secret").is_some_and(truthy) {
        return Err(AppError::upstream(
            "The mail server created a device password but did not return it",
            None,
        ));
    }
    // From the session document: our own session never stored a username, and
    // an empty one made a credential of ":secret" the server refused.
    let username = str_field(&session, "username");
    if username.is_empty() {
        return Err(AppError::upstream(
            "The mail server did not say which account this session belongs to",
            None,
        ));
    }
    created.insert("username".into(), json!(username));
    Ok(created)
}

/// Python truthiness for JSON: null, false, 0, "" and empty containers are false.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// x:AppPassword/set create, returning the method response.
///
/// The password first asks for the account's permissions, which Stalwart 0.16
/// and tokens with full access accept. Stalwart 1.0.3 refuses that from a
/// scoped token (forbidden), so it is retried with the interoperable
/// permissions listed, and once more without any the server names as not
/// grantable (permissions the account itself lacks).
async fn create_app_password_call(
    st: &AppState,
    credential: &str,
    using: &[Value],
    account: &str,
    props: Map<String, Value>,
) -> Result<Value> {
    let mut permissions: Option<Value> = None;
    let mut exclude: BTreeSet<String> = BTreeSet::new();
    let mut response = json!({});
    for _ in 0..3 {
        let mut new = props.clone();
        if let Some(p) = &permissions {
            new.insert("permissions".into(), p.clone());
        }
        let data = jmap_request(
            st,
            credential,
            &json!({
                "using": using,
                "methodCalls": [["x:AppPassword/set", {"accountId": account, "create": {"new": new}}, "0"]],
            }),
        )
        .await?;
        response = first_response(&data);
        let error = response.pointer("/notCreated/new");
        if !is_forbidden(error) {
            return Ok(response);
        }
        if permissions.is_none() {
            permissions = Some(replace_permissions(&BTreeSet::new()));
            continue;
        }
        let refused: BTreeSet<String> = refused_permissions(error)
            .difference(&exclude)
            .cloned()
            .collect();
        if refused.is_empty() {
            return Ok(response);
        }
        exclude.extend(refused);
        permissions = Some(replace_permissions(&exclude));
    }
    Ok(response)
}

const SIEVE_USING: [&str; 2] = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve"];

/// {id, name, content, isActive} of the active (or first) script; None if the
/// server has no Sieve.
pub async fn get_sieve_script(st: &AppState, credential: &str) -> Result<Option<Value>> {
    let session = session_for(st, credential).await?;
    let account = account_id(&session)?;
    if session
        .pointer("/capabilities/urn:ietf:params:jmap:sieve")
        .is_none()
    {
        return Ok(None);
    }
    let result = jmap_request(
        st,
        credential,
        &json!({
            "using": SIEVE_USING,
            "methodCalls": [["SieveScript/get", {"accountId": account, "ids": null}, "s"]],
        }),
    )
    .await?;
    let scripts = first_response(&result)
        .get("list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(first) = scripts.first() else {
        return Ok(Some(
            json!({"id": null, "name": "My Rules", "content": "", "isActive": false}),
        ));
    };
    let script = scripts
        .iter()
        .find(|s| s.get("isActive").is_some_and(truthy))
        .unwrap_or(first);

    let mut content = String::new();
    let blob_id = str_field(script, "blobId");
    if !blob_id.is_empty() {
        let url = str_field(&session, "downloadUrl")
            .replace("{accountId}", &account)
            .replace("{blobId}", blob_id)
            .replace("{name}", "script.sieve")
            .replace("{type}", "application%2Fsieve");
        let response = st
            .upstream
            .client
            .get(url)
            .header("Authorization", authorization(credential))
            .send()
            .await?;
        content = check(response, "Sieve script download")?.text().await?;
    }
    Ok(Some(json!({
        "id": script.get("id").cloned().unwrap_or(Value::Null),
        "name": script.get("name").cloned().unwrap_or_else(|| json!("My Rules")),
        "content": content,
        "isActive": script.get("isActive").cloned().unwrap_or(json!(false)),
    })))
}

/// Upload the script and create or update the SieveScript, then activate it.
pub async fn save_sieve_script(
    st: &AppState,
    credential: &str,
    script_id: Option<&str>,
    name: &str,
    content: &str,
    make_active: bool,
) -> Result<Value> {
    let session = session_for(st, credential).await?;
    let account = account_id(&session)?;
    let upload_url = str_field(&session, "uploadUrl").replace("{accountId}", &account);
    let response = st
        .upstream
        .client
        .post(upload_url)
        .header("Authorization", authorization(credential))
        .header("Content-Type", "application/sieve")
        .body(content.to_string())
        .send()
        .await?;
    let uploaded: Value = check(response, "Sieve blob upload")?.json().await?;
    let blob_id = uploaded
        .get("blobId")
        .cloned()
        .ok_or_else(|| AppError::upstream("Sieve upload returned no blobId", None))?;

    let call = match script_id {
        Some(id) => {
            json!(["SieveScript/set", {"accountId": account, "update": {id: {"blobId": blob_id}}}, "s"])
        }
        None => {
            json!(["SieveScript/set", {"accountId": account, "create": {"new": {"name": name, "blobId": blob_id}}}, "s"])
        }
    };
    let result = jmap_request(
        st,
        credential,
        &json!({"using": SIEVE_USING, "methodCalls": [call]}),
    )
    .await?;
    let actual_id = match script_id {
        Some(id) => Value::String(id.to_string()),
        None => first_response(&result)
            .pointer("/created/new/id")
            .cloned()
            .unwrap_or(Value::Null),
    };
    if make_active && truthy(&actual_id) {
        jmap_request(
            st,
            credential,
            &json!({
                "using": SIEVE_USING,
                "methodCalls": [["SieveScript/set", {"accountId": account, "onSuccessActivateScript": actual_id}, "sa"]],
            }),
        )
        .await?;
    }
    Ok(json!({"id": actual_id, "name": name}))
}

/// One of `max` stream slots, released on drop.
struct StreamSlot<'a>(&'a AtomicUsize);

impl<'a> StreamSlot<'a> {
    fn acquire(counter: &'a AtomicUsize, max: usize) -> Option<StreamSlot<'a>> {
        let mut current = counter.load(Ordering::SeqCst);
        loop {
            if current >= max {
                return None;
            }
            match counter.compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Some(StreamSlot(counter)),
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for StreamSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The upstream event stream as a response body. The response is already 200
/// when this starts, so a failure ends the stream (the client reconnects)
/// rather than changing the status.
pub fn event_stream(st: Arc<AppState>, credential: String) -> Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    tokio::spawn(async move {
        match pump_events(&st, &credential, &tx).await {
            Ok(()) => {}
            Err(e @ AppError::Upstream { .. }) => tracing::info!("JMAP event stream ended: {e}"),
            Err(e) => tracing::error!("JMAP event stream error: {e}"),
        }
    });
    Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<_, Infallible>(chunk), rx))
    }))
}

async fn pump_events(
    st: &AppState,
    credential: &str,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<()> {
    let session = session_for(st, credential).await?;
    let url = str_field(&session, "eventSourceUrl");
    if url.is_empty() {
        return Ok(());
    }
    let url = url
        .replace("{types}", "*")
        .replace("{closeafter}", "no")
        .replace("{ping}", "30");
    // Refuse rather than queue: a client waiting on a slot looks like a hung
    // connection, and every queued waiter is memory never given back.
    let Some(_slot) = StreamSlot::acquire(&st.open_streams, st.settings.max_event_streams) else {
        return Err(AppError::upstream(
            "Too many concurrent event streams",
            Some(503),
        ));
    };
    let response = st
        .upstream
        .stream
        .get(url)
        .header("Authorization", authorization(credential))
        .header("Accept", "text/event-stream")
        .send()
        .await?;
    if matches!(response.status().as_u16(), 401 | 403) {
        st.jmap_sessions.invalidate(credential);
    }
    let mut body = check(response, "JMAP event stream")?.bytes_stream();
    loop {
        tokio::select! {
            // The browser went away: stop holding an upstream connection for it.
            _ = tx.closed() => return Ok(()),
            chunk = body.next() => match chunk {
                None => return Ok(()),
                Some(chunk) => {
                    if tx.send(chunk?).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_round_trips_and_is_not_keyed_by_the_token() {
        let cache = SessionCache::default();
        cache.set("token-a", Arc::new(json!({"apiUrl": "http://x/api"})));
        assert_eq!(
            *cache.get("token-a").unwrap(),
            json!({"apiUrl": "http://x/api"})
        );
        let entries = cache.entries.lock().unwrap();
        assert!(!entries.contains_key("token-a"));
        assert_eq!(entries.keys().next().unwrap().len(), 64);
    }

    #[test]
    fn entries_expire() {
        let cache = SessionCache::default();
        cache.set("token-a", Arc::new(json!({"apiUrl": "x"})));
        assert!(cache
            .get_at(
                "token-a",
                Instant::now() + CACHE_TTL + Duration::from_secs(1)
            )
            .is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted() {
        let cache = SessionCache::default();
        for i in 0..CACHE_MAXSIZE {
            cache.set(&format!("token-{i}"), Arc::new(json!({"apiUrl": i})));
        }
        assert!(cache.get("token-0").is_some());
        cache.set("token-new", Arc::new(json!({"apiUrl": "new"})));
        assert_eq!(cache.len(), CACHE_MAXSIZE);
        assert!(cache.get("token-0").is_some(), "kept: recently used");
        assert!(cache.get("token-1").is_none(), "evicted: oldest untouched");
    }

    #[test]
    fn invalidate_drops_the_entry() {
        let cache = SessionCache::default();
        cache.set("token-a", Arc::new(json!({"apiUrl": "x"})));
        cache.invalidate("token-a");
        assert!(cache.get("token-a").is_none());
    }

    #[test]
    fn stream_slots_are_bounded() {
        let counter = AtomicUsize::new(0);
        let a = StreamSlot::acquire(&counter, 2).unwrap();
        let _b = StreamSlot::acquire(&counter, 2).unwrap();
        assert!(StreamSlot::acquire(&counter, 2).is_none());
        drop(a);
        assert!(StreamSlot::acquire(&counter, 2).is_some());
    }
}
