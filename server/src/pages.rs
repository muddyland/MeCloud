//! The server-rendered pages of the device pairing flow.
//!
//! Plain HTML with a form POST: nothing to fetch, and on the consent page no
//! script at all, so what is approved is exactly what is displayed.

use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::crypto::{html_escape, sha256};
use crate::devices::loopback_origin;

fn page(body: String) -> Response {
    let mut response = Html(body).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The approval screen.
pub fn pairing_page(name: &str, sealed: &str, username: &str) -> Response {
    let safe_name = html_escape(name);
    let safe_user = html_escape(if username.is_empty() {
        "this account"
    } else {
        username
    });
    let safe_sealed = html_escape(sealed);
    page(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect a device</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
        background:#f6f7f9; color:#111827;
        font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
 @media (prefers-color-scheme: dark) {{ body {{ background:#0b1220; color:#f3f4f6; }}
   .card {{ background:#111827 !important; border-color:#1f2937 !important; }}
   .muted {{ color:#9ca3af !important; }} li {{ color:#d1d5db; }} }}
 .card {{ max-width:26rem; width:100%; margin:1rem; padding:1.75rem;
          background:#fff; border:1px solid #e5e7eb; border-radius:14px; }}
 h1 {{ font-size:1.05rem; margin:0 0 .35rem; }}
 .muted {{ color:#6b7280; font-size:.85rem; margin:0 0 1.1rem; }}
 ul {{ margin:0 0 1.3rem; padding-left:1.1rem; font-size:.9rem; }}
 li {{ margin-bottom:.3rem; }}
 .row {{ display:flex; gap:.6rem; align-items:center; }}
 button {{ padding:.6rem 1.15rem; border:0; border-radius:9px; font-size:.95rem;
           font-weight:600; background:#3b82f6; color:#fff; cursor:pointer; }}
 a.cancel {{ color:#6b7280; font-size:.9rem; text-decoration:none; padding:.6rem; }}
</style></head><body>
<div class="card">
  <h1>Connect {safe_name}?</h1>
  <p class="muted">Signed in as {safe_user}</p>
  <ul>
    <li>Read and change the files in your drive</li>
    <li>Stay connected in the background, until you disconnect it on the mail server</li>
  </ul>
  <form method="post" action="/auth/device/approve" class="row">
    <input type="hidden" name="request_token" value="{safe_sealed}">
    <button type="submit">Connect</button>
    <a class="cancel" href="/">Cancel</a>
  </form>
</div></body></html>"#
    ))
}

/// Send the browser on to the waiting client (clients from before the nonce).
///
/// Deliberately not a redirect. The response to a form POST is still part of
/// that submission as far as `form-action` is concerned, and WebKit and
/// Firefox check redirects against it — so a 303 to the loopback port is
/// blocked by our own `form-action 'self'`. A meta refresh is an ordinary
/// top-level navigation instead, and the visible link covers a browser with
/// refreshes turned off.
pub fn handoff_page(target: &str) -> Response {
    let safe = html_escape(target);
    page(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="0;url={safe}">
<title>Connecting…</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
        background:#f6f7f9; color:#111827;
        font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; text-align:center; }}
 @media (prefers-color-scheme: dark) {{ body {{ background:#0b1220; color:#f3f4f6; }} }}
 p {{ margin:.4rem; }} .muted {{ color:#6b7280; font-size:.88rem; }}
</style></head><body><div>
  <p>Connecting to MeCloud on this computer…</p>
  <p class="muted">If nothing happens, <a href="{safe}">finish connecting</a>.</p>
</div></body></html>"#
    ))
}

pub const HANDOFF_SUBMIT_JS: &str = "document.getElementById('handoff').submit();";

pub fn handoff_submit_hash() -> String {
    format!(
        "'sha256-{}'",
        STANDARD.encode(sha256(HANDOFF_SUBMIT_JS.as_bytes()))
    )
}

/// Post the device token to the waiting client, with the client's nonce.
///
/// A POST rather than the legacy `?token=` navigation, so a year-long
/// credential never lands in browser history. The page carries its own policy:
/// `form-action` names exactly the loopback origin this request was sealed
/// with, so the relaxation applies to this one response and nowhere else. The
/// one script is pinned by hash; the button is the no-script path.
pub fn handoff_form(redirect: &str, token: &str, state: &str) -> Response {
    let origin = loopback_origin(redirect);
    let csp = format!(
        "default-src 'none'; script-src {}; style-src 'unsafe-inline'; form-action {origin}; \
         base-uri 'none'; frame-ancestors 'none';",
        handoff_submit_hash()
    );
    let safe_redirect = html_escape(redirect);
    let safe_token = html_escape(token);
    let safe_state = html_escape(state);
    let mut response = page(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Connecting…</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
        background:#f6f7f9; color:#111827;
        font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; text-align:center; }}
 @media (prefers-color-scheme: dark) {{ body {{ background:#0b1220; color:#f3f4f6; }} }}
 p {{ margin:.4rem; }} .muted {{ color:#6b7280; font-size:.88rem; }}
 button {{ margin-top:.8rem; padding:.55rem 1.1rem; border:0; border-radius:9px;
           font-weight:600; background:#3b82f6; color:#fff; cursor:pointer; }}
</style></head><body><div>
  <p>Connecting to MeCloud on this computer…</p>
  <form id="handoff" method="post" action="{safe_redirect}">
    <input type="hidden" name="token" value="{safe_token}">
    <input type="hidden" name="state" value="{safe_state}">
    <p class="muted">If nothing happens, press the button.</p>
    <button type="submit">Finish connecting</button>
  </form>
</div><script>{HANDOFF_SUBMIT_JS}</script></body></html>"#
    ));
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&csp) {
        headers.insert(CONTENT_SECURITY_POLICY, value);
    }
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}
