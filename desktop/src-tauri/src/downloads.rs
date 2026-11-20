//! Downloads started in the main window.
//!
//! The window is a bare webview: no download bar, no "open" button, nowhere a
//! saved file shows up. Before the client handled downloads at all, a click on
//! Download in Files appeared to do nothing. So:
//!
//! - a file the sync already has on this computer is not downloaded again; its
//!   local copy opens in the app the system uses for it, as a double-click in
//!   the file manager would;
//! - anything else is saved to the Downloads folder under a name that does
//!   not overwrite an existing file, and shown in the file manager once it is
//!   complete.
//!
//! "Already has" means the sync's last record of the path names the very blob
//! being downloaded. If the server has a newer version, the record names an
//! older blob, nothing matches, and the download fetches the server's copy.

use std::path::{Path, PathBuf};

use url::Url;

use crate::syncdb::SyncDb;

/// The web UI's blob route, relative to the server's base URL.
const BLOB_ROUTE: &str = "api/files/blob/";

/// The blob id a URL downloads, if it is the server's own blob route.
pub fn blob_id(server: &Url, url: &Url) -> Option<String> {
    if url.origin() != server.origin() {
        return None;
    }
    let base = server.path().trim_end_matches('/');
    let rest = url.path().strip_prefix(base)?.strip_prefix('/')?.strip_prefix(BLOB_ROUTE)?;
    let id = percent_encoding::percent_decode_str(rest).decode_utf8().ok()?;
    // The same shape the server accepts (server/src/blobs.rs validate_blob_id).
    let ok = (1..=256).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-=.:~".contains(&b));
    ok.then(|| id.into_owned())
}

/// The synced copy of a blob under `root`, if one is on disk.
///
/// A recorded path only counts if it is a regular file that really lies
/// inside the sync folder: a symlink pointing out of it is not "the synced
/// copy", whatever the database says.
pub fn local_copy(root: &Path, paths: &[String]) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    paths.iter().find_map(|relative| {
        if Path::new(relative).is_absolute() || relative.split('/').any(|c| c == "..") {
            return None;
        }
        let full = root.join(relative).canonicalize().ok()?;
        (full.starts_with(&root) && full.is_file()).then_some(full)
    })
}

/// Extensions that run something rather than show it. A web page decides
/// which file is "downloaded", so the client never launches one of these on
/// its own; it shows the file in the file manager instead and leaves the
/// double-click to the user.
const LAUNCHABLE: &[&str] = &[
    "desktop", "sh", "bash", "zsh", "fish", "csh", "ksh", "command", "run", "bin", "appimage",
    "flatpak", "flatpakref", "snap", "deb", "rpm", "py", "pyw", "pl", "rb", "php", "jar",
    "exe", "msi", "msix", "appx", "bat", "cmd", "com", "scr", "pif", "cpl", "ps1", "psm1",
    "vbs", "vbe", "js", "jse", "mjs", "wsf", "wsh", "hta", "lnk", "url", "reg", "inf",
    "app", "pkg", "dmg", "workflow", "scpt", "applescript", "terminal", "tool",
];

/// Whether a file can be handed to the system's default app without risk of
/// running it.
pub fn safe_to_open(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if LAUNCHABLE.contains(&ext.as_str()) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An executable bit makes some desktops offer to run the file.
        if std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(true) {
            return false;
        }
    }
    true
}

/// Open a file the way a double-click would, or show it in its folder when
/// opening could run it.
pub fn open_or_reveal(path: &Path) {
    let result = if safe_to_open(path) {
        tauri_plugin_opener::open_path(path, None::<&str>)
    } else {
        tauri_plugin_opener::reveal_item_in_dir(path)
    };
    if let Err(e) = result {
        eprintln!("[mecloud] could not open {}: {e}", path.display());
    }
}

/// Show a finished download in the file manager.
pub fn reveal(path: &Path) {
    if let Err(e) = tauri_plugin_opener::reveal_item_in_dir(path) {
        eprintln!("[mecloud] could not show {}: {e}", path.display());
    }
}

/// The synced copy a download URL names, if this computer has it.
pub fn synced_copy(server: &Url, url: &Url, root: Option<&Path>, db_path: Option<&Path>) -> Option<PathBuf> {
    let blob = blob_id(server, url)?;
    let (root, db_path) = (root?, db_path?);
    if !db_path.exists() {
        return None;
    }
    let paths = SyncDb::open(db_path).ok()?.paths_for_blob(&blob).ok()?;
    local_copy(root, &paths)
}

/// Where a download goes when the system names no Downloads folder: wry falls
/// back to the working directory, which for a desktop app is usually `/` or
/// the home folder itself. `~/Downloads` instead, under a name that is free.
pub fn fallback_destination(home: &Path, proposed: &Path) -> Option<PathBuf> {
    let name = proposed.file_name()?.to_string_lossy().into_owned();
    let dir = home.join("Downloads");
    std::fs::create_dir_all(&dir).ok()?;
    Some(free_name(&dir, &name))
}

/// `name` in `dir`, or `name (1)`, `name (2)`… — whichever does not exist yet.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = match name.split_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    let mut candidate = dir.join(name);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mecloud-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_the_servers_own_blob_route_names_a_blob() {
        let server = url("https://cloud.example/");
        assert_eq!(
            blob_id(&server, &url("https://cloud.example/api/files/blob/Gab-1?name=a.pdf&type=x")).as_deref(),
            Some("Gab-1")
        );
        // Percent-encoded by the web UI.
        assert_eq!(blob_id(&server, &url("https://cloud.example/api/files/blob/n%3A12")).as_deref(), Some("n:12"));
        for other in [
            "https://evil.example/api/files/blob/Gab",
            "http://cloud.example/api/files/blob/Gab",
            "https://cloud.example:8443/api/files/blob/Gab",
            "https://cloud.example/api/desktop/releases/x",
            "https://cloud.example/api/files/blob/",
            "https://cloud.example/api/files/blob/a%2Fb",
            "https://cloud.example/api/files/blob/a/b",
        ] {
            assert_eq!(blob_id(&server, &url(other)), None, "{other}");
        }
    }

    #[test]
    fn a_server_under_a_path_keeps_its_prefix() {
        let server = url("https://example.org/cloud/");
        assert_eq!(blob_id(&server, &url("https://example.org/cloud/api/files/blob/B1")).as_deref(), Some("B1"));
        assert_eq!(blob_id(&server, &url("https://example.org/api/files/blob/B1")), None);
        let bare = url("https://example.org/cloud");
        assert_eq!(blob_id(&bare, &url("https://example.org/cloud/api/files/blob/B1")).as_deref(), Some("B1"));
    }

    #[test]
    fn the_local_copy_must_exist_inside_the_sync_folder() {
        let root = tempdir("root");
        std::fs::create_dir_all(root.join("Docs")).unwrap();
        std::fs::write(root.join("Docs/scan.pdf"), b"%PDF").unwrap();
        let paths = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let found = local_copy(&root, &paths(&["gone.pdf", "Docs/scan.pdf"])).unwrap();
        assert_eq!(found, root.canonicalize().unwrap().join("Docs/scan.pdf"));
        assert!(local_copy(&root, &paths(&["gone.pdf"])).is_none());
        assert!(local_copy(&root, &paths(&["Docs"])).is_none(), "a folder is not a file");
        assert!(local_copy(&root, &paths(&["../root/Docs/scan.pdf"])).is_none());

        #[cfg(unix)]
        {
            let outside = tempdir("outside");
            std::fs::write(outside.join("secret.pdf"), b"x").unwrap();
            std::os::unix::fs::symlink(outside.join("secret.pdf"), root.join("link.pdf")).unwrap();
            assert!(local_copy(&root, &paths(&["link.pdf"])).is_none());
        }
    }

    #[test]
    fn a_synced_copy_is_found_through_the_database() {
        let root = tempdir("sync");
        std::fs::write(root.join("a.pdf"), b"%PDF").unwrap();
        let db_path = tempdir("db").join("sync.db");
        SyncDb::open(&db_path).unwrap().record("a.pdf", "L", "B1", 4, Some("n1")).unwrap();
        let server = url("https://cloud.example/");
        let hit = synced_copy(&server, &url("https://cloud.example/api/files/blob/B1?name=a.pdf"), Some(&root), Some(&db_path));
        assert!(hit.is_some_and(|p| p.ends_with("a.pdf")));
        // A newer version on the server: download that instead.
        assert!(synced_copy(&server, &url("https://cloud.example/api/files/blob/B2"), Some(&root), Some(&db_path)).is_none());
        // No sync set up.
        assert!(synced_copy(&server, &url("https://cloud.example/api/files/blob/B1"), None, Some(&db_path)).is_none());
        assert!(synced_copy(&server, &url("https://cloud.example/api/files/blob/B1"), Some(&root), Some(&root.join("none.db"))).is_none());
    }

    #[test]
    fn launchers_and_executables_are_shown_not_opened() {
        let dir = tempdir("open");
        for (name, safe) in [
            ("scan.pdf", true), ("photo.JPG", true), ("notes.md", true), ("archive.tar.gz", true),
            ("evil.desktop", false), ("setup.EXE", false), ("run.sh", false), ("x.js", false), ("app.AppImage", false),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, b"x").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            assert_eq!(safe_to_open(&path), safe, "{name}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("looks-harmless.pdf");
            std::fs::write(&path, b"x").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(!safe_to_open(&path));
        }
        assert!(!safe_to_open(&dir.join("missing.pdf")), "unreadable metadata is not safe");
    }

    #[test]
    fn downloads_never_overwrite() {
        let dir = tempdir("names");
        assert_eq!(free_name(&dir, "a.pdf"), dir.join("a.pdf"));
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        std::fs::write(dir.join("a (1).pdf"), b"x").unwrap();
        assert_eq!(free_name(&dir, "a.pdf"), dir.join("a (2).pdf"));
        std::fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(free_name(&dir, "README"), dir.join("README (1)"));
        std::fs::write(dir.join(".env"), b"x").unwrap();
        assert_eq!(free_name(&dir, ".env"), dir.join(".env (1)"));

        let home = tempdir("home");
        let dest = fallback_destination(&home, Path::new("/whatever/report.pdf")).unwrap();
        assert_eq!(dest, home.join("Downloads/report.pdf"));
        assert!(home.join("Downloads").is_dir());
    }
}
