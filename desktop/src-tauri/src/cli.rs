//! Flags that do one job and exit, without a window.

use std::sync::{Arc, Mutex};

use crate::config::{self, Config};
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
