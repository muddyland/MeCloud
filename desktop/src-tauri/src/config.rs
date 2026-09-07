//! Where the client keeps the one thing it needs to know before it can do
//! anything: which server it belongs to.
//!
//! Deliberately not the place for credentials. The embedded web UI signs in
//! through the server's ordinary OAuth2 flow and the session lives in the
//! webview's own cookie jar, so there is nothing secret in this file and it
//! stays readable, editable and safe to copy between machines.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Base URL of the MeCloud server, with no trailing slash.
    #[serde(default)]
    pub server_url: Option<String>,
    /// Whether this client registered itself as the desktop mail handler.
    #[serde(default)]
    pub handle_mailto: bool,
    /// Local folder kept in step with the drive.
    #[serde(default)]
    pub sync_folder: Option<String>,
    /// Whether the user has sync switched on.
    #[serde(default)]
    pub sync_enabled: bool,
    /// Install updates without asking. Off by default: replacing a binary on
    /// someone's machine is something they should opt into.
    #[serde(default)]
    pub auto_update: bool,
}

impl Config {
    pub fn is_configured(&self) -> bool {
        self.server_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    pub fn sync_path(&self) -> Option<PathBuf> {
        self.sync_folder.as_deref().filter(|p| !p.is_empty()).map(PathBuf::from)
    }
}

/// Where the sync database lives.
///
/// In the config directory, not the sync folder: a database inside the folder
/// it describes would be synced to the server, downloaded onto the next
/// machine, and read there as an authoritative record of a sync that never
/// happened on it.
pub fn sync_db_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("sync.db"))
}

/// The device token, kept apart from the config.
///
/// Its own file so it can be `0600` without making the whole config secret,
/// and so "forget this device" is deleting one file.
pub fn token_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("device.token"))
}

pub fn load_token() -> Option<String> {
    let text = fs::read_to_string(token_path()?).ok()?;
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub fn save_token(token: &str) -> Result<(), String> {
    let dir = config_dir().ok_or("No config directory on this system")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("device.token");
    fs::write(&path, token).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    restrict(&path);
    Ok(())
}

pub fn clear_token() -> Result<(), String> {
    let Some(path) = token_path() else { return Ok(()) };
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Owner-only permissions on the token file. A no-op off Unix.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mecloud"))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

/// Read the stored config. A missing or unreadable file is not an error — it
/// means "not set up yet", which is a state the app has to handle on first run
/// regardless, so there is no second failure mode worth inventing here.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(config: &Config) -> Result<(), String> {
    let dir = config_dir().ok_or("No config directory on this system")?;
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = dir.join("config.json");
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// Trim a URL the user typed into something that can be joined with a path.
///
/// People paste "example.com/", "https://example.com/mail" and
/// " https://example.com " with equal confidence, and every one of those has
/// to become the same base or the app silently talks to the wrong place.
/// Whether the input already names a scheme.
///
/// "://" settles it, and so does a leading word before a colon — except that
/// "cloud.example.com:8000" is a host and a port, not a scheme, and treating
/// it as one would reject a perfectly ordinary address. Getting this wrong in
/// the other direction is worse: "mailto:a@b.c" with https:// glued on parses
/// as userinfo and silently resolves to the host b.c.
fn has_scheme(input: &str) -> bool {
    if input.contains("://") {
        return true;
    }
    let Some(colon) = input.find(':') else { return false };
    let (head, rest) = (&input[..colon], &input[colon + 1..]);

    let looks_like_scheme = head.starts_with(|c: char| c.is_ascii_alphabetic())
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let looks_like_port = {
        let digits: String = rest.chars().take_while(|c| *c != '/').collect();
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    };

    looks_like_scheme && !looks_like_port
}

pub fn normalise_server_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter the address of your MeCloud server.".into());
    }

    let with_scheme = if has_scheme(trimmed) {
        trimmed.to_string()
    } else {
        // Assume TLS rather than plain HTTP: guessing wrong in the other
        // direction would silently downgrade the connection.
        format!("https://{trimmed}")
    };

    let parsed = url::Url::parse(&with_scheme)
        .map_err(|_| format!("\"{trimmed}\" is not a valid address."))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("{other}: is not a web address.")),
    }
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err(format!("\"{trimmed}\" has no host name."));
    }
    // Credentials in a server address are always a mistake, and this function
    // drops everything it does not carry forward — so refuse rather than
    // quietly connecting somewhere the user did not mean.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Leave the username and password out of the address.".into());
    }

    let mut base = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap());
    if let Some(port) = parsed.port() {
        base.push_str(&format!(":{port}"));
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_plain_host_and_assumes_tls() {
        assert_eq!(normalise_server_url("example.com").unwrap(), "https://example.com");
    }

    #[test]
    fn keeps_an_explicit_scheme_and_port() {
        assert_eq!(
            normalise_server_url("http://localhost:8000").unwrap(),
            "http://localhost:8000"
        );
    }

    #[test]
    fn strips_paths_queries_and_stray_whitespace() {
        // Pasting the address bar from a signed-in session is the common case.
        assert_eq!(
            normalise_server_url("  https://cloud.example.com/mail?x=1  ").unwrap(),
            "https://cloud.example.com"
        );
        assert_eq!(
            normalise_server_url("https://cloud.example.com/").unwrap(),
            "https://cloud.example.com"
        );
    }

    #[test]
    fn keeps_a_bare_host_and_port_a_host_and_port() {
        // The colon here is a port, not a scheme.
        assert_eq!(
            normalise_server_url("cloud.example.com:8000").unwrap(),
            "https://cloud.example.com:8000"
        );
    }

    #[test]
    fn refuses_credentials_in_the_address() {
        assert!(normalise_server_url("https://ada:hunter2@example.com").is_err());
    }

    #[test]
    fn rejects_empty_and_non_web_addresses() {
        assert!(normalise_server_url("").is_err());
        assert!(normalise_server_url("   ").is_err());
        assert!(normalise_server_url("ftp://example.com").is_err());
        assert!(normalise_server_url("mailto:a@b.c").is_err());
    }

    #[test]
    fn a_config_with_no_sync_folder_has_no_sync_path() {
        assert!(Config::default().sync_path().is_none());
        assert!(Config { sync_folder: Some(String::new()), ..Default::default() }
            .sync_path()
            .is_none());
        assert_eq!(
            Config { sync_folder: Some("/home/ada/MeCloud".into()), ..Default::default() }
                .sync_path()
                .unwrap(),
            PathBuf::from("/home/ada/MeCloud")
        );
    }

    #[test]
    fn the_sync_database_lives_outside_the_synced_folder() {
        // Inside it, the database would sync to the server and then be read on
        // another machine as a record of a sync that never happened there.
        let db = sync_db_path().unwrap();
        assert!(db.ends_with("mecloud/sync.db"), "got {}", db.display());
    }

    #[test]
    fn a_default_config_is_not_configured() {
        assert!(!Config::default().is_configured());
        assert!(!Config { server_url: Some(String::new()), ..Default::default() }.is_configured());
        assert!(Config { server_url: Some("https://x".into()), ..Default::default() }.is_configured());
    }
}
