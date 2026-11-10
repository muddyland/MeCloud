//! The primitives behind every token this server issues.
//!
//! Session cookies, device tokens and pairing requests are Fernet tokens
//! (AES-128-CBC + HMAC-SHA256, <https://github.com/fernet/spec>); download
//! links are a bare HMAC-SHA256. Every key is derived from SESSION_SECRET with
//! HKDF-SHA256 under its own `info` string, so a token minted for one purpose
//! is worthless for any other.
//!
//! These are byte-for-byte the formats the Python backend produced with
//! `cryptography`, which is the point: replacing the server must not sign
//! anyone out or unpair a single desktop client. `tests/compat.rs` holds
//! tokens the Python implementation minted, and checks they still open.

use std::time::{SystemTime, UNIX_EPOCH};

use aes::Aes128;
use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;
type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;

/// URL-safe base64 that accepts input with or without padding, and emits it
/// padded (what Fernet tokens look like).
pub const B64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// URL-safe base64 without padding (download tokens, PKCE, OAuth state).
pub const B64URL_NOPAD: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds since the epoch with a fractional part, as Python's `time.time()`.
/// The session's `token_expires_at` has always been stored this way.
pub fn now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the operating system has no randomness to give");
    buf
}

/// HKDF-SHA256 with no salt, 32 bytes out — `cryptography`'s
/// `HKDF(SHA256(), length=32, salt=None, info=info)`.
pub fn hkdf32(secret: &[u8], info: &[u8]) -> [u8; 32] {
    let hk = hkdf::Hkdf::<Sha256>::new(None, secret);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Constant-time equality. Unequal lengths are unequal, and say so early —
/// which leaks only the length, as Python's `hmac.compare_digest` does.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Fernet as specified, with `cryptography`'s acceptance rules.
#[derive(Clone)]
pub struct Fernet {
    signing: [u8; 16],
    encryption: [u8; 16],
}

/// How far in the future a token's timestamp may be before it is refused.
const MAX_CLOCK_SKEW: u64 = 60;

/// Version byte + timestamp + IV.
const HEADER_LEN: usize = 1 + 8 + 16;
const MAC_LEN: usize = 32;

impl Fernet {
    pub fn new(key: [u8; 32]) -> Self {
        let mut signing = [0u8; 16];
        let mut encryption = [0u8; 16];
        signing.copy_from_slice(&key[..16]);
        encryption.copy_from_slice(&key[16..]);
        Fernet {
            signing,
            encryption,
        }
    }

    /// The key a Python `Fernet(urlsafe_b64encode(HKDF(...)))` would use.
    pub fn derived(secret: &str, info: &[u8]) -> Self {
        Fernet::new(hkdf32(secret.as_bytes(), info))
    }

    pub fn encrypt(&self, data: &[u8]) -> String {
        self.encrypt_at(data, now_secs(), random_bytes::<16>())
    }

    pub fn encrypt_at(&self, data: &[u8], timestamp: u64, iv: [u8; 16]) -> String {
        let ciphertext = Encryptor::new_from_slices(&self.encryption, &iv)
            .expect("16-byte key and IV")
            .encrypt_padded_vec_mut::<Pkcs7>(data);
        let mut token = Vec::with_capacity(HEADER_LEN + ciphertext.len() + MAC_LEN);
        token.push(0x80);
        token.extend_from_slice(&timestamp.to_be_bytes());
        token.extend_from_slice(&iv);
        token.extend_from_slice(&ciphertext);
        let mac = hmac_sha256(&self.signing, &token);
        token.extend_from_slice(&mac);
        B64URL.encode(token)
    }

    /// Plaintext and issue time, or None for anything that is not a valid,
    /// unexpired token under this key. `ttl` None skips the age check.
    pub fn decrypt(&self, token: &str, ttl: Option<u64>) -> Option<(Vec<u8>, u64)> {
        self.decrypt_at(token, ttl, now_secs())
    }

    pub fn decrypt_at(&self, token: &str, ttl: Option<u64>, now: u64) -> Option<(Vec<u8>, u64)> {
        let raw = B64URL.decode(token.trim().as_bytes()).ok()?;
        if raw.len() < HEADER_LEN + 16 + MAC_LEN || raw[0] != 0x80 {
            return None;
        }
        let timestamp = u64::from_be_bytes(raw[1..9].try_into().ok()?);
        if let Some(ttl) = ttl {
            if timestamp.saturating_add(ttl) < now {
                return None;
            }
        }
        if now.saturating_add(MAX_CLOCK_SKEW) < timestamp {
            return None;
        }
        let (signed, mac) = raw.split_at(raw.len() - MAC_LEN);
        if !ct_eq(&hmac_sha256(&self.signing, signed), mac) {
            return None;
        }
        let iv = &signed[9..HEADER_LEN];
        let ciphertext = &signed[HEADER_LEN..];
        if ciphertext.len() % 16 != 0 {
            return None;
        }
        let plain = Decryptor::new_from_slices(&self.encryption, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .ok()?;
        Some((plain, timestamp))
    }
}

/// Python's `html.escape(s, quote=True)`.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// Characters `urllib.parse.quote` leaves alone with `safe=""`: the RFC 3986
/// unreserved set.
pub const QUOTE_ALL: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `urllib.parse.quote`'s default, which also keeps `/`.
pub const QUOTE_PATH: &percent_encoding::AsciiSet = &QUOTE_ALL.remove(b'/');

pub fn quote(text: &str) -> String {
    percent_encoding::utf8_percent_encode(text, QUOTE_ALL).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fernet_round_trips() {
        let f = Fernet::derived("secret-secret-secret-secret-0001", b"test");
        let token = f.encrypt(b"hello");
        let (plain, ts) = f.decrypt(&token, Some(60)).unwrap();
        assert_eq!(plain, b"hello");
        assert!(ts.abs_diff(now_secs()) <= 1);
    }

    #[test]
    fn fernet_matches_the_spec_vector() {
        // https://github.com/fernet/spec/blob/master/generate.json
        let key = B64URL
            .decode("cw_0x689RpI-jtRR7oE8h_eQsKImvJapLeSbXpwF4e4=")
            .unwrap();
        let f = Fernet::new(key.try_into().unwrap());
        let iv: [u8; 16] = (0u8..16).collect::<Vec<_>>().try_into().unwrap();
        let token = f.encrypt_at(b"hello", 499162800, iv);
        assert_eq!(
            token,
            "gAAAAAAdwJ6wAAECAwQFBgcICQoLDA0ODy021cpGVWKZ_eEwCGM4BLLF_5CV9dOPmrhuVUPgJobwOz7JcbmrR64jVmpU4IwqDA=="
        );
        assert_eq!(
            f.decrypt_at(&token, Some(60), 499162801).unwrap().0,
            b"hello"
        );
    }

    #[test]
    fn fernet_refuses_what_it_should() {
        let f = Fernet::derived("k", b"a");
        let token = f.encrypt_at(b"x", 1_000, [7; 16]);
        assert!(f.decrypt_at(&token, Some(10), 1_011).is_none(), "expired");
        assert!(
            f.decrypt_at(&token, Some(10), 1_010).is_some(),
            "on the boundary"
        );
        assert!(f.decrypt_at(&token, None, 1_000_000).is_some(), "no ttl");
        assert!(f.decrypt_at(&token, None, 900).is_none(), "from the future");
        assert!(
            Fernet::derived("k", b"b")
                .decrypt_at(&token, None, 1_000)
                .is_none(),
            "other key"
        );
        let mut raw = B64URL.decode(&token).unwrap();
        let last = raw.len() - 40;
        raw[last] ^= 1;
        assert!(
            f.decrypt_at(&B64URL.encode(raw), None, 1_000).is_none(),
            "tampered"
        );
        for junk in ["", "nonsense", "a.b.c", &"x".repeat(200)] {
            assert!(f.decrypt(junk, None).is_none(), "{junk}");
        }
    }

    #[test]
    fn html_escape_matches_python() {
        assert_eq!(
            html_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#x27;&amp;&#x27;&lt;/a&gt;"
        );
    }

    #[test]
    fn quote_matches_python() {
        assert_eq!(quote("a b/c~é"), "a%20b%2Fc~%C3%A9");
        assert_eq!(
            percent_encoding::utf8_percent_encode("tok/+=", QUOTE_PATH).to_string(),
            "tok/%2B%3D"
        );
    }
}
