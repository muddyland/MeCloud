//! A scan, from the scanner to a PDF in the drive.
//!
//! Upload is immediate: the document goes straight to the server through the
//! same proxy the sync engine uses, into the drive folder the user picked. It
//! does not wait for, depend on, or even need the sync folder — a computer
//! that does not sync at all can still scan. Tags go into the drive's shared
//! tag index (`tags.rs`) and into the PDF's own Keywords.
//!
//! The document stays open afterwards, so **Add a page** can put another
//! sheet from the glass onto the end of it. That replaces the file's contents
//! on the server — same node, same name, same tags — rather than making a
//! second file, which is what multi-page scanning on a flatbed has to look
//! like if every scan is uploaded the moment it is made.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::api::{self, Api, Created, Remote};
use crate::scanner::{self, pdf, Scanned, Scanner, Settings};
use crate::tags;

/// What the Scan screen shows while it works and after.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Status {
    /// `idle`, `scanning`, `uploading`, `done`, `error` or `cancelled`.
    pub state: String,
    pub detail: String,
    pub pages: usize,
    pub node_id: Option<String>,
    pub file_name: Option<String>,
    pub folder_path: Option<String>,
    pub tags: Vec<String>,
    /// Whether another page can be scanned onto the end of this document.
    pub can_add_page: bool,
    /// The file is safe on the server, but something after that failed —
    /// usually the tags. Shown beside a success, not instead of one.
    pub warning: Option<String>,
}

/// The document last saved, kept so a page can be added to it.
#[derive(Debug, Clone)]
pub struct Open {
    pub node_id: String,
    pub name: String,
    pub folder_path: Option<String>,
    pub pages: Vec<pdf::Page>,
    pub tags: Vec<String>,
    pub scanner: Scanner,
    pub settings: Settings,
}

/// Scan state shared by the window, the tray and the worker thread.
#[derive(Default)]
pub struct Session {
    pub status: Mutex<Status>,
    pub cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub open: Mutex<Option<Open>>,
}

impl Session {
    pub fn status_now(&self) -> Status {
        let mut status = self.status.lock().map(|s| s.clone()).unwrap_or_default();
        if status.state.is_empty() {
            status.state = "idle".into();
        }
        status
    }

    pub fn busy(&self) -> bool {
        matches!(self.status_now().state.as_str(), "scanning" | "uploading")
    }

    pub fn set(&self, change: impl FnOnce(&mut Status)) {
        if let Ok(mut status) = self.status.lock() {
            change(&mut status);
        }
    }

    /// Claim the scanner for one job, or refuse if one is running.
    pub fn begin(&self) -> Result<Arc<AtomicBool>, String> {
        let mut slot = self.cancel.lock().unwrap();
        if slot.is_some() {
            return Err("A scan is already running.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(cancel.clone());
        Ok(cancel)
    }

    pub fn end(&self) {
        *self.cancel.lock().unwrap() = None;
    }

    pub fn cancel(&self) {
        if let Some(c) = self.cancel.lock().unwrap().as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }
}

/// What to scan, and where it goes.
#[derive(Debug, Clone)]
pub struct Job {
    pub scanner: Scanner,
    pub settings: Settings,
    pub folder_id: Option<String>,
    pub folder_path: Option<String>,
    /// As typed; empty means the default name.
    pub name: String,
    pub tags: Vec<String>,
}

/// The default name: "Scan 2026-11-08 14.31". Local time, because that is
/// the time on the clock beside the scanner; dots rather than a colon, which
/// Windows and macOS file systems refuse.
pub fn default_stem(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("Scan %Y-%m-%d %H.%M").to_string()
}

/// A file name from what the user typed: no path separators, no control
/// characters, a length any file system takes, and a `.pdf` ending.
pub fn file_name(typed: &str, fallback_stem: &str) -> String {
    let cleaned: String = typed
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { ' ' } else { c })
        .collect();
    let mut stem = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if stem.to_ascii_lowercase().ends_with(".pdf") {
        stem.truncate(stem.len() - 4);
        stem = stem.trim_end().to_string();
    }
    // "." and ".." are not names, and a leading dot would hide the file.
    let stem = stem.trim_start_matches('.').trim().to_string();
    let mut stem = if stem.is_empty() { fallback_stem.to_string() } else { stem };
    if stem.chars().count() > 200 {
        stem = stem.chars().take(200).collect::<String>().trim_end().to_string();
    }
    format!("{stem}.pdf")
}

/// `name`, or `name (2)`, `name (3)`… — the first not already in the folder.
/// Compared case-insensitively: a case-insensitive file system on another
/// computer cannot hold both "Scan.pdf" and "scan.pdf".
pub fn unique_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .expect("an unused name")
}

/// Names already used in a folder, lower-cased.
pub fn names_in(nodes: &[api::Node], folder: Option<&str>) -> HashSet<String> {
    nodes.iter().filter(|n| n.parent_id.as_deref() == folder).map(|n| n.name.to_lowercase()).collect()
}

/// A drive folder a scan can go into.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Folder {
    /// None for the top of the drive.
    pub id: Option<String>,
    /// "" for the top, otherwise "Documents/Receipts".
    pub path: String,
}

/// Every folder in the drive, the top first, the rest by path.
pub fn folders(nodes: &[api::Node]) -> Result<Vec<Folder>, String> {
    let listing = api::to_tree(nodes)?;
    let mut out = vec![Folder { id: None, path: String::new() }];
    let mut rest: Vec<Folder> = listing
        .folder_ids
        .into_iter()
        // The tag index's folder is the apps' business, not a place for scans.
        .filter(|(path, _)| path != tags::DIR && !path.starts_with(&format!("{}/", tags::DIR)))
        .map(|(path, id)| Folder { id: Some(id), path })
        .collect();
    rest.sort_by_key(|f| f.path.to_lowercase());
    out.extend(rest);
    Ok(out)
}

/// The finished document's bytes.
fn document(scanned: &Scanned, title: &str, keywords: &[String]) -> Result<Vec<u8>, String> {
    match scanned {
        Scanned::Pdf(bytes) => Ok(bytes.clone()),
        Scanned::Pages(pages) => pdf::build(
            pages,
            &pdf::Meta {
                title: title.to_string(),
                keywords: keywords.to_vec(),
                created: pdf::pdf_date(chrono::Utc::now()),
            },
        ),
    }
}

fn stem_of(name: &str) -> &str {
    name.strip_suffix(".pdf").unwrap_or(name)
}

/// The tags as the drive will spell them, read before anything is saved so
/// the PDF's Keywords and the index agree. If the index cannot be read, the
/// tags as typed: the scan must not fail over its tags.
fn canonical_tags(api: &Api, account_id: &str, typed: &[String]) -> Vec<String> {
    let known = tags::load(api, account_id).map(|i| i.all_tags()).unwrap_or_default();
    tags::canonicalize(typed.iter().map(String::as_str), &known)
}

/// What a saved scan turned out to be.
#[derive(Debug, Clone)]
pub struct Saved {
    pub open: Open,
    pub warning: Option<String>,
    /// A finished PDF from the scanner cannot be extended page by page.
    pub extendable: bool,
}

/// Scan, upload, tag.
pub fn run(api: &Api, account_id: &str, job: Job, progress: &dyn Fn(&str, &str), cancel: &AtomicBool) -> Result<Saved, String> {
    let scanned = scanner::scan(&job.scanner, job.settings, &|line| progress("scanning", line), cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled.".into());
    }

    progress("uploading", "Saving to your drive…");
    let wanted = file_name(&job.name, &default_stem(chrono::Local::now()));
    let tag_list = canonical_tags(api, account_id, &job.tags);
    let bytes = document(&scanned, stem_of(&wanted), &tag_list)?;
    let (blob_id, size) = api.upload_bytes(&bytes)?;

    // The name is checked against the folder first, and the server's own
    // refusal is the backstop for a file that appeared in the meantime.
    let nodes = api.nodes(account_id)?;
    let mut taken = names_in(&nodes, job.folder_id.as_deref());
    let mut saved: Option<(String, String)> = None;
    for _ in 0..20 {
        let name = unique_name(&wanted, &taken);
        match api.create_typed_file(account_id, &name, job.folder_id.as_deref(), &blob_id, size, "application/pdf")? {
            Created::Done { id, .. } => {
                saved = Some((id, name));
                break;
            }
            Created::Refused(kind) if kind == "alreadyExists" => {
                taken.insert(name.to_lowercase());
            }
            Created::Refused(kind) => return Err(format!("The server would not save the scan ({kind}).")),
        }
    }
    let (node_id, name) = saved.ok_or("The server would not save the scan under any name.")?;

    let mut warning = None;
    let mut stored_tags = Vec::new();
    if !tag_list.is_empty() {
        progress("uploading", "Saving tags…");
        match tags::set_tags(api, account_id, &node_id, &tag_list) {
            Ok(stored) => stored_tags = stored,
            Err(e) => warning = Some(format!("Saved, but the tags were not: {e}")),
        }
    }

    let (kept, extendable) = match scanned {
        Scanned::Pages(p) => (p, true),
        Scanned::Pdf(_) => (Vec::new(), false),
    };
    Ok(Saved {
        open: Open {
            node_id,
            name,
            folder_path: job.folder_path,
            pages: kept,
            tags: stored_tags,
            scanner: job.scanner,
            settings: job.settings,
        },
        warning,
        extendable,
    })
}

/// Scan more and put it on the end of the open document.
pub fn add_page(
    api: &Api, account_id: &str, open: &mut Open, progress: &dyn Fn(&str, &str), cancel: &AtomicBool,
) -> Result<(), String> {
    if open.pages.is_empty() {
        return Err("This document came from the scanner as a finished PDF, so pages cannot be added to it.".into());
    }
    let scanned = scanner::scan(&open.scanner, open.settings, &|line| progress("scanning", line), cancel)?;
    let Scanned::Pages(more) = scanned else {
        return Err("The scanner sent a finished PDF, which cannot be joined onto this document.".into());
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled.".into());
    }
    progress("uploading", "Saving to your drive…");
    let mut pages = open.pages.clone();
    pages.extend(more);
    let bytes = document(&Scanned::Pages(pages.clone()), stem_of(&open.name), &open.tags)?;
    let (blob_id, size) = api.upload_bytes(&bytes)?;
    api.update_file(account_id, &open.node_id, &blob_id, size)?;
    // Only once the server has it: a failed upload leaves the document as it
    // was, and the next try starts from there.
    open.pages = pages;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_lowercase()).collect()
    }

    #[test]
    fn default_names_are_local_time_without_colons() {
        let at = chrono::Local.with_ymd_and_hms(2026, 11, 8, 14, 31, 5).unwrap();
        assert_eq!(default_stem(at), "Scan 2026-11-08 14.31");
    }

    #[test]
    fn typed_names_are_made_safe() {
        assert_eq!(file_name("", "Scan X"), "Scan X.pdf");
        assert_eq!(file_name("  Gas bill  ", "S"), "Gas bill.pdf");
        assert_eq!(file_name("Gas bill.PDF", "S"), "Gas bill.pdf");
        assert_eq!(file_name("2026/11 tax\\return", "S"), "2026 11 tax return.pdf");
        assert_eq!(file_name("..", "S"), "S.pdf");
        assert_eq!(file_name(".hidden", "S"), "hidden.pdf");
        assert_eq!(file_name("tab\there", "S"), "tab here.pdf");
        assert_eq!(file_name(&"é".repeat(300), "S").chars().count(), 204);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        assert_eq!(unique_name("Scan.pdf", &taken(&[])), "Scan.pdf");
        assert_eq!(unique_name("Scan.pdf", &taken(&["scan.PDF"])), "Scan (2).pdf");
        assert_eq!(unique_name("Scan.pdf", &taken(&["Scan.pdf", "Scan (2).pdf"])), "Scan (3).pdf");
    }

    fn folder(id: &str, name: &str, parent: Option<&str>) -> api::Node {
        api::Node { id: id.into(), name: name.into(), parent_id: parent.map(str::to_string), blob_id: None, size: 0, is_folder: true }
    }

    #[test]
    fn folders_list_the_top_first_and_hide_the_tag_index() {
        let nodes = vec![
            folder("r", "Receipts", Some("d")),
            folder("d", "Documents", None),
            folder("m", tags::DIR, None),
            folder("a", "archive", None),
        ];
        let paths: Vec<String> = folders(&nodes).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["", "archive", "Documents", "Documents/Receipts"]);
    }

    #[test]
    fn names_are_per_folder() {
        let mut nodes = vec![folder("d", "Documents", None)];
        nodes.push(api::Node { id: "f".into(), name: "Scan.pdf".into(), parent_id: Some("d".into()), blob_id: Some("b".into()), size: 1, is_folder: false });
        assert!(names_in(&nodes, Some("d")).contains("scan.pdf"));
        assert!(!names_in(&nodes, None).contains("scan.pdf"));
    }

    #[test]
    fn one_scan_at_a_time() {
        let session = Session::default();
        let first = session.begin().unwrap();
        assert!(session.begin().is_err());
        session.cancel();
        assert!(first.load(Ordering::Relaxed));
        session.end();
        assert!(session.begin().is_ok());
    }
}
