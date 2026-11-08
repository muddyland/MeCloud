//! Flags that do one job and exit, without a window.

use std::sync::{Arc, Mutex};

use crate::config::{self, Config};
use crate::scanner::{self, discover, Mode, Paper, Scanner, Settings, Source};
use crate::sync::{SharedStatus, State, Status};
use crate::{run_pass, socket, update, Pass};

/// Act on a headless flag and exit, or return to start the app normally.
pub fn handle(loaded: &Config) {
    let args: Vec<String> = std::env::args().collect();

    // Ask a running client to act on a path. This is how Dolphin reaches it:
    // KDE service menus run a command rather than hosting a plugin, so there
    // has to be a command to run.
    if let Some(i) = args.iter().position(|a| a == "--share" || a == "--open") {
        let command = if args[i] == "--share" { "SHARE" } else { "OPEN" };
        let Some(path) = args.get(i + 1) else {
            eprintln!("{} needs a file path", args[i]);
            std::process::exit(2);
        };
        // Absolute, because the client resolves it against the sync folder
        // and a file manager may launch us from anywhere.
        let full = std::fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.clone());
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

    // Update handling without a window, for a scripted rollout or for finding
    // out why a client is not taking one.
    if args.iter().any(|a| a == "--check-update" || a == "--update") {
        let install = args.iter().any(|a| a == "--update");
        let (Some(base), Some(token)) = (loaded.server_url.clone(), config::load_token()) else {
            eprintln!("Not connected to a server, or this computer is not paired.");
            std::process::exit(1);
        };
        match update::check(&base, &token) {
            Ok(info) => {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                if install && info.newer {
                    match update::install(&base, &token, &info) {
                        Ok(path) => println!("installed to {}", path.display()),
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

    // Scanners this computer can see, as JSON.
    if args.iter().any(|a| a == "--scanners") {
        let (found, notes) = discover::discover(std::time::Duration::from_secs(3), &loaded.scan.manual);
        let out = serde_json::json!({ "scanners": found, "notes": notes });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        std::process::exit(0);
    }

    // Scan straight into the drive with no window — for a scanner's button
    // (scanbd runs a command), a keyboard shortcut, or a script.
    if args.iter().any(|a| a == "--scan") {
        match scan_from_args(loaded, &args) {
            Ok(out) => {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }

    // A single pass, for a cron job or for working out why a sync is not
    // doing what was expected. Exits with the result.
    if args.iter().any(|a| a == "--sync-once") {
        let status: SharedStatus = Arc::new(Mutex::new(Status::default()));
        match run_pass(loaded, &status) {
            Pass::Busy => {
                eprintln!("A sync is already running (in the tray client, perhaps).");
                std::process::exit(1);
            }
            Pass::Ran { used_exemption: true } => {
                // Spent, as the app would. Read fresh from disk so nothing a
                // running client saved in the meantime is overwritten.
                let mut current = config::load();
                current.allow_bulk_delete_once = false;
                let _ = config::save(&current);
            }
            _ => {}
        }
        let final_status = status.lock().map(|s| s.clone()).unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&final_status).unwrap_or_default());
        std::process::exit(match final_status.state {
            State::Error => 1,
            _ => 0,
        });
    }
}

/// The value after `--flag`, if given.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// `--scan [--scanner ID|ADDRESS] [--source flatbed|feeder|duplex]
/// [--mode color|gray|bw] [--resolution DPI] [--paper a4|letter|legal|full]
/// [--folder PATH] [--name NAME] [--tags "a, b"]`
///
/// Anything not given comes from the last scan made in the window, so a
/// scanner button can run a bare `mecloud-desktop --scan`.
fn scan_from_args(loaded: &Config, args: &[String]) -> Result<serde_json::Value, String> {
    let server = loaded.server_url.clone().ok_or("Not connected to a server.")?;
    let token = config::load_token().ok_or("This computer is not connected to your account.")?;
    let prefs = &loaded.scan;

    let scanner: Scanner = match flag(args, "--scanner") {
        None => prefs.scanner.clone().ok_or("No scanner given, and none used before. Pass --scanner (see --scanners).")?,
        Some(wanted) => {
            let known: Vec<Scanner> = prefs.manual.iter().chain(prefs.scanner.iter()).cloned().collect();
            if let Some(s) = known.into_iter().find(|s| s.id == wanted) {
                s
            } else if let Some(device) = wanted.strip_prefix("sane:") {
                Scanner::sane(device, device)
            } else if wanted.contains("://") || wanted.contains('.') && !wanted.starts_with("escl:") {
                let root = scanner::escl::root_from_input(wanted)?;
                Scanner::escl(&root, None, &root, true)
            } else {
                let (found, _) = discover::discover(std::time::Duration::from_secs(3), &prefs.manual);
                found.into_iter().find(|s| s.id == wanted).ok_or(format!("No scanner \"{wanted}\" was found."))?
            }
        }
    };

    let remembered = prefs.settings.unwrap_or(Settings {
        source: Source::Flatbed,
        mode: Mode::Color,
        resolution: 300,
        paper: Paper::for_locale(std::env::var("LC_PAPER").or_else(|_| std::env::var("LANG")).ok().as_deref()),
    });
    let parse_or = |name: &str, current: Option<String>| flag(args, name).map(str::to_string).or(current);
    let wanted = Settings {
        source: match flag(args, "--source") {
            Some(v) => Source::parse(v).ok_or(format!("Unknown --source {v}"))?,
            None => remembered.source,
        },
        mode: match flag(args, "--mode") {
            Some(v) => Mode::parse(v).ok_or(format!("Unknown --mode {v}"))?,
            None => remembered.mode,
        },
        resolution: match flag(args, "--resolution") {
            Some(v) => v.parse().map_err(|_| format!("Unknown --resolution {v}"))?,
            None => remembered.resolution,
        },
        paper: match flag(args, "--paper") {
            Some(v) => Paper::parse(v).ok_or(format!("Unknown --paper {v}"))?,
            None => remembered.paper,
        },
    };
    let settings = scanner::fit(&scanner::options(&scanner)?, wanted)?;

    let api = crate::api::Api::new(&server, &token)?;
    let account = api.account_id()?;

    // A folder by path, as the web UI shows it; otherwise the last one used.
    let (folder_id, folder_path) = match parse_or("--folder", None) {
        None => (prefs.folder_id.clone(), prefs.folder_path.clone()),
        Some(path) => {
            let path = path.trim_matches('/').to_string();
            if path.is_empty() {
                (None, None)
            } else {
                let folders = crate::scanning::folders(&crate::api::Remote::nodes(&api, &account)?)?;
                let found = folders
                    .into_iter()
                    .find(|f| f.path.eq_ignore_ascii_case(&path))
                    .ok_or(format!("There is no folder \"{path}\" in your drive."))?;
                (found.id, Some(found.path))
            }
        }
    };

    let job = crate::scanning::Job {
        scanner,
        settings,
        folder_id,
        folder_path,
        name: flag(args, "--name").unwrap_or_default().to_string(),
        tags: crate::tags::split_input(flag(args, "--tags").unwrap_or_default()),
    };
    let saved = crate::scanning::run(
        &api,
        &account,
        job,
        &|_, line| eprintln!("{line}"),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    Ok(serde_json::json!({
        "node_id": saved.open.node_id,
        "name": saved.open.name,
        "folder": saved.open.folder_path.unwrap_or_default(),
        "pages": saved.open.pages.len(),
        "tags": saved.open.tags,
        "warning": saved.warning,
    }))
}
