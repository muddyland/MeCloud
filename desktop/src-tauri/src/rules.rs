//! Which files sync, and which are none of the client's business.
//!
//! Three separate questions end up here because they all answer "should this
//! pass touch this path", and answering them in one place is what lets the
//! engine apply them to *every* input — local tree, remote tree and baseline
//! alike. That symmetry is the whole safety property: a path the rules exclude
//! is invisible on all three sides, so excluding it produces no work at all.
//! Filtering only the local side would make every newly-excluded file look
//! deleted, and the client would dutifully propose removing it from the server.
//!
//! Patterns are the familiar gitignore-ish subset, chosen because it is what
//! people already have in their fingers:
//!
//! | Pattern | Matches |
//! |---|---|
//! | `*.log` | any `.log` file, at any depth |
//! | `node_modules/` | that directory anywhere, and everything inside it |
//! | `Photos/raw` | only that path from the root of the sync folder |
//! | `**/cache` | `cache` at any depth |
//! | `draft-?.txt` | `draft-1.txt`, not `draft-10.txt` |
//!
//! A pattern with no slash matches a *name* at any depth. A pattern with a
//! slash is anchored to the sync folder's root, which is the rule people trip
//! over least: `Photos/raw` never surprises anyone by matching
//! `Archive/Photos/raw`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Names never synced, in any folder, regardless of what the user configures.
///
/// The sync database lives in the config directory rather than the synced
/// folder, so it is not here; these are the things other software leaves
/// behind that no one means to share. Conflict markers are deliberately absent
/// — they are real files the user must see.
const IGNORED_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", ".Trash-1000"];

const IGNORED_PREFIXES: &[&str] = &[".~lock.", "~$"];
const IGNORED_SUFFIXES: &[&str] = &[".part", ".crdownload", ".swp", ".tmp"];

/// Whether a single path component is one of the built-in exclusions.
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

/// One parsed exclusion pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    raw: String,
    segments: Vec<String>,
    /// No slash in the pattern: match a name at any depth.
    by_name: bool,
    /// Trailing slash: only matches a directory, never a file of that name.
    dir_only: bool,
}

impl Pattern {
    /// Parse one line. Blank lines and `#` comments are not patterns.
    pub fn parse(raw: &str) -> Option<Self> {
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            return None;
        }

        let dir_only = text.ends_with('/');
        // A leading slash is the *only* thing that anchors a single-segment
        // pattern: `build` means "anything called build, anywhere", while
        // `/build` means the one at the root. Stripping the slash before
        // deciding loses that distinction and silently widens the rule.
        let anchored = text.starts_with('/');
        let core = text.trim_matches('/');
        if core.is_empty() {
            return None;
        }

        let segments: Vec<String> = core.split('/').map(str::to_string).collect();
        Some(Self {
            raw: text.to_string(),
            by_name: !anchored && segments.len() == 1,
            segments,
            dir_only,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether this pattern excludes the given sync-folder-relative path.
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return false;
        }

        if self.by_name {
            let pat = &self.segments[0];
            // A name pattern excludes the file it names *and* everything under
            // a directory it names, so it is checked against every component.
            // `dir_only` means the last component does not count: `build/`
            // should not exclude a file called `build`.
            let last = parts.len() - 1;
            return parts
                .iter()
                .enumerate()
                .any(|(i, part)| !(self.dir_only && i == last) && matches_segment(pat, part));
        }

        // Anchored: the pattern must match from the root, either the whole
        // path or a leading run of it — matching a directory prefix is what
        // makes `Photos/raw` exclude everything inside it.
        let pattern: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        for end in 1..=parts.len() {
            if self.dir_only && end == parts.len() {
                // The prefix is the file itself, not a directory containing it.
                continue;
            }
            if matches_path(&pattern, &parts[..end]) {
                return true;
            }
        }
        false
    }
}

/// Match one path component against one pattern segment (`*` and `?`).
///
/// Iterative with a backtrack point rather than recursive: the inputs are
/// user-supplied, and `*` chains in a recursive matcher are the classic way to
/// turn a settings field into a hang.
fn matches_segment(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while t < txt.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(s) = star {
            // Give the last `*` one more character and try again.
            p = s + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|c| *c == '*')
}

/// Match a segmented pattern against segmented text, with `**` spanning any
/// number of components.
fn matches_path(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((head, rest)) if *head == "**" => {
            // Zero or more components, so every suffix is a candidate.
            (0..=text.len()).any(|skip| matches_path(rest, &text[skip..]))
        }
        Some((head, rest)) => match text.split_first() {
            Some((first, tail)) if matches_segment(head, first) => matches_path(rest, tail),
            _ => false,
        },
    }
}

/// The complete set of "should this pass touch this path" answers.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    patterns: Vec<Pattern>,
    /// Top-level folders the user has switched off in selective sync.
    excluded_folders: BTreeSet<String>,
    /// Uploads above this wait for the user to agree. `None` means never ask.
    pub confirm_over_bytes: Option<u64>,
    /// Whether dot-files sync. On by default, and deliberately so — see
    /// `Config::sync_hidden`.
    pub sync_hidden: bool,
}

impl Rules {
    pub fn new(
        patterns: &[String],
        excluded_folders: &[String],
        confirm_over_bytes: Option<u64>,
        sync_hidden: bool,
    ) -> Self {
        Self {
            patterns: patterns.iter().filter_map(|p| Pattern::parse(p)).collect(),
            excluded_folders: excluded_folders
                .iter()
                .map(|f| f.trim_matches('/').to_string())
                .filter(|f| !f.is_empty())
                .collect(),
            confirm_over_bytes,
            sync_hidden,
        }
    }

    /// Everything synced, nothing configured — the state before the user has
    /// opened the rules screen.
    pub fn permissive() -> Self {
        Self { sync_hidden: true, ..Default::default() }
    }

    /// Whether this path is out of scope for sync entirely.
    ///
    /// `path` is relative to the sync folder, with `/` separators.
    pub fn excludes(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return false;
        }

        if parts.iter().any(|p| is_ignored(p)) {
            return true;
        }
        if !self.sync_hidden && parts.iter().any(|p| p.starts_with('.')) {
            return true;
        }
        // Selective sync works on top-level folders, which is the granularity
        // the picker offers; anything finer belongs in a pattern.
        if parts.len() > 1 && self.excluded_folders.contains(parts[0]) {
            return true;
        }
        self.patterns.iter().any(|p| p.matches(path))
    }

    /// Whether an upload of this size should wait to be agreed to.
    pub fn needs_confirmation(&self, size: u64) -> bool {
        self.confirm_over_bytes.is_some_and(|limit| size > limit)
    }

    /// Which of the supplied pattern lines could not be understood, so the
    /// settings screen can say so instead of silently dropping them.
    pub fn unparsed(patterns: &[String]) -> Vec<String> {
        patterns
            .iter()
            .filter(|line| {
                let text = line.trim();
                !text.is_empty() && !text.starts_with('#') && Pattern::parse(text).is_none()
            })
            .cloned()
            .collect()
    }

    pub fn excluded_folders(&self) -> &BTreeSet<String> {
        &self.excluded_folders
    }
}

/// A top-level folder offered in the selective-sync picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderChoice {
    pub name: String,
    /// Files anywhere beneath it, so the picker can say what is at stake.
    pub files: usize,
    pub bytes: u64,
    pub included: bool,
}

/// Summarise a remote tree into the top-level folders a user can switch off.
///
/// Files sitting at the root of the drive are not offered: there is no folder
/// to switch off, and presenting them individually would be a file picker
/// rather than a selective-sync one.
pub fn folder_choices<'a, I>(paths: I, excluded: &BTreeSet<String>) -> Vec<FolderChoice>
where
    I: IntoIterator<Item = (&'a String, u64)>,
{
    let mut totals: std::collections::BTreeMap<String, (usize, u64)> = Default::default();
    for (path, size) in paths {
        let Some((top, _)) = path.split_once('/') else { continue };
        let entry = totals.entry(top.to_string()).or_default();
        entry.0 += 1;
        entry.1 += size;
    }

    // A folder the user has switched off has no files in the remote tree we
    // were handed — it was filtered out — so it would vanish from its own
    // picker. Put it back, with what we know: that it is excluded.
    for name in excluded {
        totals.entry(name.clone()).or_default();
    }

    totals
        .into_iter()
        .map(|(name, (files, bytes))| FolderChoice {
            included: !excluded.contains(&name),
            name,
            files,
            bytes,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(patterns: &[&str]) -> Rules {
        let owned: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        Rules::new(&owned, &[], None, true)
    }

    #[test]
    fn a_name_pattern_matches_at_any_depth() {
        let r = rules(&["*.log"]);
        assert!(r.excludes("debug.log"));
        assert!(r.excludes("a/b/debug.log"));
        assert!(!r.excludes("debug.log.txt"));
    }

    #[test]
    fn a_directory_pattern_takes_everything_under_it() {
        let r = rules(&["node_modules/"]);
        assert!(r.excludes("node_modules/left-pad/index.js"));
        assert!(r.excludes("app/node_modules/x"));
        // The trailing slash says directory, so a *file* of that name stays.
        assert!(!r.excludes("node_modules"));
    }

    #[test]
    fn a_pattern_with_a_slash_is_anchored_to_the_root() {
        let r = rules(&["Photos/raw"]);
        assert!(r.excludes("Photos/raw"));
        assert!(r.excludes("Photos/raw/DSC_0001.NEF"));
        // The trap this avoids: matching the same name further down.
        assert!(!r.excludes("Archive/Photos/raw/DSC_0001.NEF"));
    }

    #[test]
    fn a_leading_slash_means_the_same_as_a_slash_anywhere() {
        assert!(rules(&["/build"]).excludes("build/out.o"));
        assert!(!rules(&["/build"]).excludes("src/build/out.o"));
    }

    #[test]
    fn double_star_spans_any_number_of_folders() {
        let r = rules(&["**/cache"]);
        assert!(r.excludes("cache"));
        assert!(r.excludes("a/cache"));
        assert!(r.excludes("a/b/c/cache/file"));
        assert!(!r.excludes("a/caches"));
    }

    #[test]
    fn question_mark_matches_exactly_one_character() {
        let r = rules(&["draft-?.txt"]);
        assert!(r.excludes("draft-1.txt"));
        assert!(!r.excludes("draft-10.txt"));
        assert!(!r.excludes("draft-.txt"));
    }

    #[test]
    fn stars_do_not_cross_folder_boundaries() {
        // `*` is a segment-level wildcard; without this `a/*.txt` would quietly
        // mean `a/**/*.txt` and exclude far more than it says.
        let r = rules(&["a/*.txt"]);
        assert!(r.excludes("a/one.txt"));
        assert!(!r.excludes("a/b/one.txt"));
    }

    #[test]
    fn a_pathological_pattern_still_terminates() {
        // A recursive matcher goes exponential on this shape. The settings
        // field is user input, so it has to be the kind that cannot hang.
        let r = rules(&["*a*a*a*a*a*a*a*a*a*a*a*a*b"]);
        assert!(!r.excludes(&"a".repeat(64)));
    }

    #[test]
    fn blank_lines_and_comments_are_not_patterns() {
        assert_eq!(Pattern::parse(""), None);
        assert_eq!(Pattern::parse("   "), None);
        assert_eq!(Pattern::parse("# ignore the below"), None);
        assert_eq!(Pattern::parse("/"), None);
        assert!(Rules::unparsed(&["".into(), "# c".into(), "*.log".into()]).is_empty());
    }

    #[test]
    fn built_in_exclusions_apply_with_no_configuration() {
        let r = rules(&[]);
        assert!(r.excludes(".DS_Store"));
        assert!(r.excludes("holiday/Thumbs.db"));
        assert!(r.excludes("notes.txt~"));
        assert!(r.excludes("half.crdownload"));
        assert!(!r.excludes("holiday/beach.jpg"));
    }

    #[test]
    fn hidden_files_sync_unless_asked_otherwise() {
        // This default is load-bearing. Flipping it would make every dot-file
        // an existing user syncs look deleted on the next pass, which is the
        // exact shape of the bug that emptied someone's drive.
        assert!(!Rules::permissive().excludes(".bashrc"));

        let hide = Rules::new(&[], &[], None, false);
        assert!(hide.excludes(".bashrc"));
        assert!(hide.excludes("a/.config/settings.json"));
        assert!(!hide.excludes("a/visible.txt"));
    }

    #[test]
    fn selective_sync_switches_off_a_top_level_folder() {
        let r = Rules::new(&[], &["Photos".into()], None, true);
        assert!(r.excludes("Photos/2024/beach.jpg"));
        assert!(!r.excludes("Documents/tax.pdf"));
        // The folder's own name as a root-level *file* is a different thing.
        assert!(!r.excludes("Photos"));
    }

    #[test]
    fn the_size_threshold_is_a_ceiling_not_a_floor() {
        let r = Rules::new(&[], &[], Some(1000), true);
        assert!(!r.needs_confirmation(1000));
        assert!(r.needs_confirmation(1001));
        assert!(!Rules::permissive().needs_confirmation(u64::MAX));
    }

    #[test]
    fn folder_choices_report_what_switching_off_would_cost() {
        let paths: Vec<(String, u64)> = vec![
            ("Photos/a.jpg".into(), 100),
            ("Photos/b/c.jpg".into(), 200),
            ("Docs/x.pdf".into(), 50),
            ("loose.txt".into(), 9),
        ];
        let borrowed: Vec<(&String, u64)> = paths.iter().map(|(p, s)| (p, *s)).collect();
        let choices = folder_choices(borrowed, &BTreeSet::new());

        assert_eq!(choices.len(), 2, "root-level files are not folders");
        let photos = choices.iter().find(|c| c.name == "Photos").unwrap();
        assert_eq!((photos.files, photos.bytes, photos.included), (2, 300, true));
    }

    #[test]
    fn an_excluded_folder_still_appears_in_its_own_picker() {
        // It has no files in the filtered tree, so without this it would
        // disappear the moment it was switched off and could never be
        // switched back on.
        let excluded: BTreeSet<String> = ["Photos".to_string()].into_iter().collect();
        let choices = folder_choices(Vec::new(), &excluded);
        assert_eq!(choices.len(), 1);
        assert!(!choices[0].included);
        assert_eq!(choices[0].files, 0);
    }
}
