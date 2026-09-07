//! MeCloud desktop client.
//!
//! The shell around the web apps: a native window that hosts the MeCloud web
//! UI, a tray icon that keeps it one click away, and the desktop integration a
//! web page cannot do for itself — being the system's mail handler, and
//! staying resident when its window is closed.
//!
//! Authentication deliberately lives in the webview. The server already runs an
//! OAuth2 authorization-code flow with PKCE and keeps the tokens in an
//! encrypted, HttpOnly session cookie; loading the server in a webview uses
//! exactly that flow, so the client never handles a credential and there is no
//! second, weaker login path to attack. When the sync engine lands it will need
//! its own token, obtained from the server rather than invented here.

pub mod config;
pub mod deeplink;
pub mod handlers;
pub mod api;
pub mod pairing;
pub mod reconcile;
pub mod scan;
pub mod socket;
pub mod sync;
pub mod syncdb;
pub mod update;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use config::Config;
use deeplink::Target;
use sync::{SharedStatus, State, Status};
use update::UpdateInfo;

/// Live state, so the tray, the setup window and the sync thread agree.
pub struct AppState {
    pub config: Mutex<Config>,
    pub status: SharedStatus,
    pub update: Mutex<UpdateInfo>,
}

/// How often a pass runs when nothing else prompts one.
///
/// A poll rather than a filesystem watcher: a watcher tells you about local
/// changes only, so a remote change still needs a poll, and a scan of a few
/// thousand files costs milliseconds. Watching is worth adding for
/// responsiveness, not for correctness.
const SYNC_INTERVAL: Duration = Duration::from_secs(30);

/// How often to ask the server whether it ships a newer client. Rarely: an
/// update is not urgent, and this runs on someone else's machine.
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

const MAIN: &str = "main";
const SETUP: &str = "setup";

// ── Commands, callable only from the local setup page ───────────────────────
//
// The main window points at the server, and remote origins are not granted IPC
// (see capabilities/default.json), so nothing reachable from the network can
// call these.

#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn is_mailto_registered() -> bool {
    handlers::is_registered()
}

/// Validate a server address by asking it to describe itself.
///
/// `/api/config` is unauthenticated and returns the instance's name, so it
/// answers both questions that matter at setup time — is this reachable, and
/// is it actually MeCloud — without needing the user to be signed in yet.
#[tauri::command]
async fn probe_server(url: String) -> Result<String, String> {
    let base = config::normalise_server_url(&url)?;
    let endpoint = format!("{base}/api/config");

    let response = tauri::async_runtime::spawn_blocking(move || {
        // No async HTTP client in the dependency tree yet — one blocking GET at
        // setup time does not justify pulling one in.
        std::process::Command::new("curl")
            .args(["-fsS", "--max-time", "10", &endpoint])
            .output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|_| "Could not run the network check on this system.".to_string())?;

    if !response.status.success() {
        return Err(format!("Could not reach {base}."));
    }
    let body = String::from_utf8_lossy(&response.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| format!("{base} answered, but not like a MeCloud server."))?;
    parsed
        .get("appName")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{base} answered, but not like a MeCloud server."))
}

#[tauri::command]
fn save_server(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    url: String,
    handle_mailto: bool,
) -> Result<(), String> {
    let base = config::normalise_server_url(&url)?;

    if handle_mailto {
        handlers::register_mailto()?;
    } else if handlers::is_registered() {
        handlers::unregister_mailto()?;
    }

    // Preserve whatever sync settings are already there: this command is the
    // server address, and re-running setup must not silently turn sync off.
    let next = Config { server_url: Some(base.clone()), handle_mailto, ..config_now(&state) };
    config::save(&next)?;
    *state.config.lock().unwrap() = next;

    open_main(&app, None);
    if let Some(win) = app.get_webview_window(SETUP) {
        let _ = win.hide();
    }
    Ok(())
}

#[tauri::command]
fn sync_status(state: tauri::State<'_, AppState>) -> Status {
    state.status.lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
fn is_paired() -> bool {
    config::load_token().is_some()
}

/// Pair this computer with the account, through the user's browser session.
///
/// Blocking on a background thread: it waits for a person to read a page and
/// press a button, which is not something to hold the UI thread for.
#[tauri::command]
async fn pair_device(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let server = state
        .config
        .lock()
        .unwrap()
        .server_url
        .clone()
        .ok_or("Connect to a server first.")?;

    let device_name = hostname()
        .map(|h| format!("MeCloud on {h}"))
        .unwrap_or_else(|| "MeCloud Desktop".into());

    let listener = pairing::Listener::bind()?;
    let url = listener.pairing_url(&server, &device_name);

    // The user's browser, not the app's webview: the approval has to happen in
    // a session they can see and trust, and their browser is where they are
    // already signed in.
    if let Err(e) = tauri_plugin_opener::open_url(&url, None::<&str>) {
        return Err(format!("Could not open your browser: {e}"));
    }

    let token = tauri::async_runtime::spawn_blocking(move || listener.wait_for_token())
        .await
        .map_err(|e| e.to_string())??;

    config::save_token(&token)?;
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = ui.get_webview_window(SETUP).map(|w| w.set_focus());
    });
    Ok(device_name)
}

#[tauri::command]
fn unpair_device(state: tauri::State<'_, AppState>) -> Result<(), String> {
    config::clear_token()?;
    if let Ok(mut status) = state.status.lock() {
        *status = Status {
            state: State::NotConfigured,
            detail: "This computer is no longer connected.".into(),
            ..Default::default()
        };
    }
    Ok(())
}

#[tauri::command]
fn set_sync(
    state: tauri::State<'_, AppState>, folder: String, enabled: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();

    if enabled {
        let trimmed = folder.trim();
        if trimmed.is_empty() {
            return Err("Choose a folder to sync.".into());
        }
        let path = std::path::PathBuf::from(trimmed);
        // Created rather than demanded: the natural thing to type is a folder
        // that does not exist yet.
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Could not use {}: {e}", path.display()))?;
        if config::load_token().is_none() {
            return Err("Connect this computer to your account first.".into());
        }
        // Pointing at a different folder invalidates the baseline entirely:
        // it describes files that were in the *old* folder. Kept, it says
        // every one of them existed here and is now gone, and the next pass
        // deletes them from the server. This is how an account gets emptied by
        // changing a setting.
        let chosen = path.display().to_string();
        if config.sync_folder.as_deref() != Some(chosen.as_str()) {
            if let Some(db) = config::sync_db_path() {
                let _ = std::fs::remove_file(&db);
                // SQLite's WAL companions describe the file just removed.
                let _ = std::fs::remove_file(db.with_extension("db-wal"));
                let _ = std::fs::remove_file(db.with_extension("db-shm"));
            }
        }
        config.sync_folder = Some(chosen);
    }
    config.sync_enabled = enabled;

    config::save(&config)?;
    *state.config.lock().unwrap() = config;

    if let Ok(mut status) = state.status.lock() {
        if !enabled {
            *status = Status { state: State::Paused, detail: "Sync is off".into(), ..Default::default() };
        }
    }
    Ok(())
}

/// Run one pass now, if everything needed is in place.
#[tauri::command]
async fn sync_now(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap().clone();
    let status = state.status.clone();
    tauri::async_runtime::spawn_blocking(move || run_pass(&config, &status))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_autostart() -> bool {
    handlers::is_autostart_enabled()
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    handlers::set_autostart(enabled)
}

#[tauri::command]
fn update_info(state: tauri::State<'_, AppState>) -> UpdateInfo {
    state.update.lock().map(|u| u.clone()).unwrap_or_default()
}

#[tauri::command]
async fn check_for_update(state: tauri::State<'_, AppState>) -> Result<UpdateInfo, String> {
    let base = state.config.lock().unwrap().server_url.clone().ok_or("Connect to a server first.")?;
    let token = config::load_token().ok_or("Connect this computer to your account first.")?;
    let found = tauri::async_runtime::spawn_blocking(move || update::check(&base, &token))
        .await
        .map_err(|e| e.to_string())??;
    if let Ok(mut slot) = state.update.lock() {
        *slot = found.clone();
    }
    Ok(found)
}

/// Download, verify and install. The caller is told to restart; nothing is
/// restarted underneath them mid-sync.
#[tauri::command]
async fn install_update(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let base = state.config.lock().unwrap().server_url.clone().ok_or("Connect to a server first.")?;
    let token = config::load_token().ok_or("Connect this computer to your account first.")?;
    let info = state.update.lock().map(|u| u.clone()).unwrap_or_default();
    if !info.newer {
        return Err("This is already the current version.".into());
    }
    let version = info.available.clone().unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || update::install(&base, &token, &info))
        .await
        .map_err(|e| e.to_string())??;
    Ok(version)
}

/// Let one refused pass through.
///
/// One pass, not a setting: the guard exists because a mass deletion is nearly
/// always a symptom, and a permanent exemption would mean it never fires again
/// for the one time it is right.
#[tauri::command]
fn confirm_bulk_delete(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();
    config.allow_bulk_delete_once = true;
    config::save(&config)?;
    *state.config.lock().unwrap() = config;
    Ok(())
}

#[tauri::command]
fn set_auto_update(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();
    config.auto_update = enabled;
    config::save(&config)?;
    *state.config.lock().unwrap() = config;
    Ok(())
}

/// Check, and install if the user asked for that.
///
/// Installing replaces the binary on disk; the running process is untouched
/// and keeps going until it is next started. Restarting underneath someone
/// mid-sync to save them one manual step is not a trade worth making.
fn update_pass(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (Some(base), auto) = ({
        let config = state.config.lock().unwrap();
        (config.server_url.clone(), config.auto_update)
    }) else { return };
    let Some(token) = config::load_token() else { return };

    let Ok(found) = update::check(&base, &token) else { return };
    if let Ok(mut slot) = state.update.lock() {
        *slot = found.clone();
    }
    if found.newer && auto {
        match update::install(&base, &token, &found) {
            Ok(_) => log_update(&format!(
                "Installed {} — it will be running after the next restart.",
                found.available.unwrap_or_default()
            )),
            Err(e) => log_update(&format!("Update failed: {e}")),
        }
    }
}

fn log_update(message: &str) {
    eprintln!("[mecloud] {message}");
}

fn config_now(state: &tauri::State<'_, AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
}

/// One reconciliation pass. Returns quietly when sync is not set up — that is
/// a state, not a failure.
pub fn run_pass(config: &Config, status: &SharedStatus) {
    let (Some(server), Some(folder)) = (config.server_url.clone(), config.sync_path()) else {
        return;
    };
    if !config.sync_enabled {
        return;
    }
    let Some(token) = config::load_token() else {
        publish_error(status, State::NotConfigured, "This computer is not connected to your account.");
        return;
    };
    let Some(db_path) = config::sync_db_path() else { return };

    let api = match api::Api::new(&server, &token) {
        Ok(a) => a,
        Err(e) => return publish_error(status, State::Error, &e),
    };
    let account_id = match api.account_id() {
        Ok(id) => id,
        Err(e) => return publish_error(status, State::Error, &e),
    };
    let db = match syncdb::SyncDb::open(&db_path) {
        Ok(db) => db,
        Err(e) => return publish_error(status, State::Error, &e),
    };

    let engine = sync::Engine {
        folder,
        api,
        account_id,
        db,
        allow_bulk_delete: config.allow_bulk_delete_once,
    };
    let outcome = engine.run_once(status);

    // Consumed, whatever happened: an exemption that survived its pass would
    // quietly disarm the guard for every pass after it.
    if config.allow_bulk_delete_once {
        let mut cleared = config.clone();
        cleared.allow_bulk_delete_once = false;
        let _ = config::save(&cleared);
    }
    let _ = outcome;
}

fn publish_error(status: &SharedStatus, state: State, detail: &str) {
    if let Ok(mut guard) = status.lock() {
        *guard = Status { state, detail: detail.to_string(), ..Default::default() };
    }
}

// ── Windows ─────────────────────────────────────────────────────────────────

/// Show the main window, pointed at the server (optionally at a deep link).
pub fn open_main(app: &AppHandle, target: Option<Target>) {
    let config = app.state::<AppState>().config.lock().unwrap().clone();
    let Some(server) = config.server_url.clone() else {
        open_setup(app);
        return;
    };

    let url = match target {
        Some(t) => t.to_url(&server),
        None => server.clone(),
    };
    let parsed = match url.parse() {
        Ok(u) => u,
        Err(_) => {
            open_setup(app);
            return;
        }
    };

    if let Some(window) = app.get_webview_window(MAIN) {
        // Only navigate for an explicit target: doing it unconditionally would
        // throw away whatever the user was in the middle of every time they
        // clicked the tray.
        if url != server {
            let _ = window.navigate(parsed);
        }
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }

    let built = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(parsed))
        .title("MeCloud")
        .inner_size(1280.0, 860.0)
        .min_inner_size(480.0, 480.0)
        .resizable(true)
        .build();

    if let Ok(window) = built {
        // Closing the window leaves the client running in the tray, the way a
        // desktop client is expected to behave. Quit is on the tray menu.
        let handle = window.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = handle.hide();
            }
        });
    }
}

pub fn open_setup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETUP) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, SETUP, WebviewUrl::App("index.html".into()))
        .title("MeCloud — Setup")
        .inner_size(560.0, 820.0)
        // Resizable: the sync panel grows with what it has to say, and a
        // fixed height that clips the buttons is worse than a resizable window.
        .resizable(true)
        .build();
}

/// Route a deep link to the right window, setting up first if it has to.
pub fn handle_target(app: &AppHandle, target: Option<Target>) {
    let configured = app.state::<AppState>().config.lock().unwrap().is_configured();
    if configured {
        open_main(app, target);
    } else {
        open_setup(app);
    }
}

// ── Tray ────────────────────────────────────────────────────────────────────

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open MeCloud", true, None::<&str>)?;
    let compose = MenuItem::with_id(app, "compose", "New message", true, None::<&str>)?;

    let mail = MenuItem::with_id(app, "app:/mail", "Mail", true, None::<&str>)?;
    let files = MenuItem::with_id(app, "app:/files", "Files", true, None::<&str>)?;
    let calendar = MenuItem::with_id(app, "app:/calendar", "Calendar", true, None::<&str>)?;
    let contacts = MenuItem::with_id(app, "app:/contacts", "Contacts", true, None::<&str>)?;
    let notes = MenuItem::with_id(app, "app:/notes", "Notes", true, None::<&str>)?;
    let apps = Submenu::with_id_and_items(
        app,
        "apps",
        "Go to",
        true,
        &[&mail, &files, &calendar, &contacts, &notes],
    )?;

    // The status line is not clickable; "Sync now" beneath it is.
    let sync = MenuItem::with_id(app, "syncstatus", "Sync: starting…", false, None::<&str>)?;
    let sync_now_item = MenuItem::with_id(app, "syncnow", "Sync now", true, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Preferences…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit MeCloud", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &open,
            &compose,
            &PredefinedMenuItem::separator(app)?,
            &apps,
            &PredefinedMenuItem::separator(app)?,
            &sync,
            &sync_now_item,
            &updates,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("MeCloud")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => open_main(app, None),
            "compose" => open_main(
                app,
                Some(Target::Compose {
                    to: String::new(),
                    cc: String::new(),
                    subject: String::new(),
                    body: String::new(),
                }),
            ),
            "settings" => open_setup(app),
            "updates" => {
                // The check is network work and cannot run on the main thread;
                // opening the window afterwards may only run there.
                let handle = app.clone();
                std::thread::spawn(move || {
                    update_pass(&handle);
                    let ui = handle.clone();
                    let _ = handle.run_on_main_thread(move || open_setup(&ui));
                });
            }
            "syncnow" => {
                let config = app.state::<AppState>().config.lock().unwrap().clone();
                let status = app.state::<AppState>().status.clone();
                std::thread::spawn(move || run_pass(&config, &status));
            }
            "quit" => app.exit(0),
            other => {
                if let Some(path) = other.strip_prefix("app:") {
                    // Only the fixed routes above can reach this, so the leak
                    // of a 'static str is bounded by the size of the menu.
                    let leaked: &'static str = Box::leak(path.to_string().into_boxed_str());
                    open_main(app, Some(Target::App(leaked)));
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_main(tray.app_handle(), None);
            }
        })
        .build(app)?;

    Ok(())
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// The background pass, on its own thread.
///
/// A plain thread with a sleep rather than a scheduler: there is one job, it
/// must not run twice at once, and a loop makes both obvious.
fn spawn_sync_loop(app: AppHandle) {
    std::thread::spawn(move || {
        // Let the window and tray appear before the first scan competes for I/O.
        std::thread::sleep(Duration::from_secs(3));
        loop {
            let state = app.state::<AppState>();
            let config = state.config.lock().unwrap().clone();
            let status = state.status.clone();
            drop(state);

            run_pass(&config, &status);
            update_tray_label(&app);
            std::thread::sleep(SYNC_INTERVAL);
        }
    });
}

/// Answer the file manager's questions about paths.
///
/// The status comes from the sync database rather than the last run's summary:
/// the file manager asks about whatever the user happens to be looking at, and
/// only the baseline knows about a file that was synced days ago.
fn start_file_manager_socket(app: AppHandle) {
    let handle = app.clone();
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

                // Opening the database per question rather than holding it: the
                // sync thread owns its own connection, and two writers to one
                // SQLite handle is not a race worth inventing for a badge.
                let tracked = match (&relative, config::sync_db_path()) {
                    (Some(rel), Some(db_path)) => syncdb::SyncDb::open(&db_path)
                        .ok()
                        .and_then(|db| db.baselines().ok())
                        .map(|b| b.contains_key(rel.as_str()))
                        .unwrap_or(false),
                    _ => false,
                };
                let has_problem = relative
                    .as_ref()
                    .map(|rel| status.problems.iter().any(|p| p.starts_with(rel.as_str())))
                    .unwrap_or(false);
                let syncing = status.state == State::Syncing;

                let verdict = socket::status_for(
                    &full,
                    root.as_deref(),
                    tracked,
                    has_problem,
                    syncing,
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
    let state = app.state::<AppState>();
    let Ok(config) = state.config.lock() else { return };
    let (Some(root), Some(server)) = (config.sync_path(), config.server_url.clone()) else { return };
    drop(config);

    let full = std::path::PathBuf::from(path);
    let Ok(relative) = full.strip_prefix(&root) else { return };
    let relative = relative.to_string_lossy().replace('\\', "/");

    // The Files app already knows how to find a file by path and, from its own
    // menu, how to share one — so this hands it the path rather than
    // reimplementing either here.
    let mut url = format!("{}/files?path={}", server.trim_end_matches('/'), urlencode(&relative));
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
                open_main(&handle, None);
            }
        }
    });
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The update check, on its own slow loop.
fn spawn_update_loop(app: AppHandle) {
    std::thread::spawn(move || {
        // Not at the same moment as the first sync: two network jobs racing on
        // a cold start makes the first sync look slower than it is.
        std::thread::sleep(Duration::from_secs(20));
        loop {
            update_pass(&app);
            update_tray_label(&app);
            std::thread::sleep(UPDATE_INTERVAL);
        }
    });
}

/// Reflect the current status in the tray, the way a desktop client does.
///
/// **Must not touch the tray directly.** This is called from the sync and
/// update loops, which are ordinary background threads, and on Linux the tray
/// and every window are GTK objects that may only be used from the thread that
/// initialised GTK. Calling into them from anywhere else does not fail loudly —
/// it corrupts GTK's state, and the symptom is a window that stops responding
/// and cannot even be closed. Everything below is computed here and applied
/// there.
fn update_tray_label(app: &AppHandle) {
    let status = app.state::<AppState>().status.lock().map(|s| s.clone()).unwrap_or_default();
    let label = match status.state {
        State::NotConfigured => "Sync: not set up".to_string(),
        State::Paused => "Sync: off".to_string(),
        State::Syncing => "Sync: working…".to_string(),
        State::Idle => format!("Sync: {}", status.detail),
        State::Problems => format!("Sync: {} problem(s)", status.problems.len()),
        State::Error => format!("Sync: {}", status.detail),
        // The one state the user has to act on, so it says so in as few words
        // as a tooltip allows.
        State::NeedsConfirmation => "Sync: stopped — needs your confirmation".to_string(),
    };
    // An available update belongs in the tooltip: it is the one thing the user
    // has to act on, and the tray is where they will see it.
    let update = app.state::<AppState>().update.lock().map(|u| u.clone()).unwrap_or_default();
    let suffix = match (&update.available, update.newer) {
        (Some(v), true) => format!("\nUpdate available: {v}"),
        _ => String::new(),
    };
    let tooltip = format!("MeCloud — {label}{suffix}");
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("tray") {
            let _ = tray.set_tooltip(Some(&tooltip));
        }
    });
}

pub fn run() {
    let loaded = config::load();

    // A headless single pass, for a cron job or for working out why a sync is
    // not doing what was expected. Exits with the result rather than starting
    // a window.
    // Ask a running client to act on a path. This is how Dolphin reaches it:
    // KDE service menus run a command rather than hosting a plugin, so there
    // has to be a command to run.
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--share" || a == "--open") {
            let command = if args[i] == "--share" { "SHARE" } else { "OPEN" };
            let Some(path) = args.get(i + 1) else {
                eprintln!("{} needs a file path", args[i]);
                std::process::exit(2);
            };
            // Absolute, because the client resolves it against the sync folder
            // and a file manager may launch us from anywhere.
            let full = std::fs::canonicalize(path)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.clone());
            match socket::send(&format!("{command}:{full}")) {
                Ok(reply) => {
                    println!("{reply}");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
    }

    // Update handling without a window, for a scripted rollout or for finding
    // out why a client is not taking one.
    if std::env::args().any(|a| a == "--check-update" || a == "--update") {
        let install = std::env::args().any(|a| a == "--update");
        let (Some(base), Some(token)) = (loaded.server_url.clone(), config::load_token()) else {
            eprintln!("Not connected to a server, or this computer is not paired.");
            std::process::exit(1);
        };
        match update::check(&base, &token) {
            Ok(info) => {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                if install && info.newer {
                    match update::install(&base, &token, &info) {
                        Ok(path) => {
                            println!("installed to {}", path.display());
                            std::process::exit(0);
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(1);
                        }
                    }
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }

    if std::env::args().any(|a| a == "--sync-once") {
        let status: SharedStatus = Arc::new(Mutex::new(Status::default()));
        run_pass(&loaded, &status);
        let final_status = status.lock().map(|s| s.clone()).unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&final_status).unwrap_or_default());
        std::process::exit(match final_status.state {
            State::Error => 1,
            _ => 0,
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // A mail handler must be a single instance: the desktop launches the
        // binary afresh for every clicked link, and a second copy would fight
        // the first over the tray icon and the session.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_target(app, deeplink::first_target(argv));
        }))
        .manage(AppState {
            config: Mutex::new(loaded.clone()),
            status: Arc::new(Mutex::new(Status::default())),
            update: Mutex::new(UpdateInfo { current: update::CURRENT.into(), ..Default::default() }),
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            probe_server,
            save_server,
            is_mailto_registered,
            sync_status,
            is_paired,
            pair_device,
            get_autostart,
            set_autostart,
            update_info,
            check_for_update,
            install_update,
            set_auto_update,
            confirm_bulk_delete,
            unpair_device,
            set_sync,
            sync_now,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            build_tray(&handle)?;
            // `--settings` goes straight to Preferences. The tray menu is the
            // usual route, but a tray needs a system tray to exist, and a
            // headless check or a broken desktop has neither.
            if std::env::args().any(|a| a == "--background") {
                // Launched by the session at login: the tray and the sync loop
                // are the point, and a window would land on top of whatever the
                // user was doing while they were still logging in.
            } else if std::env::args().any(|a| a == "--settings") {
                open_setup(&handle);
            } else {
                handle_target(&handle, deeplink::first_target(std::env::args()));
            }
            spawn_sync_loop(handle.clone());
            spawn_update_loop(handle.clone());
            start_file_manager_socket(handle);
            Ok(())
        })
        // Closing every window leaves the tray running rather than exiting.
        .on_window_event(|_window, _event| {})
        .build(tauri::generate_context!())
        .expect("failed to start MeCloud")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
