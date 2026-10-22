//! The two windows: the server's web UI, and the local settings page.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::deeplink::Target;
use crate::AppState;

pub const MAIN: &str = "main";
pub const SETUP: &str = "setup";

/// What to do with one navigation in the main window.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Hand it to the system browser and stay where we are.
    External,
}

/// How long after the web UI starts a sign-in it may visit other sites.
const SIGN_IN_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Where the main window may go.
///
/// It exists to show one server. A link in a mail, a note or a file preview
/// that navigated it elsewhere left the user browsing an arbitrary site inside
/// a window that looks like MeCloud, with no address bar to tell them so. So:
///
/// - the server's own origin is always allowed;
/// - starting a sign-in (`/auth/...` on the server) opens a short window in
///   which other origins are allowed too, because the OAuth flow goes to the
///   identity provider and back, and that provider is not known in advance;
///   every origin visited in that window is remembered for the session, so a
///   later silent re-login works without another round;
/// - any other web address opens in the system browser instead;
/// - `about:`, `data:` and `blob:` stay in the page (the mail body is an
///   `about:srcdoc` frame, previews are blobs); other schemes go to the system.
pub struct NavigationPolicy {
    server: Url,
    sign_in_until: Mutex<Option<Instant>>,
    trusted: Mutex<HashSet<String>>,
}

impl NavigationPolicy {
    pub fn new(server: Url) -> Self {
        Self { server, sign_in_until: Mutex::new(None), trusted: Mutex::new(HashSet::new()) }
    }

    pub fn decide(&self, url: &Url) -> Decision {
        let now = Instant::now();
        let mut until = self.sign_in_until.lock().unwrap();
        let in_sign_in = until.is_some_and(|t| now < t);
        let mut trusted = self.trusted.lock().unwrap();
        let decision = decide(&self.server, url, in_sign_in, &trusted);

        if decision == Decision::Allow && is_web(url) {
            if same_origin(&self.server, url) {
                if url.path().starts_with("/auth/") {
                    *until = Some(now + SIGN_IN_WINDOW);
                }
            } else if in_sign_in {
                trusted.insert(url.origin().ascii_serialization());
            }
        }
        decision
    }
}

fn is_web(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.origin() == b.origin()
}

/// The pure part of the policy.
pub fn decide(server: &Url, url: &Url, in_sign_in: bool, trusted: &HashSet<String>) -> Decision {
    match url.scheme() {
        "about" | "data" | "blob" => return Decision::Allow,
        "http" | "https" => {}
        _ => return Decision::External,
    }
    if same_origin(server, url) || trusted.contains(&url.origin().ascii_serialization()) || in_sign_in {
        Decision::Allow
    } else {
        Decision::External
    }
}

fn open_externally(url: &Url) {
    if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        eprintln!("[mecloud] could not open {url}: {e}");
    }
}

/// Show the main window, pointed at the server (optionally at a deep link).
pub fn open_main(app: &AppHandle, target: Option<Target>) {
    let config = app.state::<AppState>().config_now();
    let Some(server) = config.server_url.clone() else {
        open_setup(app);
        return;
    };

    let url = match target {
        Some(t) => t.to_url(&server),
        None => server.clone(),
    };
    let (Ok(parsed), Ok(server_url)) = (url.parse::<Url>(), server.parse::<Url>()) else {
        open_setup(app);
        return;
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

    let policy = NavigationPolicy::new(server_url);
    let built = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(parsed))
        .title("MeCloud")
        .inner_size(1280.0, 860.0)
        .min_inner_size(480.0, 480.0)
        .resizable(true)
        .on_navigation(move |url| match policy.decide(url) {
            Decision::Allow => true,
            Decision::External => {
                open_externally(url);
                false
            }
        })
        // `target=_blank` links — every link in a mail — ask for a new window.
        // They belong in the user's browser, not in a bare webview with no
        // address bar.
        .on_new_window(|url, _features| {
            if is_web(&url) || url.scheme() == "mailto" {
                open_externally(&url);
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .build();

    if let Ok(window) = built {
        // Closing the window leaves the client running in the tray, the way a
        // desktop client is expected to behave — but only if there is a tray
        // to leave it in. Without one, hiding the window strands the process
        // with no way to reach it and no way to stop it, so there the X button
        // means what it usually means.
        let handle = window.clone();
        let has_tray = app.state::<AppState>().has_tray();
        let quit = app.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if has_tray {
                    api.prevent_close();
                    let _ = handle.hide();
                } else {
                    quit.exit(0);
                }
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
        .title("MeCloud — Settings")
        // Wide enough for the navigation rail beside a pane, short enough to
        // fit a laptop screen. The panes scroll inside the frame.
        .inner_size(880.0, 660.0)
        .min_inner_size(680.0, 480.0)
        .resizable(true)
        .build();
}

/// Route a deep link to the right window, setting up first if it has to.
pub fn handle_target(app: &AppHandle, target: Option<Target>) {
    if app.state::<AppState>().config_now().is_configured() {
        open_main(app, target);
    } else {
        open_setup(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn the_server_is_always_allowed_and_strangers_go_to_the_browser() {
        let server = u("https://cloud.example.com");
        let none = HashSet::new();
        assert_eq!(decide(&server, &u("https://cloud.example.com/mail"), false, &none), Decision::Allow);
        assert_eq!(decide(&server, &u("https://evil.example/phish"), false, &none), Decision::External);
        // Same host, other port or scheme: another origin.
        assert_eq!(decide(&server, &u("http://cloud.example.com/"), false, &none), Decision::External);
    }

    #[test]
    fn in_page_documents_stay_in_the_page() {
        let server = u("https://cloud.example.com");
        for url in ["about:srcdoc", "about:blank", "data:text/plain,hi", "blob:https://cloud.example.com/1"] {
            assert_eq!(decide(&server, &u(url), false, &HashSet::new()), Decision::Allow, "{url}");
        }
        assert_eq!(decide(&server, &u("mailto:a@b.c"), false, &HashSet::new()), Decision::External);
    }

    #[test]
    fn signing_in_may_visit_the_identity_provider_and_it_is_remembered() {
        let policy = NavigationPolicy::new(u("https://cloud.example.com"));
        let idp = u("https://auth.example.org/ui/oauth2?x=1");
        assert_eq!(policy.decide(&idp), Decision::External, "not before a sign-in starts");

        assert_eq!(policy.decide(&u("https://cloud.example.com/auth/login")), Decision::Allow);
        assert_eq!(policy.decide(&idp), Decision::Allow);
        assert_eq!(policy.decide(&u("https://cloud.example.com/auth/callback?code=c")), Decision::Allow);

        // After the window closes, the provider is still trusted; others are not.
        *policy.sign_in_until.lock().unwrap() = None;
        assert_eq!(policy.decide(&u("https://auth.example.org/ui/login")), Decision::Allow);
        assert_eq!(policy.decide(&u("https://elsewhere.example/")), Decision::External);
    }
}
