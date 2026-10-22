//! Reading the local side of the sync folder.
//!
//! Produces the same shape the reconciler wants from the remote side, so the
//! two can be compared directly: relative path → content hash and size.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::reconcile::{Entry, Tree};
use crate::rules::{is_ignored, Rules};

/// SHA-256 of a file's contents, streamed.
///
/// Streamed rather than read into memory: the sync folder is where large
/// files live, and hashing must not be the thing that decides how large a
/// file the client can handle.
pub fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// The modification time of an entry, in whole seconds since the epoch.
pub fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What one walk of the sync folder found.
#[derive(Debug, Default)]
pub struct LocalScan {
    /// Relative path → content hash and size.
    pub tree: Tree,
    /// Things that could not be read. Any of these means the walk may have
    /// missed files, so nothing may be concluded deleted from it.
    pub problems: Vec<String>,
    /// Things deliberately left alone and worth telling the user about —
    /// symbolic links. Unlike `problems` they do not make the walk untrustworthy.
    pub skipped: Vec<String>,
    /// Relative path → mtime, to record with the baseline.
    pub mtimes: std::collections::HashMap<String, i64>,
}

/// Walk the folder, reusing a recorded hash when a file looks untouched.
///
/// Hashing every file on every pass is correct but expensive: a pass runs every
/// thirty seconds, so a 5 GB folder means re-reading 5 GB from disk 120 times an
/// hour, which is what made the client feel heavy. A file whose size *and*
/// modification time both match what was recorded is taken as unchanged and its
/// stored hash reused.
///
/// The trade is explicit: a file edited in place, in under a second, without
/// changing its length, keeping its mtime, would be missed until something else
/// touches it. Every sync client makes this bargain, because the alternative is
/// reading the whole folder continuously.
///
/// Symbolic links are not followed and not synced. Following one would sync
/// whatever it points at — possibly the whole home directory — and writing
/// through one would put server files outside the sync folder.
pub fn scan(
    root: &Path,
    known: &std::collections::HashMap<String, (u64, i64, String)>,
    rules: &Rules,
) -> LocalScan {
    let mut out = LocalScan::default();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Pruning whole directories here rather than filtering files
            // afterwards is what makes an excluded `node_modules` cost
            // nothing instead of a full walk of it.
            e.depth() == 0
                || e.path()
                    .strip_prefix(root)
                    .ok()
                    .and_then(|r| r.to_str())
                    .map(|r| !rules.excludes(&r.replace('\\', "/")))
                    .unwrap_or(false)
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.problems.push(e.to_string());
                continue;
            }
        };
        if entry.depth() > 0 && entry.path_is_symlink() {
            let shown = entry.path().strip_prefix(root).unwrap_or(entry.path()).display().to_string();
            out.skipped.push(format!(
                "{shown}: symbolic links are not synced (move the real folder here, or add an ignore rule)"
            ));
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else { continue };
        let Some(rel) = relative.to_str() else {
            // A name that is not UTF-8 cannot be expressed as a JMAP file
            // name, so it is reported rather than silently dropped.
            out.problems.push(format!("{}: name is not valid UTF-8", entry.path().display()));
            continue;
        };
        let key = rel.replace('\\', "/");
        let meta = entry.metadata().ok();
        let mtime = meta.as_ref().map(mtime_of).unwrap_or(0);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        // Unchanged length and timestamp: trust the recorded hash rather than
        // reading the file again.
        if let Some((known_size, known_mtime, known_hash)) = known.get(&key) {
            if *known_size == size && *known_mtime == mtime && mtime != 0 {
                out.tree.insert(key.clone(), Entry { version: known_hash.clone(), size });
                out.mtimes.insert(key, mtime);
                continue;
            }
        }

        match hash_file(entry.path()) {
            Ok((hash, hashed_size)) => {
                out.tree.insert(key.clone(), Entry { version: hash, size: hashed_size });
                out.mtimes.insert(key, mtime);
            }
            Err(e) => out.problems.push(e),
        }
    }
    out
}

/// Turn a relative sync path into an absolute one, refusing anything that
/// would escape the sync folder.
///
/// The remote side supplies these names, so they are untrusted input: a node
/// called "../.ssh/authorized_keys" must not be writable outside the folder
/// the user chose to sync. Lexical checks are not enough on their own: a
/// symbolic link *inside* the folder (`Desktop -> ~/Desktop`) would carry a
/// write, rename or delete straight through it. So every component that
/// already exists below the root must be a real file or directory. The root
/// itself may be a link — pointing the sync folder at another disk is normal.
pub fn resolve(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') {
        return None;
    }
    let mut out = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        if is_ignored(part) {
            return None;
        }
        out.push(part);
        if std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scan_plain(root: &Path) -> (Tree, Vec<String>) {
        let s = scan(root, &Default::default(), &Rules::permissive());
        (s.tree, s.problems)
    }

    fn scan_with_stamps_t(
        root: &Path, known: &std::collections::HashMap<String, (u64, i64, String)>, rules: &Rules,
    ) -> ((Tree, Vec<String>), std::collections::HashMap<String, i64>) {
        let s = scan(root, known, rules);
        ((s.tree, s.problems), s.mtimes)
    }

    #[test]
    fn hashes_are_content_addressed_not_name_addressed() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("b.txt"), b"hello").unwrap();
        let (tree, problems) = scan_plain(&dir);
        assert!(problems.is_empty());
        assert_eq!(tree["a.txt"].version, tree["b.txt"].version);
        assert_eq!(tree["a.txt"].size, 5);
    }

    #[test]
    fn different_contents_hash_differently() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("b.txt"), b"world").unwrap();
        let (tree, _) = scan_plain(&dir);
        assert_ne!(tree["a.txt"].version, tree["b.txt"].version);
    }

    #[test]
    fn nested_paths_use_forward_slashes() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::write(dir.join("a/b/c.txt"), b"x").unwrap();
        let (tree, _) = scan_plain(&dir);
        assert!(tree.contains_key("a/b/c.txt"), "got {:?}", tree.keys().collect::<Vec<_>>());
    }

    #[test]
    fn junk_files_are_not_synced() {
        let dir = tempdir();
        fs::write(dir.join(".DS_Store"), b"x").unwrap();
        fs::write(dir.join("notes.txt~"), b"x").unwrap();
        fs::write(dir.join("big.iso.part"), b"x").unwrap();
        fs::write(dir.join("real.txt"), b"x").unwrap();
        let (tree, _) = scan_plain(&dir);
        assert_eq!(tree.keys().collect::<Vec<_>>(), vec!["real.txt"]);
    }

    #[test]
    fn ignored_directories_are_not_descended_into() {
        let dir = tempdir();
        fs::create_dir_all(dir.join(".Trash-1000/deep")).unwrap();
        fs::write(dir.join(".Trash-1000/deep/x.txt"), b"x").unwrap();
        let (tree, _) = scan_plain(&dir);
        assert!(tree.is_empty());
    }

    #[test]
    fn an_unchanged_file_is_not_read_again() {
        let dir = tempdir();
        let path = dir.join("a.txt");
        fs::write(&path, b"hello").unwrap();

        let ((first, _), stamps) = scan_with_stamps_t(&dir, &Default::default(), &Rules::permissive());
        let real = first["a.txt"].version.clone();

        // Claim a different hash for the same size and mtime: if the scan
        // re-read the file it would disagree with us.
        let mut known = std::collections::HashMap::new();
        known.insert("a.txt".to_string(), (5u64, stamps["a.txt"], "PRETEND".to_string()));
        let ((second, _), _) = scan_with_stamps_t(&dir, &known, &Rules::permissive());
        assert_eq!(second["a.txt"].version, "PRETEND", "should have trusted the stamp");
        assert_ne!(real, "PRETEND");
    }

    #[test]
    fn a_changed_size_forces_a_re_read() {
        let dir = tempdir();
        let path = dir.join("a.txt");
        fs::write(&path, b"hello").unwrap();
        let ((_, _), stamps) = scan_with_stamps_t(&dir, &Default::default(), &Rules::permissive());

        let mut known = std::collections::HashMap::new();
        // Recorded as a different length, so the stamp cannot be trusted.
        known.insert("a.txt".to_string(), (999u64, stamps["a.txt"], "PRETEND".to_string()));
        let ((tree, _), _) = scan_with_stamps_t(&dir, &known, &Rules::permissive());
        assert_ne!(tree["a.txt"].version, "PRETEND");
    }

    #[test]
    fn a_file_with_no_usable_timestamp_is_always_hashed() {
        // mtime 0 means "unknown"; trusting it would skip the file forever.
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        let mut known = std::collections::HashMap::new();
        known.insert("a.txt".to_string(), (5u64, 0i64, "PRETEND".to_string()));
        let ((tree, _), _) = scan_with_stamps_t(&dir, &known, &Rules::permissive());
        assert_ne!(tree["a.txt"].version, "PRETEND");
    }

    #[test]
    fn an_empty_folder_scans_to_nothing_rather_than_failing() {
        let (tree, problems) = scan_plain(&tempdir());
        assert!(tree.is_empty() && problems.is_empty());
    }

    // ── resolve ─────────────────────────────────────────────────────────────

    #[test]
    fn a_normal_relative_path_resolves_under_the_root() {
        let root = Path::new("/sync");
        assert_eq!(resolve(root, "a/b.txt").unwrap(), Path::new("/sync/a/b.txt"));
    }

    #[test]
    fn a_path_that_would_escape_the_sync_folder_is_refused() {
        // The remote supplies these names, so this is untrusted input.
        let root = Path::new("/sync");
        for bad in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "",
            "a//b.txt",
            "./a.txt",
            "a\\b.txt",
        ] {
            assert!(resolve(root, bad).is_none(), "should refuse {bad:?}");
        }
    }

    #[test]
    fn a_remote_name_that_we_would_never_upload_is_not_written_either() {
        assert!(resolve(Path::new("/sync"), "a/.DS_Store").is_none());
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "mecloud-scan-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn excluded_folders_are_pruned_rather_than_walked() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("node_modules/left-pad")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("node_modules/left-pad/index.js"), b"junk").unwrap();
        fs::write(dir.join("src/main.rs"), b"code").unwrap();

        let rules = Rules::new(&["node_modules/".to_string()], &[], None, true);
        let ((tree, problems), _) = scan_with_stamps_t(&dir, &Default::default(), &rules);

        assert!(problems.is_empty());
        assert_eq!(tree.keys().collect::<Vec<_>>(), vec!["src/main.rs"]);
    }

    #[test]
    fn an_excluded_path_is_reported_the_same_as_one_that_is_not_there() {
        // The engine relies on this: an excluded file must be absent from the
        // local tree, not present-and-flagged, or the reconciler would see a
        // file it has a baseline for and nothing else, and call it deleted.
        let dir = tempdir();
        fs::write(dir.join("keep.txt"), b"a").unwrap();
        fs::write(dir.join("secret.key"), b"b").unwrap();

        let rules = Rules::new(&["*.key".to_string()], &[], None, true);
        let ((tree, _), stamps) = scan_with_stamps_t(&dir, &Default::default(), &rules);

        assert!(!tree.contains_key("secret.key"));
        assert!(!stamps.contains_key("secret.key"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_reported_and_not_followed() {
        let dir = tempdir();
        let elsewhere = tempdir();
        fs::write(elsewhere.join("private.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("Desktop")).unwrap();
        fs::write(dir.join("real.txt"), b"x").unwrap();

        let s = scan(&dir, &Default::default(), &Rules::permissive());
        assert_eq!(s.tree.keys().collect::<Vec<_>>(), vec!["real.txt"]);
        // Reported, but not as a read failure: a link must not switch off
        // deletions for the whole folder.
        assert!(s.problems.is_empty());
        assert_eq!(s.skipped.len(), 1);
        assert!(s.skipped[0].contains("Desktop"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_refuses_to_write_through_a_symlink_inside_the_folder() {
        let dir = tempdir();
        let elsewhere = tempdir();
        std::os::unix::fs::symlink(&elsewhere, dir.join("Desktop")).unwrap();
        assert!(resolve(&dir, "Desktop/new.txt").is_none());
        assert!(resolve(&dir, "Desktop").is_none());
        // A link as the final component is refused too: File::create follows it.
        std::os::unix::fs::symlink(elsewhere.join("x"), dir.join("file.txt")).unwrap();
        assert!(resolve(&dir, "file.txt").is_none());
        assert!(resolve(&dir, "fine/new.txt").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_sync_root_itself_may_be_a_symlink() {
        let real = tempdir();
        let link = std::env::temp_dir().join(format!("mecloud-rootlink-{}", std::process::id()));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        fs::write(real.join("a.txt"), b"x").unwrap();
        assert!(resolve(&link, "a.txt").is_some());
        let s = scan(&link, &Default::default(), &Rules::permissive());
        assert!(s.tree.contains_key("a.txt"));
        let _ = fs::remove_file(&link);
    }

    #[test]
    fn the_clients_own_scratch_files_are_never_synced_or_written() {
        let dir = tempdir();
        fs::write(dir.join("report.txt.mecloud-part"), b"half").unwrap();
        fs::write(dir.join("report.txt.mecloud-verify"), b"x").unwrap();
        fs::create_dir_all(dir.join(".mecloud-trash")).unwrap();
        fs::write(dir.join(".mecloud-trash/old.txt"), b"x").unwrap();
        let (tree, _) = scan_plain(&dir);
        assert!(tree.is_empty(), "{:?}", tree.keys().collect::<Vec<_>>());
        assert!(resolve(&dir, "a.mecloud-part").is_none());
        assert!(resolve(&dir, ".mecloud-trash/old.txt").is_none());
    }
}
