//! Registering with the desktop as an application, and as the mail handler.
//!
//! Linux only for now. This writes a .desktop entry into the user's own
//! directory rather than touching anything system-wide, so it needs no
//! elevation and uninstalling is deleting one file.

use std::fs;
use std::path::PathBuf;
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
