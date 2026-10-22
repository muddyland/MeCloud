//! Answering the file manager extension over the local socket.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::sync::State;
use crate::windows::{self, MAIN};
use crate::{config, encode, rules, socket, syncdb, update, AppState};

/// Look one path up in the baseline, opening the database at most once.
///
/// A file asks about itself; a folder asks whether it holds anything tracked.
/// Both are a single indexed lookup, because this runs once per visible row on
/// the file manager's main thread.
fn tracked_state(handle: &Mutex<Option<syncdb::SyncDb>>, relative: &str, is_dir: bool) -> bool {
    let Ok(mut slot) = handle.lock() else { return false };
    if slot.is_none() {
        let Some(path) = config::sync_db_path() else { return false };
        *slot = syncdb::SyncDb::open(&path).ok();
    }
    let Some(db) = slot.as_ref() else { return false };
    if is_dir {
        db.has_tracked_under(relative).unwrap_or(false)
    } else {
        db.is_tracked(relative).unwrap_or(false)
    }
}

/// Start answering. The status comes from the sync database rather than the
/// last pass's summary: the file manager asks about whatever the user happens
/// to be looking at, and only the baseline knows about a file synced days ago.
pub fn start(app: AppHandle) {
    let handle = app.clone();
    // Opened lazily and reused. Reads only, and SQLite is in WAL mode, so it
    // never blocks the sync thread's writes.
    let db_handle: Arc<Mutex<Option<syncdb::SyncDb>>> = Arc::new(Mutex::new(None));
    let result = socket::serve(move |request| {
        let state = handle.state::<AppState>();
        match request {
            socket::Request::Version => Some(format!("VERSION:{}", update::CURRENT)),

            socket::Request::Status(path) => {
                let config = state.config.lock().ok()?.clone();
                let status = state.status.lock().ok()?.clone();
                let root = config.sync_path();

                let full = std::path::PathBuf::from(&path);
                let relative = root
                    .as_ref()
                    .and_then(|r| full.strip_prefix(r).ok())
                    .map(|r| r.to_string_lossy().replace('\\', "/"));

                // A folder is judged by what is inside it: it has no baseline
                // row of its own, so the file question always answered "no"
                // and every folder showed "syncing" for ever.
                let is_dir = full.is_dir();
                let tracked = relative.as_ref().is_some_and(|rel| tracked_state(&db_handle, rel, is_dir));
                let has_problem =
                    relative.as_ref().is_some_and(|rel| socket::problem_touches(&status.problems, rel, is_dir));
                // Selective sync only: an ignore pattern means "never sync
                // this", which is not the same claim as "this lives on the
                // drive and not here".
                let excluded = relative
                    .as_ref()
                    .is_some_and(|rel| rules::in_excluded_folder(rel, &config.excluded_folders));

                let verdict = socket::status_for(
                    &full,
                    root.as_deref(),
                    socket::Known { tracked, excluded, has_problem, syncing: status.state == State::Syncing },
                );
                Some(format!("STATUS:{}:{}", verdict.wire(), path))
            }

            // The client knows how to do these; the extension only has to ask.
            socket::Request::Share(path) => {
                open_relative(&handle, &path, true);
                Some(format!("OK:SHARE:{path}"))
            }
            socket::Request::Open(path) => {
                open_relative(&handle, &path, false);
                Some(format!("OK:OPEN:{path}"))
            }

            socket::Request::Unknown(command) => Some(format!("ERROR:UNKNOWN:{command}")),
        }
    });
    if let Err(e) = result {
        eprintln!("[mecloud] file manager integration unavailable: {e}");
    }
}

/// Show a synced path in the web UI, optionally starting a share.
///
/// Window work, so it is marshalled to the main thread like everything else
/// the background touches.
fn open_relative(app: &AppHandle, path: &str, share: bool) {
    let config = app.state::<AppState>().config_now();
    let (Some(root), Some(server)) = (config.sync_path(), config.server_url) else { return };

    let full = std::path::PathBuf::from(path);
    let Ok(relative) = full.strip_prefix(&root) else { return };
    let relative = relative.to_string_lossy().replace('\\', "/");

    // The Files app already knows how to find a file by path and, from its own
    // menu, how to share one — so this hands it the path rather than
    // reimplementing either here.
    let mut url = format!("{}/files?path={}", server.trim_end_matches('/'), encode::path(&relative));
    if share {
        url.push_str("&share=1");
    }

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(parsed) = url.parse() {
            if let Some(window) = handle.get_webview_window(MAIN) {
                let _ = window.navigate(parsed);
                let _ = window.show();
                let _ = window.set_focus();
            } else {
                windows::open_main(&handle, None);
            }
        }
    });
}
