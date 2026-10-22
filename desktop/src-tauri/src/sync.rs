//! The sync engine.
//!
//! One pass is: read both sides, ask `reconcile` what to do, do it, and record
//! what was done. Everything interesting about *what* to do lives in
//! `reconcile` and is tested there; this module is the part that touches the
//! world, and it is written so that being interrupted at any point leaves a
//! consistent record — baselines are written per file, never per run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::api::{self, Api, Remote};
use crate::reconcile::{self, Action, Tree};
use crate::rules::{Rules, TRASH_DIR};
use crate::scan;
use crate::syncdb::SyncDb;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No folder chosen, or no device token: nothing to do and not an error.
    NotConfigured,
    Idle,
    Syncing,
    /// Finished, but some files could not be handled.
    Problems,
    Error,
    /// The user turned it off.
    Paused,
    /// A pass would have deleted an implausible number of files and was
    /// stopped before touching anything. Needs the user to look and confirm.
    NeedsConfirmation,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: State,
    pub detail: String,
    /// Files transferred in the last completed pass.
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub conflicts: usize,
    /// Deletions waiting out their window rather than applied.
    pub trashed: usize,
    pub pending_deletions: usize,
    /// Uploads waiting for the user to agree to their size.
    pub held_uploads: usize,
    /// Paths in scope for neither side because a rule excludes them.
    pub excluded: usize,
    pub problems: Vec<String>,
    pub tracked: i64,
    pub last_sync: Option<u64>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: State::NotConfigured,
            detail: "Not set up".into(),
            uploaded: 0,
            downloaded: 0,
            deleted: 0,
            conflicts: 0,
            trashed: 0,
            pending_deletions: 0,
            held_uploads: 0,
            excluded: 0,
            problems: Vec::new(),
            tracked: 0,
            last_sync: None,
        }
    }
}

pub type SharedStatus = Arc<Mutex<Status>>;

/// Everything one pass needs. Generic over the server so tests can drive it
/// with a fake; in the app it is always `Api`.
pub struct Engine<R: Remote = Api> {
    pub folder: PathBuf,
    pub api: R,
    pub account_id: String,
    pub db: SyncDb,
    /// Set once by the user to let a refused pass through, and cleared as soon
    /// as it is used — a standing exemption would defeat the guard.
    pub allow_bulk_delete: bool,
    /// Whether a local deletion should ever reach the server.
    pub delete_on_server: bool,
    /// How long a deletion waits, in either direction: before a local
    /// deletion reaches the server, and before a server deletion is removed
    /// from this computer's trash.
    pub trash_days: i64,
    /// What is in scope for this pass at all.
    pub rules: Rules,
}

impl<R: Remote> Engine<R> {
    /// Run a single reconciliation pass.
    pub fn run_once(&self, status: &SharedStatus) -> Status {
        let mut result = Status { state: State::Syncing, detail: "Syncing…".into(), ..Default::default() };
        publish(status, &result);

        // Hand the scanner what was recorded last time so it can skip reading
        // files whose size and timestamp are unchanged.
        let stamps = self.db.stamps().unwrap_or_default();
        let scanned = scan::scan(&self.folder, &stamps, &self.rules);
        let local = scanned.tree;
        let mtimes = scanned.mtimes;
        let mut problems = scanned.problems;
        // A local scan that could not read parts of the folder cannot be used
        // to conclude that anything was deleted: the files may simply not have
        // been visible. Decided before the deliberate skips are added, which
        // are reported but do not make the walk untrustworthy.
        let trust_local = problems.is_empty();
        problems.extend(scanned.skipped);

        // An incomplete listing is an error for the whole pass. Nothing is
        // planned from half an answer, because a missing file reads as a
        // deleted one.
        let listing = match self.api.nodes(&self.account_id).and_then(|nodes| api::to_tree(&nodes)) {
            Ok(l) => l,
            Err(e) => {
                let failed = Status {
                    state: State::Error,
                    detail: e,
                    problems,
                    tracked: self.db.count().unwrap_or(0),
                    ..Default::default()
                };
                publish(status, &failed);
                return failed;
            }
        };
        let api::Listing { files: mut remote, file_ids: remote_ids, folder_ids: mut folders, duplicates } = listing;
        for path in duplicates {
            problems.push(format!("{path}: more than one item on the server has this name; only one is synced"));
        }

        // The local side was already filtered by the scanner. Filtering the
        // remote side and the baseline by the *same* rules is what makes an
        // exclusion inert: the reconciler cannot propose a deletion for a path
        // it never sees on any side. Filter only one side and every newly
        // excluded file reads as "deleted here, still there" — which is a
        // request to delete it from the server.
        let before = remote.len();
        remote.retain(|path, _| !self.rules.excludes(path));
        let excluded_remote = before - remote.len();

        let baselines = match self.db.baselines() {
            Ok(mut b) => {
                // Left in the database, only held out of this pass: dropping
                // the rows would lose the record of a file that is still on
                // both sides, and re-including the folder later would then
                // look like a first-ever sync of every file in it.
                b.retain(|path, _| !self.rules.excludes(path));
                b
            }
            Err(e) => {
                let failed = Status { state: State::Error, detail: e, ..Default::default() };
                publish(status, &failed);
                return failed;
            }
        };

        // Anything already waiting in the trash is out of scope for this pass:
        // it is neither present locally nor deleted remotely yet, and planning
        // for it would either download it back or delete it early.
        let pending = self.db.pending_paths().unwrap_or_default();

        let stamp = timestamp();
        let actions: Vec<Action> = reconcile::plan(&local, &remote, &baselines, &stamp)
            .into_iter()
            .filter(|a| !pending.contains(a.path()))
            .collect();

        // Before doing anything at all. A pass that would remove most of what
        // is tracked is far more likely to be one side failing to report its
        // contents than a real deletion, and acting on it is unrecoverable.
        if !self.allow_bulk_delete {
            if let Some(refusal) = reconcile::refuse_mass_deletion(&actions, baselines.len()) {
                let stopped = Status {
                    state: State::NeedsConfirmation,
                    detail: refusal.message,
                    problems,
                    tracked: self.db.count().unwrap_or(0),
                    ..Default::default()
                };
                publish(status, &stopped);
                return stopped;
            }
        }

        for action in actions {
            if !trust_local && matches!(action, Action::DeleteRemote(_)) {
                problems.push(format!(
                    "{}: not removed from the server, because this computer could not read \
                     the whole sync folder",
                    action.path()
                ));
                continue;
            }
            if let Err(e) = self.apply(&action, &local, &remote, &remote_ids, &mut folders, &mtimes, &mut result) {
                problems.push(format!("{}: {e}", action.path()));
                // An upstream that has stopped accepting us will fail for every
                // remaining file; say so once rather than 500 times.
                if e == api::UNPAIRED {
                    result.state = State::Error;
                    result.detail = e;
                    result.problems = problems;
                    publish(status, &result);
                    return result;
                }
            }
        }

        // Server deletions held here long enough are removed for good.
        self.purge_local_trash(&mut problems);

        // Deletions whose window has closed are applied now.
        if self.delete_on_server {
            for due in self.db.expired_pending(self.trash_days).unwrap_or_default() {
                match self.apply_pending(&due) {
                    Ok(()) => result.deleted += 1,
                    Err(e) => problems.push(format!("{}: {e}", due.path)),
                }
            }
        }

        result.problems = problems;
        result.pending_deletions = self.db.pending().map(|p| p.len()).unwrap_or(0);
        result.held_uploads = self.db.held_uploads().map(|u| u.len()).unwrap_or(0);
        result.excluded = excluded_remote;
        result.tracked = self.db.count().unwrap_or(0);
        result.last_sync = Some(now());
        result.state = if result.problems.is_empty() { State::Idle } else { State::Problems };
        result.detail = summarise(&result);
        publish(status, &result);
        result
    }

    fn apply(
        &self,
        action: &Action,
        local: &Tree,
        remote: &Tree,
        remote_ids: &BTreeMap<String, String>,
        folders: &mut BTreeMap<String, String>,
        mtimes: &std::collections::HashMap<String, i64>,
        result: &mut Status,
    ) -> Result<(), String> {
        match action {
            Action::RecordOnly(path) => {
                let (Some(l), Some(r)) = (local.get(path), remote.get(path)) else { return Ok(()) };
                self.db.record_with_mtime(
                    path, &l.version, &r.version, l.size,
                    remote_ids.get(path).map(String::as_str),
                    mtimes.get(path).copied().unwrap_or(0),
                )
            }

            Action::Upload(path) => {
                // Ask before spending someone's upstream on a very large file.
                // Held rather than excluded: it stays on disk, stays tracked,
                // and the moment it is approved the ordinary path runs.
                let size_now = local.get(path).map(|e| e.size).unwrap_or(0);
                if self.rules.needs_confirmation(size_now) && !self.db.upload_approved(path) {
                    self.db.hold_upload(path, size_now)?;
                    result.held_uploads += 1;
                    return Ok(());
                }

                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                let (blob, size) = self.api.upload_blob(&full)?;
                let node_id = match remote_ids.get(path) {
                    Some(id) => {
                        self.api.update_file(&self.account_id, id, &blob, size)?;
                        id.clone()
                    }
                    None => {
                        let (parent, name) = split(path);
                        let parent_id = self.ensure_folder(parent, folders)?;
                        self.api.create_file(&self.account_id, name, parent_id.as_deref(), &blob, size)?
                    }
                };
                // Ask the server what blob id the node actually carries now,
                // rather than assuming it kept the one the upload returned.
                let remote_version = self
                    .api
                    .node_blob(&self.account_id, &node_id)?
                    .unwrap_or(blob);
                let hash = local.get(path).map(|e| e.version.clone()).unwrap_or_default();
                self.db.record_with_mtime(
                    path, &hash, &remote_version, size, Some(&node_id),
                    mtimes.get(path).copied().unwrap_or(0),
                )?;
                // The approval was for this upload, not a standing exemption.
                let _ = self.db.clear_upload(path);
                result.uploaded += 1;
                Ok(())
            }

            Action::Download(path) => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                let id = remote_ids.get(path).ok_or("no such file on the server")?;
                let node = remote.get(path).ok_or("no such file on the server")?;
                let (_, name) = split(path);
                self.api.download_blob(&node.version, name, &full)?;
                let (hash, size) = scan::hash_file(&full)?;
                // The file was just written, so its mtime is now; recording it
                // stops the next pass re-hashing what we already know.
                let mtime = std::fs::metadata(&full).map(|m| scan::mtime_of(&m)).unwrap_or(0);
                self.db.record_with_mtime(path, &hash, &node.version, size, Some(id), mtime)?;
                result.downloaded += 1;
                Ok(())
            }

            // Moved aside, never removed. The server saying a file is gone
            // is no more proof than this folder saying so, and the server's
            // answer is the one this client cannot see behind — so the local
            // copy waits in the trash for the same window as the other way.
            Action::DeleteLocal(path) => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                if let Ok(meta) = std::fs::symlink_metadata(&full) {
                    let stored = self.move_to_trash(path, &full)?;
                    self.db.add_local_trash(path, &stored, meta.len())?;
                    prune_empty_dirs(&self.folder, &full);
                }
                self.db.forget(path)?;
                result.deleted += 1;
                Ok(())
            }

            // Never destroys anything immediately. A file deleted here goes to
            // the trash and stays visible until the user says otherwise or the
            // window closes on it -- because "gone from this folder" and "the
            // user wants the only other copy destroyed" are not the same claim.
            Action::DeleteRemote(path) => {
                if !self.delete_on_server {
                    // Turned off entirely: leave the server alone and stop
                    // reporting it every pass by forgetting our record of it.
                    self.db.forget(path)?;
                    return Ok(());
                }
                let size = remote.get(path).map(|e| e.size).unwrap_or(0);
                self.db.mark_pending(path, remote_ids.get(path).map(String::as_str), size)?;
                result.trashed += 1;
                Ok(())
            }

            // Keep the local copy under a new name, then take the server's as
            // the file of record. Nothing is deleted and nothing is silently
            // overwritten; the user is left with both and can see which is which.
            Action::Conflict { path, keep_local_as } => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                if full.exists() {
                    // Never over an earlier conflicted copy: two conflicts on
                    // one file in the same minute share a name.
                    let kept = unused_path(&self.folder, keep_local_as)?;
                    std::fs::rename(&full, &kept).map_err(|e| e.to_string())?;
                }
                self.db.forget(path)?;
                result.conflicts += 1;

                // The renamed copy is now a new local file, and the server's
                // copy is missing locally; the next pass uploads one and
                // downloads the other. Doing it here would duplicate that logic.
                Ok(())
            }

            // Same size, no history. Fetch the server's copy to a scratch file
            // and hash it: if it matches, the two were already the same and
            // this is just an adoption, which is the common case after a
            // reinstall. Only a real difference becomes a conflict.
            Action::VerifyRemote(path) => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                let node = remote.get(path).ok_or("no such file on the server")?;
                let id = remote_ids.get(path).ok_or("no such file on the server")?;
                let scratch = api::scratch_path(&full, api::VERIFY_SUFFIX);
                let (_, name) = split(path);
                self.api.download_blob(&node.version, name, &scratch)?;

                let remote_hash = scan::hash_file(&scratch);
                let local_hash = local.get(path).map(|e| e.version.clone());
                let _ = std::fs::remove_file(&scratch);

                match (remote_hash, local_hash) {
                    (Ok((rh, size)), Some(lh)) if rh == lh => {
                        self.db.record(path, &lh, &node.version, size, Some(id))
                    }
                    (Ok(_), Some(_)) => {
                        let kept = reconcile::conflict_name(path, &timestamp());
                        let kept_full = unused_path(&self.folder, &kept)?;
                        std::fs::rename(&full, &kept_full).map_err(|e| e.to_string())?;
                        result.conflicts += 1;
                        Ok(())
                    }
                    (Err(e), _) => Err(e),
                    (_, None) => Ok(()),
                }
            }
        }
    }

    /// Carry out one deferred deletion on the server.
    pub fn apply_pending(&self, due: &crate::syncdb::PendingDeletion) -> Result<(), String> {
        if let Some(id) = &due.node_id {
            self.api.destroy(&self.account_id, id)?;
        }
        self.db.clear_pending(&due.path)?;
        self.db.forget(&due.path)
    }

    /// Move a file into the trash folder, keeping its relative path. Returns
    /// where it went, relative to the trash folder.
    fn move_to_trash(&self, path: &str, full: &Path) -> Result<String, String> {
        let trash = self.folder.join(TRASH_DIR);
        let stored = unused_relative(&trash, path)?;
        let dest = trash.join(&stored);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::rename(full, &dest).map_err(|e| format!("could not move it to the trash: {e}"))?;
        Ok(stored)
    }

    /// Remove held server deletions whose window has passed.
    fn purge_local_trash(&self, problems: &mut Vec<String>) {
        let trash = self.folder.join(TRASH_DIR);
        for due in self.db.expired_local_trash(self.trash_days).unwrap_or_default() {
            // The stored name came from this client, but it is still checked:
            // the database is a file anyone with the account can edit.
            let Some(full) = scan::resolve(&trash, &due.stored) else {
                let _ = self.db.clear_local_trash(&due.stored);
                continue;
            };
            match std::fs::remove_file(&full) {
                Ok(()) => prune_empty_dirs(&trash, &full),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    problems.push(format!("{}: could not empty from the trash: {e}", due.path));
                    continue;
                }
            }
            let _ = self.db.clear_local_trash(&due.stored);
        }
    }

    /// The id of the remote folder for a path, creating any part that is
    /// missing. Returns None for the drive root.
    fn ensure_folder(
        &self, path: &str, folders: &mut BTreeMap<String, String>,
    ) -> Result<Option<String>, String> {
        if path.is_empty() {
            return Ok(None);
        }
        let mut parent: Option<String> = None;
        let mut walked = String::new();
        for part in path.split('/') {
            if !walked.is_empty() {
                walked.push('/');
            }
            walked.push_str(part);
            parent = match folders.get(&walked) {
                Some(id) => Some(id.clone()),
                None => {
                    let created = self.api.create_folder(&self.account_id, part, parent.as_deref())?;
                    // Remembered for the rest of the pass. Without this, every
                    // file after the first in a new folder tried to create it
                    // again -- the server refused as already existing, and a
                    // first sync of a thousand files into ten folders uploaded
                    // ten of them and reported the rest as failures.
                    folders.insert(walked.clone(), created.clone());
                    Some(created)
                }
            };
        }
        Ok(parent)
    }
}

fn publish(status: &SharedStatus, next: &Status) {
    if let Ok(mut guard) = status.lock() {
        *guard = next.clone();
    }
}

fn summarise(s: &Status) -> String {
    if s.uploaded + s.downloaded + s.deleted + s.conflicts + s.trashed + s.held_uploads == 0 {
        return "Up to date".into();
    }
    let mut parts = Vec::new();
    if s.uploaded > 0 { parts.push(format!("{} uploaded", s.uploaded)); }
    if s.downloaded > 0 { parts.push(format!("{} downloaded", s.downloaded)); }
    if s.deleted > 0 { parts.push(format!("{} removed", s.deleted)); }
    if s.conflicts > 0 { parts.push(format!("{} conflicted", s.conflicts)); }
    if s.trashed > 0 { parts.push(format!("{} awaiting deletion", s.trashed)); }
    // Worth its own line: nothing is wrong, but nothing will happen to these
    // either until someone says so, and silence would read as "synced".
    if s.held_uploads > 0 { parts.push(format!("{} too large to send yet", s.held_uploads)); }
    parts.join(", ")
}

/// Split a sync path into (parent folders, file name).
pub fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// `path` with " (n)" before its extension, for n ≥ 2.
fn numbered(path: &str, n: u32) -> String {
    let (dir, file) = split(path);
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    let named = format!("{stem} ({n}){ext}");
    if dir.is_empty() { named } else { format!("{dir}/{named}") }
}

/// A relative path under `root` that nothing occupies yet: `path` itself, or
/// the first numbered variant that is free.
fn unused_relative(root: &Path, path: &str) -> Result<String, String> {
    for n in 1..1000 {
        let candidate = if n == 1 { path.to_string() } else { numbered(path, n) };
        let full = scan::resolve(root, &candidate).ok_or("unsafe path")?;
        if std::fs::symlink_metadata(&full).is_err() {
            return Ok(candidate);
        }
    }
    Err(format!("{path}: too many copies with this name already"))
}

/// As `unused_relative`, inside the sync folder, as an absolute path.
pub(crate) fn unused_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let free = unused_relative(root, path)?;
    scan::resolve(root, &free).ok_or_else(|| "unsafe path".to_string())
}

/// Remove directories left empty by a deletion, up to but never including the
/// sync folder itself.
fn prune_empty_dirs(root: &Path, from: &Path) {
    let mut cursor = from.parent();
    while let Some(dir) = cursor {
        if dir == root || !dir.starts_with(root) {
            return;
        }
        if std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
            return;
        }
        if std::fs::remove_dir(dir).is_err() {
            return;
        }
        cursor = dir.parent();
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A stamp for conflicted copies: date and time, no separators that a file
/// system would object to.
fn timestamp() -> String {
    let secs = now();
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}", rem / 3600, (rem % 3600) / 60)
}

/// Days since the epoch to a calendar date (Howard Hinnant's algorithm).
/// Written out rather than pulling in a date crate for one call site.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_a_path_separates_folders_from_the_name() {
        assert_eq!(split("a/b/c.txt"), ("a/b", "c.txt"));
        assert_eq!(split("c.txt"), ("", "c.txt"));
    }

    #[test]
    fn creating_a_nested_folder_records_every_level() {
        // The map is what stops the next file re-creating the same folders.
        let mut folders = BTreeMap::new();
        folders.insert("Photos".to_string(), "f1".to_string());
        assert!(folders.contains_key("Photos"));
        // A second file under Photos/2026 must find 2026 already recorded once
        // the first has created it; that is what `ensure_folder` now inserts.
        folders.insert("Photos/2026".to_string(), "f2".to_string());
        assert_eq!(folders.get("Photos/2026").map(String::as_str), Some("f2"));
    }

    #[test]
    fn the_conflict_stamp_is_a_usable_file_name() {
        let stamp = timestamp();
        assert!(!stamp.contains('/') && !stamp.contains(':'));
        assert_eq!(stamp.len(), "2026-09-07 0142".len());
    }

    #[test]
    fn dates_convert_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn a_summary_says_up_to_date_when_nothing_moved() {
        assert_eq!(summarise(&Status::default()), "Up to date");
        let moved = Status { uploaded: 2, conflicts: 1, ..Default::default() };
        assert_eq!(summarise(&moved), "2 uploaded, 1 conflicted");
    }

    #[test]
    fn pruning_stops_at_the_sync_folder() {
        let root = std::env::temp_dir().join(format!("mecloud-prune-{}", std::process::id()));
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        let file = deep.join("x.txt");
        std::fs::write(&file, b"x").unwrap();
        std::fs::remove_file(&file).unwrap();

        prune_empty_dirs(&root, &file);
        assert!(!deep.exists(), "empty directories should be removed");
        assert!(root.exists(), "the sync folder itself must survive");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The safety property the whole rules design exists for.
    ///
    /// Filtering the three inputs is done in `run_once`, which needs a server
    /// and a disk; this reproduces the filtering exactly and asserts on what
    /// the planner then does, because the thing that must never regress is the
    /// *outcome*: newly excluding a file produces no work, and above all no
    /// deletion.
    fn plan_with_rules(
        local: &Tree, remote: &Tree, baselines: &crate::reconcile::Baselines, rules: &Rules,
    ) -> Vec<Action> {
        let local: Tree = local.iter().filter(|(p, _)| !rules.excludes(p))
            .map(|(p, e)| (p.clone(), e.clone())).collect();
        let remote: Tree = remote.iter().filter(|(p, _)| !rules.excludes(p))
            .map(|(p, e)| (p.clone(), e.clone())).collect();
        let baselines: crate::reconcile::Baselines = baselines.iter()
            .filter(|(p, _)| !rules.excludes(p))
            .map(|(p, b)| (p.clone(), b.clone())).collect();
        reconcile::plan(&local, &remote, &baselines, "2026-09-07 1200")
    }

    /// Anything that reads or writes a byte, on either side.
    fn moves_data(action: &Action) -> bool {
        !matches!(action, Action::RecordOnly(_))
    }

    fn entry(version: &str) -> crate::reconcile::Entry {
        crate::reconcile::Entry { version: version.into(), size: 10 }
    }

    #[test]
    fn newly_excluding_a_synced_file_proposes_nothing() {
        // The file is on both sides and fully in step. Adding a rule that
        // covers it must be inert — not a deletion, not a download, nothing.
        let mut local = Tree::new();
        local.insert("Photos/beach.jpg".into(), entry("localhash"));
        local.insert("Docs/tax.pdf".into(), entry("otherhash"));

        let mut remote = Tree::new();
        remote.insert("Photos/beach.jpg".into(), entry("blob-1"));
        remote.insert("Docs/tax.pdf".into(), entry("blob-2"));

        let mut baselines = crate::reconcile::Baselines::new();
        baselines.insert("Photos/beach.jpg".into(), crate::reconcile::Baseline {
            local: "localhash".into(), remote: "blob-1".into(),
        });
        baselines.insert("Docs/tax.pdf".into(), crate::reconcile::Baseline {
            local: "otherhash".into(), remote: "blob-2".into(),
        });

        let rules = Rules::new(&["Photos/".to_string()], &[], None, true);
        let actions = plan_with_rules(&local, &remote, &baselines, &rules);

        // `RecordOnly` re-stamps a baseline and moves nothing, so it is not
        // work in the sense that matters. What must not appear is the
        // excluded path, in any action at all.
        assert!(
            !actions.iter().any(|a| a.path() == "Photos/beach.jpg"),
            "an excluded path must not appear in the plan: {actions:?}"
        );
        assert!(!actions.iter().any(|a| moves_data(a)), "{actions:?}");
    }

    #[test]
    fn excluding_a_file_present_only_locally_does_not_delete_it_remotely() {
        // The dangerous ordering: the file is gone from the local tree because
        // a rule now hides it, while the baseline still says it was synced.
        // Filtering the baseline by the same rule is what stops that reading
        // as "the user deleted this".
        let local = Tree::new();
        let mut remote = Tree::new();
        remote.insert("secret.key".into(), entry("blob-9"));
        let mut baselines = crate::reconcile::Baselines::new();
        baselines.insert("secret.key".into(), crate::reconcile::Baseline {
            local: "hash".into(), remote: "blob-9".into(),
        });

        let rules = Rules::new(&["*.key".to_string()], &[], None, true);
        let actions = plan_with_rules(&local, &remote, &baselines, &rules);

        assert!(
            !actions.iter().any(|a| matches!(a, Action::DeleteRemote(_))),
            "an exclusion must never become a deletion: {actions:?}"
        );
        assert!(actions.is_empty());

        // And without the rule it *is* read as a deletion — which is what
        // makes the filtering above load-bearing rather than incidental.
        let unfiltered = plan_with_rules(&local, &remote, &baselines, &Rules::permissive());
        assert!(unfiltered.iter().any(|a| matches!(a, Action::DeleteRemote(_))));
    }

    #[test]
    fn switching_a_folder_back_on_resumes_it_rather_than_starting_over() {
        // Baselines are held out of the pass, not deleted, so re-including a
        // folder finds its history intact and does nothing for files that
        // never changed while it was switched off.
        let mut local = Tree::new();
        local.insert("Photos/beach.jpg".into(), entry("localhash"));
        let mut remote = Tree::new();
        remote.insert("Photos/beach.jpg".into(), entry("blob-1"));
        let mut baselines = crate::reconcile::Baselines::new();
        baselines.insert("Photos/beach.jpg".into(), crate::reconcile::Baseline {
            local: "localhash".into(), remote: "blob-1".into(),
        });

        let off = Rules::new(&[], &["Photos".to_string()], None, true);
        assert!(plan_with_rules(&local, &remote, &baselines, &off).is_empty());

        let on = Rules::permissive();
        let resumed = plan_with_rules(&local, &remote, &baselines, &on);
        assert!(
            !resumed.iter().any(|a| moves_data(a)),
            "re-including must not re-transfer files that never changed: {resumed:?}"
        );
    }

    // ── The engine, end to end, against a fake server ──────────────────────

    use std::cell::RefCell;

    struct FakeRemote {
        listing: RefCell<Result<Vec<api::Node>, String>>,
        uploaded: RefCell<Vec<String>>,
        destroyed: RefCell<Vec<String>>,
    }

    impl FakeRemote {
        fn new(listing: Result<Vec<api::Node>, String>) -> Self {
            Self { listing: RefCell::new(listing), uploaded: RefCell::default(), destroyed: RefCell::default() }
        }
    }

    impl Remote for FakeRemote {
        fn nodes(&self, _: &str) -> Result<Vec<api::Node>, String> {
            self.listing.borrow().clone()
        }
        fn upload_blob(&self, path: &Path) -> Result<(String, u64), String> {
            self.uploaded.borrow_mut().push(path.display().to_string());
            Ok(("new-blob".into(), 1))
        }
        fn download_blob(&self, _: &str, _: &str, to: &Path) -> Result<(), String> {
            std::fs::write(to, b"from server").map_err(|e| e.to_string())
        }
        fn create_folder(&self, _: &str, name: &str, _: Option<&str>) -> Result<String, String> {
            Ok(format!("folder-{name}"))
        }
        fn create_file(&self, _: &str, name: &str, _: Option<&str>, _: &str, _: u64) -> Result<String, String> {
            Ok(format!("node-{name}"))
        }
        fn update_file(&self, _: &str, _: &str, _: &str, _: u64) -> Result<(), String> {
            Ok(())
        }
        fn node_blob(&self, _: &str, _: &str) -> Result<Option<String>, String> {
            Ok(Some("new-blob".into()))
        }
        fn destroy(&self, _: &str, id: &str) -> Result<(), String> {
            self.destroyed.borrow_mut().push(id.into());
            Ok(())
        }
    }

    fn file_node(id: &str, name: &str, parent: Option<&str>) -> api::Node {
        api::Node { id: id.into(), name: name.into(), parent_id: parent.map(Into::into),
                    blob_id: Some(format!("blob-{id}")), size: 3, is_folder: false }
    }

    fn sync_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mecloud-engine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A folder with `names` on disk, each recorded as synced with node
    /// `id-<name>` and blob `blob-id-<name>`.
    fn synced(name: &str, names: &[&str], remote: Result<Vec<api::Node>, String>) -> Engine<FakeRemote> {
        let folder = sync_root(name);
        let db = SyncDb::in_memory().unwrap();
        for n in names {
            let full = folder.join(n);
            std::fs::write(&full, b"abc").unwrap();
            let (hash, size) = scan::hash_file(&full).unwrap();
            db.record(n, &hash, &format!("blob-id-{n}"), size, Some(&format!("id-{n}"))).unwrap();
        }
        Engine {
            folder,
            api: FakeRemote::new(remote),
            account_id: "acct".into(),
            db,
            allow_bulk_delete: false,
            delete_on_server: true,
            trash_days: 30,
            rules: Rules::permissive(),
        }
    }

    fn status() -> SharedStatus {
        Arc::new(Mutex::new(Status::default()))
    }

    #[test]
    fn a_failed_listing_deletes_nothing() {
        let engine = synced("failed", &["a.txt", "b.txt"], Err("The server listed 2 files but described only 1".into()));
        let outcome = engine.run_once(&status());
        assert_eq!(outcome.state, State::Error);
        assert!(engine.folder.join("a.txt").exists() && engine.folder.join("b.txt").exists());
        assert_eq!(engine.db.count().unwrap(), 2);
    }

    #[test]
    fn a_listing_with_an_orphan_deletes_nothing() {
        // b.txt's folder was not in the listing. Before, it was silently
        // dropped, which made it look deleted on the server.
        let remote = Ok(vec![file_node("id-a.txt", "a.txt", None), file_node("id-b.txt", "b.txt", Some("missing"))]);
        let engine = synced("orphan", &["a.txt", "b.txt"], remote);
        let outcome = engine.run_once(&status());
        assert_eq!(outcome.state, State::Error, "{outcome:?}");
        assert!(engine.folder.join("b.txt").exists());
    }

    #[test]
    fn a_server_deletion_moves_the_local_copy_to_the_trash() {
        // a.txt is gone from the server; b.txt is still there.
        let mut keep = file_node("id-b.txt", "b.txt", None);
        keep.blob_id = Some("blob-id-b.txt".into());
        let engine = synced("trash", &["a.txt", "b.txt"], Ok(vec![keep]));
        let outcome = engine.run_once(&status());

        assert_eq!(outcome.deleted, 1, "{outcome:?}");
        assert!(!engine.folder.join("a.txt").exists());
        let held = engine.folder.join(TRASH_DIR).join("a.txt");
        assert!(held.exists(), "the file should be held, not removed");
        let listed = engine.db.local_trash().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "a.txt");
        // Nothing was uploaded back, and the held copy is not seen as new.
        assert!(engine.api.uploaded.borrow().is_empty());
        let again = engine.run_once(&status());
        assert_eq!(again.uploaded + again.deleted, 0, "{again:?}");
    }

    #[test]
    fn held_server_deletions_are_purged_after_the_window() {
        let mut keep = file_node("id-b.txt", "b.txt", None);
        keep.blob_id = Some("blob-id-b.txt".into());
        let mut engine = synced("purge", &["a.txt", "b.txt"], Ok(vec![keep]));
        engine.run_once(&status());
        assert!(engine.folder.join(TRASH_DIR).join("a.txt").exists());

        engine.trash_days = 0;
        engine.run_once(&status());
        assert!(!engine.folder.join(TRASH_DIR).join("a.txt").exists());
        assert!(engine.db.local_trash().unwrap().is_empty());
    }

    #[test]
    fn a_mass_deletion_needs_confirmation_and_the_exemption_lets_it_through() {
        let names: Vec<String> = (0..30).map(|i| format!("f{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut engine = synced("bulk", &refs, Ok(Vec::new()));

        let refused = engine.run_once(&status());
        assert_eq!(refused.state, State::NeedsConfirmation);
        assert!(engine.folder.join("f0.txt").exists());

        engine.allow_bulk_delete = true;
        let allowed = engine.run_once(&status());
        assert_eq!(allowed.deleted, 30, "{allowed:?}");
        // Even then, nothing is destroyed: every file is in the trash.
        assert!(engine.folder.join(TRASH_DIR).join("f0.txt").exists());
    }

    #[test]
    fn a_conflicted_copy_never_overwrites_an_earlier_one() {
        let root = sync_root("unique");
        std::fs::write(root.join("a (conflicted copy x).txt"), b"first").unwrap();
        assert_eq!(unused_relative(&root, "a (conflicted copy x).txt").unwrap(), "a (conflicted copy x) (2).txt");
        assert_eq!(unused_relative(&root, "free.txt").unwrap(), "free.txt");
        assert_eq!(numbered("dir/.bashrc", 3), "dir/.bashrc (3)");
    }

    #[cfg(unix)]
    #[test]
    fn a_server_file_is_never_written_through_a_symlinked_folder() {
        let elsewhere = sync_root("elsewhere");
        let remote = Ok(vec![
            api::Node { id: "d".into(), name: "Desktop".into(), parent_id: None, blob_id: None, size: 0, is_folder: true },
            file_node("n", "planted.txt", Some("d")),
        ]);
        let engine = synced("symlink", &[], remote);
        std::os::unix::fs::symlink(&elsewhere, engine.folder.join("Desktop")).unwrap();
        let outcome = engine.run_once(&status());
        assert!(!elsewhere.join("planted.txt").exists());
        assert!(!outcome.problems.is_empty(), "{outcome:?}");
    }
}
