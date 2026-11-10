//! Encrypted cookie sessions.
//!
//! The session holds OAuth access and refresh tokens, so the cookie is sealed
//! with Fernet rather than merely signed: a signed-only cookie is plain base64
//! that anyone holding it can read. A stolen cookie is still a stolen session,
//! but it is no longer a bearer token that can be replayed straight against
//! the mail server, and the tokens never appear in plaintext in browser
//! storage, proxy logs or a disk image.
//!
//! The format is the Python backend's exactly (same key derivation, same JSON
//! object inside), so sessions survive the change of server.

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{Map, Value};

use crate::crypto::{now_secs, Fernet};
use crate::AppState;

/// Browsers drop cookies over ~4 KB. Warn well before that, so a large
/// upstream token does not silently sign everyone out.
const COOKIE_WARN_BYTES: usize = 3_500;

/// HKDF `info` for the cookie key. An input to the key derivation, not
/// branding: renaming it re-keys every session in existence.
const SESSION_INFO: &[u8] = b"jmap-webmail.session.v1";

/// The request's session: a JSON object, shared between the middleware and
/// the handler so the middleware sees what the handler changed.
#[derive(Clone, Default)]
pub struct Session(Arc<Mutex<Map<String, Value>>>);

impl Session {
    pub fn from_map(map: Map<String, Value>) -> Session {
        Session(Arc::new(Mutex::new(map)))
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.0.lock().unwrap().get(key).cloned()
    }

    /// A string value, if present, a string, and not empty.
    pub fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.as_f64())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.lock().unwrap().contains_key(key)
    }

    pub fn insert(&self, key: &str, value: impl Into<Value>) {
        self.0.lock().unwrap().insert(key.to_string(), value.into());
    }

    pub fn remove(&self, key: &str) -> Option<Value> {
        self.0.lock().unwrap().shift_remove(key)
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    pub fn keys(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }

    pub fn snapshot(&self) -> Map<String, Value> {
        self.0.lock().unwrap().clone()
    }
}

/// Put on a response to sign the user out on the way back.
#[derive(Clone, Copy, Debug)]
pub struct ClearSession;

pub struct SessionCodec {
    fernet: Fernet,
    pub cookie_name: String,
    pub max_age: u64,
    flags: String,
}

impl SessionCodec {
    pub fn new(secret: &str, https_only: bool, max_age: u64) -> SessionCodec {
        let mut flags = "httponly; samesite=lax".to_string();
        let mut cookie_name = "session".to_string();
        if https_only {
            flags.push_str("; secure");
            // __Host- pins the cookie to this exact origin: no Domain, Path=/,
            // Secure. A sibling subdomain cannot set it.
            cookie_name = "__Host-session".into();
        }
        SessionCodec {
            fernet: Fernet::derived(secret, SESSION_INFO),
            cookie_name,
            max_age,
            flags,
        }
    }

    /// The session and when it was issued. Anything unreadable — expired,
    /// tampered with, sealed under a rotated secret — is an empty session.
    pub fn decode(&self, token: &str) -> (Map<String, Value>, Option<u64>) {
        self.decode_at(token, now_secs())
    }

    pub fn decode_at(&self, token: &str, now: u64) -> (Map<String, Value>, Option<u64>) {
        let Some((plain, issued)) = self.fernet.decrypt_at(token, Some(self.max_age), now) else {
            return (Map::new(), None);
        };
        match serde_json::from_slice::<Value>(&plain) {
            Ok(Value::Object(map)) => (map, Some(issued)),
            _ => (Map::new(), None),
        }
    }

    pub fn encode(&self, session: &Map<String, Value>) -> String {
        let json = serde_json::to_vec(session).expect("a JSON object serialises");
        self.fernet.encrypt(&json)
    }

    pub fn set_cookie(&self, session: &Map<String, Value>) -> String {
        let token = self.encode(session);
        if token.len() > COOKIE_WARN_BYTES {
            tracing::warn!(
                "Session cookie is {} bytes — approaching the ~4 KB browser limit.",
                token.len()
            );
        }
        format!(
            "{}={}; path=/; Max-Age={}; {}",
            self.cookie_name, token, self.max_age, self.flags
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!(
            "{}=null; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT; {}",
            self.cookie_name, self.flags
        )
    }
}

/// A cookie's value from the request. The last one wins when it repeats.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(COOKIE) {
        let Ok(text) = header.to_str() else { continue };
        for part in text.split(';') {
            let (key, value) = part.split_once('=').unwrap_or(("", part));
            if key.trim() == name {
                let value = value.trim();
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value);
                found = Some(value.to_string());
            }
        }
    }
    found
}

pub async fn middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let codec = &state.session_codec;
    let (map, issued_at, initially_empty) = match cookie(request.headers(), &codec.cookie_name) {
        Some(token) => {
            let (map, issued) = codec.decode(&token);
            let empty = map.is_empty();
            (map, issued, empty)
        }
        None => (Map::new(), None, true),
    };
    let initial = map.clone();
    let session = Session::from_map(map);
    request.extensions_mut().insert(session.clone());

    let mut response = next.run(request).await;

    if response.extensions().get::<ClearSession>().is_some() {
        session.clear();
    }
    let current = session.snapshot();
    let header = if !current.is_empty() {
        // Re-issued once past half its life, so an active user is never
        // signed out mid-task, without a Set-Cookie on every response.
        let stale = issued_at.is_some_and(|t| now_secs().saturating_sub(t) > codec.max_age / 2);
        (current != initial || stale || issued_at.is_none()).then(|| codec.set_cookie(&current))
    } else if !initially_empty {
        Some(codec.clear_cookie())
    } else {
        None
    };
    if let Some(value) = header.and_then(|h| HeaderValue::from_str(&h).ok()) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_parsed_like_starlette() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("a=1; session=abc==; b; session=\"last=\""),
        );
        assert_eq!(cookie(&headers, "session").as_deref(), Some("last="));
        assert_eq!(cookie(&headers, "a").as_deref(), Some("1"));
        assert_eq!(cookie(&headers, "nope"), None);
    }

    #[test]
    fn the_cookie_hides_its_contents() {
        let codec = SessionCodec::new("unit-test-secret-that-is-long-enough", false, 60);
        let mut map = Map::new();
        map.insert(
            "access_token".into(),
            "super-secret-access-token-value".into(),
        );
        let header = codec.set_cookie(&map);
        assert!(!header.contains("super-secret") && !header.contains("access_token"));
        let token = header.split_once('=').unwrap().1.split(';').next().unwrap();
        assert_eq!(codec.decode(token).0, map);
        let other = SessionCodec::new("a-completely-different-secret", false, 60);
        assert!(other.decode(token).0.is_empty());
    }

    #[test]
    fn https_only_sets_secure_and_the_host_prefix() {
        let codec = SessionCodec::new("x", true, 60);
        let header = codec.set_cookie(&Map::new());
        assert!(header.starts_with("__Host-session="));
        assert!(header.contains("; secure") && header.contains("httponly"));
        assert!(codec.clear_cookie().contains("expires=Thu, 01 Jan 1970"));
    }
}
