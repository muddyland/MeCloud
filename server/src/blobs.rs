//! Blob upload and download proxying.
//!
//! FileNode metadata rides the ordinary JMAP proxy; only the bytes need these
//! routes, because the bearer token lives in the encrypted session and never
//! reaches the browser, which therefore cannot call the JMAP upload and
//! download endpoints itself. Both directions stream: a 25 MB file buffered
//! per concurrent request is an easy way to lose a small container.

use serde_json::Value;

use crate::crypto::quote;
use crate::error::{AppError, Result};

/// Media types a browser may render inline. Everything else downloads. Short
/// on purpose: anything that can carry script (text/html, image/svg+xml,
/// application/xhtml+xml) is absent, because a rendered document inherits
/// *our* origin and would run as first-party script.
pub const INLINE_SAFE_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "application/pdf",
    "text/plain",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "audio/flac",
    "audio/mp4",
    "video/mp4",
    "video/webm",
    "video/ogg",
];

pub const OCTET_STREAM: &str = "application/octet-stream";

/// A blob id is an opaque server token, pinned to a conservative shape so it
/// can never inject path segments or query parameters into the URL template.
pub fn validate_blob_id(blob_id: &str) -> Result<()> {
    let ok = (1..=256).contains(&blob_id.len())
        && blob_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-=.:~".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(AppError::upstream("Malformed blob id", Some(400)))
    }
}

/// The Content-Type we are prepared to put our own origin behind.
pub fn safe_content_type(declared: Option<&str>, inline: bool) -> String {
    let base = declared
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if inline && INLINE_SAFE_TYPES.contains(&base.as_str()) {
        base
    } else {
        OCTET_STREAM.to_string()
    }
}

/// The policy a blob is served under. Downloads get a blanket `sandbox`.
/// Inline previews cannot: a fully restrictive sandbox disables plugins, and
/// the browser's PDF viewer is one. What keeps a preview safe is earlier —
/// only INLINE_SAFE_TYPES render inline, with `nosniff` — and this policy
/// still grants no way to run anything.
///
/// Two allowances are needed for a preview to work, and neither lets a script
/// run:
///
/// - `style-src 'unsafe-inline'`. WebKit shows a PDF, image or video as a
///   document it builds itself, and sizes that document's content with
///   inline `style` attributes. The response's CSP covers that document, so
///   under `default-src 'none'` the styles are dropped. In WebKitGTK, which is
///   the desktop client's webview and GNOME Web's engine, the PDF viewer then
///   shrinks to the default 300×150 frame size. None of the inline-safe types
///   can carry markup of its own, so the only inline styles this unlocks are
///   the browser's.
/// - `frame-ancestors`. The web UI shows a preview in an iframe on its own
///   origin, so `'self'` is always allowed. Without that, the default
///   `FRAME_ANCESTORS='none'` (sent as `X-Frame-Options: DENY`) would block
///   the app's own preview. Any origin the deployment lets frame the app is
///   added too: frame-ancestors checks every ancestor, so a preview inside an
///   app that is itself framed by a dashboard needs the dashboard listed as
///   well.
pub fn blob_csp(inline: bool, frame_ancestors: &str) -> String {
    if !inline {
        return "sandbox; default-src 'none'; base-uri 'none'".into();
    }
    let mut ancestors = vec!["'self'"];
    ancestors.extend(
        frame_ancestors
            .split_whitespace()
            .filter(|s| *s != "'none'" && *s != "'self'"),
    );
    format!(
        "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data: blob:; \
         media-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'none'; \
         frame-ancestors {}",
        ancestors.join(" ")
    )
}

/// The X-Frame-Options to send with an inline preview, if any.
///
/// Browsers that know CSP ignore this when frame-ancestors is present. It is
/// for older ones, and it has to agree with the CSP. `SAMEORIGIN` does that
/// when only the app's own origin may frame the preview. X-Frame-Options can't
/// name another origin, so with a configured list it is left out and
/// frame-ancestors decides alone.
pub fn inline_x_frame_options(frame_ancestors: &str) -> Option<&'static str> {
    match frame_ancestors {
        "'none'" | "'self'" => Some("SAMEORIGIN"),
        _ => None,
    }
}

/// RFC 6266: an ASCII `filename` fallback and the real name as UTF-8
/// `filename*`. The name is attacker-controlled header material.
pub fn content_disposition(name: Option<&str>, inline: bool) -> String {
    let raw = name.unwrap_or("download").trim();
    let raw = if raw.is_empty() { "download" } else { raw };
    let raw: String = raw.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    let mut ascii: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.-+ ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if ascii.is_empty() {
        ascii = "download".into();
    }
    let disposition = if inline { "inline" } else { "attachment" };
    format!(
        "{disposition}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        quote(&raw)
    )
}

fn template<'a>(session: &'a Value, key: &str) -> &'a str {
    session.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The download URL for a blob. Every substitution is percent-encoded: these
/// values come from the client and end up in a URL we then fetch.
pub fn download_url(
    session: &Value,
    account_id: &str,
    blob_id: &str,
    name: &str,
    type_: &str,
) -> Result<String> {
    let template = template(session, "downloadUrl");
    if template.is_empty() {
        return Err(AppError::upstream(
            "Server does not advertise a download endpoint",
            Some(503),
        ));
    }
    let name = if name.is_empty() { "download" } else { name };
    let type_ = if type_.is_empty() {
        OCTET_STREAM
    } else {
        type_
    };
    Ok(template
        .replace("{accountId}", &quote(account_id))
        .replace("{blobId}", &quote(blob_id))
        .replace("{name}", &quote(name))
        .replace("{type}", &quote(type_)))
}

pub fn upload_url(session: &Value, account_id: &str) -> Result<String> {
    let template = template(session, "uploadUrl");
    if template.is_empty() {
        return Err(AppError::upstream(
            "Server does not advertise an upload endpoint",
            Some(503),
        ));
    }
    Ok(template.replace("{accountId}", &quote(account_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_blob_ids_are_accepted() {
        for id in ["abc123", "G-a_b.c", "n:1234", &"x".repeat(256)] {
            assert!(validate_blob_id(id).is_ok(), "{id}");
        }
    }

    #[test]
    fn hostile_blob_ids_are_rejected() {
        for id in [
            "",
            "../../etc/passwd",
            "a/b",
            "a?x=1",
            "a#frag",
            "a b",
            "http://evil.example/x",
            &"x".repeat(257),
            "a%2f..%2f",
            "abc\n",
        ] {
            let err = validate_blob_id(id).unwrap_err();
            assert_eq!(err.upstream_status(), Some(400), "{id}");
        }
    }

    #[test]
    fn script_capable_types_never_render_inline() {
        for declared in [
            Some("text/html"),
            Some("image/svg+xml"),
            Some("application/xhtml+xml"),
            Some("text/html; charset=utf-8"),
            Some("application/javascript"),
            Some("text/xml"),
            None,
            Some(""),
            Some("nonsense"),
        ] {
            assert_eq!(
                safe_content_type(declared, true),
                OCTET_STREAM,
                "{declared:?}"
            );
        }
    }

    #[test]
    fn allow_listed_types_render_inline_only_when_asked() {
        for t in INLINE_SAFE_TYPES {
            assert_eq!(safe_content_type(Some(t), true), *t);
            assert_eq!(safe_content_type(Some(t), false), OCTET_STREAM);
        }
        assert_eq!(
            safe_content_type(Some("IMAGE/PNG; charset=binary"), true),
            "image/png"
        );
    }

    #[test]
    fn dispositions() {
        assert!(content_disposition(Some("notes.txt"), false).starts_with("attachment;"));
        assert!(content_disposition(Some("photo.png"), true).starts_with("inline;"));
        assert!(content_disposition(Some(""), false).contains("filename=\"download\""));
        assert!(content_disposition(None, false).contains("filename=\"download\""));
        let header = content_disposition(Some("réunion été.pdf"), false);
        assert!(header.contains("filename*=UTF-8''") && header.contains("%C3%A9"));
    }

    #[test]
    fn header_injection_via_filename_is_neutralised() {
        let header = content_disposition(
            Some("a\";\r\nSet-Cookie: stolen=1\r\n\r\n<script>.txt"),
            false,
        );
        assert!(!header.contains('\r') && !header.contains('\n'));
        let ascii = header
            .split_once("filename=\"")
            .unwrap()
            .1
            .split_once('"')
            .unwrap()
            .0;
        assert!(!ascii.contains('"'));
    }

    #[test]
    fn any_filename_yields_an_ascii_header() {
        for name in [
            "réunion été.pdf",
            "文件.txt",
            "Отчёт.txt",
            "Ελληνικά.md",
            "emoji 🎉.png",
            "naïve–dash.txt",
        ] {
            for inline in [true, false] {
                let header = content_disposition(Some(name), inline);
                assert!(header.is_ascii(), "{header}");
                assert!(header.contains("filename*=UTF-8''"));
            }
        }
    }

    #[test]
    fn policies() {
        let download = blob_csp(false, "'none'");
        assert!(download.contains("sandbox") && download.contains("default-src 'none'"));
        let inline = blob_csp(true, "'none'");
        assert!(!inline.contains("sandbox") && !inline.contains("script-src"));
        for needed in [
            "default-src 'none'",
            "object-src 'none'",
            "form-action 'none'",
            "img-src",
            "media-src",
            "style-src 'unsafe-inline'",
            "frame-ancestors 'self'",
        ] {
            assert!(inline.contains(needed), "{needed}");
        }
        // Inline styles only: nothing that runs.
        assert!(!inline.contains("unsafe-eval") && !inline.contains("script"));
        assert!(inline.ends_with("frame-ancestors 'self'"));
    }

    #[test]
    fn preview_framing_follows_the_configured_ancestors() {
        assert!(blob_csp(true, "'self'").ends_with("frame-ancestors 'self'"));
        assert!(blob_csp(true, "https://dash.example https://b.example")
            .ends_with("frame-ancestors 'self' https://dash.example https://b.example"));
        assert_eq!(inline_x_frame_options("'none'"), Some("SAMEORIGIN"));
        assert_eq!(inline_x_frame_options("'self'"), Some("SAMEORIGIN"));
        assert_eq!(inline_x_frame_options("https://dash.example"), None);
    }

    #[test]
    fn substitutions_are_encoded() {
        let session = serde_json::json!({"downloadUrl": "https://m/d/{accountId}/{blobId}/{name}?accept={type}"});
        assert_eq!(
            download_url(&session, "a/b", "B1", "x y.txt", "text/plain").unwrap(),
            "https://m/d/a%2Fb/B1/x%20y.txt?accept=text%2Fplain"
        );
        assert_eq!(
            download_url(&session, "a", "B1", "", "").unwrap(),
            "https://m/d/a/B1/download?accept=application%2Foctet-stream"
        );
        assert_eq!(
            download_url(&serde_json::json!({}), "a", "b", "", "")
                .unwrap_err()
                .upstream_status(),
            Some(503)
        );
    }
}
