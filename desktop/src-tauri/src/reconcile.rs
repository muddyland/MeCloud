//! Deciding what to do, given what changed where.
//!
//! Two-way sync is a three-way comparison: the local tree, the remote tree, and
//! what was true at the end of the last successful sync. Without that third
//! input a deletion is indistinguishable from a file that has not arrived yet,
//! and the usual result is that syncing an empty folder deletes everything.
//!
//! This module is pure — no filesystem, no network, no clock — so every case
//! below is a test rather than an experiment run against real data.

use std::collections::BTreeMap;

/// What we know about a file, on one side, right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// An opaque marker that changes when the content changes **on that side**.
    ///
    /// Locally it is the SHA-256 of the contents — the only honest way to tell
    /// "changed" from "touched", since mtimes move for reasons that have
    /// nothing to do with content. Remotely it is the blob id, because JMAP
    /// exposes no content hash and downloading every file to compute one would
    /// defeat the point of syncing.
    ///
    /// The two are therefore never comparable with each other, only with what
    /// the same side reported last time. That asymmetry is the whole reason
    /// `Baseline` remembers both.
    pub version: String,
    pub size: u64,
}

/// What the last successful sync recorded for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    /// The local content hash as of that sync.
    pub local: String,
    /// The remote blob id as of that sync.
    pub remote: String,
}

/// One unit of work. Names say which way the bytes move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Upload(String),
    Download(String),
    DeleteLocal(String),
    DeleteRemote(String),
    /// Both sides changed since the baseline, differently. Nothing is thrown
    /// away: the local file is renamed and both survive.
    Conflict { path: String, keep_local_as: String },
    /// Already in step — record the baseline and stop looking.
    RecordOnly(String),
    /// Both sides have this path, we have never synced it, and they are the
    /// same size. That is the "reconnected an existing folder" case, and
    /// calling it a conflict would litter a perfectly good folder with copies.
    /// The engine fetches the remote copy and compares before deciding.
    VerifyRemote(String),
}

impl Action {
    pub fn path(&self) -> &str {
        match self {
            Action::Upload(p)
            | Action::Download(p)
            | Action::DeleteLocal(p)
            | Action::DeleteRemote(p)
            | Action::RecordOnly(p)
            | Action::VerifyRemote(p) => p,
            Action::Conflict { path, .. } => path,
        }
    }
}

pub type Tree = BTreeMap<String, Entry>;
pub type Baselines = BTreeMap<String, Baseline>;

/// Where a conflicted copy of the local file goes.
///
/// Nextcloud's convention, and a good one: the name says what happened, when,
/// and keeps the extension so the file still opens in the right application.
pub fn conflict_name(path: &str, stamp: &str) -> String {
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    // Split on the LAST dot, and only if it is not a leading dot: ".bashrc" is
    // a name, not an extension, and "archive.tar.gz" keeps ".gz".
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    format!("{dir}{stem} (conflicted copy {stamp}){ext}")
}

/// Work out what to do for every path either side knows about.
///
/// `stamp` names conflicted copies; it is passed in rather than read from a
/// clock so the caller controls it and the tests are deterministic.
pub fn plan(local: &Tree, remote: &Tree, baseline: &Baselines, stamp: &str) -> Vec<Action> {
    let mut paths: Vec<&String> = local.keys().chain(remote.keys()).collect();
    paths.sort();
    paths.dedup();

    let mut actions = Vec::new();
    for path in paths {
        let l = local.get(path);
        let r = remote.get(path);
        let b = baseline.get(path);

        let action = match (l, r, b) {
            // Nothing anywhere: not reachable, since the path came from a side.
            (None, None, _) => continue,

            // ── Only one side has it ──────────────────────────────────────
            // New locally (no baseline) → upload. Gone remotely (baseline) →
            // the remote deleted it, so delete locally. This is the pair the
            // baseline exists for.
            (Some(_), None, None) => Action::Upload(path.clone()),
            (Some(l), None, Some(b)) => {
                if l.version == b.local {
                    Action::DeleteLocal(path.clone())
                } else {
                    // Deleted remotely, but changed locally since. The edit is
                    // the more expensive thing to lose, so it wins and goes up.
                    Action::Upload(path.clone())
                }
            }
            (None, Some(_), None) => Action::Download(path.clone()),
            (None, Some(r), Some(b)) => {
                if r.version == b.remote {
                    Action::DeleteRemote(path.clone())
                } else {
                    Action::Download(path.clone())
                }
            }

            // ── Both sides have it, never synced ──────────────────────────
            // Different sizes settle it: they are not the same file, and
            // neither may be overwritten by the other.
            (Some(l), Some(r), None) if l.size != r.size => Action::Conflict {
                path: path.clone(),
                keep_local_as: conflict_name(path, stamp),
            },
            // Same size, no history — most often a folder being reconnected to
            // an account it already matches. Worth one download to be sure.
            (Some(_), Some(_), None) => Action::VerifyRemote(path.clone()),

            (Some(l), Some(r), Some(b)) => {
                let local_changed = l.version != b.local;
                let remote_changed = r.version != b.remote;
                match (local_changed, remote_changed) {
                    (true, false) => Action::Upload(path.clone()),
                    (false, true) => Action::Download(path.clone()),
                    (true, true) => Action::Conflict {
                        path: path.clone(),
                        keep_local_as: conflict_name(path, stamp),
                    },
                    // Neither side moved since the last sync: in step.
                    (false, false) => Action::RecordOnly(path.clone()),
                }
            }
        };
        actions.push(action);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: &str = "2026-09-07";

    /// Sizes are fixed unless a test cares, so "differs" means "differs in
    /// content", which is what these cases are about.
    fn tree(items: &[(&str, &str)]) -> Tree {
        items
            .iter()
            .map(|(p, v)| (p.to_string(), Entry { version: (*v).into(), size: 10 }))
            .collect()
    }
    /// Baselines are written as (path, local_version, remote_version).
    fn base(items: &[(&str, &str, &str)]) -> Baselines {
        items
            .iter()
            .map(|(p, l, r)| {
                (p.to_string(), Baseline { local: (*l).into(), remote: (*r).into() })
            })
            .collect()
    }
    fn plan_of(
        local: &[(&str, &str)], remote: &[(&str, &str)], b: &[(&str, &str, &str)],
    ) -> Vec<Action> {
        plan(&tree(local), &tree(remote), &base(b), STAMP)
    }

    #[test]
    fn nothing_anywhere_is_nothing_to_do() {
        assert!(plan_of(&[], &[], &[]).is_empty());
    }

    #[test]
    fn a_new_local_file_is_uploaded() {
        assert_eq!(plan_of(&[("a.txt", "h1")], &[], &[]), vec![Action::Upload("a.txt".into())]);
    }

    #[test]
    fn a_new_remote_file_is_downloaded() {
        assert_eq!(plan_of(&[], &[("a.txt", "h1")], &[]), vec![Action::Download("a.txt".into())]);
    }

    #[test]
    fn a_file_neither_side_touched_is_only_recorded() {
        assert_eq!(
            plan_of(&[("a.txt", "L1")], &[("a.txt", "R1")], &[("a.txt", "L1", "R1")]),
            vec![Action::RecordOnly("a.txt".into())]
        );
    }

    #[test]
    fn a_remote_deletion_removes_the_local_copy() {
        // The baseline is what makes this a deletion rather than a new file.
        assert_eq!(
            plan_of(&[("a.txt", "L1")], &[], &[("a.txt", "L1", "R1")]),
            vec![Action::DeleteLocal("a.txt".into())]
        );
    }

    #[test]
    fn a_local_deletion_removes_the_remote_copy() {
        assert_eq!(
            plan_of(&[], &[("a.txt", "R1")], &[("a.txt", "L1", "R1")]),
            vec![Action::DeleteRemote("a.txt".into())]
        );
    }

    #[test]
    fn without_a_baseline_an_empty_side_never_deletes() {
        // The failure mode this whole design exists to prevent: a first sync,
        // or a lost database, must not read as "delete everything".
        let actions = plan_of(&[("a.txt", "h1"), ("b/c.txt", "h2")], &[], &[]);
        assert!(actions.iter().all(|a| matches!(a, Action::Upload(_))));

        let actions = plan_of(&[], &[("a.txt", "h1"), ("b/c.txt", "h2")], &[]);
        assert!(actions.iter().all(|a| matches!(a, Action::Download(_))));
    }

    #[test]
    fn a_local_edit_beats_a_remote_deletion() {
        // Deleting is cheap to redo; the edit is not. It goes back up.
        assert_eq!(
            plan_of(&[("a.txt", "L2")], &[], &[("a.txt", "L1", "R1")]),
            vec![Action::Upload("a.txt".into())]
        );
    }

    #[test]
    fn a_remote_edit_beats_a_local_deletion() {
        assert_eq!(
            plan_of(&[], &[("a.txt", "R2")], &[("a.txt", "L1", "R1")]),
            vec![Action::Download("a.txt".into())]
        );
    }

    #[test]
    fn a_one_sided_edit_moves_in_that_direction() {
        assert_eq!(
            plan_of(&[("a.txt", "L2")], &[("a.txt", "R1")], &[("a.txt", "L1", "R1")]),
            vec![Action::Upload("a.txt".into())]
        );
        assert_eq!(
            plan_of(&[("a.txt", "L1")], &[("a.txt", "R2")], &[("a.txt", "L1", "R1")]),
            vec![Action::Download("a.txt".into())]
        );
    }

    #[test]
    fn edits_on_both_sides_conflict_and_keep_both() {
        assert_eq!(
            plan_of(&[("a.txt", "L2")], &[("a.txt", "R2")], &[("a.txt", "L1", "R1")]),
            vec![Action::Conflict {
                path: "a.txt".into(),
                keep_local_as: "a (conflicted copy 2026-09-07).txt".into(),
            }]
        );
    }

    #[test]
    fn unsynced_files_of_different_sizes_conflict_rather_than_overwrite() {
        // No baseline and different sizes: neither is a version of the other,
        // so neither may be overwritten.
        let local = tree(&[("a.txt", "L1")]);
        let mut remote = tree(&[("a.txt", "R1")]);
        remote.get_mut("a.txt").unwrap().size = 99;
        assert!(matches!(
            plan(&local, &remote, &Baselines::new(), STAMP)[0],
            Action::Conflict { .. }
        ));
    }

    #[test]
    fn unsynced_files_of_the_same_size_are_checked_before_being_judged() {
        // Reconnecting a folder that already matches the account is the common
        // case; declaring a conflict would litter it with copies of every file.
        assert_eq!(
            plan_of(&[("a.txt", "L1")], &[("a.txt", "R1")], &[]),
            vec![Action::VerifyRemote("a.txt".into())]
        );
    }

    #[test]
    fn a_change_on_each_side_is_a_conflict_even_when_sizes_match() {
        // Equal sizes are not equal contents, and there is a baseline saying
        // both sides moved. Keeping both is the only answer that loses nothing.
        assert!(matches!(
            plan_of(&[("a.txt", "L2")], &[("a.txt", "R2")], &[("a.txt", "L1", "R1")])[0],
            Action::Conflict { .. }
        ));
    }

    #[test]
    fn every_path_from_either_side_is_considered_once() {
        let actions = plan_of(
            &[("a", "1"), ("b", "2")],
            &[("b", "2"), ("c", "3")],
            &[],
        );
        let paths: Vec<&str> = actions.iter().map(|a| a.path()).collect();
        assert_eq!(paths, vec!["a", "b", "c"]);
    }

    // ── conflict naming ─────────────────────────────────────────────────────

    #[test]
    fn a_conflicted_copy_keeps_its_extension() {
        assert_eq!(
            conflict_name("notes.txt", "2026-09-07"),
            "notes (conflicted copy 2026-09-07).txt"
        );
    }

    #[test]
    fn a_conflicted_copy_stays_in_its_folder() {
        assert_eq!(
            conflict_name("a/b/notes.txt", "S"),
            "a/b/notes (conflicted copy S).txt"
        );
    }

    #[test]
    fn a_dotfile_is_a_name_not_an_extension() {
        assert_eq!(conflict_name(".bashrc", "S"), ".bashrc (conflicted copy S)");
    }

    #[test]
    fn only_the_last_extension_is_kept() {
        assert_eq!(
            conflict_name("archive.tar.gz", "S"),
            "archive.tar (conflicted copy S).gz"
        );
    }

    #[test]
    fn a_file_with_no_extension_still_gets_a_sensible_name() {
        assert_eq!(conflict_name("README", "S"), "README (conflicted copy S)");
    }
}
