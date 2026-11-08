//! File tags, shared with the web Files app.
//!
//! JMAP's FileNode has no keywords property and the MeCloud server keeps no
//! state of its own, so tags live in the drive itself: one JSON file,
//! `.mecloud/tags.json` at the top level, keyed by FileNode id. A rename or a
//! move keeps a file's id, so it keeps its tags; the web app reads the same
//! file and is where tags are searched and edited afterwards.
//!
//! ```json
//! { "version": 1, "tags": { "<node id>": ["Tax", "Insurance 2026"] } }
//! ```
//!
//! The rules here are the web app's (`frontend/src/lib/tags.js`) and must stay
//! identical: the same tag typed on either side has to be the same tag.
//!
//! Writing is read-modify-write under JMAP's `ifInState`, so two clients
//! tagging at once cannot silently undo each other: the second write is
//! refused, re-reads, and applies its change on top. Unknown top-level keys are
//! carried through untouched, an index from a newer format is never written,
//! and one this client cannot read is never replaced.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value};

use crate::api::{self, Api, Created, Node};

pub const DIR: &str = ".mecloud";
pub const FILE: &str = "tags.json";
pub const VERSION: u64 = 1;
pub const MAX_TAG_CHARS: usize = 40;
pub const MAX_TAGS_PER_FILE: usize = 20;
const ATTEMPTS: usize = 5;
/// The index is small; anything this size is not one this client wrote.
const MAX_INDEX_BYTES: usize = 8 * 1024 * 1024;

/// One tag, tidied, or None if it cannot be a tag.
///
/// Leading `#` is dropped (people type hashtags), runs of whitespace become
/// one space, and a comma is refused because it is what separates tags in
/// every input box. Length is counted in characters, not bytes.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('#').trim();
    let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty()
        || collapsed.chars().count() > MAX_TAG_CHARS
        || collapsed.contains(',')
        || collapsed.chars().any(char::is_control)
    {
        return None;
    }
    Some(collapsed)
}

/// Normalise a list, drop duplicates (case-insensitively, keeping the first),
/// adopt the spelling of any tag already in use, and cap the count.
///
/// Adopting the existing spelling is what stops "tax" and "Tax" becoming two
/// tags that each find half the files.
pub fn canonicalize<'a>(tags: impl IntoIterator<Item = &'a str>, known: &[String]) -> Vec<String> {
    let known_by_key: BTreeMap<String, &String> = known.iter().map(|k| (k.to_lowercase(), k)).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in tags {
        let Some(tag) = normalize_tag(raw) else { continue };
        let key = tag.to_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        out.push(known_by_key.get(&key).map(|k| (*k).clone()).unwrap_or(tag));
        if out.len() == MAX_TAGS_PER_FILE {
            break;
        }
    }
    out
}

/// Split what someone typed into a tags box: commas and newlines separate.
pub fn split_input(input: &str) -> Vec<String> {
    input.split([',', '\n']).filter_map(normalize_tag).collect()
}

/// The parsed index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Index {
    pub version: u64,
    pub tags: BTreeMap<String, Vec<String>>,
    /// Top-level keys this client does not know, written back as found.
    pub extra: Map<String, Value>,
}

impl Index {
    pub fn empty() -> Self {
        Self { version: VERSION, ..Default::default() }
    }

    /// Every tag in use, each spelling once.
    pub fn all_tags(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for tags in self.tags.values() {
            for tag in tags {
                if seen.insert(tag.to_lowercase()) {
                    out.push(tag.clone());
                }
            }
        }
        out.sort_by_key(|t| t.to_lowercase());
        out
    }

    /// Tags with how many files carry each, most used first.
    pub fn counts(&self) -> Vec<(String, usize)> {
        let mut by_key: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for tags in self.tags.values() {
            for tag in tags {
                by_key.entry(tag.to_lowercase()).or_insert_with(|| (tag.clone(), 0)).1 += 1;
            }
        }
        let mut out: Vec<(String, usize)> = by_key.into_values().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        out
    }

    /// Set a file's tags outright. An empty list removes its entry.
    pub fn set(&mut self, node_id: &str, tags: &[String]) -> Vec<String> {
        let known = self.all_tags();
        let canonical = canonicalize(tags.iter().map(String::as_str), &known);
        if canonical.is_empty() {
            self.tags.remove(node_id);
        } else {
            self.tags.insert(node_id.to_string(), canonical.clone());
        }
        canonical
    }

    /// Drop entries for files that no longer exist. Only ever called with a
    /// complete listing: from a partial one this would delete live tags.
    pub fn prune(&mut self, live: &HashSet<&str>) {
        self.tags.retain(|id, _| live.contains(id.as_str()));
    }
}

/// Why an index cannot be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unwritable {
    Newer(u64),
    Unreadable,
}

/// Read the index, saying whether it may be written back.
pub fn parse(bytes: &[u8]) -> Result<Index, (Unwritable, Index)> {
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(bytes) else {
        return Err((Unwritable::Unreadable, Index::empty()));
    };
    let version = match map.remove("version") {
        None => VERSION,
        Some(v) => match v.as_u64() {
            Some(n) => n,
            None => return Err((Unwritable::Unreadable, Index::empty())),
        },
    };
    let mut index = Index { version, ..Default::default() };
    match map.remove("tags") {
        None => {}
        Some(Value::Object(entries)) => {
            for (id, value) in entries {
                // A value of the wrong shape is skipped, not fatal: one odd
                // entry should not hide every other file's tags.
                let Some(list) = value.as_array() else { continue };
                let tags: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if !tags.is_empty() {
                    index.tags.insert(id, tags);
                }
            }
        }
        Some(_) => return Err((Unwritable::Unreadable, Index::empty())),
    }
    index.extra = map;
    if version > VERSION {
        return Err((Unwritable::Newer(version), index));
    }
    Ok(index)
}

/// The index as written: two-space indent, sorted ids, trailing newline, so a
/// diff of it over WebDAV stays readable.
pub fn serialize(index: &Index) -> Vec<u8> {
    let mut map = index.extra.clone();
    map.insert("version".into(), Value::from(VERSION));
    let tags: Map<String, Value> = index
        .tags
        .iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(id, t)| (id.clone(), Value::from(t.clone())))
        .collect();
    map.insert("tags".into(), Value::Object(tags));
    let mut out = serde_json::to_vec_pretty(&Value::Object(map)).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Where the index lives, if it exists yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    pub folder_id: Option<String>,
    pub file_id: Option<String>,
}

/// Find `.mecloud/tags.json` in a listing.
pub fn locate(nodes: &[Node]) -> Location {
    let folder_id = nodes
        .iter()
        .filter(|n| n.is_folder && n.parent_id.is_none() && n.name == DIR)
        .map(|n| n.id.clone())
        .min();
    let file_id = folder_id.as_ref().and_then(|dir| {
        nodes
            .iter()
            .filter(|n| !n.is_folder && n.parent_id.as_deref() == Some(dir) && n.name == FILE)
            .map(|n| n.id.clone())
            .min()
    });
    Location { folder_id, file_id }
}

/// What tagging needs from the server. A trait so the retry logic can be
/// driven by a fake that refuses on cue.
pub trait TagRemote {
    fn listing(&self, account_id: &str) -> Result<Vec<Node>, String>;
    /// The FileNode state and the blob of `node`, read together.
    fn read_state(&self, account_id: &str, node: Option<&str>) -> Result<(String, Option<String>), String>;
    fn fetch(&self, blob_id: &str) -> Result<Vec<u8>, String>;
    fn store(&self, bytes: &[u8]) -> Result<(String, u64), String>;
    fn create_if(
        &self, account_id: &str, name: &str, parent: Option<&str>, file: Option<(&str, u64)>, state: &str,
    ) -> Result<Created, String>;
    fn update_if(&self, account_id: &str, node: &str, blob: &str, size: u64, state: &str) -> Result<Created, String>;
}

impl TagRemote for Api {
    fn listing(&self, account_id: &str) -> Result<Vec<Node>, String> {
        api::Remote::nodes(self, account_id)
    }
    fn read_state(&self, account_id: &str, node: Option<&str>) -> Result<(String, Option<String>), String> {
        self.node_and_state(account_id, node)
    }
    fn fetch(&self, blob_id: &str) -> Result<Vec<u8>, String> {
        self.download_bytes(blob_id, FILE, MAX_INDEX_BYTES)
    }
    fn store(&self, bytes: &[u8]) -> Result<(String, u64), String> {
        self.upload_bytes(bytes)
    }
    fn create_if(
        &self, account_id: &str, name: &str, parent: Option<&str>, file: Option<(&str, u64)>, state: &str,
    ) -> Result<Created, String> {
        Api::create_if(self, account_id, name, parent, file.map(|(b, s)| (b, s, "application/json")), state)
    }
    fn update_if(&self, account_id: &str, node: &str, blob: &str, size: u64, state: &str) -> Result<Created, String> {
        self.update_blob_if(account_id, node, blob, size, state)
    }
}

/// Read the index. A missing one is empty; an unreadable one is an error.
pub fn load(remote: &impl TagRemote, account_id: &str) -> Result<Index, String> {
    let nodes = remote.listing(account_id)?;
    let Some(file_id) = locate(&nodes).file_id else { return Ok(Index::empty()) };
    let (_, blob) = remote.read_state(account_id, Some(&file_id))?;
    let Some(blob) = blob else { return Ok(Index::empty()) };
    match parse(&remote.fetch(&blob)?) {
        Ok(index) => Ok(index),
        Err((Unwritable::Newer(_), index)) => Ok(index),
        Err((Unwritable::Unreadable, _)) => Err(UNREADABLE.into()),
    }
}

const UNREADABLE: &str = "The tag index (.mecloud/tags.json) could not be read, so tags were left alone.";

/// Give one file these tags, merged into whatever is on the server now.
/// Returns the tags as stored — canonical spellings, duplicates removed.
pub fn set_tags(remote: &impl TagRemote, account_id: &str, node_id: &str, tags: &[String]) -> Result<Vec<String>, String> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            // Someone else wrote first. A short, growing pause, so two
            // clients retrying together do not collide every time.
            std::thread::sleep(std::time::Duration::from_millis(150 * attempt as u64));
        }
        let nodes = remote.listing(account_id)?;
        let location = locate(&nodes);
        let (state, blob) = remote.read_state(account_id, location.file_id.as_deref())?;

        let mut index = match blob {
            Some(blob) if location.file_id.is_some() => match parse(&remote.fetch(&blob)?) {
                Ok(index) => index,
                Err((Unwritable::Newer(v), _)) => {
                    return Err(format!(
                        "Tags were not saved: another MeCloud app uses a newer tag format (version {v}). \
                         Update this one."
                    ))
                }
                Err((Unwritable::Unreadable, _)) => return Err(UNREADABLE.into()),
            },
            _ => Index::empty(),
        };

        let stored = index.set(node_id, tags);
        // The listing above is complete or it is an error (see api::list_all),
        // so pruning from it cannot drop the tags of a file that still exists.
        let live: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).chain([node_id]).collect();
        index.prune(&live);

        let bytes = serialize(&index);
        let (blob_id, size) = remote.store(&bytes)?;

        let outcome = match (&location.folder_id, &location.file_id) {
            (_, Some(file)) => remote.update_if(account_id, file, &blob_id, size, &state)?,
            (Some(folder), None) => remote.create_if(account_id, FILE, Some(folder), Some((&blob_id, size)), &state)?,
            (None, None) => match remote.create_if(account_id, DIR, None, None, &state)? {
                Created::Done { id: folder, new_state } => {
                    remote.create_if(account_id, FILE, Some(&folder), Some((&blob_id, size)), &new_state)?
                }
                refused => refused,
            },
        };
        match outcome {
            Created::Done { .. } => return Ok(stored),
            refused if refused.is_conflict() => {
                last = "your drive kept changing while tags were being saved".into();
            }
            Created::Refused(kind) => return Err(format!("The server would not save the tags ({kind}).")),
        }
    }
    Err(format!("Tags were not saved: {last}. Try again."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn node(id: &str, name: &str, parent: Option<&str>, folder: bool) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(str::to_string),
            blob_id: if folder { None } else { Some(format!("blob-{id}")) },
            size: 1,
            is_folder: folder,
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalising_a_tag() {
        assert_eq!(normalize_tag("  #Tax  "), Some("Tax".into()));
        assert_eq!(normalize_tag("##  home   office "), Some("home office".into()));
        assert_eq!(normalize_tag("a\tb"), Some("a b".into()));
        assert_eq!(normalize_tag(""), None);
        assert_eq!(normalize_tag("###"), None);
        assert_eq!(normalize_tag("a,b"), None);
        assert_eq!(normalize_tag("bell\u{7}"), None);
        // Forty characters, counted as characters: these are two bytes each.
        assert!(normalize_tag(&"é".repeat(40)).is_some());
        assert_eq!(normalize_tag(&"é".repeat(41)), None);
    }

    #[test]
    fn canonical_spelling_and_duplicates() {
        let known = strs(&["Tax", "Insurance"]);
        assert_eq!(canonicalize(["tax", "TAX", "new", "#insurance", "bad,tag"], &known), strs(&["Tax", "new", "Insurance"]));
        let many: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        assert_eq!(canonicalize(many.iter().map(String::as_str), &[]).len(), MAX_TAGS_PER_FILE);
    }

    #[test]
    fn splitting_typed_input() {
        assert_eq!(split_input("Tax, insurance\n#2026,, "), strs(&["Tax", "insurance", "2026"]));
    }

    #[test]
    fn round_trip_keeps_unknown_keys_and_drops_empty_entries() {
        let raw = br#"{"version":1,"tags":{"b":["X"],"a":["Y","Z"],"c":[],"d":"oops"},"colours":{"a":"red"}}"#;
        let index = parse(raw).unwrap();
        assert_eq!(index.tags.len(), 2);
        let out = String::from_utf8(serialize(&index)).unwrap();
        assert!(out.ends_with("}\n"));
        assert!(out.contains("\"colours\""));
        assert!(out.find("\"a\"").unwrap() < out.find("\"b\"").unwrap(), "ids sorted");
        assert!(out.contains("\n  \"tags\""), "two-space indent");
        assert_eq!(parse(out.as_bytes()).unwrap(), index);
    }

    #[test]
    fn a_newer_or_damaged_index_is_never_written() {
        let (why, index) = parse(br#"{"version":2,"tags":{"a":["X"]}}"#).unwrap_err();
        assert_eq!(why, Unwritable::Newer(2));
        assert_eq!(index.tags["a"], strs(&["X"]), "still readable");
        assert_eq!(parse(b"not json").unwrap_err().0, Unwritable::Unreadable);
        assert_eq!(parse(br#"{"tags":[]}"#).unwrap_err().0, Unwritable::Unreadable);
        assert_eq!(parse(br#"{"version":"1"}"#).unwrap_err().0, Unwritable::Unreadable);
        // No version at all is version 1.
        assert_eq!(parse(br#"{"tags":{}}"#).unwrap().version, 1);
    }

    #[test]
    fn counts_and_all_tags_merge_spellings() {
        let mut index = Index::empty();
        index.tags.insert("a".into(), strs(&["Tax", "Home"]));
        index.tags.insert("b".into(), strs(&["tax"]));
        assert_eq!(index.all_tags(), strs(&["Home", "Tax"]));
        assert_eq!(index.counts(), vec![("Tax".to_string(), 2), ("Home".to_string(), 1)]);
    }

    #[test]
    fn finds_the_index_only_at_the_top_level() {
        let nodes = vec![
            node("d", DIR, None, true),
            node("f", FILE, Some("d"), false),
            node("x", DIR, Some("other"), true),
            node("other", "Docs", None, true),
        ];
        assert_eq!(locate(&nodes), Location { folder_id: Some("d".into()), file_id: Some("f".into()) });
        assert_eq!(locate(&nodes[2..]), Location::default());
    }

    /// A drive in memory that can be told to refuse the next N writes as a
    /// conflict, the way a server does when someone else wrote first.
    struct Fake {
        nodes: RefCell<Vec<Node>>,
        blobs: RefCell<BTreeMap<String, Vec<u8>>>,
        state: RefCell<u64>,
        refuse_next: RefCell<usize>,
        writes: RefCell<usize>,
    }

    impl Fake {
        fn new(nodes: Vec<Node>) -> Self {
            Self {
                nodes: RefCell::new(nodes),
                blobs: RefCell::new(BTreeMap::new()),
                state: RefCell::new(1),
                refuse_next: RefCell::new(0),
                writes: RefCell::new(0),
            }
        }
        fn bump(&self) -> String {
            *self.state.borrow_mut() += 1;
            self.state.borrow().to_string()
        }
        fn index(&self) -> Index {
            let id = locate(&self.nodes.borrow()).file_id.unwrap();
            let blob = self.nodes.borrow().iter().find(|n| n.id == id).unwrap().blob_id.clone().unwrap();
            parse(&self.blobs.borrow()[&blob]).unwrap()
        }
        fn conflict(&self, state: &str) -> Option<Created> {
            if *self.refuse_next.borrow() > 0 {
                *self.refuse_next.borrow_mut() -= 1;
                // Another writer got in: the state moves on.
                self.bump();
                return Some(Created::Refused("stateMismatch".into()));
            }
            if state != self.state.borrow().to_string() {
                return Some(Created::Refused("stateMismatch".into()));
            }
            None
        }
    }

    impl TagRemote for Fake {
        fn listing(&self, _: &str) -> Result<Vec<Node>, String> {
            Ok(self.nodes.borrow().clone())
        }
        fn read_state(&self, _: &str, node: Option<&str>) -> Result<(String, Option<String>), String> {
            let blob = node.and_then(|id| self.nodes.borrow().iter().find(|n| n.id == id).and_then(|n| n.blob_id.clone()));
            Ok((self.state.borrow().to_string(), blob))
        }
        fn fetch(&self, blob: &str) -> Result<Vec<u8>, String> {
            self.blobs.borrow().get(blob).cloned().ok_or("no blob".into())
        }
        fn store(&self, bytes: &[u8]) -> Result<(String, u64), String> {
            let id = format!("up{}", self.blobs.borrow().len());
            self.blobs.borrow_mut().insert(id.clone(), bytes.to_vec());
            Ok((id, bytes.len() as u64))
        }
        fn create_if(
            &self, _: &str, name: &str, parent: Option<&str>, file: Option<(&str, u64)>, state: &str,
        ) -> Result<Created, String> {
            if let Some(refusal) = self.conflict(state) {
                return Ok(refusal);
            }
            *self.writes.borrow_mut() += 1;
            let id = format!("n{}", self.nodes.borrow().len());
            self.nodes.borrow_mut().push(Node {
                id: id.clone(),
                name: name.into(),
                parent_id: parent.map(str::to_string),
                blob_id: file.map(|(b, _)| b.to_string()),
                size: file.map(|(_, s)| s).unwrap_or(0),
                is_folder: file.is_none(),
            });
            Ok(Created::Done { id, new_state: self.bump() })
        }
        fn update_if(&self, _: &str, node: &str, blob: &str, _: u64, state: &str) -> Result<Created, String> {
            if let Some(refusal) = self.conflict(state) {
                return Ok(refusal);
            }
            *self.writes.borrow_mut() += 1;
            for n in self.nodes.borrow_mut().iter_mut() {
                if n.id == node {
                    n.blob_id = Some(blob.into());
                }
            }
            Ok(Created::Done { id: node.into(), new_state: self.bump() })
        }
    }

    #[test]
    fn the_first_tag_creates_the_folder_and_the_index() {
        let fake = Fake::new(vec![node("scan", "Scan.pdf", None, false)]);
        let stored = set_tags(&fake, "acc", "scan", &strs(&["#Tax", "tax", "Home"])).unwrap();
        assert_eq!(stored, strs(&["Tax", "Home"]));
        assert_eq!(fake.index().tags["scan"], stored);
        let loc = locate(&fake.nodes.borrow());
        assert!(loc.folder_id.is_some() && loc.file_id.is_some());
    }

    #[test]
    fn later_tags_join_existing_spellings_and_keep_other_files() {
        let fake = Fake::new(vec![node("a", "a.pdf", None, false), node("b", "b.pdf", None, false)]);
        set_tags(&fake, "acc", "a", &strs(&["Tax"])).unwrap();
        let stored = set_tags(&fake, "acc", "b", &strs(&["TAX", "Car"])).unwrap();
        assert_eq!(stored, strs(&["Tax", "Car"]));
        let index = fake.index();
        assert_eq!(index.tags["a"], strs(&["Tax"]));
        assert_eq!(index.tags["b"], strs(&["Tax", "Car"]));
    }

    #[test]
    fn a_conflicting_write_is_retried_on_top_of_the_new_state() {
        let fake = Fake::new(vec![node("a", "a.pdf", None, false)]);
        set_tags(&fake, "acc", "a", &strs(&["One"])).unwrap();
        *fake.refuse_next.borrow_mut() = 2;
        set_tags(&fake, "acc", "a", &strs(&["Two"])).unwrap();
        assert_eq!(fake.index().tags["a"], strs(&["Two"]));
    }

    #[test]
    fn gives_up_after_repeated_conflicts_without_writing() {
        let fake = Fake::new(vec![node("a", "a.pdf", None, false)]);
        set_tags(&fake, "acc", "a", &strs(&["One"])).unwrap();
        let writes = *fake.writes.borrow();
        *fake.refuse_next.borrow_mut() = ATTEMPTS;
        assert!(set_tags(&fake, "acc", "a", &strs(&["Two"])).is_err());
        assert_eq!(*fake.writes.borrow(), writes);
        assert_eq!(fake.index().tags["a"], strs(&["One"]));
    }

    #[test]
    fn tags_of_deleted_files_are_pruned_but_never_the_file_being_tagged() {
        let fake = Fake::new(vec![node("a", "a.pdf", None, false)]);
        set_tags(&fake, "acc", "a", &strs(&["One"])).unwrap();
        fake.nodes.borrow_mut().retain(|n| n.id != "a");
        // "new" is not in the listing yet (a node created a moment ago on a
        // lagging server must not lose its tags to the prune).
        set_tags(&fake, "acc", "new", &strs(&["Two"])).unwrap();
        let index = fake.index();
        assert!(!index.tags.contains_key("a"));
        assert_eq!(index.tags["new"], strs(&["Two"]));
    }

    #[test]
    fn an_empty_list_removes_the_entry() {
        let fake = Fake::new(vec![node("a", "a.pdf", None, false)]);
        set_tags(&fake, "acc", "a", &strs(&["One"])).unwrap();
        set_tags(&fake, "acc", "a", &[]).unwrap();
        assert!(fake.index().tags.is_empty());
    }

    #[test]
    fn a_newer_index_is_left_alone() {
        let fake = Fake::new(vec![node("d", DIR, None, true), node("f", FILE, Some("d"), false), node("a", "a", None, false)]);
        fake.blobs.borrow_mut().insert("blob-f".into(), br#"{"version":9,"tags":{}}"#.to_vec());
        let error = set_tags(&fake, "acc", "a", &strs(&["X"])).unwrap_err();
        assert!(error.contains("newer"), "{error}");
        assert_eq!(*fake.writes.borrow(), 0);
        // Reading it still works.
        assert!(load(&fake, "acc").unwrap().tags.is_empty());
    }
}
