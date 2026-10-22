//! MeCloud desktop client.
//!
//! The shell around the web apps: a native window that hosts the MeCloud web
//! UI, a tray icon that keeps it one click away, and the desktop integration a
//! web page cannot do for itself — being the system's mail handler, syncing a
//! folder, and staying resident when its window is closed.
//!
//! Authentication for the window lives in the webview. The server runs an
//! OAuth2 authorization-code flow with PKCE and keeps the tokens in an
//! encrypted, HttpOnly session cookie; loading the server in a webview uses
//! exactly that flow, so the window never handles a credential. The sync
//! engine, which runs with no window open, has its own device token from
//! `pairing`.
//!
//! Layout: `commands` (what the setup page may call), `windows`, `tray`,
//! `background` (the sync and update loops), `fm_bridge` (the file-manager
//! socket) and `cli` (headless flags). This file holds the shared state and
//! the one entry to a sync pass.

pub mod api;
pub mod config;
pub mod deeplink;
pub mod encode;
pub mod handlers;
pub mod pairing;
pub mod reconcile;
pub mod rules;
pub mod scan;
pub mod socket;
pub mod sync;
pub mod syncdb;
pub mod update;

mod background;
mod cli;
mod commands;
mod fm_bridge;
mod tray;
mod windows;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use config::Config;
use sync::{SharedStatus, State, Status};
use update::UpdateInfo;

/// Live state, so the tray, the setup window and the sync thread agree.
pub struct AppState {
    pub config: Mutex<Config>,
    pub status: SharedStatus,
    pub update: Mutex<UpdateInfo>,
    /// Set to cancel the pairing currently waiting, if any.
    pub pairing: Mutex<Option<Arc<AtomicBool>>>,
    /// Whether a tray icon was actually created.
    ///
    /// Not a given: a session with no StatusNotifier host, a bare window
    /// manager or a headless run all leave the client without one, and the
    /// client survives that rather than refusing to start. It matters because
    /// "close to the tray" is only sensible when the tray exists.
    tray: AtomicBool,
}

impl AppState {
    fn new(config: Config) -> Self {
        Self {
            config: Mutex::new(config),
            status: Arc::new(Mutex::new(Status::default())),
            update: Mutex::new(UpdateInfo { current: update::CURRENT.into(), ..Default::default() }),
            pairing: Mutex::new(None),
            tray: AtomicBool::new(false),
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.load(Ordering::Relaxed)
    }

    pub fn set_tray(&self, present: bool) {
        self.tray.store(present, Ordering::Relaxed);
    }

    pub fn config_now(&self) -> Config {
        self.config.lock().unwrap().clone()
    }
}

/// Change the configuration, on disk and in memory, as one step.
///
/// Every setting goes through here. The old pattern — clone, edit, save,
/// store — let two writers each start from the same snapshot, so the second
/// silently undid the first; the sync loop did exactly that to any setting
/// changed while a pass was running. Holding the lock across the save makes
/// each change apply to the latest configuration.
pub fn update_config<T>(state: &AppState, change: impl FnOnce(&mut Config) -> T) -> Result<T, String> {
    let mut guard = state.config.lock().unwrap();
    let mut next = guard.clone();
    let out = change(&mut next);
    config::save(&next)?;
    *guard = next;
    Ok(out)
}

/// How a request for a pass turned out.
#[derive(Debug, PartialEq, Eq)]
pub enum Pass {
    /// It ran. `used_exemption` says whether the one-shot bulk-delete
    /// permission was in force, so the caller can spend it.
    Ran { used_exemption: bool },
    /// Sync is not set up or is switched off.
    Skipped,
    /// Another pass, in this process or another, is still running.
    Busy,
}

static PASS_RUNNING: AtomicBool = AtomicBool::new(false);

/// Held for the length of a pass. Two passes at once both upload the same new
/// file — leaving a duplicate on the server — and both download into the same
/// scratch file, recording the interleaved result as the baseline.
struct PassGuard {
    _file: Option<std::fs::File>,
}

impl PassGuard {
    fn acquire() -> Option<Self> {
        if PASS_RUNNING.swap(true, Ordering::AcqRel) {
            return None;
        }
        // Across processes too: the tray client and a `--sync-once` from cron
        // share a folder and a database.
        let file = config::pass_lock_path().and_then(|path| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()
        });
        if let Some(f) = &file {
            if let Err(std::fs::TryLockError::WouldBlock) = f.try_lock() {
                PASS_RUNNING.store(false, Ordering::Release);
                return None;
            }
            // Any other error means the file system has no locks; the
            // in-process guard still holds.
        }
        Some(Self { _file: file })
    }
}

impl Drop for PassGuard {
    fn drop(&mut self) {
        // Closing the file releases the OS lock.
        PASS_RUNNING.store(false, Ordering::Release);
    }
}

/// Run `work` while no pass can start, or refuse if one is running. For
/// changes a pass must not see half-done: wiping the baseline, putting a file
/// back from the trash.
pub(crate) fn with_pass_lock<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    let _guard = PassGuard::acquire().ok_or("A sync is running; try again in a moment.")?;
    Ok(work())
}

/// One reconciliation pass. Returns quietly when sync is not set up — that is
/// a state, not a failure.
pub fn run_pass(config: &Config, status: &SharedStatus) -> Pass {
    let (Some(server), Some(folder)) = (config.server_url.clone(), config.sync_path()) else {
        return Pass::Skipped;
    };
    if !config.sync_enabled {
        return Pass::Skipped;
    }
    let Some(_guard) = PassGuard::acquire() else { return Pass::Busy };

    // Failing before the engine runs leaves the exemption unspent.
    let ran = Pass::Ran { used_exemption: false };
    let Some(token) = config::load_token() else {
        publish_error(status, State::NotConfigured, "This computer is not connected to your account.");
        return ran;
    };
    let Some(db_path) = config::sync_db_path() else { return ran };

    let api = match api::Api::new(&server, &token) {
        Ok(a) => a,
        Err(e) => {
            publish_error(status, State::Error, &e);
            return ran;
        }
    };
    let account_id = match api.account_id() {
        Ok(id) => id,
        Err(e) => {
            publish_error(status, State::Error, &e);
            return ran;
        }
    };
    let db = match syncdb::SyncDb::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            publish_error(status, State::Error, &e);
            return ran;
        }
    };

    let engine = sync::Engine {
        folder,
        api,
        account_id,
        db,
        allow_bulk_delete: config.allow_bulk_delete_once,
        delete_on_server: config.delete_on_server,
        trash_days: config.trash_days,
        rules: config.rules(),
    };
    engine.run_once(status);
    Pass::Ran { used_exemption: config.allow_bulk_delete_once }
}

/// A pass with the app's own state: the latest configuration in, and the
/// bulk-delete exemption spent afterwards — in memory as well as on disk. It
/// used to be cleared only on disk, from a stale copy, so the guard stayed off
/// until restart and the next settings save wrote the exemption back.
pub fn run_pass_now(state: &AppState) -> Result<(), String> {
    let config = state.config_now();
    match run_pass(&config, &state.status) {
        Pass::Busy => Err("A sync is already running.".into()),
        Pass::Ran { used_exemption: true } => {
            update_config(state, |c| c.allow_bulk_delete_once = false).map(|_| ())
        }
        _ => Ok(()),
    }
}

fn publish_error(status: &SharedStatus, state: State, detail: &str) {
    if let Ok(mut guard) = status.lock() {
        *guard = Status { state, detail: detail.to_string(), ..Default::default() };
    }
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
}

pub fn run() {
    let loaded = config::load();

    // Headless flags exit before any window exists.
    cli::handle(&loaded);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // A mail handler must be a single instance: the desktop launches the
        // binary afresh for every clicked link, and a second copy would fight
        // the first over the tray icon and the session.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            windows::handle_target(app, deeplink::first_target(argv));
        }))
        .manage(AppState::new(loaded))
        .invoke_handler(commands::handler())
        .setup(move |app| {
            let handle = app.handle().clone();

            // A tray is a convenience, not a dependency. A desktop that cannot
            // give us one — KDE with no StatusNotifier host yet, a bare window
            // manager, no writable XDG_RUNTIME_DIR — must not lose the whole
            // client: Tauri turns a setup-hook error into an abort.
            let has_tray = match tray::build(&handle) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("[mecloud] no tray icon available ({e}); continuing without one");
                    false
                }
            };
            handle.state::<AppState>().set_tray(has_tray);

            let background = std::env::args().any(|a| a == "--background");
            if background && has_tray {
                // Launched by the session at login: the tray and the sync loop
                // are the point, and a window would land on top of whatever the
                // user was doing while they were still logging in.
            } else if background {
                // No tray to minimise to. Starting invisibly here would leave
                // the client running with no way at all to reach it.
                eprintln!("[mecloud] starting with a window, since there is no tray to hide in");
                windows::handle_target(&handle, None);
            } else if std::env::args().any(|a| a == "--settings") {
                // Straight to Preferences, for a desktop with no tray.
                windows::open_setup(&handle);
            } else {
                windows::handle_target(&handle, deeplink::first_target(std::env::args()));
            }
            background::place_sidebar_entry_once(&handle.state::<AppState>());
            background::spawn_sync_loop(handle.clone());
            background::spawn_update_loop(handle.clone());
            fm_bridge::start(handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start MeCloud")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // `None` is "the last window closed" — the client lives in the
                // tray, so that must not end the process. `Some(_)` is a
                // deliberate `app.exit(n)`, only ever reached from Quit.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that touch the config directory share one, set once, and take
    /// turns with it: they all read and write the same config.json.
    fn isolated_config() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("mecloud-lib-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // Set once, before any test reads it, under `Once`.
            std::env::set_var("MECLOUD_CONFIG_DIR", &dir);
        });
        turn
    }

    #[test]
    fn a_change_applies_to_the_latest_config_not_a_stale_copy() {
        let _turn = isolated_config();
        let state = AppState::new(Config::default());
        let stale = state.config_now();
        update_config(&state, |c| c.sync_interval_secs = 120).unwrap();
        // A second writer starting from `stale` used to undo the first.
        update_config(&state, |c| c.trash_days = 9).unwrap();
        let now = state.config_now();
        assert_eq!(now.sync_interval_secs, 120);
        assert_eq!(now.trash_days, 9);
        assert_ne!(stale.sync_interval_secs, 120);
        assert_eq!(config::load().sync_interval_secs, 120, "and on disk");
    }

    #[test]
    fn the_bulk_delete_exemption_is_spent_in_memory_and_on_disk() {
        let _turn = isolated_config();
        let state = AppState::new(Config::default());
        update_config(&state, |c| c.allow_bulk_delete_once = true).unwrap();
        // What `run_pass_now` does once a pass has run with it.
        let spent = Pass::Ran { used_exemption: state.config_now().allow_bulk_delete_once };
        assert_eq!(spent, Pass::Ran { used_exemption: true });
        update_config(&state, |c| c.allow_bulk_delete_once = false).unwrap();
        assert!(!state.config_now().allow_bulk_delete_once);
        assert!(!config::load().allow_bulk_delete_once);
        // And a later, unrelated save does not bring it back.
        update_config(&state, |c| c.sync_hidden = false).unwrap();
        assert!(!config::load().allow_bulk_delete_once);
    }

    #[test]
    fn only_one_pass_runs_at_a_time() {
        let _turn = isolated_config();
        let first = PassGuard::acquire().expect("free at first");
        assert!(PassGuard::acquire().is_none(), "a second pass must be refused");
        drop(first);
        assert!(PassGuard::acquire().is_some(), "and allowed once the first is done");
    }

    #[test]
    fn a_pass_with_sync_off_is_skipped_not_busy() {
        let _turn = isolated_config();
        let status: SharedStatus = Arc::new(Mutex::new(Status::default()));
        assert_eq!(run_pass(&Config::default(), &status), Pass::Skipped);
    }
}
