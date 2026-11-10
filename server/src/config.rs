//! Settings, from the environment and an optional `.env` file.
//!
//! The variable names, defaults and validation are the ones the Python
//! backend's `pydantic-settings` model had, so an existing `.env` works
//! unchanged. Names are case-insensitive; the process environment wins over
//! `.env`, which is read from the working directory if it exists.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ratelimit::{parse_limits, Limit};

#[derive(Clone, Debug)]
pub struct Settings {
    pub app_name: String,
    pub stalwart_url: String,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub oauth_redirect_uri: String,
    pub session_secret: String,
    pub oauth_scope: String,
    pub frontend_static_dir: PathBuf,
    pub desktop_download_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub environment: String,
    pub app_url: String,

    /// Idle lifetime of the session cookie, in seconds. Sliding: the cookie
    /// is re-issued once past half its life.
    pub session_max_age: u64,
    /// Largest body the JMAP proxy and every other route accept.
    pub max_request_bytes: u64,
    /// File uploads get their own, much larger ceiling. Keeping the two apart
    /// matters: the tight cap is what stops a hostile JMAP payload.
    pub max_upload_bytes: u64,
    pub upstream_connect_timeout: f64,
    pub upstream_read_timeout: f64,
    /// CSP frame-ancestors source list, as configured (see `frame_ancestors_value`).
    pub frame_ancestors: String,
    pub rate_limit_jmap: Vec<Limit>,
    pub rate_limit_upload: Vec<Limit>,
    pub rate_limit_download: Vec<Limit>,
    /// Cap on concurrent Server-Sent Event streams, process-wide.
    pub max_event_streams: usize,
    /// Which peers may set X-Forwarded-For (uvicorn's FORWARDED_ALLOW_IPS).
    pub forwarded_allow_ips: String,
}

const DEFAULT_SCOPE: &str =
    "openid offline_access urn:ietf:params:jmap:core urn:ietf:params:jmap:mail \
urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts \
urn:ietf:params:oauth:scope:calendars urn:ietf:params:oauth:scope:files \
urn:stalwart:oauth:scope:security";

impl Settings {
    /// The process environment over `./.env`.
    pub fn from_env() -> Result<Self, String> {
        let mut vars = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(".env") {
            for (k, v) in parse_dotenv(&text) {
                vars.insert(k.to_ascii_uppercase(), v);
            }
        }
        for (k, v) in std::env::vars() {
            vars.insert(k.to_ascii_uppercase(), v);
        }
        Self::from_map(&vars)
    }

    /// From an explicit map of (upper-case) names, for tests and `from_env`.
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, String> {
        let get = |name: &str| vars.get(name).cloned();
        let text = |name: &str, default: &str| get(name).unwrap_or_else(|| default.to_string());
        let required = |name: &str| get(name).ok_or_else(|| format!("{name} must be set"));
        fn number<T: std::str::FromStr>(
            name: &str,
            value: Option<String>,
            default: T,
        ) -> Result<T, String> {
            match value {
                None => Ok(default),
                Some(v) => v
                    .trim()
                    .parse()
                    .map_err(|_| format!("{name} must be a number, not {v:?}")),
            }
        }
        let limits = |name: &str, default: &str| {
            let value = text(name, default);
            parse_limits(&value).map_err(|e| format!("{name}: {e}"))
        };

        let settings = Settings {
            app_name: text("APP_NAME", "MeCloud"),
            stalwart_url: text("STALWART_URL", "https://mail.example.com"),
            oauth_client_id: required("OAUTH_CLIENT_ID")?,
            oauth_client_secret: required("OAUTH_CLIENT_SECRET")?,
            oauth_redirect_uri: text("OAUTH_REDIRECT_URI", "http://localhost:8000/auth/callback"),
            session_secret: required("SESSION_SECRET")?,
            oauth_scope: text("OAUTH_SCOPE", DEFAULT_SCOPE),
            frontend_static_dir: PathBuf::from(text("FRONTEND_STATIC_DIR", "static")),
            desktop_download_dir: PathBuf::from(text("DESKTOP_DOWNLOAD_DIR", "/app/downloads")),
            host: text("HOST", "0.0.0.0"),
            port: number("PORT", get("PORT"), 8000)?,
            environment: text("ENVIRONMENT", "production"),
            app_url: text("APP_URL", ""),
            session_max_age: number("SESSION_MAX_AGE", get("SESSION_MAX_AGE"), 60 * 60 * 24 * 7)?,
            max_request_bytes: number("MAX_REQUEST_BYTES", get("MAX_REQUEST_BYTES"), 1_048_576)?,
            max_upload_bytes: number(
                "MAX_UPLOAD_BYTES",
                get("MAX_UPLOAD_BYTES"),
                64 * 1024 * 1024,
            )?,
            upstream_connect_timeout: number(
                "UPSTREAM_CONNECT_TIMEOUT",
                get("UPSTREAM_CONNECT_TIMEOUT"),
                5.0,
            )?,
            upstream_read_timeout: number(
                "UPSTREAM_READ_TIMEOUT",
                get("UPSTREAM_READ_TIMEOUT"),
                30.0,
            )?,
            frame_ancestors: text("FRAME_ANCESTORS", "'none'"),
            rate_limit_jmap: limits("RATE_LIMIT_JMAP", "600/minute")?,
            rate_limit_upload: limits("RATE_LIMIT_UPLOAD", "240/minute")?,
            rate_limit_download: limits("RATE_LIMIT_DOWNLOAD", "240/minute")?,
            max_event_streams: number("MAX_EVENT_STREAMS", get("MAX_EVENT_STREAMS"), 64)?,
            forwarded_allow_ips: text("FORWARDED_ALLOW_IPS", "127.0.0.1"),
        };
        for (name, value) in [
            (
                "UPSTREAM_CONNECT_TIMEOUT",
                settings.upstream_connect_timeout,
            ),
            ("UPSTREAM_READ_TIMEOUT", settings.upstream_read_timeout),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(format!("{name} must be a positive number of seconds"));
            }
        }
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<(), String> {
        if self.is_production() && self.app_url.is_empty() {
            return Err("APP_URL must be set when ENVIRONMENT is 'production'. \
                        Set ENVIRONMENT=development for local development without HTTPS."
                .into());
        }
        // The session secret is the only thing standing between a cookie and
        // the OAuth tokens sealed inside it: refuse to boot with a placeholder.
        if self.is_production() {
            if self.session_secret.chars().count() < 32 {
                return Err("SESSION_SECRET must be at least 32 characters in production.".into());
            }
            if self.session_secret.to_lowercase().contains("change-me") {
                return Err(
                    "SESSION_SECRET is still the example placeholder — generate a real one.".into(),
                );
            }
        }
        Ok(())
    }

    pub fn is_production(&self) -> bool {
        self.environment.to_lowercase() != "development"
    }

    pub fn jmap_session_url(&self) -> String {
        format!("{}/.well-known/jmap", self.stalwart_url)
    }

    pub fn openid_config_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-authorization-server",
            self.stalwart_url
        )
    }

    /// The frame-ancestors list with anything that could end the directive or
    /// inject a header removed — this lands directly in a response header.
    pub fn frame_ancestors_value(&self) -> String {
        let mut raw = self
            .frame_ancestors
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for bad in [';', '\n', '\r', ','] {
            raw = raw.replace(bad, " ");
        }
        let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() {
            "'none'".into()
        } else {
            cleaned
        }
    }

    /// The legacy equivalent, where one exists. X-Frame-Options cannot express
    /// an allow-list, so for a specific origin it is omitted and
    /// frame-ancestors governs alone; DENY alongside would block the frame.
    pub fn x_frame_options(&self) -> Option<&'static str> {
        match self.frame_ancestors_value().as_str() {
            "'none'" => Some("DENY"),
            "'self'" => Some("SAMEORIGIN"),
            _ => None,
        }
    }

    /// Host header allow-list; None means any host (development).
    pub fn allowed_hosts(&self) -> Option<Vec<String>> {
        if !self.is_production() {
            return None;
        }
        let mut hosts = vec!["localhost".to_string(), "127.0.0.1".to_string()];
        for url in [&self.app_url, &self.oauth_redirect_uri] {
            if let Some(host) = url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
            {
                let host = host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_lowercase();
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
        }
        hosts.sort();
        Some(hosts)
    }

    pub fn cors_origins(&self) -> Vec<String> {
        if self.is_production() {
            vec![self.app_url.clone()]
        } else {
            vec![
                "http://localhost:5173".into(),
                "http://localhost:8000".into(),
            ]
        }
    }
}

/// `KEY=value` lines; `#` comments, `export ` prefixes and matching quotes
/// are handled, which covers what a hand-written `.env` contains.
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let mut value = value.trim();
        let quoted = value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')));
        if quoted {
            value = &value[1..value.len() - 1];
        } else if let Some(i) = value.find(" #") {
            value = value[..i].trim_end();
        }
        out.push((key.to_string(), value.to_string()));
    }
    out
}

#[cfg(test)]
pub(crate) fn test_vars() -> HashMap<String, String> {
    [
        ("ENVIRONMENT", "development"),
        ("STALWART_URL", "http://mail.test"),
        ("OAUTH_CLIENT_ID", "test-client"),
        ("OAUTH_CLIENT_SECRET", "test-secret"),
        ("SESSION_SECRET", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(extra: &[(&str, &str)]) -> Result<Settings, String> {
        let mut vars = test_vars();
        for (k, v) in extra {
            vars.insert(k.to_string(), v.to_string());
        }
        Settings::from_map(&vars)
    }

    #[test]
    fn frame_ancestors_self_downgrades_to_sameorigin() {
        assert_eq!(
            with(&[("FRAME_ANCESTORS", "'self'")])
                .unwrap()
                .x_frame_options(),
            Some("SAMEORIGIN")
        );
    }

    #[test]
    fn an_explicit_origin_drops_x_frame_options() {
        let s = with(&[("FRAME_ANCESTORS", "https://dash.example")]).unwrap();
        assert_eq!(s.x_frame_options(), None);
        assert_eq!(s.frame_ancestors_value(), "https://dash.example");
    }

    #[test]
    fn frame_ancestors_is_sanitised() {
        for (hostile, expected) in [
            ("'self'; script-src *", "'self' script-src *"),
            ("'self'\r\nX-Evil: 1", "'self' X-Evil: 1"),
            ("   ", "'none'"),
        ] {
            assert_eq!(
                with(&[("FRAME_ANCESTORS", hostile)])
                    .unwrap()
                    .frame_ancestors_value(),
                expected
            );
        }
    }

    #[test]
    fn production_needs_an_app_url_and_a_real_secret() {
        assert!(with(&[("ENVIRONMENT", "production")])
            .unwrap_err()
            .contains("APP_URL"));
        let prod = [
            ("ENVIRONMENT", "production"),
            ("APP_URL", "https://mail.example.com"),
        ];
        assert!(with(&[prod[0], prod[1], ("SESSION_SECRET", "short")])
            .unwrap_err()
            .contains("32"));
        let placeholder = "change-me-to-a-random-64-char-hex-string";
        assert!(with(&[prod[0], prod[1], ("SESSION_SECRET", placeholder)])
            .unwrap_err()
            .contains("placeholder"));
        assert!(with(&prod).is_ok());
    }

    #[test]
    fn the_host_allow_list_names_the_configured_hosts() {
        let s = with(&[
            ("ENVIRONMENT", "production"),
            ("APP_URL", "https://Mail.Example.com"),
            (
                "OAUTH_REDIRECT_URI",
                "https://mail.example.com/auth/callback",
            ),
        ])
        .unwrap();
        assert_eq!(
            s.allowed_hosts().unwrap(),
            ["127.0.0.1", "localhost", "mail.example.com"]
        );
        assert_eq!(with(&[]).unwrap().allowed_hosts(), None);
    }

    #[test]
    fn bad_numbers_and_limits_refuse_to_boot() {
        assert!(with(&[("PORT", "eighty")]).is_err());
        assert!(with(&[("RATE_LIMIT_JMAP", "lots")]).is_err());
        assert!(with(&[("UPSTREAM_READ_TIMEOUT", "0")]).is_err());
    }

    #[test]
    fn dotenv_lines() {
        let parsed =
            parse_dotenv("# c\nA=1\nexport B = \"two words\"\nC=x # note\n\nD='q'\nnoequals\n");
        assert_eq!(
            parsed,
            [("A", "1"), ("B", "two words"), ("C", "x"), ("D", "q")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
        );
    }
}
