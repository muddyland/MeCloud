//! MeCloud's web server: the encrypted-session OAuth login, the JMAP proxy
//! (requests, event stream, blobs), device pairing for the desktop client,
//! and the SvelteKit frontend.
//!
//! Stateless by design: everything a user needs between requests lives in
//! their encrypted cookie, or in a token only this server can open. Restarting
//! it, or running several, loses nothing.

pub mod auth;
pub mod blobs;
pub mod config;
pub mod crypto;
pub mod devices;
pub mod downloads;
pub mod error;
pub mod jmap;
pub mod pages;
pub mod permissions;
pub mod ratelimit;
pub mod routes;
pub mod session;
pub mod upstream;
pub mod web;

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderValue, Method};
use axum::middleware::from_fn_with_state;
use axum::routing::{get, on, post, MethodFilter};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use config::Settings;

pub struct AppState {
    pub settings: Settings,
    pub upstream: upstream::Upstream,
    pub session_codec: session::SessionCodec,
    pub device_key: crypto::Fernet,
    pub pairing_key: crypto::Fernet,
    pub download_key: [u8; 32],
    pub oauth: auth::OAuth,
    pub jmap_sessions: jmap::SessionCache,
    pub limiter: ratelimit::Limiter,
    pub proxies: ratelimit::TrustedProxies,
    pub allowed_hosts: Option<Vec<String>>,
    pub open_streams: AtomicUsize,
    /// The canonical static directory, when there is a built frontend.
    pub static_root: Option<PathBuf>,
    /// The app's Content-Security-Policy, with the shell's script hashes.
    pub csp: String,
}

impl AppState {
    pub fn new(settings: Settings) -> AppState {
        let secret = settings.session_secret.clone();
        let static_root = std::fs::canonicalize(&settings.frontend_static_dir)
            .ok()
            .filter(|p| p.is_dir());
        // Hashed from the file actually served; no built frontend (API only,
        // tests) means nothing inline to allow.
        let hashes = static_root
            .as_ref()
            .and_then(|root| std::fs::read_to_string(root.join("index.html")).ok())
            .map(|html| web::inline_script_hashes(&html))
            .unwrap_or_default();
        AppState {
            upstream: upstream::Upstream::new(&settings),
            session_codec: session::SessionCodec::new(
                &secret,
                settings.is_production(),
                settings.session_max_age,
            ),
            device_key: crypto::Fernet::derived(&secret, devices::DEVICE_INFO),
            pairing_key: crypto::Fernet::derived(&secret, devices::PAIRING_INFO),
            download_key: downloads::key(&secret),
            oauth: auth::OAuth::default(),
            jmap_sessions: jmap::SessionCache::default(),
            limiter: ratelimit::Limiter::default(),
            proxies: ratelimit::TrustedProxies::parse(&settings.forwarded_allow_ips),
            allowed_hosts: settings.allowed_hosts(),
            open_streams: AtomicUsize::new(0),
            csp: web::app_csp(&hashes, &settings.frame_ancestors_value()),
            static_root,
            settings,
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let origins: Vec<HeaderValue> = state
        .settings
        .cors_origins()
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .max_age(Duration::from_secs(600));

    let any_api = MethodFilter::GET
        .or(MethodFilter::POST)
        .or(MethodFilter::PUT)
        .or(MethodFilter::DELETE)
        .or(MethodFilter::PATCH);

    Router::new()
        .route("/health", get(routes::health))
        .route("/api/config", get(routes::public_config))
        .route("/manifest.webmanifest", get(routes::web_manifest))
        .route("/auth/login", get(routes::login))
        .route("/auth/callback", get(routes::callback))
        .route("/auth/logout", post(routes::logout))
        .route("/auth/me", get(routes::me))
        .route("/auth/device", get(routes::device_pairing))
        .route("/auth/device/approve", post(routes::device_approve))
        .route("/api/desktop/releases", get(routes::desktop_releases))
        .route("/api/desktop/token", post(routes::desktop_token))
        .route(
            "/api/desktop/download/{artifact}",
            get(routes::desktop_download),
        )
        .route("/api/jmap/session", get(routes::jmap_session))
        .route("/api/jmap", post(routes::jmap_proxy))
        .route("/api/jmap/events", get(routes::jmap_events))
        .route("/api/sieve", get(routes::sieve_get).put(routes::sieve_put))
        .route(web::UPLOAD_PATH, post(routes::upload_blob))
        .route("/api/files/blob/{blob_id}", get(routes::download_blob))
        .route("/api/{*rest}", on(any_api, routes::api_not_found))
        .fallback(web::serve_frontend)
        .method_not_allowed_fallback(routes::method_not_allowed)
        // Innermost first: session -> CORS -> body limit -> security headers
        // -> client address and access log -> Host check.
        .layer(from_fn_with_state(state.clone(), session::middleware))
        .layer(cors)
        .layer(from_fn_with_state(state.clone(), web::body_limit))
        .layer(from_fn_with_state(state.clone(), web::security_headers))
        .layer(from_fn_with_state(state.clone(), web::client_and_log))
        .layer(from_fn_with_state(state.clone(), web::host_check))
        .with_state(state)
}

/// Discover the OAuth endpoints ahead of the first login. A failure is only
/// logged; the first login tries again.
pub async fn warm_up(state: &AppState) {
    if let Err(e) = auth::endpoints(state).await {
        tracing::warn!(
            "OAuth discovery warm-up failed (will retry on first login): {}",
            error::AppError::from(e)
        );
    }
}
