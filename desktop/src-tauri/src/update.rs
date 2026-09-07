//! Knowing about, and taking, a new version.
//!
//! The server ships the client, so the server is where "what is current?" is
//! answered: `/api/desktop/releases` reports the version it packaged and the
//! SHA-256 of each artefact. The client compares that with its own version and,
//! if asked, downloads the tarball, checks the digest, and replaces itself.
//!
//! **Threat model, stated plainly.** The digest is fetched over the same
//! authenticated TLS channel as the file, so it proves the download arrived
//! intact — not that the server is honest. A client that installs what this
//! server sends is already trusting it completely: it hands the same server a
//! device token and syncs every file through it. What a separate signing key
//! would add is protection against a *compromised* server, and that is a
//! different and larger promise than anything else here makes. It is worth
//! adding; it is not what this does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// This build's version, from the crate metadata.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub available: Option<String>,
    pub newer: bool,
    /// Set only when an update is both available and downloadable here.
    pub artifact_key: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct Releases {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    key: String,
    platform: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

/// Whether `candidate` is a later version than `current`.
///
/// Dotted numbers, compared component by component, with a missing component
/// treated as zero so "0.2" and "0.2.0" are the same version. Anything
/// unparseable makes the answer "no": refusing to update on a version string
/// nobody understands is the safe direction, and it means a server sending
/// nonsense cannot talk a client into replacing its own binary.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> {
        // Drop any pre-release or build suffix before comparing.
        let core = v.trim().split(['-', '+']).next().unwrap_or("");
        if core.is_empty() {
            return None;
        }
        core.split('.').map(|p| p.parse::<u64>().ok()).collect()
    };
    let (Some(a), Some(b)) = (parse(candidate), parse(current)) else { return false };

    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// Ask the server what it ships.
pub fn check(base: &str, token: &str) -> Result<UpdateInfo, String> {
    let http = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let response = http
        .get(format!("{}/api/desktop/releases", base.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("Could not check for updates: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Could not check for updates ({}).", response.status()));
    }
    let releases: Releases = response.json().map_err(|e| e.to_string())?;

    let mut info = UpdateInfo { current: CURRENT.to_string(), ..Default::default() };
    let Some(available) = releases.version else { return Ok(info) };
    info.newer = is_newer(&available, CURRENT);
    info.available = Some(available);

    if info.newer {
        // Only offer to install what this platform can actually run.
        if let Some(art) = releases.artifacts.iter().find(|a| a.platform == "linux") {
            info.artifact_key = Some(art.key.clone());
            info.sha256 = art.sha256.clone();
            info.size = art.size;
        }
    }
    Ok(info)
}

/// SHA-256 of a file, as lowercase hex.
pub fn digest(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Download the new build, verify it, and put it in place.
///
/// Returns the path of the binary now installed. The caller decides whether to
/// restart; replacing the file does not disturb the running process, because
/// Unix keeps the open inode alive until it exits.
pub fn install(base: &str, token: &str, info: &UpdateInfo) -> Result<PathBuf, String> {
    let key = info.artifact_key.as_deref().ok_or("There is no update for this platform.")?;
    let expected = info.sha256.as_deref().ok_or(
        "The server did not publish a checksum for that download, so it will not be installed.",
    )?;

    let http = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let base = base.trim_end_matches('/');

    // Same two steps the web UI takes: trade the session — here, the device
    // token — for a short-lived signed link, then fetch it.
    let minted: serde_json::Value = http
        .post(format!("{base}/api/desktop/token?artifact={key}"))
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("Could not start the download: {e}"))?
        .json()
        .map_err(|e| e.to_string())?;
    let link = minted.get("token").and_then(|v| v.as_str()).ok_or("The server would not issue a download link.")?;

    let staging = std::env::temp_dir().join(format!("mecloud-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let archive = staging.join("update.tar.gz");

    let mut response = http
        .get(format!("{base}/api/desktop/download/{key}?token={link}"))
        .send()
        .map_err(|e| format!("Download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed ({}).", response.status()));
    }
    {
        let mut file = std::fs::File::create(&archive).map_err(|e| e.to_string())?;
        response.copy_to(&mut file).map_err(|e| e.to_string())?;
    }

    // Checked before anything is unpacked, let alone run.
    let actual = digest(&archive)?;
    if actual != expected {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("The download did not match the checksum the server published, so it was discarded.".into());
    }

    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("Could not unpack the update: {e}"))?;
    if !status.success() {
        return Err("Could not unpack the update.".into());
    }

    let new_binary = find_binary(&staging).ok_or("The update did not contain a client binary.")?;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;

    // Replaced by rename, which is atomic: there is never a moment where the
    // installed path holds a half-written file. The running process keeps its
    // own inode until it exits, so this is safe to do to ourselves.
    let beside = target.with_extension("new");
    std::fs::copy(&new_binary, &beside)
        .map_err(|e| format!("Could not write next to {}: {e}", target.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&beside, &target)
        .map_err(|e| format!("Could not replace {}: {e}", target.display()))?;

    let _ = std::fs::remove_dir_all(&staging);
    Ok(target)
}

fn find_binary(dir: &Path) -> Option<PathBuf> {
    for entry in walkdir::WalkDir::new(dir).max_depth(3).into_iter().flatten() {
        if entry.file_type().is_file() && entry.file_name() == "mecloud-desktop" {
            return Some(entry.path().to_path_buf());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_version_is_newer() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"), "components compare as numbers, not text");
    }

    #[test]
    fn the_same_or_older_version_is_not_newer() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn missing_components_count_as_zero() {
        assert!(!is_newer("0.2", "0.2.0"));
        assert!(is_newer("0.2.1", "0.2"));
    }

    #[test]
    fn a_version_nobody_can_parse_never_triggers_an_update() {
        // The safe direction: a server sending nonsense must not be able to
        // talk a client into replacing its own binary.
        for bad in ["", "latest", "v0.2.0", "0.2.0.x", "not-a-version", "  "] {
            assert!(!is_newer(bad, "0.1.0"), "{bad:?} should not count as newer");
        }
        assert!(!is_newer("0.2.0", "garbage"));
    }

    #[test]
    fn pre_release_suffixes_are_ignored_for_comparison() {
        assert!(is_newer("0.2.0-rc1", "0.1.0"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
    }

    #[test]
    fn the_digest_is_the_standard_sha256() {
        let path = std::env::temp_dir().join(format!("mecloud-digest-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            digest(&path).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_file(&path);
    }
}
