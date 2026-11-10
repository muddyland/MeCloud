//! A fake Stalwart, and a client for the app under test.
//!
//! The fake speaks just enough of OAuth discovery, the token endpoint, JMAP
//! (session, API, upload, download, event source) for the server to be
//! exercised end to end over real HTTP, rather than with its internals
//! patched out.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use mecloud_server::{router, AppState, Settings};
use serde_json::{json, Map, Value};
use tower::ServiceExt;

pub const USERNAME: &str = "alice@test.lan";

#[derive(Default)]
pub struct Fake {
    pub base: Mutex<String>,
    pub token_calls: AtomicUsize,
    pub token_delay_ms: AtomicU64,
    /// Answers for the token endpoint, in order; then the default.
    pub token_responses: Mutex<VecDeque<(u16, Value)>>,
    pub token_requests: Mutex<Vec<HashMap<String, String>>>,
    pub session_fetches: AtomicUsize,
    pub session_auth: Mutex<Vec<String>>,
    pub jmap_requests: Mutex<Vec<(String, Value)>>,
    pub jmap_responses: Mutex<VecDeque<(u16, Value)>>,
    /// blobId -> (content type, bytes)
    pub blobs: Mutex<HashMap<String, (String, Vec<u8>)>>,
    pub uploads: Mutex<Vec<(String, String, Vec<u8>)>>,
    pub event_auth: Mutex<Vec<String>>,
    pub events_open: AtomicUsize,
}

impl Fake {
    pub fn base(&self) -> String {
        self.base.lock().unwrap().clone()
    }

    pub fn answer_token(&self, status: u16, body: Value) {
        self.token_responses
            .lock()
            .unwrap()
            .push_back((status, body));
    }

    pub fn answer_jmap(&self, status: u16, body: Value) {
        self.jmap_responses
            .lock()
            .unwrap()
            .push_back((status, body));
    }

    pub fn last_jmap(&self) -> (String, Value) {
        self.jmap_requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a JMAP request was made")
    }
}

type F = State<Arc<Fake>>;

fn auth_of(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn discovery(State(f): F) -> Json<Value> {
    let base = f.base();
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
    }))
}

async fn token(State(f): F, body: Bytes) -> Response {
    f.token_calls.fetch_add(1, Ordering::SeqCst);
    let form: HashMap<String, String> = form_urlencoded::parse(&body).into_owned().collect();
    f.token_requests.lock().unwrap().push(form);
    let delay = f.token_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    let (status, body) = f.token_responses.lock().unwrap().pop_front().unwrap_or((
        200,
        json!({"access_token": "new-token", "refresh_token": "rt-2", "expires_in": 3600, "token_type": "bearer"}),
    ));
    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
}

pub fn session_document(base: &str) -> Value {
    json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:sieve": {},
        },
        "accounts": {"a1": {"name": USERNAME}, "a2": {"name": "shared"}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "a1"},
        "username": USERNAME,
        "apiUrl": format!("{base}/jmap/"),
        "downloadUrl": format!("{base}/jmap/download/{{accountId}}/{{blobId}}/{{name}}?accept={{type}}"),
        "uploadUrl": format!("{base}/jmap/upload/{{accountId}}/"),
        "eventSourceUrl": format!("{base}/jmap/eventsource/?types={{types}}&closeafter={{closeafter}}&ping={{ping}}"),
        "state": "s1",
    })
}

fn rejected(auth: &str) -> bool {
    auth.is_empty() || auth.contains("revoked")
}

async fn jmap_session(State(f): F, headers: HeaderMap) -> Response {
    f.session_fetches.fetch_add(1, Ordering::SeqCst);
    let auth = auth_of(&headers);
    f.session_auth.lock().unwrap().push(auth.clone());
    if rejected(&auth) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(session_document(&f.base())).into_response()
}

async fn jmap_api(State(f): F, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let auth = auth_of(&headers);
    f.jmap_requests
        .lock()
        .unwrap()
        .push((auth.clone(), body.clone()));
    if rejected(&auth) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some((status, answer)) = f.jmap_responses.lock().unwrap().pop_front() {
        return (StatusCode::from_u16(status).unwrap(), Json(answer)).into_response();
    }
    // Default: echo the calls back, as Core/echo would.
    let calls = body["methodCalls"].clone();
    Json(json!({"methodResponses": calls, "sessionState": "s1"})).into_response()
}

async fn jmap_download(
    State(f): F,
    headers: HeaderMap,
    Path((account, blob, _name)): Path<(String, String, String)>,
) -> Response {
    if rejected(&auth_of(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert_eq!(account, "a1", "the first account is the one used");
    let Some((content_type, bytes)) = f.blobs.lock().unwrap().get(&blob).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_str(&content_type).unwrap(),
    );
    response
}

async fn jmap_upload(
    State(f): F,
    Path(account): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if rejected(&auth_of(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let size = body.len();
    let blob_id = format!("G{size}");
    f.blobs
        .lock()
        .unwrap()
        .insert(blob_id.clone(), (content_type.clone(), body.to_vec()));
    f.uploads
        .lock()
        .unwrap()
        .push((account.clone(), content_type.clone(), body.to_vec()));
    Json(json!({"accountId": account, "blobId": blob_id, "type": content_type, "size": size}))
        .into_response()
}

struct OpenEvent(Arc<Fake>);
impl Drop for OpenEvent {
    fn drop(&mut self) {
        self.0.events_open.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn jmap_events(State(f): F, headers: HeaderMap, request: Request) -> Response {
    let auth = auth_of(&headers);
    f.event_auth
        .lock()
        .unwrap()
        .push(format!("{auth} {}", request.uri()));
    if rejected(&auth) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    f.events_open.fetch_add(1, Ordering::SeqCst);
    let guard = OpenEvent(f.clone());
    let first = Bytes::from_static(b"event: state\ndata: {\"changed\":{}}\n\n");
    let stream = futures_util::stream::unfold((0u32, guard), move |(n, guard)| {
        let first = first.clone();
        async move {
            if n > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let chunk = if n == 0 {
                first
            } else {
                Bytes::from_static(b": ping\n\n")
            };
            Some((Ok::<_, std::convert::Infallible>(chunk), (n + 1, guard)))
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

pub async fn start_fake() -> Arc<Fake> {
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route("/.well-known/oauth-authorization-server", get(discovery))
        .route("/token", post(token))
        .route("/.well-known/jmap", get(jmap_session))
        .route("/jmap/", post(jmap_api))
        .route("/jmap/download/{account}/{blob}/{name}", get(jmap_download))
        .route("/jmap/upload/{account}/", post(jmap_upload))
        .route("/jmap/eventsource/", get(jmap_events))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *fake.base.lock().unwrap() = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    fake
}

pub const SECRET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub fn settings(stalwart_url: &str, extra: &[(&str, &str)]) -> Settings {
    let mut vars: HashMap<String, String> = [
        ("ENVIRONMENT", "development"),
        ("STALWART_URL", stalwart_url),
        ("OAUTH_CLIENT_ID", "test-client"),
        ("OAUTH_CLIENT_SECRET", "test-secret"),
        ("OAUTH_REDIRECT_URI", "http://localhost:8000/auth/callback"),
        ("SESSION_SECRET", SECRET),
        ("APP_NAME", "Test Mail"),
        ("FRONTEND_STATIC_DIR", "/nonexistent"),
        ("DESKTOP_DOWNLOAD_DIR", "/nonexistent"),
        ("UPSTREAM_CONNECT_TIMEOUT", "2"),
        ("UPSTREAM_READ_TIMEOUT", "5"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    Settings::from_map(&vars).expect("test settings are valid")
}

pub struct TestApp {
    pub state: Arc<AppState>,
    pub router: Router,
    pub fake: Arc<Fake>,
    pub cookie: Mutex<Option<String>>,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text()))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }
}

impl TestApp {
    pub async fn new() -> TestApp {
        TestApp::with(&[]).await
    }

    pub async fn with(extra: &[(&str, &str)]) -> TestApp {
        let fake = start_fake().await;
        let settings = settings(&fake.base(), extra);
        TestApp::from_settings(settings, fake)
    }

    pub fn from_settings(settings: Settings, fake: Arc<Fake>) -> TestApp {
        let state = Arc::new(AppState::new(settings));
        TestApp {
            router: router(state.clone()),
            state,
            fake,
            cookie: Mutex::new(None),
        }
    }

    /// A signed-in browser: an encrypted session cookie holding `session`.
    pub fn sign_in(&self, session: Value) {
        let Value::Object(map) = session else {
            panic!("a session is an object")
        };
        let token = self.state.session_codec.encode(&map);
        *self.cookie.lock().unwrap() =
            Some(format!("{}={token}", self.state.session_codec.cookie_name));
    }

    pub fn sign_in_with_token(&self, token: &str) {
        let expires = mecloud_server::crypto::now_f64() + 3600.0;
        self.sign_in(
            json!({"access_token": token, "refresh_token": "rt-1", "token_expires_at": expires}),
        );
    }

    /// What the current cookie holds.
    pub fn session(&self) -> Map<String, Value> {
        let cookie = self.cookie.lock().unwrap().clone().unwrap_or_default();
        let token = cookie.split_once('=').map(|(_, v)| v).unwrap_or("");
        self.state.session_codec.decode(token).0
    }

    pub async fn send(&self, mut request: axum::http::Request<Body>) -> TestResponse {
        if !request.headers().contains_key("host") {
            request
                .headers_mut()
                .insert("host", HeaderValue::from_static("localhost"));
        }
        if let Some(cookie) = self.cookie.lock().unwrap().clone() {
            if !request.headers().contains_key("cookie") {
                request
                    .headers_mut()
                    .insert("cookie", HeaderValue::from_str(&cookie).unwrap());
            }
        }
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        for set in headers.get_all("set-cookie") {
            let set = set.to_str().unwrap();
            let pair = set.split(';').next().unwrap();
            let mut jar = self.cookie.lock().unwrap();
            if set.contains("expires=Thu, 01 Jan 1970") {
                *jar = None;
            } else {
                *jar = Some(pair.to_string());
            }
        }
        let body = tokio::time::timeout(Duration::from_secs(10), response.into_body().collect())
            .await
            .expect("the response body finishes")
            .unwrap()
            .to_bytes();
        TestResponse {
            status,
            headers,
            body,
        }
    }

    pub async fn get(&self, path: &str) -> TestResponse {
        self.send(axum::http::Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    pub async fn post(&self, path: &str, body: impl Into<Body>) -> TestResponse {
        self.send(axum::http::Request::post(path).body(body.into()).unwrap())
            .await
    }

    pub async fn post_json(&self, path: &str, body: Value) -> TestResponse {
        self.send(
            axum::http::Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    pub async fn put_json(&self, path: &str, body: Value) -> TestResponse {
        self.send(
            axum::http::Request::put(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }
}

pub fn echo_request() -> Value {
    json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {"hello": true}, "c1"]],
    })
}
