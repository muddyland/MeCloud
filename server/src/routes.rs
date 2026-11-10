//! The routes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, LOCATION,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};

use crate::auth::{self, ExchangeError, RefreshError};
use crate::blobs;
use crate::crypto::{now_f64, now_secs};
use crate::devices::{self, DEVICE_TOKEN_MAX_AGE};
use crate::downloads;
use crate::error::{detail, AppError, Result};
use crate::jmap;
use crate::pages;
use crate::ratelimit::Limit;
use crate::session::Session;
use crate::upstream::{authorization, check};
use crate::web::{with_charset, ClientIp};
use crate::AppState;

type St = State<Arc<AppState>>;

/// The query string, last value winning for a repeated name (as Starlette).
fn query(uri: &Uri) -> HashMap<String, String> {
    form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .into_owned()
        .collect()
}

fn redirect(status: StatusCode, location: &str) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

/// One request against an endpoint's limits, keyed by client IP.
fn rate_limit(
    st: &AppState,
    endpoint: &'static str,
    client: &ClientIp,
    limits: &[Limit],
) -> Result<()> {
    st.limiter
        .hit(endpoint, &client.0, limits)
        .map_err(|retry_after| {
            // With a Retry-After the client can act on: a folder drop fires one
            // upload per file, so hitting the limit is ordinary, and the client
            // waits the advised time rather than failing the upload.
            AppError::http(429, "Too many requests — please slow down.")
                .with_header("retry-after", retry_after.to_string())
        })
}

const LOGIN_LIMIT: [Limit; 1] = [Limit::per_minute(20)];

/// The upstream credential for this request, refreshing the OAuth token when
/// it is close to expiry.
///
/// A paired desktop client authenticates with a bearer device token rather
/// than the cookie: it syncs with no window open, so there is no browser
/// session to borrow.
pub async fn require_auth(st: &AppState, headers: &HeaderMap, session: &Session) -> Result<String> {
    let header = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if let Some(presented) = devices::bearer_token(header) {
        return match devices::read(&st.device_key, &presented, DEVICE_TOKEN_MAX_AGE) {
            // The device's own app password: no refresh, no round trip, and
            // valid until revoked in App Passwords.
            Some(device) => Ok(device.basic_credential()),
            None => Err(AppError::http(401, "This device is no longer paired")),
        };
    }

    let token = session
        .get_str("access_token")
        .ok_or_else(AppError::unauthenticated)?;
    let expires_at = session.get_f64("token_expires_at").unwrap_or(0.0);
    // Refreshed five minutes before expiry.
    if expires_at == 0.0 || now_f64() <= expires_at - 300.0 {
        return Ok(token);
    }
    let Some(refresh_token) = session.get_str("refresh_token") else {
        session.clear();
        return Err(AppError::session_expired());
    };
    let tokens = match auth::refresh_access_token(st, &refresh_token).await {
        Ok(tokens) => tokens,
        Err(RefreshError::Rejected(_)) => {
            // The server refused the refresh token itself: only a new sign-in helps.
            session.clear();
            return Err(AppError::session_expired());
        }
        Err(RefreshError::Transient(reason)) => {
            // The mail server is restarting, slow, or unreachable. Not the
            // user's session failing, so not a sign-out: while the current
            // token is inside its lifetime, keep using it.
            tracing::warn!("Token refresh failed transiently: {reason}");
            if now_f64() < expires_at {
                return Ok(token);
            }
            return Err(
                AppError::http(503, "Mail server unavailable, try again shortly")
                    .with_header("retry-after", "10"),
            );
        }
    };
    let new_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    session.insert("access_token", new_token.clone());
    if let Some(rt) = tokens.get("refresh_token") {
        session.insert("refresh_token", rt.clone());
    }
    session.insert("token_expires_at", now_f64() + expires_in(&tokens));
    if new_token != token {
        st.jmap_sessions.invalidate(&token);
    }
    Ok(new_token)
}

fn expires_in(tokens: &Value) -> f64 {
    tokens
        .get("expires_in")
        .and_then(Value::as_f64)
        .unwrap_or(3600.0)
}

pub async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

pub async fn public_config(State(st): St) -> Json<Value> {
    Json(json!({"appName": st.settings.app_name, "stalwartUrl": st.settings.stalwart_url}))
}

pub async fn web_manifest(State(st): St) -> Json<Value> {
    let name = &st.settings.app_name;
    let short = name.split_whitespace().next().unwrap_or("MeCloud");
    let icon = |src: &str, sizes: &str, purpose: &str| json!({"src": src, "sizes": sizes, "type": "image/png", "purpose": purpose});
    Json(json!({
        "name": name,
        "short_name": short,
        "description": format!("{name} — mail, calendar, contacts, notes and files"),
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "orientation": "portrait-primary",
        "background_color": "#ffffff",
        "theme_color": "#2563eb",
        "categories": ["productivity", "utilities"],
        "icons": [
            icon("/icons/icon-192.png", "192x192", "any"),
            icon("/icons/icon-512.png", "512x512", "any"),
            icon("/icons/icon-maskable-192.png", "192x192", "maskable"),
            icon("/icons/icon-maskable-512.png", "512x512", "maskable"),
        ],
    }))
}

// ── Sign-in ─────────────────────────────────────────────────────────────────

pub async fn login(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
) -> Result<Response> {
    rate_limit(&st, "login", &ip, &LOGIN_LIMIT)?;
    let url = auth::authorization_url(&st, &session).await?;
    Ok(redirect(StatusCode::TEMPORARY_REDIRECT, &url))
}

pub async fn callback(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    uri: Uri,
) -> Result<Response> {
    let q = query(&uri);
    let code = q
        .get("code")
        .ok_or_else(|| AppError::missing("query", "code"))?;
    let state = q
        .get("state")
        .ok_or_else(|| AppError::missing("query", "state"))?;
    rate_limit(&st, "callback", &ip, &LOGIN_LIMIT)?;

    let tokens = match auth::exchange_code(&st, &session, code, state).await {
        Ok(tokens) => tokens,
        Err(ExchangeError::Rejected(reason)) => {
            // State mismatch or missing PKCE, most often after a restart
            // mid-login: log what the session held, and start over.
            let prefix: String = state.chars().take(8).collect();
            tracing::warn!(
                "OAuth callback rejected ({reason}). session_keys={:?}  url_state={prefix}…",
                session.keys()
            );
            session.clear();
            return Ok(redirect(StatusCode::TEMPORARY_REDIRECT, "/auth/login"));
        }
        Err(ExchangeError::Failed(e)) => {
            tracing::error!("OAuth callback error: {e}");
            return Err(AppError::http(502, "Authentication failed"));
        }
    };
    let Some(access_token) = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    else {
        tracing::error!("OAuth token endpoint returned no access_token");
        return Err(AppError::http(502, "Authentication failed"));
    };

    // Read before the rotation below clears it, and validated: it came in on
    // a query parameter.
    let destination = auth::safe_local_path(session.get_str("after_login").as_deref())
        .unwrap_or_else(|| "/".into());

    // Rotate on privilege change, so a pre-login cookie planted by an
    // attacker cannot be upgraded into a signed-in one.
    session.clear();
    session.insert("access_token", access_token);
    session.insert("token_expires_at", now_f64() + expires_in(&tokens));
    if let Some(rt) = tokens.get("refresh_token") {
        session.insert("refresh_token", rt.clone());
    }
    Ok(redirect(StatusCode::TEMPORARY_REDIRECT, &destination))
}

/// POST only: a GET logout is triggered cross-site by any `<img>`.
pub async fn logout(Extension(session): Extension<Session>) -> Json<Value> {
    session.clear();
    Json(json!({"ok": true}))
}

pub async fn me(Extension(session): Extension<Session>) -> Json<Value> {
    if session.get_str("access_token").is_none() {
        return Json(json!({"authenticated": false, "email": null}));
    }
    Json(json!({"authenticated": true, "email": session.get("email").unwrap_or(Value::Null)}))
}

// ── Device pairing ──────────────────────────────────────────────────────────

/// Ask the signed-in user to approve a device. Requiring the session is the
/// point: it proves a person with an account is present, not merely a
/// process on the machine.
pub async fn device_pairing(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    uri: Uri,
) -> Result<Response> {
    let q = query(&uri);
    let redirect_to = q.get("redirect").map(String::as_str).unwrap_or("");
    let name = q
        .get("name")
        .map(String::as_str)
        .unwrap_or("MeCloud Desktop");
    let state = q.get("state").map(String::as_str).unwrap_or("");
    rate_limit(&st, "device_pairing", &ip, &LOGIN_LIMIT)?;

    if !devices::is_loopback_redirect(redirect_to) {
        return Err(AppError::http(
            400,
            "A device can only be paired to an address on this computer.",
        ));
    }
    if !state.is_empty() && !devices::is_valid_state(state) {
        return Err(AppError::http(400, "Invalid pairing request."));
    }
    let Some(access_token) = session.get_str("access_token") else {
        // Sign in first, then come back to this exact request. Path and query
        // only: the callback refuses anything absolute.
        let back = match uri.query() {
            Some(q) if !q.is_empty() => format!("/auth/device?{q}"),
            _ => "/auth/device".into(),
        };
        session.insert("after_login", back);
        return Ok(redirect(StatusCode::FOUND, "/auth/login"));
    };
    let sealed = devices::seal_pairing(&st.pairing_key, redirect_to, name, state);
    // Named from the JMAP session: our own session has no username in it.
    let who = jmap::get_jmap_session(&st, &access_token)
        .await
        .ok()
        .and_then(|s| {
            s.get("username")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    Ok(pages::pairing_page(name, &sealed, &who))
}

/// Mint the device token and hand it back over the loopback redirect.
pub async fn device_approve(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    request: Request,
) -> Result<Response> {
    rate_limit(&st, "device_approve", &ip, &LOGIN_LIMIT)?;
    let Some(access_token) = session.get_str("access_token") else {
        return Err(AppError::unauthenticated());
    };
    let body = read_body(request).await?;
    let request_token = form_urlencoded::parse(&body)
        .find(|(k, _)| k == "request_token")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();

    let Some((redirect_to, name, state)) = devices::open_pairing(&st.pairing_key, &request_token)
    else {
        return Err(AppError::http(
            400,
            "This pairing request has expired. Try again.",
        ));
    };
    // Checked again after unsealing: the value acted on must be the checked one.
    if !devices::is_loopback_redirect(&redirect_to) {
        return Err(AppError::http(400, "Invalid pairing address."));
    }

    // A credential of the device's own, so nothing that happens to this
    // browser session can invalidate it, and revoking it signs nobody out.
    let created = match jmap::create_app_password(&st, &access_token, &name).await {
        Ok(created) => created,
        Err(AppError::Upstream { message, .. }) => return Err(AppError::http(502, message)),
        Err(other) => return Err(other),
    };
    let text = |key: &str| match created.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let token = devices::issue(
        &st.device_key,
        &text("secret"),
        &text("id"),
        &text("username"),
        &name,
    );
    if !state.is_empty() {
        return Ok(pages::handoff_form(&redirect_to, &token, &state));
    }
    // A client from before the nonce: the legacy GET hand-off it understands.
    let joiner = if redirect_to.contains('?') { "&" } else { "?" };
    let token = percent_encoding::utf8_percent_encode(&token, crate::crypto::QUOTE_PATH);
    Ok(pages::handoff_page(&format!(
        "{redirect_to}{joiner}token={token}"
    )))
}

// ── Desktop downloads ───────────────────────────────────────────────────────

/// What this build shipped. The version is also what the desktop client
/// compares against its own to decide whether an update is available.
pub async fn desktop_releases(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    require_auth(&st, &headers, &session).await?;
    rate_limit(&st, "desktop_releases", &ip, &[Limit::per_minute(60)])?;
    let dir = &st.settings.desktop_download_dir;
    Ok(Json(
        json!({"version": downloads::version(dir), "artifacts": downloads::available(dir)}),
    ))
}

/// Trade the session for a short-lived link to one artefact. The session is
/// checked here, once; the link then works on its own — in a download
/// manager, or a curl on another machine.
pub async fn desktop_token(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<Value>> {
    require_auth(&st, &headers, &session).await?;
    let q = query(&uri);
    let key = q
        .get("artifact")
        .ok_or_else(|| AppError::missing("query", "artifact"))?;
    rate_limit(&st, "desktop_token", &ip, &[Limit::per_minute(30)])?;
    let dir = &st.settings.desktop_download_dir;
    let Some(art) = downloads::artifact(key).filter(|a| downloads::path_for(dir, a).is_file())
    else {
        return Err(AppError::http(404, "No such download"));
    };
    Ok(Json(json!({
        "token": downloads::mint(&st.download_key, art.key, now_secs()),
        "url": format!("/api/desktop/download/{}", art.key),
        "expires_in": downloads::TOKEN_TTL_SECONDS,
        "filename": art.filename,
    })))
}

/// Serve an artefact against a signed token. No session: the token is the
/// authorisation, and requiring both would break what the token is for.
pub async fn desktop_download(
    State(st): St,
    Extension(ip): Extension<ClientIp>,
    Path(key): Path<String>,
    request: Request,
) -> Result<Response> {
    let q = query(request.uri());
    let token = q.get("token").map(String::as_str).unwrap_or("");
    rate_limit(
        &st,
        "desktop_download",
        &ip,
        &st.settings.rate_limit_download,
    )?;
    let Some(art) = downloads::artifact(&key) else {
        return Err(AppError::http(404, "No such download"));
    };
    if !downloads::verify_now(&st.download_key, token, art.key) {
        // One message for missing, malformed and expired alike.
        return Err(AppError::http(
            403,
            "This download link is not valid or has expired.",
        ));
    }
    let path = downloads::path_for(&st.settings.desktop_download_dir, art);
    if !path.is_file() {
        return Err(AppError::http(404, "No such download"));
    }
    use tower::ServiceExt as _;
    let served = tower_http::services::ServeFile::new(&path)
        .oneshot(request)
        .await;
    let mut response = match served {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(art.media_type));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", art.filename))
    {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

// ── JMAP ────────────────────────────────────────────────────────────────────

async fn read_body(request: Request) -> Result<bytes::Bytes> {
    // The body-limit layer has already capped what can arrive.
    axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| AppError::http(400, "Could not read the request body"))
}

fn parse_json(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|e| {
        AppError::invalid(&["body"], "json_invalid", format!("JSON decode error: {e}"))
    })
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// A JMAP request envelope (RFC 8620 §3.3), checked rather than trusted: it
/// is forwarded with the session's credential attached. A method call is
/// exactly `[name, arguments, callId]`. Only `using` and `methodCalls` go on.
pub fn validate_jmap_request(payload: &Value) -> Result<Value> {
    let bad = |field: &str, msg: &str| AppError::invalid(&["body", field], "value_error", msg);
    let object = payload
        .as_object()
        .ok_or_else(|| AppError::invalid(&["body"], "model_type", "Input should be an object"))?;
    let using = object
        .get("using")
        .ok_or_else(|| AppError::missing("body", "using"))?;
    let calls = object
        .get("methodCalls")
        .ok_or_else(|| AppError::missing("body", "methodCalls"))?;

    let using_list = using
        .as_array()
        .ok_or_else(|| bad("using", "Input should be a valid list"))?;
    if !(1..=20).contains(&using_list.len()) {
        return Err(bad("using", "using must list 1-20 capabilities"));
    }
    for urn in using_list {
        let urn = urn
            .as_str()
            .ok_or_else(|| bad("using", "Input should be a valid string"))?;
        if !(1..=255).contains(&char_len(urn)) {
            return Err(bad("using", "capability URNs must be 1-255 characters"));
        }
    }

    let call_list = calls
        .as_array()
        .ok_or_else(|| bad("methodCalls", "Input should be a valid list"))?;
    if !(1..=50).contains(&call_list.len()) {
        return Err(bad("methodCalls", "methodCalls must hold 1-50 calls"));
    }
    for call in call_list {
        let parts = call.as_array().filter(|p| p.len() == 3).ok_or_else(|| {
            bad(
                "methodCalls",
                "each method call must be [name, arguments, callId]",
            )
        })?;
        if !parts[0]
            .as_str()
            .is_some_and(|n| (1..=128).contains(&char_len(n)))
        {
            return Err(bad(
                "methodCalls",
                "method name must be a string of 1-128 characters",
            ));
        }
        if !parts[1].is_object() {
            return Err(bad("methodCalls", "method arguments must be an object"));
        }
        if !parts[2]
            .as_str()
            .is_some_and(|c| (1..=64).contains(&char_len(c)))
        {
            return Err(bad(
                "methodCalls",
                "method callId must be a string of 1-64 characters",
            ));
        }
    }
    Ok(json!({"using": using, "methodCalls": calls}))
}

pub async fn jmap_session(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let credential = require_auth(&st, &headers, &session).await?;
    rate_limit(&st, "jmap_session", &ip, &[Limit::per_minute(60)])?;
    Ok(Json(
        (*jmap::get_jmap_session(&st, &credential).await?).clone(),
    ))
}

pub async fn jmap_proxy(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    request: Request,
) -> Result<Json<Value>> {
    let credential = require_auth(&st, request.headers(), &session).await?;
    let payload = validate_jmap_request(&parse_json(&read_body(request).await?)?)?;
    rate_limit(&st, "jmap_proxy", &ip, &st.settings.rate_limit_jmap)?;
    Ok(Json(jmap::jmap_request(&st, &credential, &payload).await?))
}

pub async fn jmap_events(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Response> {
    let credential = require_auth(&st, &headers, &session).await?;
    rate_limit(&st, "jmap_events", &ip, &[Limit::per_minute(30)])?;
    let mut response = Response::new(jmap::event_stream(st.clone(), credential));
    let h = response.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

// ── Sieve ───────────────────────────────────────────────────────────────────

pub async fn sieve_get(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let credential = require_auth(&st, &headers, &session).await?;
    rate_limit(&st, "sieve_get", &ip, &[Limit::per_minute(60)])?;
    match jmap::get_sieve_script(&st, &credential).await? {
        Some(script) => Ok(Json(script)),
        None => Err(AppError::http(503, "Sieve not supported by this server")),
    }
}

/// pydantic's lax bool.
fn lax_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => match n.as_f64() {
            Some(0.0) => Some(false),
            Some(1.0) => Some(true),
            _ => None,
        },
        Value::String(s) => bool_word(s),
        _ => None,
    }
}

fn bool_word(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        _ => None,
    }
}

pub struct SieveSave {
    pub id: Option<String>,
    pub name: String,
    pub content: String,
    pub make_active: bool,
}

pub fn validate_sieve_save(payload: &Value) -> Result<SieveSave> {
    let bad = |field: &str, msg: &str| AppError::invalid(&["body", field], "value_error", msg);
    let object = payload
        .as_object()
        .ok_or_else(|| AppError::invalid(&["body"], "model_type", "Input should be an object"))?;
    let string = |field: &str, max: usize| -> Result<Option<String>> {
        match object.get(field) {
            None => Ok(None),
            Some(Value::String(s)) if char_len(s) <= max => Ok(Some(s.clone())),
            Some(Value::String(_)) => Err(bad(
                field,
                &format!("String should have at most {max} characters"),
            )),
            Some(_) => Err(bad(field, "Input should be a valid string")),
        }
    };
    let id = match object.get("id") {
        Some(Value::Null) | None => None,
        Some(_) => string("id", 128)?,
    };
    let name = string("name", 255)?.unwrap_or_else(|| "My Rules".into());
    let content = string("content", 65_536)?.ok_or_else(|| AppError::missing("body", "content"))?;
    let make_active = match object.get("makeActive") {
        None => true,
        Some(v) => {
            lax_bool(v).ok_or_else(|| bad("makeActive", "Input should be a valid boolean"))?
        }
    };
    Ok(SieveSave {
        id,
        name,
        content,
        make_active,
    })
}

pub async fn sieve_put(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    request: Request,
) -> Result<Json<Value>> {
    let credential = require_auth(&st, request.headers(), &session).await?;
    let save = validate_sieve_save(&parse_json(&read_body(request).await?)?)?;
    rate_limit(&st, "sieve_put", &ip, &[Limit::per_minute(30)])?;
    let saved = jmap::save_sieve_script(
        &st,
        &credential,
        save.id.as_deref(),
        &save.name,
        &save.content,
        save.make_active,
    )
    .await?;
    Ok(Json(saved))
}

// ── File storage blobs (JMAP for File Storage) ─────────────────────────────

/// Stream a request body up to the JMAP upload endpoint.
pub async fn upload_blob(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    request: Request,
) -> Result<Json<Value>> {
    let credential = require_auth(&st, request.headers(), &session).await?;
    rate_limit(&st, "upload_blob", &ip, &st.settings.rate_limit_upload)?;
    let jmap_session = jmap::session_for(&st, &credential).await?;
    let account = jmap::account_id(&jmap_session)?;
    let url = blobs::upload_url(&jmap_session, &account)?;
    // Upstream decides what it accepts; the claim is passed on, but never
    // trusted when the bytes are served back (blobs::safe_content_type).
    let declared = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let declared = if declared.is_empty() {
        blobs::OCTET_STREAM
    } else {
        declared
    };
    let content_type = declared.split(';').next().unwrap_or("").trim().to_string();

    let body = reqwest::Body::wrap_stream(request.into_body().into_data_stream());
    let response = st
        .upstream
        .client
        .post(url)
        .header("Authorization", authorization(&credential))
        .header("Content-Type", content_type)
        .body(body)
        .send()
        .await?;
    let result: Value = check(response, "Blob upload")?.json().await?;
    if !result.get("blobId").is_some_and(|_| result.is_object()) {
        return Err(AppError::upstream(
            "Upload endpoint returned no blobId",
            None,
        ));
    }
    let field = |k: &str| result.get(k).cloned().unwrap_or(Value::Null);
    Ok(Json(
        json!({"blobId": field("blobId"), "type": field("type"), "size": field("size")}),
    ))
}

/// Stream a blob back to the browser, defanged.
///
/// Serving user-supplied bytes from our own origin is the sharp edge: an HTML
/// or SVG file rendered inline would run as first-party script. So: an
/// allow-list of types that may render inline (everything else becomes
/// application/octet-stream), an attachment disposition unless the type is on
/// it, and a per-response CSP — `sandbox` for downloads.
pub async fn download_blob(
    State(st): St,
    Extension(session): Extension<Session>,
    Extension(ip): Extension<ClientIp>,
    Path(blob_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response> {
    let credential = require_auth(&st, &headers, &session).await?;
    let q = query(&uri);
    let name = q.get("name").cloned().unwrap_or_default();
    let type_ = q.get("type").cloned().unwrap_or_default();
    let inline = match q.get("inline") {
        None => false,
        Some(v) => bool_word(v).ok_or_else(|| {
            AppError::invalid(
                &["query", "inline"],
                "bool_parsing",
                "Input should be a valid boolean",
            )
        })?,
    };
    rate_limit(&st, "download_blob", &ip, &st.settings.rate_limit_download)?;

    blobs::validate_blob_id(&blob_id)?;
    let jmap_session = jmap::session_for(&st, &credential).await?;
    let account = jmap::account_id(&jmap_session)?;
    let url = blobs::download_url(&jmap_session, &account, &blob_id, &name, &type_)?;
    // Opened, and its status known, before answering: a 404 discovered halfway
    // through the body would reach the browser as a truncated file.
    let upstream = st
        .upstream
        .stream
        .get(url)
        .header("Authorization", authorization(&credential))
        .send()
        .await?;
    if upstream.status() == StatusCode::NOT_FOUND {
        return Err(AppError::upstream("Blob not found", Some(404)));
    }
    let upstream = check(upstream, "Blob download")?;

    let upstream_type = upstream
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let declared = if type_.is_empty() {
        upstream_type.as_deref()
    } else {
        Some(type_.as_str())
    };
    let resolved = blobs::safe_content_type(declared, inline);
    let render_inline = inline && resolved != blobs::OCTET_STREAM;
    let length = upstream.headers().get(CONTENT_LENGTH).cloned();

    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    let h = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&resolved) {
        h.insert(CONTENT_TYPE, value);
    }
    if let Ok(value) =
        HeaderValue::from_str(&blobs::content_disposition(Some(&name), render_inline))
    {
        h.insert(CONTENT_DISPOSITION, value);
    }
    let ancestors = st.settings.frame_ancestors_value();
    if let Ok(value) = HeaderValue::from_str(&blobs::blob_csp(render_inline, &ancestors)) {
        h.insert(CONTENT_SECURITY_POLICY, value);
    }
    // Set here so the security-header middleware leaves it. Its global value
    // is DENY by default, which would block the app's own preview frame.
    if render_inline {
        if let Some(xfo) = blobs::inline_x_frame_options(&ancestors) {
            h.insert("x-frame-options", HeaderValue::from_static(xfo));
        }
    }
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Some(length) = length.filter(|l| !l.is_empty()) {
        h.insert(CONTENT_LENGTH, length);
    }
    with_charset(&mut response);
    Ok(response)
}

// ── Fallbacks ───────────────────────────────────────────────────────────────

/// Unknown /api paths are a JSON 404, never the SPA shell: a typo in a fetch
/// URL used to come back as index.html and surface as a JSON parse error.
pub async fn api_not_found() -> Response {
    detail(StatusCode::NOT_FOUND, "Not found")
}

pub async fn method_not_allowed() -> Response {
    detail(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: Value, args: Value, id: Value) -> Value {
        json!([name, args, id])
    }

    #[test]
    fn a_valid_envelope_is_accepted_and_trimmed() {
        let out = validate_jmap_request(&json!({
            "using": ["urn:ietf:params:jmap:core"],
            "methodCalls": [["Mailbox/get", {"accountId": "1"}, "m"]],
            "createdIds": {"x": "y"},
        }))
        .unwrap();
        assert_eq!(
            out,
            json!({"using": ["urn:ietf:params:jmap:core"], "methodCalls": [["Mailbox/get", {"accountId": "1"}, "m"]]})
        );
    }

    #[test]
    fn malformed_envelopes_are_rejected() {
        let core = json!(["urn:ietf:params:jmap:core"]);
        for bad_call in [
            json!(["Mailbox/get", {}]),
            json!(["Mailbox/get", {}, "m", "extra"]),
            call(json!(123), json!({}), json!("m")),
            call(json!("Mailbox/get"), json!("not-an-object"), json!("m")),
            call(json!("Mailbox/get"), json!({}), json!(5)),
            call(json!(""), json!({}), json!("m")),
            call(json!("Mailbox/get"), json!({}), json!("")),
            json!("not-a-list"),
        ] {
            assert!(
                validate_jmap_request(&json!({"using": core, "methodCalls": [bad_call]})).is_err(),
                "{bad_call}"
            );
        }
        let one = json!([["Mailbox/get", {}, "m"]]);
        for bad in [
            json!({"using": core}),
            json!({"methodCalls": one}),
            json!({"using": core, "methodCalls": []}),
            json!({"using": [], "methodCalls": one}),
            json!({"using": ["u".repeat(300)], "methodCalls": one}),
            json!({"using": (0..21).map(|i| format!("urn:test:{i}")).collect::<Vec<_>>(), "methodCalls": one}),
            json!({"using": core, "methodCalls": vec![json!(["Mailbox/get", {}, "q"]); 51]}),
            json!([1, 2]),
        ] {
            assert!(validate_jmap_request(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sieve_saves_are_checked() {
        let ok = validate_sieve_save(&json!({"content": "require [];"})).unwrap();
        assert_eq!(
            (ok.name.as_str(), ok.make_active, ok.id),
            ("My Rules", true, None)
        );
        assert!(
            !validate_sieve_save(&json!({"content": "", "makeActive": "false"}))
                .unwrap()
                .make_active
        );
        for bad in [
            json!({}),
            json!({"content": "x".repeat(70_000)}),
            json!({"content": "x", "name": "n".repeat(300)}),
            json!({"content": "x", "id": "i".repeat(129)}),
            json!({"content": 5}),
            json!({"content": "x", "name": null}),
            json!({"content": "x", "makeActive": "maybe"}),
        ] {
            assert!(validate_sieve_save(&bad).is_err(), "{bad}");
        }
    }
}
