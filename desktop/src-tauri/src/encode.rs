//! Percent-encoding, in one place.
//!
//! There used to be four hand-rolled encoders and two decoders spread over the
//! crate, each slightly different. These wrap the `percent-encoding` crate that
//! `url` already depends on, with the two character sets the client needs.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// Everything except RFC 3986 unreserved characters is escaped.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// As `COMPONENT`, but `/` survives: for a relative path carried in a query.
const PATH: &AsciiSet = &COMPONENT.remove(b'/');

/// Encode one query value or path segment.
pub fn component(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT).to_string()
}

/// Encode a slash-separated relative path, keeping its slashes.
pub fn path(value: &str) -> String {
    utf8_percent_encode(value, PATH).to_string()
}

/// Decode percent-escapes. Invalid UTF-8 is replaced rather than refused.
/// `+` is left alone: this is for URL paths, not form bodies.
pub fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_escape_what_would_break_a_query() {
        assert_eq!(component("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(component("plain-name_1.txt~"), "plain-name_1.txt~");
        assert_eq!(component("a/b"), "a%2Fb");
        assert_eq!(component("Ada's laptop"), "Ada%27s%20laptop");
    }

    #[test]
    fn paths_keep_their_slashes() {
        assert_eq!(path("Photos/2026/beach day.jpg"), "Photos/2026/beach%20day.jpg");
    }

    #[test]
    fn decoding_round_trips_what_is_encoded() {
        for value in ["plain", "a b", "a/b?c=d&e", "Ada's laptop", "unicode ✓", "100%"] {
            assert_eq!(decode(&component(value)), value);
        }
        // A stray percent sign is kept, not dropped.
        assert_eq!(decode("100%"), "100%");
    }
}
