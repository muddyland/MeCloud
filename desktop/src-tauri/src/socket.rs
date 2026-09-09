//! The local socket the file manager talks to.
//!
//! A file manager cannot ask the sync engine anything by itself, so the client
//! answers questions about paths on a Unix socket in the user's runtime
//! directory. This is the shape Nextcloud uses, and for the same reason: the
//! extension has to be a tiny, dependency-free thing running inside someone
//! else's process, so all the knowledge stays on this side of the wire.
//!
//! The protocol is newline-delimited `COMMAND:argument`, deliberately trivial
//! to speak from Python without a library.
//!
//! ```text
//!   -> RETRIEVE_FILE_STATUS:/home/ada/MeCloud/notes.txt
//!   <- STATUS:SYNCED:/home/ada/MeCloud/notes.txt
//!   -> SHARE:/home/ada/MeCloud/notes.txt      (compose, with it attached)
//!   -> OPEN:/home/ada/MeCloud/notes.txt       (open it in the web UI)
//!   -> VERSION:
//!   <- VERSION:0.2.0
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// What the file manager should draw on a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// In the sync folder and in step with the server.
    Synced,
    /// Known, but not yet in step.
    Syncing,
    /// In the folder but deliberately never synced (see `rules::is_ignored`).
    Ignored,
    /// The last pass could not handle it.
    Error,
    /// Outside the sync folder — draw nothing at all.
    None,
}

impl FileStatus {
    pub fn wire(&self) -> &'static str {
        match self {
            FileStatus::Synced => "SYNCED",
            FileStatus::Syncing => "SYNCING",
            FileStatus::Ignored => "IGNORED",
            FileStatus::Error => "ERROR",
            FileStatus::None => "NOP",
        }
    }
}

/// One parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Status(String),
    Share(String),
    Open(String),
    Version,
    /// Anything we do not understand. Answered, not dropped: a silent socket
    /// is indistinguishable from a hung one, and the extension would wait.
    Unknown(String),
}

/// Parse a line from the socket.
///
/// The argument may contain colons — paths do — so only the first separates.
pub fn parse_request(line: &str) -> Option<Request> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let (command, argument) = match line.split_once(':') {
        Some((c, a)) => (c, a),
        None => (line, ""),
    };
    Some(match command {
        "RETRIEVE_FILE_STATUS" => Request::Status(argument.to_string()),
        "SHARE" => Request::Share(argument.to_string()),
        "OPEN" => Request::Open(argument.to_string()),
        "VERSION" => Request::Version,
        other => Request::Unknown(other.to_string()),
    })
}

/// Where the socket lives.
///
/// The runtime directory: per-user, already mode 0700, and cleaned up at
/// logout — none of which is true of a path in /tmp.
pub fn socket_path() -> Option<PathBuf> {
    // Set but pointing at nothing is common — a container, a session started
    // outside a login manager — and is not the same as usable. Checking rather
    // than trusting it is what keeps the file manager working there.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir());

    let base = runtime.unwrap_or_else(|| {
        // Ours alone, and created 0700 by `serve`.
        std::env::temp_dir().join(format!("mecloud-{}", uid()))
    });
    Some(base.join("mecloud").join("socket"))
}

fn uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

/// Whether any reported problem concerns this path, or anything inside it.
///
/// Problems are reported as `"<relative path>: <what went wrong>"`, so this is
/// a prefix test — but a careless one claims the wrong rows. `Photo` must not
/// match `Photos/x.jpg: ...`, and for a folder the separator is what makes the
/// difference between "inside it" and "starts with the same letters".
pub fn problem_touches(problems: &[String], relative: &str, is_dir: bool) -> bool {
    let relative = relative.trim_matches('/');
    problems.iter().any(|problem| {
        if relative.is_empty() {
            return is_dir;
        }
        let Some(rest) = problem.strip_prefix(relative) else { return false };
        // Either this exact path, or something beneath it.
        rest.starts_with(':') || (is_dir && rest.starts_with('/'))
    })
}

/// Decide a path's status from what the engine knows.
///
/// Pure, so the rules are testable without a socket, a database or a disk.
///
/// `is_tracked` means different things for the two kinds of subject, and the
/// caller resolves that: for a file it is "there is a baseline row for it",
/// for a folder it is "there is a baseline row for something inside it".
pub fn status_for(
    path: &Path,
    sync_root: Option<&Path>,
    is_tracked: bool,
    has_problem: bool,
    syncing: bool,
) -> FileStatus {
    let Some(root) = sync_root else { return FileStatus::None };
    // Outside the synced folder the file manager should draw nothing, rather
    // than a badge implying we have an opinion about someone else's files.
    if !path.starts_with(root) {
        return FileStatus::None;
    }
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .map(crate::rules::is_ignored)
        .unwrap_or(false)
    {
        return FileStatus::Ignored;
    }
    if has_problem {
        return FileStatus::Error;
    }
    if is_tracked && !syncing {
        return FileStatus::Synced;
    }
    // Known but mid-pass, or not recorded yet: either way it is on its way.
    FileStatus::Syncing
}

/// Serve the socket for the life of the process.
///
/// `answer` runs on the connection's thread, so it must not touch the UI.
pub fn serve<F>(answer: F) -> Result<(), String>
where
    F: Fn(Request) -> Option<String> + Send + Sync + 'static,
{
    let path = socket_path().ok_or("No runtime directory for the socket")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // A socket file left behind by a previous run would refuse the bind.
    let _ = std::fs::remove_file(&path);

    let listener =
        UnixListener::bind(&path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

    let answer = std::sync::Arc::new(answer);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let answer = answer.clone();
            std::thread::spawn(move || handle(stream, answer.as_ref()));
        }
    });
    Ok(())
}

fn handle<F>(stream: UnixStream, answer: &F)
where
    F: Fn(Request) -> Option<String>,
{
    let Ok(mut writer) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let Some(request) = parse_request(&line) else { continue };
        if let Some(reply) = answer(request) {
            if writeln!(writer, "{reply}").is_err() {
                return;
            }
            let _ = writer.flush();
        }
    }
}

/// Send one command to a running client and return its reply.
///
/// Used by the command line, which is how a file manager that cannot host a
/// plugin — Dolphin, or anything driven by a shell command — reaches the
/// client.
pub fn send(request: &str) -> Result<String, String> {
    use std::time::Duration;

    let path = socket_path().ok_or("No runtime directory for the socket")?;
    if !path.exists() {
        return Err("MeCloud is not running.".into());
    }
    let stream = UnixStream::connect(&path).map_err(|_| "MeCloud is not running.".to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    writeln!(writer, "{request}").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;

    // One line, not to EOF. The server keeps the connection open for further
    // commands, so reading to EOF only ever ends in the read timeout.
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    Ok(reply.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn an_unusable_runtime_dir_falls_back_rather_than_failing() {
        // XDG_RUNTIME_DIR set to something that does not exist is common in a
        // container; trusting it left the file manager with no socket at all.
        std::env::set_var("XDG_RUNTIME_DIR", "/definitely/not/here");
        let path = socket_path().unwrap();
        assert!(!path.starts_with("/definitely/not/here"), "got {}", path.display());
        assert!(path.ends_with("mecloud/socket"));
        std::env::remove_var("XDG_RUNTIME_DIR");
    }

    #[test]
    fn parses_the_commands_the_extension_sends() {
        assert_eq!(
            parse_request("RETRIEVE_FILE_STATUS:/home/ada/MeCloud/a.txt\n"),
            Some(Request::Status("/home/ada/MeCloud/a.txt".into()))
        );
        assert_eq!(parse_request("SHARE:/x"), Some(Request::Share("/x".into())));
        assert_eq!(parse_request("OPEN:/x"), Some(Request::Open("/x".into())));
        assert_eq!(parse_request("VERSION:"), Some(Request::Version));
    }

    #[test]
    fn a_path_may_contain_colons() {
        // Only the first colon separates; "a:b.txt" is a legal file name.
        assert_eq!(
            parse_request("RETRIEVE_FILE_STATUS:/home/ada/a:b.txt"),
            Some(Request::Status("/home/ada/a:b.txt".into()))
        );
    }

    #[test]
    fn an_unknown_command_is_reported_rather_than_dropped() {
        // A silent socket is indistinguishable from a hung one.
        assert_eq!(parse_request("WAT:x"), Some(Request::Unknown("WAT".into())));
        assert_eq!(parse_request("\n"), None);
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn a_path_outside_the_sync_folder_gets_no_badge() {
        assert_eq!(
            status_for(&p("/home/ada/Documents/x.txt"), Some(&p("/home/ada/MeCloud")), true, false, false),
            FileStatus::None
        );
        assert_eq!(status_for(&p("/anything"), None, true, false, false), FileStatus::None);
    }

    #[test]
    fn a_tracked_file_at_rest_is_synced() {
        assert_eq!(
            status_for(&p("/home/ada/MeCloud/a.txt"), Some(&p("/home/ada/MeCloud")), true, false, false),
            FileStatus::Synced
        );
    }

    #[test]
    fn an_untracked_file_is_on_its_way_rather_than_synced() {
        assert_eq!(
            status_for(&p("/home/ada/MeCloud/new.txt"), Some(&p("/home/ada/MeCloud")), false, false, false),
            FileStatus::Syncing
        );
    }

    #[test]
    fn a_tracked_file_during_a_pass_shows_as_syncing() {
        assert_eq!(
            status_for(&p("/home/ada/MeCloud/a.txt"), Some(&p("/home/ada/MeCloud")), true, false, true),
            FileStatus::Syncing
        );
    }

    #[test]
    fn a_problem_outranks_being_tracked() {
        assert_eq!(
            status_for(&p("/home/ada/MeCloud/a.txt"), Some(&p("/home/ada/MeCloud")), true, true, false),
            FileStatus::Error
        );
    }

    #[test]
    fn files_we_never_sync_say_so_rather_than_looking_stuck() {
        // Otherwise every .DS_Store sits there with a spinner forever.
        assert_eq!(
            status_for(&p("/home/ada/MeCloud/.DS_Store"), Some(&p("/home/ada/MeCloud")), false, false, false),
            FileStatus::Ignored
        );
    }

    #[test]
    fn the_wire_names_are_stable() {
        // The extension matches on these strings.
        assert_eq!(FileStatus::Synced.wire(), "SYNCED");
        assert_eq!(FileStatus::Syncing.wire(), "SYNCING");
        assert_eq!(FileStatus::Ignored.wire(), "IGNORED");
        assert_eq!(FileStatus::Error.wire(), "ERROR");
        assert_eq!(FileStatus::None.wire(), "NOP");
    }

    #[test]
    fn a_problem_inside_a_folder_marks_the_folder() {
        let problems = vec!["Photos/2024/beach.jpg: upload failed".to_string()];
        assert!(problem_touches(&problems, "Photos", true));
        assert!(problem_touches(&problems, "Photos/2024", true));
        assert!(problem_touches(&problems, "Photos/2024/beach.jpg", false));
    }

    #[test]
    fn a_problem_does_not_leak_onto_a_similarly_named_neighbour() {
        let problems = vec!["Photos/beach.jpg: upload failed".to_string()];
        // Plain `starts_with` would mark this folder, which does not contain it.
        assert!(!problem_touches(&problems, "Photo", true));
        // And a file whose name merely prefixes the failing one.
        assert!(!problem_touches(&vec!["notes.txt.bak: nope".to_string()], "notes.txt", false));
    }

    #[test]
    fn a_file_is_not_marked_by_a_problem_beneath_its_name() {
        // Only a folder can contain something; a file with the same prefix
        // is a different file.
        let problems = vec!["archive/old.zip: failed".to_string()];
        assert!(!problem_touches(&problems, "archive", false));
        assert!(problem_touches(&problems, "archive", true));
    }

    #[test]
    fn the_sync_root_collects_every_problem() {
        let problems = vec!["anything/at/all.txt: failed".to_string()];
        assert!(problem_touches(&problems, "", true));
    }

    #[test]
    fn a_folder_holding_synced_files_reads_as_synced() {
        // The bug this fixes: folders have no baseline row of their own, so
        // they were permanently "syncing" and the file manager drew a spinner
        // on a folder that was completely up to date.
        let root = Path::new("/home/me/MeCloud");
        let folder = Path::new("/home/me/MeCloud/Photos");
        assert_eq!(
            status_for(folder, Some(root), true, false, false),
            FileStatus::Synced
        );
    }
}
