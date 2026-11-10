//! The layers every request passes through, outermost first: the Host
//! allow-list, client address and access log, security headers, the body
//! size limit, CORS, and the session (session.rs).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, REFERRER_POLICY,
};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use http_body_util::BodyExt;

use crate::crypto::sha256;
use crate::error::detail;
use crate::AppState;

/// Where uploads arrive; it alone gets MAX_UPLOAD_BYTES.
pub const UPLOAD_PATH: &str = "/api/files/blob";

/// Who sent the request, after X-Forwarded-For from a trusted proxy.
#[derive(Clone, Debug)]
pub struct ClientIp(pub String);

pub async fn host_check(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(allowed) = &state.allowed_hosts {
        let host = request
            .headers()
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
            .or_else(|| request.uri().host().map(str::to_string))
            .unwrap_or_default();
        let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
        if !allowed.contains(&host) {
            return (
                StatusCode::BAD_REQUEST,
                [(CONTENT_TYPE, "text/plain; charset=utf-8")],
                "Invalid host header",
            )
                .into_response();
        }
    }
    next.run(request).await
}

pub async fn client_and_log(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let forwarded = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    let client = state.proxies.client_host(&peer, forwarded);
    request.extensions_mut().insert(ClientIp(client.clone()));
    let method = request.method().clone();
    // The path only: query strings carry download tokens and OAuth codes.
    let path = request.uri().path().to_string();
    let started = Instant::now();
    let response = next.run(request).await;
    if path != "/health" {
        tracing::info!(
            target: "access",
            "{client} \"{method} {path}\" {} {}ms",
            response.status().as_u16(),
            started.elapsed().as_millis()
        );
    }
    response
}

pub async fn security_headers(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let set = |headers: &mut axum::http::HeaderMap, name: &'static str, value: &str| {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    };
    set(headers, "x-content-type-options", "nosniff");
    // Unless the handler chose: an inline blob preview is framed by the app
    // itself, and sets SAMEORIGIN (or nothing) to match its own CSP.
    if !headers.contains_key("x-frame-options") {
        if let Some(xfo) = state.settings.x_frame_options() {
            set(headers, "x-frame-options", xfo);
        }
    }
    // Only where the handler did not choose: the device hand-off sends
    // no-referrer, and must keep it.
    if !headers.contains_key(REFERRER_POLICY) {
        set(
            headers,
            "referrer-policy",
            "strict-origin-when-cross-origin",
        );
    }
    set(
        headers,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    );
    // Likewise: the blob route and the hand-off set their own, stricter or
    // narrower policies, and must not be given the app's.
    if !headers.contains_key(CONTENT_SECURITY_POLICY) {
        set(headers, "content-security-policy", &state.csp);
    }
    set(headers, "cross-origin-opener-policy", "same-origin");
    set(headers, "cross-origin-resource-policy", "same-origin");
    if path.starts_with("/api/") || path.starts_with("/auth/") {
        // Mail content and auth responses must never land in a shared or
        // on-disk cache, nor be replayed from history after logout.
        set(
            headers,
            "cache-control",
            "no-store, no-cache, must-revalidate, private",
        );
        set(headers, "pragma", "no-cache");
    }
    if state.settings.is_production() {
        set(
            headers,
            "strict-transport-security",
            "max-age=63072000; includeSubDomains; preload",
        );
    }
    response
}

/// Reject bodies over the limit. A declared Content-Length is checked up
/// front and the body then streams through untouched (hyper holds the sender
/// to it). A body without one — chunked — is buffered up to the limit, so
/// leaving the header off is no way around the check.
pub async fn body_limit(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let limit = limit_for(&state, request.uri().path());
    let too_large = || detail(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large");
    if let Some(declared) = request.headers().get(CONTENT_LENGTH) {
        let fits = declared
            .to_str()
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .is_some_and(|n| n <= limit);
        if !fits {
            return too_large();
        }
        return next.run(request).await;
    }
    if !matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH
    ) {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let limited = http_body_util::Limited::new(body, usize::try_from(limit).unwrap_or(usize::MAX));
    match limited.collect().await {
        Ok(collected) => {
            next.run(Request::from_parts(parts, Body::from(collected.to_bytes())))
                .await
        }
        Err(e)
            if e.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some() =>
        {
            too_large()
        }
        Err(_) => detail(StatusCode::BAD_REQUEST, "Could not read the request body"),
    }
}

pub fn limit_for(state: &AppState, path: &str) -> u64 {
    if path.starts_with(UPLOAD_PATH) {
        state.settings.max_upload_bytes
    } else {
        state.settings.max_request_bytes
    }
}

/// CSP source expressions for every inline `<script>` in the SPA shell.
///
/// SvelteKit's static build boots from one inline script. Rather than allow
/// all inline script for it, the policy names that script by its SHA-256,
/// computed from the file actually served, so a rebuild cannot leave a stale
/// hash behind. An injected script matches no hash and does not run.
pub fn inline_script_hashes(index_html: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let lower = index_html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<script") {
        let open = at + start;
        let after_name = open + "<script".len();
        let Some(close) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + close;
        let attributes = &lower[after_name..tag_end];
        let Some(end) = lower[tag_end + 1..].find("</script>") else {
            break;
        };
        let body_end = tag_end + 1 + end;
        // A <script src=…> is covered by 'self'.
        if !has_src_attribute(attributes) {
            let body = &index_html[tag_end + 1..body_end];
            hashes.push(format!(
                "'sha256-{}'",
                STANDARD.encode(sha256(body.as_bytes()))
            ));
        }
        at = body_end + "</script>".len();
    }
    hashes
}

/// `\bsrc=` among a tag's attributes.
fn has_src_attribute(attributes: &str) -> bool {
    attributes.match_indices("src=").any(|(i, _)| {
        i == 0
            || !attributes.as_bytes()[i - 1].is_ascii_alphanumeric()
                && attributes.as_bytes()[i - 1] != b'_'
    })
}

/// The application's policy, with the hashes of the shell's inline scripts.
pub fn app_csp(script_hashes: &[String], frame_ancestors: &str) -> String {
    let mut script_src = vec!["'self'".to_string()];
    script_src.extend(script_hashes.iter().cloned());
    format!(
        "default-src 'self'; \
         script-src {}; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob: https:; \
         font-src 'self' data:; \
         connect-src 'self'; \
         frame-src 'self'; \
         worker-src 'self'; \
         manifest-src 'self'; \
         media-src 'self' data:; \
         object-src 'none'; \
         base-uri 'self'; \
         form-action 'self'; \
         frame-ancestors {frame_ancestors};",
        script_src.join(" ")
    )
}

// Styles stay 'unsafe-inline' (Svelte transitions and sanitised mail markup set
// inline styles; style cannot execute script). frame-src is 'self', not
// 'none': the file preview frames same-origin blob responses, which carry
// their own restrictive policy plus nosniff (blobs::blob_csp).

/// True when the single-page-app fallback may answer `path` (no leading
/// slash needed). Anything under /api/ or /auth/ is server surface: an
/// unmatched route there is a 404, never the app shell.
pub fn is_spa_path(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    !(path.starts_with("api/") || path.starts_with("auth/"))
}

/// Serve the SvelteKit build: a file if one matches, otherwise the shell.
pub async fn serve_frontend(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let Some(static_root) = state.static_root.clone() else {
        return detail(StatusCode::NOT_FOUND, "Not Found");
    };
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return crate::routes::method_not_allowed().await;
    }
    let raw_path = request.uri().path();
    let decoded = percent_encoding::percent_decode_str(raw_path)
        .decode_utf8_lossy()
        .to_string();
    let full_path = decoded.strip_prefix('/').unwrap_or(&decoded).to_string();
    if !is_spa_path(&full_path) {
        return detail(StatusCode::NOT_FOUND, "Not found");
    }
    if !full_path.is_empty() {
        if let Ok(candidate) = tokio::fs::canonicalize(static_root.join(&full_path)).await {
            let inside = candidate.starts_with(&static_root) && candidate != static_root;
            if inside
                && tokio::fs::metadata(&candidate)
                    .await
                    .is_ok_and(|m| m.is_file())
            {
                // Vite emits content-hashed names under _app/immutable; those
                // are cached hard, everything else revalidates.
                let cache = if format!("/{full_path}").contains("/_app/immutable/") {
                    "public, max-age=31536000, immutable"
                } else {
                    "public, max-age=0, must-revalidate"
                };
                return serve_file(request, &candidate, cache).await;
            }
        }
    }
    // The shell always revalidates, or users get stranded on an old build
    // that references deleted asset hashes.
    serve_file(request, &static_root.join("index.html"), "no-cache").await
}

async fn serve_file(
    request: Request,
    path: &std::path::Path,
    cache_control: &'static str,
) -> Response {
    use tower::ServiceExt as _;
    let served = tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await;
    let mut response = match served {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    }
    with_charset(&mut response);
    response
}

/// `text/*` goes out with `charset=utf-8`, as Starlette sent it.
pub fn with_charset(response: &mut Response) {
    let Some(current) = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return;
    };
    if (current.starts_with("text/") || current == "application/javascript")
        && !current.contains("charset")
    {
        if let Ok(value) = HeaderValue::from_str(&format!("{current}; charset=utf-8")) {
            response.headers_mut().insert(CONTENT_TYPE, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_script_hashes_match_what_a_browser_hashes() {
        let body = "\n  boot();\n";
        let page = format!(
            r#"<head><script src="/a.js"></script></head><body><script>{body}</script></body>"#
        );
        let expected = STANDARD.encode(sha256(body.as_bytes()));
        assert_eq!(
            inline_script_hashes(&page),
            [format!("'sha256-{expected}'")]
        );
    }

    #[test]
    fn external_scripts_are_not_hashed() {
        assert!(inline_script_hashes(r#"<script type="module" src="/x.js"></script>"#).is_empty());
        assert!(inline_script_hashes(r#"<SCRIPT defer SRC='/x.js'></SCRIPT>"#).is_empty());
    }

    #[test]
    fn server_paths_are_never_served_by_the_spa() {
        for path in [
            "api/jmap",
            "api/anything",
            "/api/jmap",
            "auth/logout",
            "auth/callback",
            "/auth/login",
        ] {
            assert!(!is_spa_path(path), "{path}");
        }
        for path in [
            "",
            "calendar",
            "contacts",
            "_app/immutable/entry/start.js",
            "icons/icon-192.png",
            "apis-and-things",
            "authors",
        ] {
            assert!(is_spa_path(path), "{path}");
        }
    }

    #[test]
    fn the_app_policy_never_allows_inline_script() {
        let csp = app_csp(&["'sha256-x'".into()], "'none'");
        let script_src = csp
            .split_once("script-src")
            .unwrap()
            .1
            .split_once(';')
            .unwrap()
            .0;
        assert!(
            !script_src.contains("unsafe-inline")
                && script_src.contains("'self'")
                && script_src.contains("sha256-x")
        );
        for needed in [
            "object-src 'none'",
            "frame-src 'self'",
            "base-uri 'self'",
            "frame-ancestors 'none';",
        ] {
            assert!(csp.contains(needed), "{needed}");
        }
    }
}
