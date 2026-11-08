//! Commands the local settings page may call.
//!
//! The main window points at the server, and remote origins are not granted IPC
//! (see capabilities/default.json), so nothing reachable from the network can
//! call these.
//!
//! Tauri runs a plain `fn` command on the main thread. Anything here that
//! touches the disk, the sync database, the network or another process is
//! therefore `async` and does its work through `blocking`, so the window never
//! freezes behind it — the Deletions pane alone used to open SQLite on the UI
//! thread every three seconds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::api::{self, Remote};
use crate::config::{self, Config};
use crate::rules::{self, TRASH_DIR};
use crate::sync::{State, Status};
use crate::update::UpdateInfo;
use crate::windows::{self, SETUP};
use crate::scanner::{self, discover, escl, Paper, Scanner, Settings};
use crate::scanning::{self, Folder, Job};
use crate::{handlers, hostname, pairing, scan, sync, syncdb, tags, update, update_config, with_pass_lock, AppState};

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_config,
        probe_server,
        save_server,
        is_mailto_registered,
        sync_status,
        is_paired,
        pair_device,
        cancel_pairing,
        get_autostart,
        set_autostart,
        update_info,
        check_for_update,
        install_update,
        set_auto_update,
        confirm_bulk_delete,
        pending_deletions,
        restore_deletion,
        apply_deletion,
        local_trash,
        restore_local,
        set_deletion_policy,
        set_sync_interval,
        get_rules,
        set_rules,
        sync_folders,
        set_folder_included,
        held_uploads,
        approve_upload,
        decline_upload,
        unpair_device,
        set_sync,
        sync_now,
        scan_overview,
        find_scanners,
        add_scanner,
        forget_scanner,
        scanner_options,
        scan_folders,
        create_scan_folder,
        known_tags,
        start_scan,
        add_scan_page,
        set_scan_tags,
        cancel_scan,
        scan_status,
        show_scan,
    ]
}

/// Run `work` on a blocking thread with the app state.
async fn blocking<T, F>(app: AppHandle, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>()))
        .await
        .map_err(|e| e.to_string())?
}

fn open_db() -> Result<syncdb::SyncDb, String> {
    syncdb::SyncDb::open(&config::sync_db_path().ok_or("No database")?)
}

// ── Account ─────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> Config {
    state.config_now()
}

#[tauri::command]
async fn is_mailto_registered() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(handlers::is_registered).await.map_err(|e| e.to_string())
}

/// Validate a server address by asking it to describe itself.
///
/// `/api/config` is unauthenticated and returns the instance's name, so it
/// answers both questions that matter at setup time — is this reachable, and
/// is it actually MeCloud — without needing the user to be signed in yet. It
/// goes through the same HTTP client as sync, so an address that passes here
/// is one sync can reach with the same certificates.
#[tauri::command]
async fn probe_server(url: String) -> Result<String, String> {
    let base = config::normalise_server_url(&url)?;
    tauri::async_runtime::spawn_blocking(move || {
        let not_mecloud = || format!("{base} answered, but not like a MeCloud server.");
        let http = api::http_client(Duration::from_secs(10))?;
        let response = http
            .get(format!("{base}/api/config"))
            .send()
            .map_err(|e| format!("Could not reach {base}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("Could not reach {base} ({}).", response.status()));
        }
        let parsed: serde_json::Value = response.json().map_err(|_| not_mecloud())?;
        parsed.get("appName").and_then(|v| v.as_str()).map(str::to_string).ok_or_else(not_mecloud)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn save_server(app: AppHandle, url: String, handle_mailto: bool) -> Result<(), String> {
    let base = config::normalise_server_url(&url)?;
    let ui = app.clone();
    blocking(app, move |state| {
        // Spawns update-desktop-database and xdg-mime, which is why this is
        // not on the main thread.
        if handle_mailto {
            handlers::register_mailto()?;
        } else if handlers::is_registered() {
            handlers::unregister_mailto()?;
        }
        // Only the address and the mail handler: re-running setup must not
        // silently turn sync off.
        update_config(state, |c| {
            c.server_url = Some(base);
            c.handle_mailto = handle_mailto;
        })
    })
    .await?;

    let handle = ui.clone();
    ui.run_on_main_thread(move || {
        windows::open_main(&handle, None);
        if let Some(win) = handle.get_webview_window(SETUP) {
            let _ = win.hide();
        }
    })
    .map_err(|e| e.to_string())
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
/// Waits on a background thread: it waits for a person to read a page and
/// press a button. Starting a second pairing cancels the first, and
/// `cancel_pairing` ends the wait early.
#[tauri::command]
async fn pair_device(app: AppHandle) -> Result<String, String> {
    let server = app.state::<AppState>().config_now().server_url.ok_or("Connect to a server first.")?;
    let device_name = hostname().map(|h| format!("MeCloud on {h}")).unwrap_or_else(|| "MeCloud Desktop".into());

    let listener = pairing::Listener::bind()?;
    let url = listener.pairing_url(&server, &device_name);

    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(previous) = app.state::<AppState>().pairing.lock().unwrap().replace(cancel.clone()) {
        previous.store(true, Ordering::Relaxed);
    }

    // The user's browser, not the app's webview: the approval has to happen in
    // a session they can see and trust, and their browser is where they are
    // already signed in.
    if let Err(e) = tauri_plugin_opener::open_url(&url, None::<&str>) {
        return Err(format!("Could not open your browser: {e}"));
    }

    let waiting = cancel.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || listener.wait_for_token(pairing::WAIT, &waiting))
        .await
        .map_err(|e| e.to_string())?;

    {
        let state = app.state::<AppState>();
        let mut slot = state.pairing.lock().unwrap();
        if slot.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel)) {
            *slot = None;
        }
    }
    let token = outcome?;
    config::save_token(&token)?;

    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = ui.get_webview_window(SETUP).map(|w| w.set_focus());
    });
    Ok(device_name)
}

#[tauri::command]
fn cancel_pairing(state: tauri::State<'_, AppState>) {
    if let Some(cancel) = state.pairing.lock().unwrap().take() {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn unpair_device(app: AppHandle) -> Result<(), String> {
    blocking(app, |state| {
        config::clear_token()?;
        if let Ok(mut status) = state.status.lock() {
            *status = Status {
                state: State::NotConfigured,
                detail: "This computer is no longer connected.".into(),
                ..Default::default()
            };
        }
        Ok(())
    })
    .await
}

// ── Sync ────────────────────────────────────────────────────────────────────

/// Count files under a path, stopping at `cap` — the question is "is there
/// anything here", not "how much", and a huge tree should not delay an answer.
fn count_files(root: &std::path::Path, cap: usize) -> usize {
    walkdir::WalkDir::new(root)
        .max_depth(6)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .take(cap)
        .count()
}

#[tauri::command]
async fn set_sync(app: AppHandle, folder: String, enabled: bool, acknowledged: bool) -> Result<(), String> {
    blocking(app, move |state| {
        let previous = state.config_now().sync_folder;
        let mut chosen = previous.clone();

        if enabled {
            let trimmed = folder.trim();
            if trimmed.is_empty() {
                return Err("Choose a folder to sync.".into());
            }
            let path = std::path::PathBuf::from(trimmed);
            // Created rather than demanded: the natural thing to type is a
            // folder that does not exist yet.
            std::fs::create_dir_all(&path).map_err(|e| format!("Could not use {}: {e}", path.display()))?;
            let picked = path.display().to_string();
            let changing = previous.as_deref() != Some(picked.as_str());

            // An existing folder with things already in it is the dangerous
            // case, and the one that cost a user their files: if it is an
            // *incomplete* copy of the drive, everything missing from it looks
            // deleted. Said plainly, once, before anything syncs.
            if changing && !acknowledged {
                let existing = count_files(&path, 200);
                if existing > 0 {
                    return Err(format!(
                        "NEEDS_ACKNOWLEDGEMENT:{} already contains {existing}{more} file(s).\n\n\
                         If this is an incomplete copy of your drive, the files it is missing will \
                         look deleted. Deletions wait in the trash on both sides rather than \
                         happening at once, so nothing is lost immediately — but check the folder \
                         is what you expect before continuing.",
                        path.display(),
                        more = if existing >= 200 { "+" } else { "" },
                    ));
                }
            }
            if config::load_token().is_none() {
                return Err("Connect this computer to your account first.".into());
            }
            // Pointing at a different folder invalidates the baseline entirely:
            // it describes files that were in the *old* folder. Kept, it says
            // every one of them existed here and is now gone, and the next pass
            // deletes them from the server. Wiped under the pass lock, so a
            // pass cannot be halfway through writing to it.
            if changing {
                with_pass_lock(|| {
                    if let Some(db) = config::sync_db_path() {
                        let _ = std::fs::remove_file(&db);
                        // SQLite's WAL companions describe the file just removed.
                        let _ = std::fs::remove_file(db.with_extension("db-wal"));
                        let _ = std::fs::remove_file(db.with_extension("db-shm"));
                    }
                })?;
            }
            chosen = Some(picked);
        }

        update_config(state, |c| {
            c.sync_folder = chosen.clone();
            c.sync_enabled = enabled;
        })?;

        // The file manager sidebar follows the folder: in when sync is switched
        // on, moved when the folder changes, out when it is switched off. All
        // best-effort — a bookmark is not worth failing a settings change over.
        if previous != chosen {
            if let Some(old) = previous.as_deref() {
                let _ = handlers::remove_from_sidebar(std::path::Path::new(old));
            }
        }
        if let Some(current) = chosen.as_deref() {
            let path = std::path::Path::new(current);
            if enabled {
                let _ = handlers::add_to_sidebar(path);
                if !state.config_now().sidebar_entry_placed {
                    let _ = update_config(state, |c| c.sidebar_entry_placed = true);
                }
            } else {
                let _ = handlers::remove_from_sidebar(path);
            }
        }

        if !enabled {
            if let Ok(mut status) = state.status.lock() {
                *status = Status { state: State::Paused, detail: "Sync is off".into(), ..Default::default() };
            }
        }
        Ok(())
    })
    .await
}

/// Run one pass now. Refused, with a message, if one is already running.
#[tauri::command]
async fn sync_now(app: AppHandle) -> Result<(), String> {
    blocking(app, crate::run_pass_now).await
}

/// How often a background pass runs. Returns what was stored, so the field
/// can show it when the value was out of range.
#[tauri::command]
fn set_sync_interval(state: tauri::State<'_, AppState>, seconds: u64) -> Result<u64, String> {
    update_config(&state, |c| {
        c.sync_interval_secs = seconds.clamp(config::MIN_SYNC_INTERVAL, config::MAX_SYNC_INTERVAL);
        c.sync_interval_secs
    })
}

/// Let one refused pass through.
///
/// One pass, not a setting: the guard exists because a mass deletion is nearly
/// always a symptom, and a permanent exemption would mean it never fires again
/// for the one time it is right.
#[tauri::command]
fn confirm_bulk_delete(state: tauri::State<'_, AppState>) -> Result<(), String> {
    update_config(&state, |c| c.allow_bulk_delete_once = true)
}

// ── Deletions ───────────────────────────────────────────────────────────────

#[tauri::command]
async fn pending_deletions() -> Result<Vec<syncdb::PendingDeletion>, String> {
    tauri::async_runtime::spawn_blocking(|| open_db().and_then(|db| db.pending()))
        .await
        .map_err(|e| e.to_string())?
}

/// Put a file back: the deletion is cancelled and the next pass fetches it.
///
/// The baseline is dropped rather than kept, so the planner sees "on the
/// server, absent here, no history" and downloads it — keeping the baseline
/// would instead read as a fresh local deletion and trash it again.
#[tauri::command]
async fn restore_deletion(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = open_db()?;
        db.clear_pending(&path)?;
        db.forget(&path)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Apply one waiting deletion to the server now, without waiting out the window.
#[tauri::command]
async fn apply_deletion(app: AppHandle, path: String) -> Result<(), String> {
    blocking(app, move |state| {
        let config = state.config_now();
        let db = open_db()?;
        let Some(due) = db.pending()?.into_iter().find(|p| p.path == path) else {
            return Err("That file is no longer waiting to be deleted.".into());
        };
        let (Some(server), Some(token)) = (config.server_url, config::load_token()) else {
            return Err("Not connected.".into());
        };
        let api = api::Api::new(&server, &token)?;
        let account_id = api.account_id()?;
        if let Some(id) = &due.node_id {
            api.destroy(&account_id, id)?;
        }
        db.clear_pending(&due.path)?;
        db.forget(&due.path)
    })
    .await
}

/// Files the server deleted, held on this computer.
#[tauri::command]
async fn local_trash() -> Result<Vec<syncdb::LocalTrashed>, String> {
    tauri::async_runtime::spawn_blocking(|| open_db().and_then(|db| db.local_trash()))
        .await
        .map_err(|e| e.to_string())?
}

/// Move a held file back into the sync folder. The next pass uploads it, since
/// the server no longer has it — that is what "put back" means here.
#[tauri::command]
async fn restore_local(app: AppHandle, stored: String) -> Result<(), String> {
    blocking(app, move |state| {
        let root = state.config_now().sync_path().ok_or("Sync is not set up.")?;
        let db = open_db()?;
        let item = db
            .local_trash()?
            .into_iter()
            .find(|t| t.stored == stored)
            .ok_or("That file is no longer in the trash.")?;
        with_pass_lock(|| -> Result<(), String> {
            let from = scan::resolve(&root.join(TRASH_DIR), &item.stored).ok_or("unsafe path")?;
            let to = sync::unused_path(&root, &item.path)?;
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&from, &to).map_err(|e| format!("Could not put it back: {e}"))?;
            db.clear_local_trash(&item.stored)
        })?
    })
    .await
}

#[tauri::command]
fn set_deletion_policy(state: tauri::State<'_, AppState>, enabled: bool, days: i64) -> Result<(), String> {
    update_config(&state, |c| {
        c.delete_on_server = enabled;
        // Zero would mean "delete on the next pass", which is the behaviour the
        // trash exists to prevent; a floor of one day keeps a way back.
        c.trash_days = days.clamp(1, 365);
    })
}

// ── Rules and selective sync ────────────────────────────────────────────────

/// The exclusion rules, as the settings screen wants them: patterns as one
/// block of text, because that is how people edit a list of lines.
#[tauri::command]
fn get_rules(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let config = state.config_now();
    serde_json::json!({
        "patterns": config.ignore_patterns.join("\n"),
        "excluded_folders": config.excluded_folders,
        "confirm_over_mb": config.confirm_over_mb,
        "sync_hidden": config.sync_hidden,
    })
}

#[tauri::command]
fn set_rules(
    state: tauri::State<'_, AppState>,
    patterns: String,
    confirm_over_mb: u64,
    sync_hidden: bool,
) -> Result<Vec<String>, String> {
    let lines: Vec<String> = patterns.lines().map(|l| l.trim().to_string()).collect();
    // Report the unusable lines rather than refusing the save: a typo in line
    // nine should not throw away the eight good rules above it, and a rule
    // that silently does nothing is worse than one the screen complains about.
    let rejected = rules::Rules::unparsed(&lines);
    update_config(&state, |c| {
        c.ignore_patterns = lines.into_iter().filter(|l| !l.is_empty()).collect();
        c.confirm_over_mb = confirm_over_mb;
        c.sync_hidden = sync_hidden;
    })?;
    Ok(rejected)
}

/// The top-level folders on the drive, and whether each one is being synced.
///
/// Read from the baseline rather than the server when possible: the picker
/// should open instantly and work offline, and the baseline already knows the
/// shape of the drive. Falling back to the server covers the case that
/// matters most — choosing what to sync *before* the first pass downloads it.
#[tauri::command]
async fn sync_folders(app: AppHandle) -> Result<Vec<rules::FolderChoice>, String> {
    blocking(app, |state| {
        let config = state.config_now();
        let excluded: std::collections::BTreeSet<String> = config.excluded_folders.iter().cloned().collect();

        let known: Vec<(String, u64)> = open_db().ok().and_then(|db| db.sizes().ok()).unwrap_or_default();
        if !known.is_empty() {
            let borrowed: Vec<(&String, u64)> = known.iter().map(|(p, s)| (p, *s)).collect();
            return Ok(rules::folder_choices(borrowed, &excluded));
        }

        let (Some(server), Some(token)) = (config.server_url, config::load_token()) else {
            return Ok(rules::folder_choices(Vec::new(), &excluded));
        };
        let api = api::Api::new(&server, &token)?;
        let account_id = api.account_id()?;
        let listing = api::to_tree(&api.nodes(&account_id)?)?;
        let borrowed: Vec<(&String, u64)> = listing.files.iter().map(|(p, e)| (p, e.size)).collect();
        Ok(rules::folder_choices(borrowed, &excluded))
    })
    .await
}

/// Switch a top-level folder on or off.
///
/// Switching one *off* leaves both copies exactly where they are — the local
/// files stay on disk and the server's stay on the server. It only stops the
/// client having an opinion about them.
#[tauri::command]
fn set_folder_included(state: tauri::State<'_, AppState>, name: String, included: bool) -> Result<(), String> {
    let name = name.trim_matches('/').to_string();
    if name.is_empty() {
        return Err("No folder given.".into());
    }
    update_config(&state, |c| {
        c.excluded_folders.retain(|f| f != &name);
        if !included {
            c.excluded_folders.push(name);
        }
    })
}

#[tauri::command]
async fn held_uploads() -> Result<Vec<syncdb::PendingUpload>, String> {
    tauri::async_runtime::spawn_blocking(|| open_db().and_then(|db| db.held_uploads()))
        .await
        .map_err(|e| e.to_string())?
}

/// Agree to one oversized upload. The next pass sends it.
#[tauri::command]
async fn approve_upload(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_db()?.approve_upload(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Decline one oversized upload, permanently.
///
/// Turned into an exclusion rule rather than a "declined" row, so that the
/// answer is visible and editable on the rules screen instead of living in a
/// database the user cannot see. The path is anchored so it means this file,
/// not every file that happens to share its name.
#[tauri::command]
async fn decline_upload(app: AppHandle, path: String) -> Result<(), String> {
    blocking(app, move |state| {
        open_db()?.clear_upload(&path)?;
        let rule = format!("/{}", path.trim_start_matches('/'));
        update_config(state, |c| {
            if !c.ignore_patterns.contains(&rule) {
                c.ignore_patterns.push(rule);
            }
        })
    })
    .await
}

// ── About ───────────────────────────────────────────────────────────────────

#[tauri::command]
async fn get_autostart() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(handlers::is_autostart_enabled).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_autostart(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || handlers::set_autostart(enabled))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn update_info(state: tauri::State<'_, AppState>) -> UpdateInfo {
    state.update.lock().map(|u| u.clone()).unwrap_or_default()
}

#[tauri::command]
async fn check_for_update(app: AppHandle) -> Result<UpdateInfo, String> {
    blocking(app, |state| {
        let base = state.config_now().server_url.ok_or("Connect to a server first.")?;
        let token = config::load_token().ok_or("Connect this computer to your account first.")?;
        let found = update::check(&base, &token)?;
        if let Ok(mut slot) = state.update.lock() {
            *slot = found.clone();
        }
        Ok(found)
    })
    .await
}

/// Download, verify and install. The caller is told to restart; nothing is
/// restarted underneath them mid-sync.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<String, String> {
    blocking(app, |state| {
        let base = state.config_now().server_url.ok_or("Connect to a server first.")?;
        let token = config::load_token().ok_or("Connect this computer to your account first.")?;
        let info = state.update.lock().map(|u| u.clone()).unwrap_or_default();
        if !info.newer {
            return Err("This is already the current version.".into());
        }
        update::install(&base, &token, &info)?;
        Ok(info.available.unwrap_or_default())
    })
    .await
}

#[tauri::command]
fn set_auto_update(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    update_config(&state, |c| c.auto_update = enabled)
}

// ── Scan ────────────────────────────────────────────────────────────────────

/// A connection as this device, for the commands that write to the drive.
fn device_api(state: &AppState) -> Result<(api::Api, String), String> {
    let server = state.config_now().server_url.ok_or("Connect to a server first.")?;
    let token = config::load_token().ok_or("Connect this computer to your account first.")?;
    let api = api::Api::new(&server, &token)?;
    let account = api.account_id()?;
    Ok((api, account))
}

#[derive(serde::Serialize)]
struct ScanOverview {
    paired: bool,
    prefs: config::ScanPrefs,
    status: scanning::Status,
    /// What a new scan is called if the name is left empty, so the field can
    /// show it as a placeholder.
    default_name: String,
    default_paper: Paper,
}

#[tauri::command]
fn scan_overview(state: tauri::State<'_, AppState>) -> ScanOverview {
    ScanOverview {
        paired: config::load_token().is_some(),
        prefs: state.config_now().scan,
        status: state.scan.status_now(),
        default_name: scanning::default_stem(chrono::Local::now()),
        default_paper: Paper::for_locale(std::env::var("LC_PAPER").or_else(|_| std::env::var("LANG")).ok().as_deref()),
    }
}

#[derive(serde::Serialize)]
struct Found {
    scanners: Vec<Scanner>,
    /// Why a source found nothing, when it could not even look.
    notes: Vec<String>,
}

/// Look for scanners for a few seconds: mDNS and SANE together.
#[tauri::command]
async fn find_scanners(app: AppHandle) -> Result<Found, String> {
    blocking(app, |state| {
        let prefs = state.config_now().scan;
        let (mut scanners, notes) = discover::discover(Duration::from_secs(3), &prefs.manual);
        // The last one used stays on the list even when it is switched off
        // or asleep, so the choice is not silently lost.
        if let Some(last) = prefs.scanner {
            if !scanners.iter().any(|s| s.id == last.id) {
                scanners.push(last);
            }
        }
        Ok(Found { scanners, notes })
    })
    .await
}

/// Add a network scanner by address, for networks where it does not
/// announce itself (another subnet, multicast filtered by a router).
#[tauri::command]
async fn add_scanner(app: AppHandle, address: String) -> Result<Scanner, String> {
    blocking(app, move |state| {
        let root = escl::root_from_input(&address)?;
        let caps = escl::Escl::new(&root)?
            .capabilities()
            .map_err(|e| format!("No eSCL scanner answered at {root}: {e}"))?;
        let name = caps.make_and_model.clone().unwrap_or_else(|| root.clone());
        let found = Scanner::escl(&root, caps.uuid.clone(), &name, true);
        let saved = found.clone();
        update_config(state, move |c| {
            c.scan.manual.retain(|s| s.id != saved.id);
            c.scan.manual.push(saved);
        })?;
        Ok(found)
    })
    .await
}

#[tauri::command]
fn forget_scanner(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    update_config(&state, |c| {
        c.scan.manual.retain(|s| s.id != id);
        if c.scan.scanner.as_ref().is_some_and(|s| s.id == id) {
            c.scan.scanner = None;
        }
    })
}

#[derive(serde::Serialize)]
struct ScannerChoices {
    options: scanner::Options,
    /// The remembered settings, fitted to what this scanner can do.
    suggested: Settings,
}

#[tauri::command]
async fn scanner_options(app: AppHandle, scanner: Scanner) -> Result<ScannerChoices, String> {
    blocking(app, move |state| {
        let options = scanner::options(&scanner)?;
        let paper = Paper::for_locale(std::env::var("LC_PAPER").or_else(|_| std::env::var("LANG")).ok().as_deref());
        let wanted = state.config_now().scan.settings.unwrap_or(Settings {
            source: scanner::Source::Flatbed,
            mode: scanner::Mode::Color,
            resolution: 300,
            paper,
        });
        let suggested = scanner::fit(&options, wanted)?;
        Ok(ScannerChoices { options, suggested })
    })
    .await
}

#[tauri::command]
async fn scan_folders(app: AppHandle) -> Result<Vec<Folder>, String> {
    blocking(app, |state| {
        let (api, account) = device_api(state)?;
        scanning::folders(&api.nodes(&account)?)
    })
    .await
}

/// A new folder in the drive, for scans to go into.
#[tauri::command]
async fn create_scan_folder(app: AppHandle, parent_id: Option<String>, name: String) -> Result<Folder, String> {
    blocking(app, move |state| {
        let name = name.trim().to_string();
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err("That is not a folder name.".into());
        }
        let (api, account) = device_api(state)?;
        let id = api.create_folder(&account, &name, parent_id.as_deref())?;
        // The path comes from a fresh listing rather than being pieced
        // together here, so it is whatever the web UI will show.
        let folders = scanning::folders(&api.nodes(&account)?)?;
        folders
            .into_iter()
            .find(|f| f.id.as_deref() == Some(id.as_str()))
            .ok_or_else(|| "The folder was created but is not listed yet.".into())
    })
    .await
}

#[derive(serde::Serialize)]
struct TagCount {
    tag: String,
    files: usize,
}

/// Tags already in use, for suggestions.
#[tauri::command]
async fn known_tags(app: AppHandle) -> Result<Vec<TagCount>, String> {
    blocking(app, |state| {
        let (api, account) = device_api(state)?;
        Ok(tags::load(&api, &account)?.counts().into_iter().map(|(tag, files)| TagCount { tag, files }).collect())
    })
    .await
}

/// Run scan work on its own thread, reporting into the session as it goes.
fn spawn_scan<F>(app: AppHandle, work: F) -> Result<(), String>
where
    F: FnOnce(&AppState, &api::Api, &str, &dyn Fn(&str, &str), &AtomicBool) -> Result<(), String> + Send + 'static,
{
    let state = app.state::<AppState>();
    let cancel = state.scan.begin()?;
    state.scan.set(|s| {
        s.state = "scanning".into();
        s.detail = "Connecting…".into();
        s.warning = None;
    });
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let report = |phase: &str, line: &str| {
            state.scan.set(|s| {
                s.state = phase.to_string();
                s.detail = line.to_string();
            })
        };
        let outcome = device_api(&state).and_then(|(api, account)| work(&state, &api, &account, &report, &cancel));
        if let Err(e) = outcome {
            let cancelled = cancel.load(Ordering::Relaxed);
            state.scan.set(|s| {
                s.state = if cancelled { "cancelled".into() } else { "error".into() };
                s.detail = if cancelled { "Cancelled. Nothing was saved.".into() } else { e };
            });
        }
        state.scan.end();
    });
    Ok(())
}

/// Scan a document into the drive. Returns at once; `scan_status` follows it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn start_scan(
    app: AppHandle, scanner: Scanner, settings: Settings, folder_id: Option<String>, folder_path: Option<String>,
    name: String, tags: Vec<String>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if config::load_token().is_none() {
        return Err("Connect this computer to your account first, on the Account screen.".into());
    }
    {
        let (scanner, folder_id, folder_path) = (scanner.clone(), folder_id.clone(), folder_path.clone());
        update_config(&state, move |c| {
            c.scan.scanner = Some(scanner);
            c.scan.settings = Some(settings);
            c.scan.folder_id = folder_id;
            c.scan.folder_path = folder_path;
        })?;
    }
    let job = Job { scanner, settings, folder_id, folder_path, name, tags };
    spawn_scan(app, move |state, api, account, report, cancel| {
        let saved = scanning::run(api, account, job, report, cancel)?;
        let open = saved.open;
        let pages = if open.pages.is_empty() { None } else { Some(open.pages.len()) };
        state.scan.set(|s| {
            s.state = "done".into();
            s.detail = match pages {
                Some(n) => format!("Saved {} — {n} page{}.", open.name, if n == 1 { "" } else { "s" }),
                None => format!("Saved {}.", open.name),
            };
            s.pages = pages.unwrap_or(0);
            s.node_id = Some(open.node_id.clone());
            s.file_name = Some(open.name.clone());
            s.folder_path = open.folder_path.clone();
            s.tags = open.tags.clone();
            s.can_add_page = saved.extendable;
            s.warning = saved.warning.clone();
        });
        *state.scan.open.lock().unwrap() = Some(open);
        Ok(())
    })
}

/// Scan another page onto the end of the document just saved.
#[tauri::command]
fn add_scan_page(app: AppHandle) -> Result<(), String> {
    if app.state::<AppState>().scan.open.lock().unwrap().is_none() {
        return Err("Scan a document first.".into());
    }
    spawn_scan(app, |state, api, account, report, cancel| {
        let mut open = state.scan.open.lock().unwrap().clone().ok_or("Scan a document first.")?;
        scanning::add_page(api, account, &mut open, report, cancel)?;
        let n = open.pages.len();
        state.scan.set(|s| {
            s.state = "done".into();
            s.detail = format!("Saved {} — {n} pages.", open.name);
            s.pages = n;
        });
        *state.scan.open.lock().unwrap() = Some(open);
        Ok(())
    })
}

/// Change the tags of the document just saved — or save them again, after
/// a scan whose tags could not be written.
#[tauri::command]
async fn set_scan_tags(app: AppHandle, tags: Vec<String>) -> Result<Vec<String>, String> {
    blocking(app, move |state| {
        let node = state.scan.open.lock().unwrap().as_ref().map(|o| o.node_id.clone()).ok_or("Scan a document first.")?;
        let (api, account) = device_api(state)?;
        let stored = tags::set_tags(&api, &account, &node, &tags)?;
        if let Some(open) = state.scan.open.lock().unwrap().as_mut() {
            open.tags = stored.clone();
        }
        let saved = stored.clone();
        state.scan.set(move |s| {
            s.tags = saved;
            s.warning = None;
        });
        Ok(stored)
    })
    .await
}

#[tauri::command]
fn cancel_scan(state: tauri::State<'_, AppState>) {
    state.scan.cancel();
}

#[tauri::command]
fn scan_status(state: tauri::State<'_, AppState>) -> scanning::Status {
    state.scan.status_now()
}

/// Open the saved scan in the Files app.
#[tauri::command]
fn show_scan(app: AppHandle) -> Result<(), String> {
    let node = app.state::<AppState>().scan.status_now().node_id.ok_or("Nothing has been scanned yet.")?;
    let handle = app.clone();
    app.run_on_main_thread(move || windows::open_main(&handle, Some(crate::deeplink::Target::Node(node))))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::count_files;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mecloud-count-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_folder_raises_no_warning() {
        // Nothing there means nothing to lose; setup should not interrupt.
        assert_eq!(count_files(&tempdir("empty"), 200), 0);
    }

    #[test]
    fn an_existing_folder_with_files_is_detected() {
        // This is the case that cost a user their files: an incomplete copy of
        // the drive makes everything missing from it look deleted.
        let dir = tempdir("full");
        std::fs::create_dir_all(dir.join("nested/deeper")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("nested/b.txt"), b"x").unwrap();
        std::fs::write(dir.join("nested/deeper/c.txt"), b"x").unwrap();
        assert_eq!(count_files(&dir, 200), 3);
    }

    #[test]
    fn counting_stops_at_the_cap() {
        let dir = tempdir("many");
        for i in 0..50 {
            std::fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }
        assert_eq!(count_files(&dir, 10), 10);
    }

    #[test]
    fn a_folder_that_does_not_exist_counts_as_empty() {
        assert_eq!(count_files(std::path::Path::new("/definitely/not/here"), 200), 0);
    }
}
