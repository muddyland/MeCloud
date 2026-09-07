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

/// Names never synced, in any folder.
///
/// The sync database lives in the config directory rather than the synced
/// folder, so it is not here; these are the things other software leaves
/// behind that no one means to share, plus our own conflict marker files
/// would be synced (they should be — they are real files the user must see).
const IGNORED_NAMES: &[&str] = &[
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    ".Trash-1000",
];

const IGNORED_PREFIXES: &[&str] = &[".~lock.", "~$"];
const IGNORED_SUFFIXES: &[&str] = &[".part", ".crdownload", ".swp", ".tmp"];

/// Whether a single path component should be skipped.
pub fn is_ignored(name: &str) -> bool {
    if IGNORED_NAMES.contains(&name) {
        return true;
    }
    if IGNORED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    if IGNORED_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    // An editor writing "file.txt" often creates "file.txt~" first; syncing
    // the intermediate wastes a round trip and confuses the baseline.
    name.ends_with('~')
}

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

/// Walk the sync folder into a tree of relative path → entry.
///
/// Errors on individual files are skipped rather than failing the run: a
/// single unreadable file should not stop everything else from syncing. They
/// are returned alongside so the caller can report them.
pub fn scan(root: &Path) -> (Tree, Vec<String>) {
    let mut tree = Tree::new();
    let mut problems = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e
                    .file_name()
                    .to_str()
                    .map(is_ignored)
                    .unwrap_or(true)
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                problems.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else { continue };
        let Some(rel) = relative.to_str() else {
            // A name that is not UTF-8 cannot be expressed as a JMAP file
            // name, so it is reported rather than silently dropped.
            problems.push(format!("{}: name is not valid UTF-8", entry.path().display()));
            continue;
        };
        match hash_file(entry.path()) {
            Ok((hash, size)) => {
                tree.insert(rel.replace('\\', "/"), Entry { version: hash, size });
            }
            Err(e) => problems.push(e),
        }
    }

    (tree, problems)
}

/// Turn a relative sync path into an absolute one, refusing anything that
/// would escape the sync folder.
///
/// The remote side supplies these names, so they are untrusted input: a node
/// called "../.ssh/authorized_keys" must not be writable outside the folder
/// the user chose to sync.
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
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hashes_are_content_addressed_not_name_addressed() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("b.txt"), b"hello").unwrap();
        let (tree, problems) = scan(&dir);
        assert!(problems.is_empty());
        assert_eq!(tree["a.txt"].version, tree["b.txt"].version);
        assert_eq!(tree["a.txt"].size, 5);
    }

    #[test]
    fn different_contents_hash_differently() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("b.txt"), b"world").unwrap();
        let (tree, _) = scan(&dir);
        assert_ne!(tree["a.txt"].version, tree["b.txt"].version);
    }

    #[test]
    fn nested_paths_use_forward_slashes() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::write(dir.join("a/b/c.txt"), b"x").unwrap();
        let (tree, _) = scan(&dir);
        assert!(tree.contains_key("a/b/c.txt"), "got {:?}", tree.keys().collect::<Vec<_>>());
    }

    #[test]
    fn junk_files_are_not_synced() {
        let dir = tempdir();
        fs::write(dir.join(".DS_Store"), b"x").unwrap();
        fs::write(dir.join("notes.txt~"), b"x").unwrap();
        fs::write(dir.join("big.iso.part"), b"x").unwrap();
        fs::write(dir.join("real.txt"), b"x").unwrap();
        let (tree, _) = scan(&dir);
        assert_eq!(tree.keys().collect::<Vec<_>>(), vec!["real.txt"]);
    }

    #[test]
    fn ignored_directories_are_not_descended_into() {
        let dir = tempdir();
        fs::create_dir_all(dir.join(".Trash-1000/deep")).unwrap();
        fs::write(dir.join(".Trash-1000/deep/x.txt"), b"x").unwrap();
        let (tree, _) = scan(&dir);
        assert!(tree.is_empty());
    }

    #[test]
    fn an_empty_folder_scans_to_nothing_rather_than_failing() {
        let (tree, problems) = scan(&tempdir());
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
}
