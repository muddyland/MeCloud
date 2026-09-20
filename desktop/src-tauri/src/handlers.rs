//! Registering with the desktop as an application, and as the mail handler.
//!
//! Linux only for now. This writes a .desktop entry into the user's own
//! directory rather than touching anything system-wide, so it needs no
//! elevation and uninstalling is deleting one file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DESKTOP_FILE: &str = "mecloud-desktop.desktop";

fn applications_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications"))
}

/// The exec path to record. An AppImage is moved around freely by whoever
/// downloaded it, so the entry has to name where the binary actually is now,
/// not where a packager guessed it would be.
fn exec_path() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .or_else(|| std::env::current_exe().ok().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "mecloud-desktop".into())
}

fn desktop_entry() -> String {
    // %u passes the URL through, which is the whole point of the registration.
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MeCloud\n\
         GenericName=Mail and files\n\
         Comment=Mail, calendar, contacts, notes and files\n\
         Exec={exec} %u\n\
         Icon=mecloud-desktop\n\
         Terminal=false\n\
         Categories=Network;Email;Office;\n\
         MimeType=x-scheme-handler/mailto;\n\
         StartupNotify=true\n\
         StartupWMClass=MeCloud\n",
        exec = exec_path()
    )
}

/// Whether a .desktop entry written by this client is currently installed.
pub fn is_registered() -> bool {
    applications_dir()
        .map(|d| d.join(DESKTOP_FILE).exists())
        .unwrap_or(false)
}

/// Install the .desktop entry and claim mailto:.
///
/// `xdg-mime` and `update-desktop-database` are best-effort: a desktop without
/// them still gets a usable entry on disk, and reporting a hard failure for a
/// missing helper would be wrong.
pub fn register_mailto() -> Result<(), String> {
    let dir = applications_dir().ok_or("No user applications directory on this system")?;
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;

    let path = dir.join(DESKTOP_FILE);
    fs::write(&path, desktop_entry())
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;

    let _ = Command::new("update-desktop-database").arg(&dir).status();
    let _ = Command::new("xdg-mime")
        .args(["default", DESKTOP_FILE, "x-scheme-handler/mailto"])
        .status();

    Ok(())
}

/// Remove the entry and let the desktop fall back to whatever it had before.
pub fn unregister_mailto() -> Result<(), String> {
    let Some(dir) = applications_dir() else { return Ok(()) };
    let path = dir.join(DESKTOP_FILE);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Could not remove {}: {e}", path.display()))?;
    }
    let _ = Command::new("update-desktop-database").arg(&dir).status();
    Ok(())
}

// ── Starting with the session ───────────────────────────────────────────────
//
// The XDG autostart spec: a .desktop entry in ~/.config/autostart is launched
// when the session begins. Same mechanism as the applications entry, different
// directory, and still nothing system-wide.

const AUTOSTART_FILE: &str = "mecloud-desktop-autostart.desktop";

fn autostart_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart"))
}

pub fn is_autostart_enabled() -> bool {
    autostart_dir().map(|d| d.join(AUTOSTART_FILE).exists()).unwrap_or(false)
}

/// The entry to launch at login.
///
/// `--background` rather than a bare launch: starting with the session should
/// put the client in the tray and begin syncing, not throw a window in the
/// user's face before they have finished logging in.
fn autostart_entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MeCloud\n\
         Comment=Keep your files in sync\n\
         Exec={exec} --background\n\
         Icon=mecloud-desktop\n\
         Terminal=false\n\
         Categories=Network;\n\
         X-GNOME-Autostart-enabled=true\n\
         StartupNotify=false\n",
        exec = exec_path()
    )
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let dir = autostart_dir().ok_or("No autostart directory on this system")?;
    let path = dir.join(AUTOSTART_FILE);

    if !enabled {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Could not remove {}: {e}", path.display()))?;
        }
        return Ok(());
    }

    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    fs::write(&path, autostart_entry())
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

// ── The file manager sidebar ────────────────────────────────────────────────
//
// GTK file managers keep their sidebar bookmarks in one plain-text file:
// `$XDG_CONFIG_HOME/gtk-3.0/bookmarks`, one entry per line, a URI followed by
// an optional label. Nautilus, Nemo and Caja all read that same file — GTK4
// Nautilus included, which still reads the `gtk-3.0` path rather than a
// `gtk-4.0` one — so one write puts MeCloud in the sidebar of every file
// manager the Python extension already badges. They watch the file, so the
// entry appears without restarting anything.
//
// This is what Nextcloud's client does on Linux, and for the same reason:
// there is no other per-application way into that sidebar.

const SIDEBAR_LABEL: &str = "MeCloud";

fn bookmarks_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("gtk-3.0").join("bookmarks"))
}

/// The `file://` URI for a folder, encoded the way the bookmarks file needs.
///
/// The encoding is not cosmetic: the label is whatever follows the first
/// space, so a folder called `My Files` written literally would be read as a
/// bookmark to `file:///home/ada/My` labelled `Files`.
fn file_uri(folder: &Path) -> Option<String> {
    url::Url::from_file_path(folder).ok().map(|u| u.to_string())
}

/// Whether a bookmarks line points at `uri`, ignoring a trailing slash — the
/// file managers write directories both ways.
fn points_at(line: &str, uri: &str) -> bool {
    let theirs = line.split_whitespace().next().unwrap_or_default();
    theirs.trim_end_matches('/') == uri.trim_end_matches('/')
}

/// The file with our entry present, or unchanged if it already is.
///
/// An existing entry is left exactly as it is, label and position included: a
/// bookmark the user has renamed or dragged up the sidebar is theirs now.
fn with_entry(existing: &str, uri: &str) -> String {
    if existing.lines().any(|l| points_at(l, uri)) {
        return existing.to_string();
    }
    let mut out = String::from(existing);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(uri);
    out.push(' ');
    out.push_str(SIDEBAR_LABEL);
    out.push('\n');
    out
}

/// The file with our entry taken out.
///
/// Only an entry still labelled the way we wrote it. One the user renamed is
/// left alone — the folder is still on their disk after sync is switched off,
/// and deleting a bookmark they made their own is not ours to do.
fn without_entry(existing: &str, uri: &str) -> String {
    existing
        .lines()
        .filter(|l| {
            !(points_at(l, uri)
                && l.split_once(char::is_whitespace).map(|(_, label)| label.trim())
                    == Some(SIDEBAR_LABEL))
        })
        .fold(String::new(), |mut out, line| {
            out.push_str(line);
            out.push('\n');
            out
        })
}

/// Replace the bookmarks file in one step.
///
/// Written aside and renamed rather than truncated and rewritten: the file
/// managers hold a watch on it, and a half-written file is a sidebar that
/// loses the user's other bookmarks for as long as it takes to finish.
fn write_bookmarks(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("No config directory on this system")?;
    fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let temp = path.with_file_name(".bookmarks.mecloud-tmp");
    fs::write(&temp, contents).map_err(|e| format!("Could not write {}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("Could not update {}: {e}", path.display())
    })
}

/// Read the bookmarks file, rewrite it with `change`, and put it back — doing
/// nothing at all if the result is what is already on disk, so switching sync
/// on twice does not touch a file the file managers are watching.
fn edit_bookmarks(
    path: &Path, folder: &Path, change: fn(&str, &str) -> String,
) -> Result<(), String> {
    let uri = file_uri(folder).ok_or("Only an absolute path can be bookmarked")?;
    let existing = fs::read_to_string(path).unwrap_or_default();
    let updated = change(&existing, &uri);
    if updated == existing {
        return Ok(());
    }
    write_bookmarks(path, &updated)
}

/// Put the sync folder in the file manager sidebar.
pub fn add_to_sidebar(folder: &Path) -> Result<(), String> {
    let path = bookmarks_path().ok_or("No config directory on this system")?;
    edit_bookmarks(&path, folder, with_entry)
}

/// Take the sync folder back out of the sidebar.
pub fn remove_from_sidebar(folder: &Path) -> Result<(), String> {
    let path = bookmarks_path().ok_or("No config directory on this system")?;
    edit_bookmarks(&path, folder, without_entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_autostart_entry_starts_quietly_and_is_enabled() {
        let entry = autostart_entry();
        // Without --background the session begins by opening a window over
        // whatever the user was doing.
        assert!(entry.lines().any(|l| l.starts_with("Exec=") && l.ends_with(" --background")));
        // GNOME honours this key; without it the entry is written and ignored.
        assert!(entry.contains("X-GNOME-Autostart-enabled=true"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        // It must not claim mailto: as well, or disabling the mail handler
        // would leave a second entry still claiming it.
        assert!(!entry.contains("MimeType="));
    }

    #[test]
    fn a_folder_with_a_space_is_encoded_so_the_label_stays_the_label() {
        let uri = file_uri(Path::new("/home/ada/My Files")).unwrap();
        // A literal space here would be read as "file:///home/ada/My" labelled
        // "Files" — a sidebar entry pointing at a folder that does not exist.
        assert_eq!(uri, "file:///home/ada/My%20Files");
        let line = with_entry("", &uri);
        assert_eq!(line, "file:///home/ada/My%20Files MeCloud\n");
    }

    #[test]
    fn the_entry_is_appended_without_disturbing_the_bookmarks_already_there() {
        let existing = "file:///home/ada/Projects Work\nfile:///home/ada/Music\n";
        let updated = with_entry(existing, "file:///home/ada/MeCloud");
        assert!(updated.starts_with(existing));
        assert_eq!(updated.lines().count(), 3);
        assert_eq!(updated.lines().last().unwrap(), "file:///home/ada/MeCloud MeCloud");
    }

    #[test]
    fn a_file_with_no_trailing_newline_does_not_swallow_the_last_bookmark() {
        let updated = with_entry("file:///home/ada/Music", "file:///home/ada/MeCloud");
        assert_eq!(
            updated,
            "file:///home/ada/Music\nfile:///home/ada/MeCloud MeCloud\n"
        );
    }

    #[test]
    fn adding_twice_leaves_one_entry() {
        let once = with_entry("", "file:///home/ada/MeCloud");
        assert_eq!(with_entry(&once, "file:///home/ada/MeCloud"), once);
        // The file managers write directories both with and without the
        // trailing slash; matching on the text alone would duplicate the entry.
        assert_eq!(
            with_entry("file:///home/ada/MeCloud/ MeCloud\n", "file:///home/ada/MeCloud"),
            "file:///home/ada/MeCloud/ MeCloud\n"
        );
    }

    #[test]
    fn an_entry_the_user_renamed_is_left_alone() {
        let theirs = "file:///home/ada/MeCloud Ada's cloud\n";
        // Not re-added under our own label...
        assert_eq!(with_entry(theirs, "file:///home/ada/MeCloud"), theirs);
        // ...and not removed when sync goes off: the folder is still there,
        // and the bookmark is theirs now.
        assert_eq!(without_entry(theirs, "file:///home/ada/MeCloud"), theirs);
    }

    #[test]
    fn removing_takes_out_our_entry_and_nothing_else() {
        let existing = "file:///home/ada/Projects Work\n\
                        file:///home/ada/MeCloud MeCloud\n\
                        file:///home/ada/Music\n";
        assert_eq!(
            without_entry(existing, "file:///home/ada/MeCloud"),
            "file:///home/ada/Projects Work\nfile:///home/ada/Music\n"
        );
        // A different folder's entry is not touched by ours going away.
        assert_eq!(without_entry(existing, "file:///home/ada/Elsewhere"), existing);
    }

    #[test]
    fn the_entry_round_trips_through_a_real_bookmarks_file() {
        let dir = tempdir();
        let path = dir.join("gtk-3.0").join("bookmarks");
        let folder = Path::new("/home/ada/MeCloud");

        // The file need not exist yet: a fresh account has no bookmarks.
        edit_bookmarks(&path, folder, with_entry).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "file:///home/ada/MeCloud MeCloud\n");

        // Nothing is left behind from the rename.
        assert!(!path.with_file_name(".bookmarks.mecloud-tmp").exists());

        edit_bookmarks(&path, folder, without_entry).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");

        fs::remove_dir_all(&dir).unwrap();
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "mecloud-handlers-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn the_entry_registers_for_mailto_and_passes_the_url_through() {
        let entry = desktop_entry();
        assert!(entry.contains("MimeType=x-scheme-handler/mailto;"));
        // Without %u the desktop launches the app with no argument, and the
        // link the user clicked is simply lost.
        assert!(entry.lines().any(|l| l.starts_with("Exec=") && l.ends_with(" %u")));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Type=Application"));
    }
}
