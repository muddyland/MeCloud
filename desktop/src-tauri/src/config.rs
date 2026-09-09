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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// One-shot permission to run a pass the mass-deletion guard refused.
    #[serde(default)]
    pub allow_bulk_delete_once: bool,
    /// Whether deleting a file here should eventually delete it on the server.
    ///
    /// On by default, because a sync that never removes anything is not a sync
    /// — but never immediate: see `trash_days`.
    #[serde(default = "default_true")]
    pub delete_on_server: bool,
    /// How long a deletion waits before it is applied to the server.
    ///
    /// The window exists because a local deletion is weak evidence: an
    /// unmounted drive, a half-restored backup or a folder swapped underneath
    /// the client all look exactly like "the user deleted these".
    #[serde(default = "default_trash_days")]
    pub trash_days: i64,
    /// Exclusion patterns, one per line, in the syntax `rules.rs` documents.
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
    /// Top-level folders switched off in selective sync.
    #[serde(default)]
    pub excluded_folders: Vec<String>,
    /// Uploads larger than this many megabytes wait to be agreed to. Zero
    /// means never ask, which is what every existing install gets.
    #[serde(default)]
    pub confirm_over_mb: u64,
    /// Whether dot-files sync.
    ///
    /// Defaults to true, and must: an existing install that suddenly stopped
    /// reporting its hidden files would have every one of them read as a
    /// deletion. The safe direction for a new default is always the one that
    /// keeps files visible to the reconciler.
    #[serde(default = "default_true")]
    pub sync_hidden: bool,
    /// Seconds between background passes.
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
}

/// Written out by hand rather than derived.
///
/// `load()` falls back to `Config::default()` for a missing or unreadable
/// file, and a derived `Default` ignores every `#[serde(default = ...)]` —
/// so a derive would hand a first run `delete_on_server: false` and
/// `trash_days: 0` while the deserialiser gave everyone else `true` and `30`.
/// Two sources of truth for one default is how a safety setting ends up off on
/// exactly the installs that never opened the settings screen.
impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: None,
            handle_mailto: false,
            sync_folder: None,
            sync_enabled: false,
            auto_update: false,
            allow_bulk_delete_once: false,
            delete_on_server: default_true(),
            trash_days: default_trash_days(),
            ignore_patterns: Vec::new(),
            excluded_folders: Vec::new(),
            confirm_over_mb: 0,
            sync_hidden: default_true(),
            sync_interval_secs: default_sync_interval(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_trash_days() -> i64 {
    30
}

fn default_sync_interval() -> u64 {
    30
}

/// The floor and ceiling on how often a pass may run.
///
/// A pass walks the sync folder and asks the server for its whole file list.
/// Below a few seconds those overlap and the client spends its life scanning;
/// the floor is not a preference, it is what stops the setting being a way to
/// melt someone's laptop and hammer their server. The ceiling is only there so
/// a typo cannot silently turn sync off for a week.
pub const MIN_SYNC_INTERVAL: u64 = 5;
pub const MAX_SYNC_INTERVAL: u64 = 3600;

impl Config {
    pub fn is_configured(&self) -> bool {
        self.server_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    /// How long to wait between passes, within the supported range.
    pub fn sync_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(
            self.sync_interval_secs.clamp(MIN_SYNC_INTERVAL, MAX_SYNC_INTERVAL),
        )
    }

    /// The exclusion rules this configuration describes.
    pub fn rules(&self) -> crate::rules::Rules {
        crate::rules::Rules::new(
            &self.ignore_patterns,
            &self.excluded_folders,
            (self.confirm_over_mb > 0).then(|| self.confirm_over_mb * 1024 * 1024),
            self.sync_hidden,
        )
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
    fn deletion_defaults_are_safe_for_a_config_written_by_an_older_version() {
        // serde defaults apply to fields absent from an existing config.json,
        // so an upgrade must not silently turn deletion into an immediate one.
        let old: Config = serde_json::from_str(r#"{"server_url":"https://x"}"#).unwrap();
        assert!(old.delete_on_server, "deletion still propagates");
        assert_eq!(old.trash_days, 30, "but never immediately");
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

    #[test]
    fn the_hand_written_default_agrees_with_the_deserialised_one() {
        // These are two independent definitions of the same thing, and the
        // one that gets used depends on whether a file happens to exist.
        let from_empty_json: Config = serde_json::from_str("{}").unwrap();
        let derived = Config::default();

        assert_eq!(derived.delete_on_server, from_empty_json.delete_on_server);
        assert_eq!(derived.trash_days, from_empty_json.trash_days);
        assert_eq!(derived.sync_hidden, from_empty_json.sync_hidden);
        assert_eq!(derived.confirm_over_mb, from_empty_json.confirm_over_mb);
        assert_eq!(derived.auto_update, from_empty_json.auto_update);
        assert_eq!(derived.sync_enabled, from_empty_json.sync_enabled);
        assert_eq!(derived.handle_mailto, from_empty_json.handle_mailto);
        assert_eq!(derived.ignore_patterns, from_empty_json.ignore_patterns);
        assert_eq!(derived.excluded_folders, from_empty_json.excluded_folders);
    }

    #[test]
    fn a_first_run_deletes_on_the_server_only_after_the_full_window() {
        let fresh = Config::default();
        assert!(fresh.delete_on_server);
        assert_eq!(fresh.trash_days, 30);
    }

    #[test]
    fn an_unconfigured_install_excludes_nothing() {
        let rules = Config::default().rules();
        assert!(!rules.excludes("Photos/beach.jpg"));
        assert!(!rules.excludes(".bashrc"));
        assert!(!rules.needs_confirmation(u64::MAX));
    }

    #[test]
    fn the_threshold_is_stored_in_megabytes_and_used_in_bytes() {
        let config = Config { confirm_over_mb: 100, ..Default::default() };
        let rules = config.rules();
        assert!(!rules.needs_confirmation(100 * 1024 * 1024));
        assert!(rules.needs_confirmation(100 * 1024 * 1024 + 1));
    }

    #[test]
    fn the_sync_interval_defaults_to_thirty_seconds() {
        assert_eq!(Config::default().sync_interval().as_secs(), 30);
        let stored: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(stored.sync_interval().as_secs(), 30);
    }

    #[test]
    fn an_out_of_range_interval_is_clamped_rather_than_obeyed() {
        // Zero would be a spin loop against the user's own server.
        let eager = Config { sync_interval_secs: 0, ..Default::default() };
        assert_eq!(eager.sync_interval().as_secs(), MIN_SYNC_INTERVAL);

        let forgetful = Config { sync_interval_secs: 999_999, ..Default::default() };
        assert_eq!(forgetful.sync_interval().as_secs(), MAX_SYNC_INTERVAL);
    }

    #[test]
    fn a_value_in_range_is_used_as_given() {
        let config = Config { sync_interval_secs: 120, ..Default::default() };
        assert_eq!(config.sync_interval().as_secs(), 120);
    }
}
