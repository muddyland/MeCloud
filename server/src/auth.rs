//! The OAuth2 authorization-code flow (PKCE), token refresh, and the checks
//! on where a login may send the browser afterwards.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::crypto::{ct_eq, random_bytes, sha256, sha256_hex, B64URL_NOPAD};
use crate::error::{AppError, Result};
use crate::session::Session;
use crate::upstream::check;
use crate::AppState;

#[derive(Clone, Debug)]
pub struct Endpoints {
    pub authorization: String,
    pub token: String,
}

/// The authorisation server's endpoints, discovered once. A failed discovery
/// is not cached, so the next login tries again.
#[derive(Default)]
pub struct OAuth {
    endpoints: OnceCell<Endpoints>,
    refresh: RefreshCoalescer,
}

/// Why discovery was refused: not an upstream fault, a document we will not
/// act on.
#[derive(Debug)]
pub enum DiscoveryError {
    Upstream(AppError),
    Invalid(String),
}

impl From<AppError> for DiscoveryError {
    fn from(e: AppError) -> Self {
        DiscoveryError::Upstream(e)
    }
}

impl From<reqwest::Error> for DiscoveryError {
    fn from(e: reqwest::Error) -> Self {
        DiscoveryError::Upstream(e.into())
    }
}

impl From<DiscoveryError> for AppError {
    fn from(e: DiscoveryError) -> Self {
        match e {
            DiscoveryError::Upstream(e) => e,
            DiscoveryError::Invalid(message) => AppError::upstream(message, None),
        }
    }
}

/// Same scheme, host and port. Endpoints must live on the configured mail
/// server: a compromised or misconfigured discovery document could otherwise
/// send the authorization code — and the client secret — anywhere.
fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin() && a.origin().is_tuple(),
        _ => false,
    }
}

pub async fn endpoints(st: &AppState) -> Result<Endpoints, DiscoveryError> {
    st.oauth
        .endpoints
        .get_or_try_init(|| async {
            let response = st
                .upstream
                .client
                .get(st.settings.openid_config_url())
                .send()
                .await?;
            let doc: Value = check(response, "OAuth discovery")?.json().await?;
            let field = |name: &str| -> Result<String, DiscoveryError> {
                let url = doc
                    .get(name)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        DiscoveryError::Invalid(format!(
                            "OAuth discovery document is missing {name}"
                        ))
                    })?;
                if !same_origin(url, &st.settings.stalwart_url) {
                    return Err(DiscoveryError::Invalid(format!(
                        "OAuth {name} points outside STALWART_URL: {url}"
                    )));
                }
                Ok(url.to_string())
            };
            Ok(Endpoints {
                authorization: field("authorization_endpoint")?,
                token: field("token_endpoint")?,
            })
        })
        .await
        .cloned()
}

/// (code_verifier, code_challenge), S256.
pub fn pkce_pair() -> (String, String) {
    let verifier = B64URL_NOPAD.encode(random_bytes::<32>());
    let challenge = B64URL_NOPAD.encode(sha256(verifier.as_bytes()));
    (verifier, challenge)
}

pub async fn authorization_url(st: &AppState, session: &Session) -> Result<String, DiscoveryError> {
    let endpoints = endpoints(st).await?;
    let state = B64URL_NOPAD.encode(random_bytes::<32>());
    let (verifier, challenge) = pkce_pair();
    session.insert("oauth_state", state.clone());
    session.insert("code_verifier", verifier);
    let s = &st.settings;
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", &s.oauth_client_id)
        .append_pair("redirect_uri", &s.oauth_redirect_uri)
        .append_pair("scope", &s.oauth_scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    Ok(format!("{}?{query}", endpoints.authorization))
}

#[derive(Debug)]
pub enum ExchangeError {
    /// The state or PKCE verifier did not check out: start the login again.
    Rejected(String),
    Failed(AppError),
}

impl<E: Into<AppError>> From<E> for ExchangeError {
    fn from(e: E) -> Self {
        ExchangeError::Failed(e.into())
    }
}

pub async fn exchange_code(
    st: &AppState,
    session: &Session,
    code: &str,
    state: &str,
) -> Result<Value, ExchangeError> {
    // Constant time: the state is a CSRF token.
    let expected = session.get_str("oauth_state");
    if !expected.is_some_and(|e| ct_eq(e.as_bytes(), state.as_bytes())) {
        return Err(ExchangeError::Rejected(
            "Invalid OAuth state parameter".into(),
        ));
    }
    let endpoints = endpoints(st).await.map_err(AppError::from)?;
    let Some(verifier) = session.get_str("code_verifier") else {
        return Err(ExchangeError::Rejected(
            "Missing PKCE code verifier in session".into(),
        ));
    };
    // Single use: whether or not the exchange succeeds, this verifier is spent.
    session.remove("code_verifier");
    session.remove("oauth_state");

    let s = &st.settings;
    let response = st
        .upstream
        .client
        .post(&endpoints.token)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &s.oauth_redirect_uri),
            ("client_id", &s.oauth_client_id),
            ("client_secret", &s.oauth_client_secret),
            ("code_verifier", &verifier),
        ])
        .send()
        .await?;
    Ok(check(response, "OAuth token exchange")?.json().await?)
}

// ── Refresh, once per refresh token ─────────────────────────────────────────
//
// A page load fires several API calls at once, all carrying the same cookie
// and so the same refresh token. Refreshing independently, the first spends
// the token (Stalwart rotates them) and the rest fail with invalid_grant —
// which logged users out at random, minutes before expiry. So one refresh runs
// per refresh token, and its result is kept briefly for the requests already
// in flight with the old cookie.

const REFRESH_RESULT_TTL: Duration = Duration::from_secs(120);

#[derive(Default)]
struct RefreshCoalescer {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    results: Mutex<HashMap<String, (Value, Instant)>>,
}

impl RefreshCoalescer {
    fn cached(&self, key: &str) -> Option<Value> {
        let mut results = self.results.lock().unwrap();
        let now = Instant::now();
        results.retain(|_, (_, expires)| *expires >= now);
        results.get(key).map(|(tokens, _)| tokens.clone())
    }
}

#[derive(Debug)]
pub enum RefreshError {
    /// The authorisation server refused the refresh token: sign in again.
    Rejected(String),
    /// A timeout, a 5xx, a DNS failure. A mail server restarting is not a
    /// reason to sign anyone out.
    Transient(String),
}

async fn post_refresh(st: &AppState, refresh_token: &str) -> Result<Value, RefreshError> {
    let endpoints = endpoints(st)
        .await
        .map_err(|e| RefreshError::Transient(AppError::from(e).to_string()))?;
    let s = &st.settings;
    let response = st
        .upstream
        .client
        .post(&endpoints.token)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &s.oauth_client_id),
            ("client_secret", &s.oauth_client_secret),
        ])
        .send()
        .await
        .map_err(|e| RefreshError::Transient(AppError::from(e).to_string()))?;
    match check(response, "OAuth token refresh") {
        Err(e) if matches!(e.upstream_status(), Some(400 | 401 | 403)) => {
            Err(RefreshError::Rejected(e.to_string()))
        }
        Err(e) => Err(RefreshError::Transient(e.to_string())),
        Ok(response) => response
            .json()
            .await
            .map_err(|e| RefreshError::Transient(AppError::from(e).to_string())),
    }
}

/// Exchange a refresh token, at most once however many callers ask.
pub async fn refresh_access_token(
    st: &AppState,
    refresh_token: &str,
) -> Result<Value, RefreshError> {
    let coalescer = &st.oauth.refresh;
    let key = sha256_hex(refresh_token.as_bytes());
    if let Some(tokens) = coalescer.cached(&key) {
        return Ok(tokens);
    }
    let lock = coalescer
        .locks
        .lock()
        .unwrap()
        .entry(key.clone())
        .or_default()
        .clone();
    let result = {
        let _guard = lock.lock().await;
        match coalescer.cached(&key) {
            Some(tokens) => Ok(tokens),
            None => match post_refresh(st, refresh_token).await {
                Ok(tokens)
                    if tokens
                        .get("access_token")
                        .and_then(Value::as_str)
                        .is_some_and(|t| !t.is_empty()) =>
                {
                    coalescer.results.lock().unwrap().insert(
                        key.clone(),
                        (tokens.clone(), Instant::now() + REFRESH_RESULT_TTL),
                    );
                    Ok(tokens)
                }
                Ok(_) => Err(RefreshError::Rejected(
                    "Token endpoint returned no access_token".into(),
                )),
                Err(e) => Err(e),
            },
        }
    };
    let mut locks = coalescer.locks.lock().unwrap();
    // Ours, and nobody else is holding or waiting on it.
    if locks.get(&key).is_some_and(|l| Arc::ptr_eq(l, &lock)) && Arc::strong_count(&lock) <= 2 {
        locks.remove(&key);
    }
    result
}

/// A post-login destination that cannot leave this site. It arrives on a
/// query parameter, so it is attacker-controlled: anything absolute,
/// scheme-relative ("//evil.example"), backslash-smuggled or not rooted is
/// refused — an open redirect on a login flow is a phishing link on the real
/// domain.
pub fn safe_local_path(value: Option<&str>) -> Option<String> {
    let candidate = value?.trim();
    if !candidate.starts_with('/') || candidate.starts_with("//") || candidate.starts_with("/\\") {
        return None;
    }
    if candidate.contains(['\\', '\n', '\r']) {
        return None;
    }
    Some(candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_path_is_kept() {
        for value in [
            "/auth/device?redirect=http%3A%2F%2F127.0.0.1%3A5000%2Fpaired",
            "/",
            "/mail?compose=1",
        ] {
            assert_eq!(safe_local_path(Some(value)).as_deref(), Some(value));
        }
    }

    #[test]
    fn a_destination_that_could_leave_the_site_is_refused() {
        for value in [
            Some("//evil.example.com/x"),
            Some("https://evil.example.com/x"),
            Some("http://evil.example.com"),
            Some("/\\evil.example.com"),
            Some("javascript:alert(1)"),
            Some("mail"),
            Some(""),
            None,
            Some("/x\r\nSet-Cookie: a=b"),
        ] {
            assert_eq!(safe_local_path(value), None, "{value:?}");
        }
    }

    #[test]
    fn origins() {
        assert!(same_origin(
            "https://mail.test/oauth/token",
            "https://mail.test"
        ));
        assert!(same_origin("https://mail.test:443/x", "https://mail.test"));
        assert!(!same_origin("https://evil.test/x", "https://mail.test"));
        assert!(!same_origin("http://mail.test/x", "https://mail.test"));
        assert!(!same_origin(
            "https://mail.test:8443/x",
            "https://mail.test"
        ));
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (verifier, challenge) = pkce_pair();
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge, B64URL_NOPAD.encode(sha256(verifier.as_bytes())));
    }
}
