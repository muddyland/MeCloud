//! Work that runs on its own schedule: sync passes and update checks.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{config, handlers, tray, update, update_config, AppState};

/// How often to ask the server whether it ships a newer client. Rarely: an
/// update is not urgent, and this runs on someone else's machine.
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The background pass, on its own thread.
///
/// A plain thread with a sleep rather than a scheduler: there is one job, and
/// a loop makes that obvious. A poll rather than a file watcher, because a
/// watcher only reports local changes and a remote change still needs a poll.
/// Overlap with a pass started from the tray or the settings page is prevented
/// by the pass lock, not by this loop.
pub fn spawn_sync_loop(app: AppHandle) {
    std::thread::spawn(move || {
        // Let the window and tray appear before the first scan competes for I/O.
        std::thread::sleep(Duration::from_secs(3));
        loop {
            let state = app.state::<AppState>();
            let _ = crate::run_pass_now(&state);
            // Re-read every pass rather than captured once, so changing the
            // interval in Preferences takes effect at the next wait.
            let wait = state.config_now().sync_interval();
            drop(state);
            tray::update_label(&app);
            std::thread::sleep(wait);
        }
    });
}

/// The update check, on its own slow loop.
pub fn spawn_update_loop(app: AppHandle) {
    std::thread::spawn(move || {
        // Not at the same moment as the first sync: two network jobs racing on
        // a cold start makes the first sync look slower than it is.
        std::thread::sleep(Duration::from_secs(20));
        loop {
            update_pass(&app);
            tray::update_label(&app);
            std::thread::sleep(UPDATE_INTERVAL);
        }
    });
}

/// Check, and install if the user asked for that.
///
/// Installing replaces the binary on disk; the running process is untouched
/// and keeps going until it is next started. Restarting underneath someone
/// mid-sync to save them one manual step is not a trade worth making.
pub fn update_pass(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config_now();
    let Some(base) = config.server_url else { return };
    let Some(token) = config::load_token() else { return };

    let Ok(found) = update::check(&base, &token) else { return };
    if let Ok(mut slot) = state.update.lock() {
        *slot = found.clone();
    }
    if found.newer && config.auto_update {
        match update::install(&base, &token, &found) {
            Ok(_) => eprintln!(
                "[mecloud] Installed {} — it will be running after the next restart.",
                found.available.unwrap_or_default()
            ),
            Err(e) => eprintln!("[mecloud] Update failed: {e}"),
        }
    }
}

/// Put an install that was already syncing into the file manager sidebar.
///
/// The entry is normally written when the sync folder is chosen, which does
/// nothing for someone who chose theirs before this client could write one. So
/// it is placed at startup too — but only ever once. A client that puts the
/// bookmark back at every launch is one the user cannot get rid of.
pub fn place_sidebar_entry_once(state: &AppState) {
    let config = state.config_now();
    if config.sidebar_entry_placed || !config.sync_enabled {
        return;
    }
    let Some(folder) = config.sync_path() else { return };
    let _ = handlers::add_to_sidebar(&folder);
    let _ = update_config(state, |c| c.sidebar_entry_placed = true);
}
