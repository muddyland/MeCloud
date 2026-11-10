//! Credentials for the desktop client's background work, and the pairing
//! handshake that issues them.
//!
//! The sync engine runs with no window open, so it needs a credential of its
//! own. A device token is the session cookie's idea with a longer life and its
//! own key: an encrypted blob holding an app password created for this device
//! alone. Nothing is stored here — this server deliberately has no database —
//! and revocation is the mail server's job: deleting the app password in App
//! Passwords disconnects that one device. Rotating SESSION_SECRET disconnects
//! every device, and every browser session with it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::crypto::{now_secs, Fernet};

/// A paired device is expected to keep working for a long time; the app
/// password inside is the real limit, and the mail server governs that.
pub const DEVICE_TOKEN_MAX_AGE: u64 = 60 * 60 * 24 * 365;

/// How long a pairing request stays open. The user has to read a consent
/// screen and click, not make a cup of tea.
pub const PAIRING_MAX_AGE: u64 = 300;

pub const DEVICE_INFO: &[u8] = b"mecloud.device.v1";
pub const PAIRING_INFO: &[u8] = b"mecloud.pairing.v1";

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub username: String,
    pub secret: String,
    pub password_id: String,
    pub name: String,
    pub issued_at: i64,
}

impl Device {
    /// An Authorization header value for the mail server.
    pub fn basic_credential(&self) -> String {
        format!(
            "Basic {}",
            STANDARD.encode(format!("{}:{}", self.username, self.secret))
        )
    }
}

/// The first `n` characters (not bytes), as Python's `s[:n]`.
pub fn truncate_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

pub fn issue(
    fernet: &Fernet,
    app_password: &str,
    password_id: &str,
    username: &str,
    name: &str,
) -> String {
    let payload = json!({
        "s": app_password,
        "pid": password_id,
        "u": username,
        "n": truncate_chars(name, 120),
        "iat": now_secs(),
    });
    fernet.encrypt(payload.to_string().as_bytes())
}

/// Python's `str(value)` for the scalars a token can hold.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.into()),
        _ => None,
    }
}

/// Decrypt a device token, or None if it is not one we issued. Every failure
/// looks the same from outside: telling them apart only helps someone probing.
pub fn read(fernet: &Fernet, token: &str, max_age: u64) -> Option<Device> {
    if token.is_empty() {
        return None;
    }
    let (plain, _) = fernet.decrypt(token, Some(max_age))?;
    let data: Value = serde_json::from_slice(&plain).ok()?;
    let text = |key: &str| data.get(key).and_then(as_text);
    Some(Device {
        username: text("u")?,
        secret: text("s")?,
        password_id: text("pid").unwrap_or_default(),
        name: text("n").unwrap_or_default(),
        issued_at: match data.get("iat") {
            None => 0,
            Some(v) => v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?,
        },
    })
}

/// The token in an Authorization header, if it is a bearer one.
pub fn bearer_token(authorization: Option<&str>) -> Option<String> {
    let header = authorization?;
    let (scheme, value) = header.split_once(' ').unwrap_or((header, ""));
    let value = value.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !value.is_empty()).then(|| value.to_string())
}

// ── Pairing handshake ───────────────────────────────────────────────────────
//
// The client cannot simply ask for a token: anything running on the machine
// could. It opens a page in the user's signed-in session, which asks them to
// approve, and the answer goes back to a loopback port the client listens on.

/// Wrap a pairing request so the consent page cannot be pointed elsewhere.
/// The redirect travels through the browser; sealed, the value the approval
/// acts on is the value this server checked.
pub fn seal_pairing(fernet: &Fernet, redirect: &str, name: &str, state: &str) -> String {
    let payload = json!({ "r": redirect, "n": truncate_chars(name, 120), "s": state });
    fernet.encrypt(payload.to_string().as_bytes())
}

/// (redirect, name, state); state is empty for a client that predates it.
pub fn open_pairing(fernet: &Fernet, sealed: &str) -> Option<(String, String, String)> {
    let (plain, _) = fernet.decrypt(sealed, Some(PAIRING_MAX_AGE))?;
    let data: Value = serde_json::from_slice(&plain).ok()?;
    let text = |key: &str| data.get(key).and_then(as_text);
    Some((
        text("r")?,
        text("n").unwrap_or_default(),
        text("s").unwrap_or_default(),
    ))
}

/// The client's nonce, binding the reply to the request it made. Without it,
/// anything that can reach the loopback port while the client waits could
/// hand it a token of its own choosing.
pub fn is_valid_state(state: &str) -> bool {
    (16..=128).contains(&state.len())
        && state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The parts of an `http://host:port/...` URL, read the way Python's
/// `urlparse` reads them: (scheme, lower-cased host without brackets, port).
fn split_url(url: &str) -> Option<(String, String, Option<u16>)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return None;
    }
    let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let port = match port {
        None | Some("") => None,
        Some(p) if p.bytes().all(|b| b.is_ascii_digit()) => Some(p.parse::<u16>().ok()?),
        Some(_) => return None,
    };
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase(), port))
}

/// Whether a pairing redirect is somewhere only this machine can receive. The
/// token goes to whatever answers it, so: plain http, a literal loopback
/// address, an unprivileged port. `localhost` is refused deliberately — it is
/// a name, and a name can be made to resolve anywhere.
pub fn is_loopback_redirect(url: &str) -> bool {
    match split_url(url) {
        Some((scheme, host, Some(port))) => {
            scheme == "http" && (host == "127.0.0.1" || host == "::1") && port >= 1024
        }
        _ => false,
    }
}

/// `scheme://host:port` of an already-validated loopback redirect, for CSP.
pub fn loopback_origin(url: &str) -> String {
    let (scheme, host, port) = split_url(url).unwrap_or_default();
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    format!("{scheme}://{host}:{}", port.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss";

    fn device_key() -> Fernet {
        Fernet::derived(SECRET, DEVICE_INFO)
    }

    #[test]
    fn a_device_token_round_trips() {
        let token = issue(&device_key(), "pw-123", "p1", "ada@x.test", "Laptop");
        let device = read(&device_key(), &token, DEVICE_TOKEN_MAX_AGE).unwrap();
        assert_eq!(
            (device.secret.as_str(), device.password_id.as_str()),
            ("pw-123", "p1")
        );
        assert_eq!(
            (device.username.as_str(), device.name.as_str()),
            ("ada@x.test", "Laptop")
        );
        let header = device.basic_credential();
        let value = header.strip_prefix("Basic ").unwrap();
        assert_eq!(STANDARD.decode(value).unwrap(), b"ada@x.test:pw-123");
    }

    #[test]
    fn a_device_token_is_bound_to_its_secret_and_purpose() {
        let token = issue(&device_key(), "pw", "p", "a", "n");
        assert!(read(
            &Fernet::derived(&"o".repeat(64), DEVICE_INFO),
            &token,
            DEVICE_TOKEN_MAX_AGE
        )
        .is_none());
        // Same primitive, different key: a pairing request is not a device.
        let sealed = seal_pairing(
            &Fernet::derived(SECRET, PAIRING_INFO),
            "http://127.0.0.1:4321/x",
            "n",
            "",
        );
        assert!(read(&device_key(), &sealed, DEVICE_TOKEN_MAX_AGE).is_none());
        for junk in ["", "nonsense", "a.b.c", &"x".repeat(200)] {
            assert!(read(&device_key(), junk, DEVICE_TOKEN_MAX_AGE).is_none());
        }
    }

    #[test]
    fn a_device_token_expires() {
        let token = issue(&device_key(), "pw", "p", "a", "n");
        assert!(read(&device_key(), &token, DEVICE_TOKEN_MAX_AGE).is_some());
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert!(read(&device_key(), &token, 1).is_none());
    }

    #[test]
    fn bearer_token_parsing() {
        for (header, expected) in [
            (Some("Bearer abc"), Some("abc")),
            (Some("bearer abc"), Some("abc")),
            (Some("BEARER   abc  "), Some("abc")),
            (Some("Basic abc"), None),
            (Some("abc"), None),
            (Some("Bearer"), None),
            (Some("Bearer   "), None),
            (Some(""), None),
            (None, None),
        ] {
            assert_eq!(bearer_token(header).as_deref(), expected, "{header:?}");
        }
    }

    #[test]
    fn a_sealed_pairing_request_round_trips_and_cannot_be_forged() {
        let key = Fernet::derived(SECRET, PAIRING_INFO);
        let sealed = seal_pairing(&key, "http://127.0.0.1:4321/paired", "Laptop", "");
        assert_eq!(
            open_pairing(&key, &sealed),
            Some((
                "http://127.0.0.1:4321/paired".into(),
                "Laptop".into(),
                "".into()
            ))
        );
        let sealed = seal_pairing(
            &key,
            "http://127.0.0.1:4321/paired",
            "Laptop",
            &"s".repeat(32),
        );
        assert_eq!(open_pairing(&key, &sealed).unwrap().2, "s".repeat(32));
        let forged = seal_pairing(
            &Fernet::derived(&"o".repeat(64), PAIRING_INFO),
            "http://127.0.0.1:1/x",
            "n",
            "",
        );
        assert!(open_pairing(&key, &forged).is_none());
        assert!(open_pairing(&key, "not-a-token").is_none());
    }

    #[test]
    fn loopback_redirects() {
        for url in [
            "http://127.0.0.1:1024/paired",
            "http://127.0.0.1:65535/paired",
            "http://[::1]:8123/paired",
        ] {
            assert!(is_loopback_redirect(url), "{url}");
        }
        for url in [
            "http://localhost:4321/paired",
            "https://127.0.0.1:4321/paired",
            "http://evil.example.com/paired",
            "http://127.0.0.1/paired",
            "http://127.0.0.1:80/paired",
            "http://127.0.0.1.evil.com:8000/x",
            "http://0.0.0.0:8000/x",
            "http://127.0.0.1:99999/x",
            "http://2130706433:8000/x",
            "file:///tmp/x",
            "",
        ] {
            assert!(!is_loopback_redirect(url), "{url}");
        }
    }

    #[test]
    fn state_shape() {
        for (state, ok) in [
            ("a".repeat(16), true),
            ("A-z_09".repeat(5), true),
            ("a".repeat(128), true),
            ("a".repeat(15), false),
            ("a".repeat(129), false),
            ("a".repeat(20) + "!", false),
            (String::new(), false),
            ("a".repeat(20) + "&token=x", false),
            ("a".repeat(20) + "\n", false),
        ] {
            assert_eq!(is_valid_state(&state), ok, "{state:?}");
        }
    }

    #[test]
    fn origins() {
        assert_eq!(
            loopback_origin("http://127.0.0.1:41234/paired"),
            "http://127.0.0.1:41234"
        );
        assert_eq!(
            loopback_origin("http://[::1]:8123/paired?x=1"),
            "http://[::1]:8123"
        );
    }
}
