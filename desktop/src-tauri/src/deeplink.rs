//! Turning what the desktop hands us into a URL in the web UI.
//!
//! Registering as the system mail handler means the app is launched with a
//! `mailto:` URI as argv, from anything on the machine that can open a link.
//! RFC 6068 is more permissive than it looks — the address list is optional,
//! headers arrive percent-encoded, and `body` can carry newlines — so this is
//! parsed rather than pattern-matched, and it is pure so it can be tested
//! without a desktop.

use url::Url;

/// A request to open something in the embedded web UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Compose a message, pre-filled.
    Compose {
        to: String,
        cc: String,
        subject: String,
        body: String,
    },
    /// Open one of the apps at its own route.
    App(&'static str),
}

impl Target {
    /// Build the URL to point the webview at, against a normalised server base.
    pub fn to_url(&self, server: &str) -> String {
        let base = server.trim_end_matches('/');
        match self {
            Target::App(path) => format!("{base}{path}"),
            Target::Compose { to, cc, subject, body } => {
                let mut url = format!("{base}/mail?compose=1");
                for (key, value) in
                    [("to", to), ("cc", cc), ("subject", subject), ("body", body)]
                {
                    if !value.is_empty() {
                        url.push('&');
                        url.push_str(key);
                        url.push('=');
                        url.push_str(&encode(value));
                    }
                }
                url
            }
        }
    }
}

/// Percent-encode a query parameter value.
///
/// Hand-rolled rather than pulled in: the set of characters that must not
/// survive into a query string is short and fixed, and everything outside
/// unreserved gets encoded, so there is nothing to get subtly wrong.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Interpret one command-line argument. Returns None for anything that is not
/// a link we handle — argv also carries flags and the binary's own path.
pub fn parse(arg: &str) -> Option<Target> {
    let trimmed = arg.trim();
    if !trimmed.to_ascii_lowercase().starts_with("mailto:") {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;

    // The path is the address list: "mailto:a@b,c@d". It may be empty, which
    // is a valid request to compose to nobody.
    let mut to = percent_decode(parsed.path());
    let mut cc = String::new();
    let mut subject = String::new();
    let mut body = String::new();

    for (key, value) in parsed.query_pairs() {
        match key.as_ref().to_ascii_lowercase().as_str() {
            // A `to` header adds to the address list rather than replacing it.
            "to" => to = join_addresses(&to, &value),
            "cc" => cc = join_addresses(&cc, &value),
            "subject" => subject = value.into_owned(),
            "body" => body = value.into_owned(),
            // bcc and everything else are dropped on purpose: the compose
            // window is about to be shown, and silently pre-filling a blind
            // copy from a link someone else wrote is not a good surprise.
            _ => {}
        }
    }

    Some(Target::Compose { to, cc, subject, body })
}

fn join_addresses(existing: &str, extra: &str) -> String {
    match (existing.is_empty(), extra.is_empty()) {
        (_, true) => existing.to_string(),
        (true, false) => extra.to_string(),
        (false, false) => format!("{existing}, {extra}"),
    }
}

/// Decode the percent-escapes in a mailto path. `Url` leaves the path encoded,
/// and the address list is a path, not a query.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).replace(',', ", ")
}

/// Pick the first handled link out of a process's arguments.
pub fn first_target<I: IntoIterator<Item = String>>(args: I) -> Option<Target> {
    args.into_iter().find_map(|a| parse(&a))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "https://cloud.example.com";

    #[test]
    fn ignores_anything_that_is_not_a_mailto() {
        assert!(parse("/usr/bin/mecloud-desktop").is_none());
        assert!(parse("--flag").is_none());
        assert!(parse("https://example.com").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn reads_a_bare_address() {
        let t = parse("mailto:ada@example.com").unwrap();
        assert_eq!(
            t,
            Target::Compose {
                to: "ada@example.com".into(),
                cc: String::new(),
                subject: String::new(),
                body: String::new(),
            }
        );
    }

    #[test]
    fn accepts_a_mailto_with_no_address_at_all() {
        // "Compose a new message" is a legitimate thing for a link to mean.
        assert_eq!(
            parse("mailto:").unwrap(),
            Target::Compose {
                to: String::new(),
                cc: String::new(),
                subject: String::new(),
                body: String::new()
            }
        );
    }

    #[test]
    fn reads_several_addresses_and_headers() {
        let t = parse("mailto:a@x.com,b@x.com?cc=c@x.com&subject=Hello%20there&body=Line%20one")
            .unwrap();
        let Target::Compose { to, cc, subject, body } = t else { panic!() };
        assert_eq!(to, "a@x.com, b@x.com");
        assert_eq!(cc, "c@x.com");
        assert_eq!(subject, "Hello there");
        assert_eq!(body, "Line one");
    }

    #[test]
    fn a_to_header_adds_to_the_path_addresses() {
        let Target::Compose { to, .. } =
            parse("mailto:a@x.com?to=b@x.com").unwrap() else { panic!() };
        assert_eq!(to, "a@x.com, b@x.com");
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let Target::Compose { subject, .. } =
            parse("mailto:a@x.com?SUBJECT=Hi").unwrap() else { panic!() };
        assert_eq!(subject, "Hi");
    }

    #[test]
    fn drops_bcc_rather_than_pre_filling_it() {
        let Target::Compose { to, cc, subject, body } =
            parse("mailto:a@x.com?bcc=secret@x.com").unwrap() else { panic!() };
        assert_eq!(to, "a@x.com");
        assert!(cc.is_empty() && subject.is_empty() && body.is_empty());
    }

    #[test]
    fn decodes_percent_escapes_in_the_address_list() {
        let Target::Compose { to, .. } =
            parse("mailto:first%2Blabel@x.com").unwrap() else { panic!() };
        assert_eq!(to, "first+label@x.com");
    }

    #[test]
    fn builds_a_compose_url_and_encodes_what_it_puts_in_it() {
        let t = parse("mailto:a@x.com?subject=Plates%20%26%20notes&body=a%20b").unwrap();
        assert_eq!(
            t.to_url(SERVER),
            "https://cloud.example.com/mail?compose=1&to=a%40x.com&subject=Plates%20%26%20notes&body=a%20b"
        );
    }

    #[test]
    fn omits_empty_parameters_rather_than_sending_blanks() {
        let t = parse("mailto:").unwrap();
        assert_eq!(t.to_url(SERVER), "https://cloud.example.com/mail?compose=1");
    }

    #[test]
    fn tolerates_a_server_url_with_a_trailing_slash() {
        assert_eq!(
            Target::App("/files").to_url("https://cloud.example.com/"),
            "https://cloud.example.com/files"
        );
    }

    #[test]
    fn finds_the_link_among_other_arguments() {
        let args = vec![
            "/usr/bin/mecloud-desktop".to_string(),
            "--flag".to_string(),
            "mailto:a@x.com".to_string(),
        ];
        assert!(first_target(args).is_some());
        assert!(first_target(vec!["/usr/bin/mecloud-desktop".to_string()]).is_none());
    }
}
