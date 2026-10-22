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

/// This build's version, from the crate metadata.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub available: Option<String>,
    pub newer: bool,
    /// What the check actually concluded, in words.
    ///
    /// Needed because `newer: false` covers two quite different situations —
    /// "you already have the current version" and "this server does not
    /// publish a client at all" — and reporting the second as the first tells
    /// the user their setup is fine when it is not.
    pub detail: String,
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
    let http = crate::api::http_client(std::time::Duration::from_secs(30))?;

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

    let Some(available) = releases.version else {
        info.detail = "This server does not publish a desktop client, so there is nothing to \
                       update to. It was probably built without one."
            .into();
        return Ok(info);
    };

    info.newer = is_newer(&available, CURRENT);
    info.available = Some(available.clone());

    if !info.newer {
        info.detail = if available == CURRENT {
            format!("Up to date — this server ships {available}.")
        } else {
            // Older on the server than here: a rollback, or a client installed
            // from somewhere else. Saying so beats an unqualified "up to date".
            format!("Up to date — this server ships {available}, older than this client.")
        };
        return Ok(info);
    }

    match releases.artifacts.iter().find(|a| a.platform == "linux") {
        Some(art) => {
            info.artifact_key = Some(art.key.clone());
            info.sha256 = art.sha256.clone();
            info.size = art.size;
            info.detail = format!("Version {available} is available.");
        }
        None => {
            // A version with no download is not an update anyone can take.
            info.newer = false;
            info.detail = format!(
                "This server reports version {available}, but publishes no download for this \
                 platform, so it cannot be installed from here."
            );
        }
    }
    Ok(info)
}

/// SHA-256 of a file, as lowercase hex.
pub fn digest(path: &Path) -> Result<String, String> {
    crate::scan::hash_file(path).map(|(hash, _)| hash)
}

/// A fresh, private directory to unpack an update in.
///
/// Under the user's cache directory with mode 0700, not a predictable name in
/// the shared /tmp: there, another account could create the directory first
/// and swap the archive or the binary between the checksum and the install.
fn staging_dir() -> Result<PathBuf, String> {
    let base = dirs::cache_dir().ok_or("No cache directory on this system")?.join("mecloud");
    let dir = base.join(format!("update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        // `mode` only applies to directories this call created; make sure of
        // the ones that matter even if they already existed.
        use std::os::unix::fs::PermissionsExt;
        for d in [&base, &dir] {
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(dir)
}

/// The system tar, by absolute path rather than whatever PATH offers.
fn tar_binary() -> Result<&'static str, String> {
    ["/usr/bin/tar", "/bin/tar"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .ok_or_else(|| "tar is not installed, so the update cannot be unpacked.".to_string())
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

    let http = crate::api::http_client(std::time::Duration::from_secs(600))?;
    let base = base.trim_end_matches('/');

    // Same two steps the web UI takes: trade the session — here, the device
    // token — for a short-lived signed link, then fetch it.
    let response = http
        .post(format!("{base}/api/desktop/token?artifact={}", crate::encode::component(key)))
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("Could not start the download: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The server would not issue a download link ({}).", response.status()));
    }
    let minted: serde_json::Value = response.json().map_err(|e| e.to_string())?;
    let link = minted.get("token").and_then(|v| v.as_str()).ok_or("The server would not issue a download link.")?;

    let staging = staging_dir()?;
    let archive = staging.join("update.tar.gz");

    let mut response = http
        .get(format!(
            "{base}/api/desktop/download/{}?token={}",
            crate::encode::component(key),
            crate::encode::component(link)
        ))
        .send()
        .map_err(|e| format!("Download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed ({}).", response.status()));
    }
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive)
            .map_err(|e| e.to_string())?;
        response.copy_to(&mut file).map_err(|e| e.to_string())?;
    }

    // Checked before anything is unpacked, let alone run.
    let actual = digest(&archive)?;
    if actual != expected {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("The download did not match the checksum the server published, so it was discarded.".into());
    }

    let status = std::process::Command::new(tar_binary()?)
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("Could not unpack the update: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
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
