//! Tokens minted by the Python backend this server replaced, and still in
//! the wild when it starts: browser session cookies, paired devices' tokens
//! (good for a year), open pairing requests and download links. Every one of
//! them must keep working, or the upgrade signs everyone out and unpairs every
//! desktop client.
//!
//! fixtures/python-tokens.json was produced by the Python implementation
//! (`cryptography`'s Fernet, `encrypt_at_time`) with a fixed secret and time.

use mecloud_server::crypto::Fernet;
use mecloud_server::session::SessionCodec;
use mecloud_server::{devices, downloads};
use serde_json::{json, Value};

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/python-tokens.json")).unwrap()
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap().to_string()
}

#[test]
fn a_python_session_cookie_opens() {
    let f = fixtures();
    let issued = f["issued_at"].as_u64().unwrap();
    let codec = SessionCodec::new(&text(&f, "secret"), true, 7 * 86_400);
    let (session, at) = codec.decode_at(&text(&f, "session"), issued + 60);
    assert_eq!(at, Some(issued));
    assert_eq!(
        Value::Object(session),
        json!({"access_token": "at-é", "refresh_token": "rt", "token_expires_at": 1794003600.25})
    );
    // Seven days on, it has expired like any other.
    assert!(codec
        .decode_at(&text(&f, "session"), issued + 7 * 86_400 + 1)
        .0
        .is_empty());
}

#[test]
fn a_python_device_token_opens() {
    let f = fixtures();
    let key = Fernet::derived(&text(&f, "secret"), devices::DEVICE_INFO);
    // Devices live a year; the fixture's issue time is in 2026.
    let issued = f["issued_at"].as_u64().unwrap();
    let (plain, _) = key
        .decrypt_at(
            &text(&f, "device"),
            Some(devices::DEVICE_TOKEN_MAX_AGE),
            issued + 86_400,
        )
        .unwrap();
    let data: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(data["u"], "ada@x.test");
    // And through the reader, which the server uses, when within a year.
    if mecloud_server::crypto::now_secs() < issued + devices::DEVICE_TOKEN_MAX_AGE {
        let device =
            devices::read(&key, &text(&f, "device"), devices::DEVICE_TOKEN_MAX_AGE).unwrap();
        assert_eq!(
            (
                device.username.as_str(),
                device.secret.as_str(),
                device.password_id.as_str()
            ),
            ("ada@x.test", "app-pw", "p1")
        );
        assert_eq!(device.name, "Laptop");
        assert_eq!(device.issued_at, issued as i64);
    }
    // The wrong purpose's key opens nothing.
    let pairing_key = Fernet::derived(&text(&f, "secret"), devices::PAIRING_INFO);
    assert!(pairing_key
        .decrypt_at(&text(&f, "device"), None, issued)
        .is_none());
}

#[test]
fn a_python_pairing_request_opens() {
    let f = fixtures();
    let key = Fernet::derived(&text(&f, "secret"), devices::PAIRING_INFO);
    let issued = f["issued_at"].as_u64().unwrap();
    let (plain, _) = key
        .decrypt_at(
            &text(&f, "pairing"),
            Some(devices::PAIRING_MAX_AGE),
            issued + 10,
        )
        .unwrap();
    let data: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(
        data,
        json!({"r": "http://127.0.0.1:4321/paired", "n": "Laptop", "s": "s".repeat(32)})
    );
}

#[test]
fn a_python_download_link_verifies() {
    let f = fixtures();
    let key = downloads::key(&text(&f, "secret"));
    let issued = f["issued_at"].as_u64().unwrap();
    let token = text(&f, "download");
    assert!(downloads::verify(&key, &token, "linux-x86_64", issued + 1));
    assert!(!downloads::verify(&key, &token, "source", issued + 1));
    assert!(!downloads::verify(
        &key,
        &token,
        "linux-x86_64",
        issued + downloads::TOKEN_TTL_SECONDS
    ));
    // And ours is byte-for-byte what Python minted.
    assert_eq!(downloads::mint(&key, "linux-x86_64", issued), token);
}
