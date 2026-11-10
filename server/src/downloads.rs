//! Serving the desktop client.
//!
//! The binary and the source archive are built into the image and served from
//! a fixed directory. Only signed-in users may download, but the download
//! itself has to work outside the app's own fetch — a browser download, a
//! `curl` on a headless box — where the session cookie cannot be relied on.
//! So the session buys a short-lived signed URL rather than the bytes: no
//! state to lose or clean up, at the cost that a link cannot be revoked
//! before it expires — which is why it lives minutes and names one artefact.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::crypto::{ct_eq, hkdf32, hmac_sha256, now_secs, B64URL_NOPAD};
use base64::Engine as _;

/// Long enough to click, start a slow download, or paste into a terminal on
/// another machine; short enough that a link left in shell history is
/// worthless.
pub const TOKEN_TTL_SECONDS: u64 = 300;

pub const DOWNLOAD_INFO: &[u8] = b"mecloud.download.v1";

#[derive(Debug, PartialEq, Eq)]
pub struct Artifact {
    pub key: &'static str,
    pub filename: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub media_type: &'static str,
    pub platform: &'static str,
}

/// A fixed table rather than a directory listing: the key that reaches the
/// filesystem is chosen from here, so a crafted key cannot name a path that
/// was never meant to be downloadable.
pub const ARTIFACTS: &[Artifact] = &[
    Artifact {
        key: "linux-x86_64",
        filename: "mecloud-desktop-linux-x86_64.tar.gz",
        label: "Linux (x86-64)",
        description: "The client, its icon and a desktop entry. Extract and run install.sh.",
        media_type: "application/gzip",
        platform: "linux",
    },
    Artifact {
        key: "source",
        filename: "mecloud-desktop-source.zip",
        label: "Source code",
        description: "Build it yourself — required for Windows and macOS.",
        media_type: "application/zip",
        platform: "source",
    },
];

pub fn artifact(key: &str) -> Option<&'static Artifact> {
    ARTIFACTS.iter().find(|a| a.key == key)
}

pub fn path_for(dir: &Path, art: &Artifact) -> PathBuf {
    dir.join(art.filename)
}

/// What the packaging step recorded. Read on every call: the directory is
/// baked into the image, and a cache would only be a way to be stale. A build
/// without the desktop stage has none, which means nothing to offer.
pub fn manifest(dir: &Path) -> Value {
    std::fs::read(dir.join("manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// The client version this server ships, if it ships one.
pub fn version(dir: &Path) -> Option<String> {
    match manifest(dir).get("version") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// The artefacts actually present, with sizes and, where recorded, digests —
/// the desktop client checks the digest before replacing its own binary.
pub fn available(dir: &Path) -> Vec<Value> {
    let manifest = manifest(dir);
    let mut out = Vec::new();
    for art in ARTIFACTS {
        let Ok(meta) = std::fs::metadata(path_for(dir, art)) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mut entry = json!({
            "key": art.key,
            "label": art.label,
            "description": art.description,
            "platform": art.platform,
            "filename": art.filename,
            "size": meta.len(),
        });
        let digest = manifest
            .get("artifacts")
            .and_then(|a| a.get(art.filename))
            .and_then(|a| a.get("sha256"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if let Some(digest) = digest {
            entry["sha256"] = Value::String(digest.to_string());
        }
        out.push(entry);
    }
    out
}

/// The signing key. Its own HKDF `info`, so a download token can never be
/// mistaken for, or turned into, a session.
pub fn key(secret: &str) -> [u8; 32] {
    hkdf32(secret.as_bytes(), DOWNLOAD_INFO)
}

pub fn mint(key: &[u8], artifact_key: &str, now: u64) -> String {
    let expires = now + TOKEN_TTL_SECONDS;
    let payload = format!("{artifact_key}:{expires}");
    let signature = hmac_sha256(key, payload.as_bytes());
    format!(
        "{}.{}",
        B64URL_NOPAD.encode(payload),
        B64URL_NOPAD.encode(signature)
    )
}

/// Whether `token` authorises `artifact_key` at `now`. Every failure is
/// `false`: there is one thing to do about any of them.
pub fn verify(key: &[u8], token: &str, artifact_key: &str, now: u64) -> bool {
    let Some((payload, signature)) = token.split_once('.') else {
        return false;
    };
    let (Ok(payload), Ok(signature)) =
        (B64URL_NOPAD.decode(payload), B64URL_NOPAD.decode(signature))
    else {
        return false;
    };
    if !ct_eq(&hmac_sha256(key, &payload), &signature) {
        return false;
    }
    let Ok(payload) = String::from_utf8(payload) else {
        return false;
    };
    let Some((key_part, expires)) = payload.rsplit_once(':') else {
        return false;
    };
    let Ok(expires) = expires.trim().parse::<u64>() else {
        return false;
    };
    key_part == artifact_key && now < expires
}

pub fn verify_now(key: &[u8], token: &str, artifact_key: &str) -> bool {
    verify(key, token, artifact_key, now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k() -> [u8; 32] {
        key(&"a".repeat(64))
    }

    #[test]
    fn a_token_is_bound_to_its_artifact_secret_and_lifetime() {
        let now = 1_000_000;
        let token = mint(&k(), "linux-x86_64", now);
        assert!(verify(&k(), &token, "linux-x86_64", now));
        assert!(!verify(&k(), &token, "source", now));
        assert!(!verify(&key(&"b".repeat(64)), &token, "linux-x86_64", now));
        assert!(verify(
            &k(),
            &token,
            "linux-x86_64",
            now + TOKEN_TTL_SECONDS - 1
        ));
        assert!(!verify(
            &k(),
            &token,
            "linux-x86_64",
            now + TOKEN_TTL_SECONDS
        ));
    }

    #[test]
    fn malformed_and_tampered_tokens_are_refused() {
        for junk in ["", "nonsense", "no-dot", "a.b", "....", "%%%.%%%"] {
            assert!(!verify_now(&k(), junk, "source"), "{junk}");
        }
        let signature = mint(&k(), "source", now_secs())
            .split_once('.')
            .unwrap()
            .1
            .to_string();
        let forged = mint(&k(), "linux-x86_64", now_secs())
            .split_once('.')
            .unwrap()
            .0
            .to_string();
        assert!(!verify_now(
            &k(),
            &format!("{forged}.{signature}"),
            "linux-x86_64"
        ));
    }

    #[test]
    fn artifact_lookup_is_a_fixed_table() {
        assert!(artifact("linux-x86_64").is_some() && artifact("source").is_some());
        for bad in ["../../etc/passwd", "linux-x86_64/../..", "", "LINUX-X86_64"] {
            assert!(artifact(bad).is_none());
        }
    }

    #[test]
    fn available_reports_only_what_is_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "mecloud-dl-{}",
            crate::crypto::sha256_hex(&crate::crypto::random_bytes::<8>())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(available(&dir).is_empty());
        std::fs::write(dir.join("mecloud-desktop-source.zip"), b"zip").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"version": "1.2.3", "artifacts": {"mecloud-desktop-source.zip": {"sha256": "ab"}}}"#,
        )
        .unwrap();
        let listed = available(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["key"], "source");
        assert_eq!(listed[0]["size"], 3);
        assert_eq!(listed[0]["sha256"], "ab");
        assert_eq!(version(&dir).as_deref(), Some("1.2.3"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
