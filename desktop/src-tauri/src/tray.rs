//! The tray icon and its menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::deeplink::Target;
use crate::sync::State;
use crate::{background, windows, AppState};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
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

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("the window icon".into()))?;

    TrayIconBuilder::with_id("tray")
        .icon(icon)
        .tooltip("MeCloud")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => windows::open_main(app, None),
            "compose" => windows::open_main(
                app,
                Some(Target::Compose {
                    to: String::new(),
                    cc: String::new(),
                    subject: String::new(),
                    body: String::new(),
                }),
            ),
            "settings" => windows::open_setup(app),
            "updates" => {
                // The check is network work and cannot run on the main thread;
                // opening the window afterwards may only run there.
                let handle = app.clone();
                std::thread::spawn(move || {
                    background::update_pass(&handle);
                    let ui = handle.clone();
                    let _ = handle.run_on_main_thread(move || windows::open_setup(&ui));
                });
            }
            "syncnow" => {
                let handle = app.clone();
                std::thread::spawn(move || {
                    // Busy is fine here: the running pass is the one asked for.
                    let _ = crate::run_pass_now(&handle.state::<AppState>());
                    update_label(&handle);
                });
            }
            "quit" => app.exit(0),
            other => {
                if let Some(path) = other.strip_prefix("app:") {
                    // Only the fixed routes above can reach this, so the leak
                    // of a 'static str is bounded by the size of the menu.
                    let leaked: &'static str = Box::leak(path.to_string().into_boxed_str());
                    windows::open_main(app, Some(Target::App(leaked)));
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                windows::open_main(tray.app_handle(), None);
            }
        })
        .build(app)?;

    Ok(())
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
pub fn update_label(app: &AppHandle) {
    let state = app.state::<AppState>();
    let status = state.status.lock().map(|s| s.clone()).unwrap_or_default();
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
    let update = state.update.lock().map(|u| u.clone()).unwrap_or_default();
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
