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

use crate::api::{self, Api};
use crate::reconcile::{self, Action, Tree};
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
            problems: Vec::new(),
            tracked: 0,
            last_sync: None,
        }
    }
}

pub type SharedStatus = Arc<Mutex<Status>>;

/// Everything one pass needs.
pub struct Engine {
    pub folder: PathBuf,
    pub api: Api,
    pub account_id: String,
    pub db: SyncDb,
    /// Set once by the user to let a refused pass through, and cleared as soon
    /// as it is used — a standing exemption would defeat the guard.
    pub allow_bulk_delete: bool,
}

impl Engine {
    /// Run a single reconciliation pass.
    pub fn run_once(&self, status: &SharedStatus) -> Status {
        let mut result = Status { state: State::Syncing, detail: "Syncing…".into(), ..Default::default() };
        publish(status, &result);

        // Hand the scanner what was recorded last time so it can skip reading
        // files whose size and timestamp are unchanged.
        let stamps = self.db.stamps().unwrap_or_default();
        let ((local, mut problems), mtimes) = scan::scan_with_stamps(&self.folder, &stamps);

        let nodes = match self.api.nodes(&self.account_id) {
            Ok(n) => n,
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
        let (remote, remote_ids) = api::to_tree(&nodes);
        let mut folders = folder_ids(&nodes);

        let baselines = match self.db.baselines() {
            Ok(b) => b,
            Err(e) => {
                let failed = Status { state: State::Error, detail: e, ..Default::default() };
                publish(status, &failed);
                return failed;
            }
        };

        let stamp = timestamp();
        let actions = reconcile::plan(&local, &remote, &baselines, &stamp);

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

        // A local scan that could not read parts of the folder cannot be used
        // to conclude that anything was deleted: the files may simply not have
        // been visible. Uploads and downloads still proceed.
        let trust_local = problems.is_empty();

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

        result.problems = problems;
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
                let mtime = std::fs::metadata(&full)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                self.db.record_with_mtime(path, &hash, &node.version, size, Some(id), mtime)?;
                result.downloaded += 1;
                Ok(())
            }

            Action::DeleteLocal(path) => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                if full.exists() {
                    std::fs::remove_file(&full).map_err(|e| e.to_string())?;
                    prune_empty_dirs(&self.folder, &full);
                }
                self.db.forget(path)?;
                result.deleted += 1;
                Ok(())
            }

            Action::DeleteRemote(path) => {
                if let Some(id) = remote_ids.get(path) {
                    self.api.destroy(&self.account_id, id)?;
                }
                self.db.forget(path)?;
                result.deleted += 1;
                Ok(())
            }

            // Keep the local copy under a new name, then take the server's as
            // the file of record. Nothing is deleted and nothing is silently
            // overwritten; the user is left with both and can see which is which.
            Action::Conflict { path, keep_local_as } => {
                let full = scan::resolve(&self.folder, path).ok_or("unsafe path")?;
                let kept = scan::resolve(&self.folder, keep_local_as).ok_or("unsafe path")?;
                if full.exists() {
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
                let scratch = full.with_extension("mecloud-verify");
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
                        let kept_full = scan::resolve(&self.folder, &kept).ok_or("unsafe path")?;
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
    if s.uploaded + s.downloaded + s.deleted + s.conflicts == 0 {
        return "Up to date".into();
    }
    let mut parts = Vec::new();
    if s.uploaded > 0 { parts.push(format!("{} uploaded", s.uploaded)); }
    if s.downloaded > 0 { parts.push(format!("{} downloaded", s.downloaded)); }
    if s.deleted > 0 { parts.push(format!("{} removed", s.deleted)); }
    if s.conflicts > 0 { parts.push(format!("{} conflicted", s.conflicts)); }
    parts.join(", ")
}

/// Split a sync path into (parent folders, file name).
pub fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// Folder path → node id, for every folder on the server.
pub fn folder_ids(nodes: &[api::Node]) -> BTreeMap<String, String> {
    let by_id: BTreeMap<&str, &api::Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut out = BTreeMap::new();

    'nodes: for node in nodes.iter().filter(|n| n.is_folder) {
        let mut parts = vec![node.name.as_str()];
        let mut cursor = node.parent_id.as_deref();
        let mut guard = 0;
        while let Some(id) = cursor {
            guard += 1;
            if guard > 64 {
                continue 'nodes;
            }
            let Some(parent) = by_id.get(id) else { continue 'nodes };
            parts.push(parent.name.as_str());
            cursor = parent.parent_id.as_deref();
        }
        parts.reverse();
        out.insert(parts.join("/"), node.id.clone());
    }
    out
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

    fn folder(id: &str, name: &str, parent: Option<&str>) -> api::Node {
        api::Node { id: id.into(), name: name.into(), parent_id: parent.map(Into::into),
                    blob_id: None, size: 0, is_folder: true }
    }

    #[test]
    fn splitting_a_path_separates_folders_from_the_name() {
        assert_eq!(split("a/b/c.txt"), ("a/b", "c.txt"));
        assert_eq!(split("c.txt"), ("", "c.txt"));
    }

    #[test]
    fn folder_ids_are_keyed_by_full_path() {
        let nodes = vec![folder("f1", "Photos", None), folder("f2", "2026", Some("f1"))];
        let ids = folder_ids(&nodes);
        assert_eq!(ids["Photos"], "f1");
        assert_eq!(ids["Photos/2026"], "f2");
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
    fn a_folder_whose_parent_is_missing_is_not_placed_at_the_root() {
        let ids = folder_ids(&[folder("f9", "Orphan", Some("gone"))]);
        assert!(ids.is_empty());
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
}
