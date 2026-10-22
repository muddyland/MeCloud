//! Talking to the MeCloud server as a paired device.
//!
//! Deliberately goes through the server's `/api/jmap` and `/api/files/blob`
//! endpoints — the same ones the web UI uses — rather than at the mail server
//! directly. The client therefore never holds a mail-server credential, never
//! needs to know where the mail server is, and inherits the proxy's limits and
//! validation for free. The device token is the only secret it carries.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::encode;
use crate::reconcile::{Entry, Tree};

/// What the sync engine needs from the server.
///
/// A trait so the engine can be driven by a fake in tests: everything that
/// decides whether a file gets deleted runs through here, and that is exactly
/// the code that must be exercised without a live server.
pub trait Remote {
    fn nodes(&self, account_id: &str) -> Result<Vec<Node>, String>;
    fn upload_blob(&self, path: &Path) -> Result<(String, u64), String>;
    fn download_blob(&self, blob_id: &str, name: &str, to: &Path) -> Result<(), String>;
    fn create_folder(&self, account_id: &str, name: &str, parent: Option<&str>) -> Result<String, String>;
    fn create_file(
        &self, account_id: &str, name: &str, parent: Option<&str>, blob_id: &str, size: u64,
    ) -> Result<String, String>;
    fn update_file(&self, account_id: &str, node_id: &str, blob_id: &str, size: u64) -> Result<(), String>;
    fn node_blob(&self, account_id: &str, node_id: &str) -> Result<Option<String>, String>;
    fn destroy(&self, account_id: &str, node_id: &str) -> Result<(), String>;
}

/// One HTTP client configuration for every request the client makes.
///
/// The setup probe, the update check and the sync engine all go through this,
/// so they agree on TLS roots and on the user agent.
pub fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // The connect timeout is what catches an unreachable server quickly;
        // the overall one is generous because a file can be large.
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(concat!("MeCloud-Desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

pub struct Api {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

/// One file or folder on the server.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub blob_id: Option<String>,
    pub size: u64,
    pub is_folder: bool,
}

#[derive(Deserialize)]
struct BlobUpload {
    #[serde(rename = "blobId")]
    blob_id: String,
    #[serde(default)]
    size: Option<u64>,
}

impl Api {
    pub fn new(base: &str, token: &str) -> Result<Self, String> {
        // Generous: a sync can move large files over a slow link.
        let http = http_client(Duration::from_secs(600))?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token: token.to_string(), http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// One JMAP request, with the device token as a bearer credential.
    fn jmap(&self, using: &[&str], calls: Value) -> Result<Value, String> {
        let body = json!({ "using": using, "methodCalls": calls });
        let response = with_retry(|| {
            self.http
                .post(self.url("/api/jmap"))
                .bearer_auth(&self.token)
                .json(&body)
                .send()
                .map_err(|e| e.to_string())
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UNPAIRED.to_string());
        }
        if !status.is_success() {
            return Err(format!("The server refused the request ({status})."));
        }
        response.json().map_err(|e| format!("Unreadable response: {e}"))
    }

    pub fn account_id(&self) -> Result<String, String> {
        let response = self
            .http
            .get(self.url("/api/jmap/session"))
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("Could not reach the server: {e}"))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UNPAIRED.to_string());
        }
        let session: Value = response.json().map_err(|e| e.to_string())?;
        pick_account(&session)
    }
}

impl Remote for Api {
    /// Every node in the drive.
    ///
    /// Paged with query + a back-reference, the way the web client does it:
    /// `ids: null` is only "everything" while the account stays under the
    /// server's maxObjectsInGet, and past that some servers truncate silently
    /// rather than refusing — which would read here as "those files were
    /// deleted". See `list_all` for what counts as a complete answer.
    fn nodes(&self, account_id: &str) -> Result<Vec<Node>, String> {
        list_all(|position| {
            self.jmap(
                &[CORE, FILENODE],
                json!([
                    ["FileNode/query", {
                        "accountId": account_id, "position": position, "limit": PAGE,
                        "calculateTotal": true
                    }, "q"],
                    ["FileNode/get", {
                        "accountId": account_id,
                        "#ids": { "resultOf": "q", "name": "FileNode/query", "path": "/ids" }
                    }, "g"]
                ]),
            )
        })
    }

    /// Streamed from disk, never read into memory: the sync folder is where
    /// the large files live. Re-opened for every attempt, because a retried
    /// request needs the body from the start again.
    fn upload_blob(&self, path: &Path) -> Result<(String, u64), String> {
        let response = with_retry(|| {
            let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let len = file.metadata().map(|m| m.len()).map_err(|e| format!("{}: {e}", path.display()))?;
            self.http
                .post(self.url("/api/files/blob"))
                .bearer_auth(&self.token)
                .header("Content-Type", "application/octet-stream")
                .body(reqwest::blocking::Body::sized(file, len))
                .send()
                .map_err(|e| e.to_string())
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UNPAIRED.to_string());
        }
        if !status.is_success() {
            return Err(format!("Upload refused ({status})."));
        }
        let blob: BlobUpload = response.json().map_err(|e| e.to_string())?;
        Ok((blob.blob_id, blob.size.unwrap_or(0)))
    }

    fn download_blob(&self, blob_id: &str, name: &str, to: &Path) -> Result<(), String> {
        let url = format!(
            "{}/api/files/blob/{}?name={}",
            self.base,
            encode::component(blob_id),
            encode::component(name)
        );
        let mut response =
            with_retry(|| self.http.get(&url).bearer_auth(&self.token).send().map_err(|e| e.to_string()))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UNPAIRED.to_string());
        }
        if !response.status().is_success() {
            return Err(format!("Download refused ({}).", response.status()));
        }

        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written beside the target and renamed into place, so an interrupted
        // download never leaves a half file under the real name. The scratch
        // name *appends* a suffix the scanner ignores: replacing the extension
        // made "a.txt" and "a.pdf" share one scratch file.
        let temp = scratch_path(to, PART_SUFFIX);
        let _ = std::fs::remove_file(&temp);
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            if let Err(e) = response.copy_to(&mut file) {
                drop(file);
                let _ = std::fs::remove_file(&temp);
                return Err(e.to_string());
            }
        }
        std::fs::rename(&temp, to).map_err(|e| e.to_string())
    }

    fn create_folder(&self, account_id: &str, name: &str, parent: Option<&str>) -> Result<String, String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id,
                "create": { "f": { "name": name, "parentId": parent } }
            }, "s"]]),
        )?;
        created_id(&data, "f").ok_or_else(|| format!("The server would not create the folder \"{name}\"."))
    }

    fn create_file(
        &self, account_id: &str, name: &str, parent: Option<&str>, blob_id: &str, size: u64,
    ) -> Result<String, String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id,
                "create": { "n": {
                    "name": name, "parentId": parent, "blobId": blob_id,
                    "type": "application/octet-stream", "size": size
                } }
            }, "s"]]),
        )?;
        created_id(&data, "n").ok_or_else(|| format!("The server would not save \"{name}\"."))
    }

    fn update_file(&self, account_id: &str, node_id: &str, blob_id: &str, size: u64) -> Result<(), String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id,
                "update": { node_id: { "blobId": blob_id, "size": size } }
            }, "s"]]),
        )?;
        if not_updated(&data) {
            return Err("The server would not update that file.".into());
        }
        Ok(())
    }

    /// The blob id the server currently reports for a node.
    ///
    /// Needed after an upload: the id returned by the blob endpoint is not
    /// necessarily the one `FileNode/get` reports for the node afterwards, and
    /// the remote version marker has to be the value the *next* listing will
    /// produce. Recording the upload's id instead makes the following pass see
    /// a change that did not happen and download the file straight back.
    fn node_blob(&self, account_id: &str, node_id: &str) -> Result<Option<String>, String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/get", {
                "accountId": account_id, "ids": [node_id], "properties": ["id", "blobId"]
            }, "g"]]),
        )?;
        Ok(data
            .get("methodResponses")
            .and_then(Value::as_array)
            .and_then(|r| r.first())
            .and_then(|r| r.get(1))
            .and_then(|r| r.get("list"))
            .and_then(Value::as_array)
            .and_then(|l| l.first())
            .and_then(|n| n.get("blobId"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    fn destroy(&self, account_id: &str, node_id: &str) -> Result<(), String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id, "destroy": [node_id], "onDestroyRemoveChildren": true
            }, "s"]]),
        )?;
        // A refusal used to be read as success: the pending row was cleared,
        // the file stayed on the server, and the next pass downloaded it back.
        let responses = data.get("methodResponses").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(problem) = method_error(&responses) {
            return Err(format!("The server would not delete that file: {problem}"));
        }
        let refused = responses
            .first()
            .and_then(|r| r.get(1))
            .and_then(|r| r.get("notDestroyed"))
            .and_then(Value::as_object)
            .is_some_and(|o| o.contains_key(node_id));
        if refused {
            return Err("The server would not delete that file.".into());
        }
        Ok(())
    }
}

pub const UNPAIRED: &str = "This computer is no longer connected to your account.";

/// A first sync of a real drive is hundreds of requests in a few seconds, and
/// the server rate-limits per IP. Being told to slow down is an expected part
/// of normal use here, not an error — the same reasoning the web client's
/// upload path already applies.
const MAX_ATTEMPTS: u32 = 5;

/// Ceiling for backoff we invent ourselves, with no advice from the server.
const MAX_BACKOFF_SECS: u64 = 30;

/// Ceiling for a wait the server explicitly asked for.
///
/// Much higher than our own backoff, and that distinction matters: the server
/// answers a rate limit with `Retry-After: 60`, and capping its advice at 30
/// meant every retry landed inside the still-closed window. All five attempts
/// were spent without one of them being able to succeed, so a first sync of a
/// few hundred files reported almost all of them as failures — then tried the
/// whole thing again thirty seconds later, forever.
const MAX_ADVISED_WAIT_SECS: u64 = 300;

/// How long to wait after a 429.
///
/// The server's `Retry-After` wins when it sends one, because it knows when the
/// window actually reopens. Otherwise exponential backoff, capped, with a
/// little jitter so a folder full of files does not resume in lockstep and trip
/// the limit again on the same tick.
pub fn retry_delay(attempt: u32, retry_after: Option<&str>) -> Duration {
    if let Some(advised) = retry_after.and_then(|v| v.trim().parse::<u64>().ok()) {
        // Waiting exactly as long as asked is the only wait that works: the
        // window does not reopen early because we were impatient.
        return Duration::from_secs(advised.min(MAX_ADVISED_WAIT_SECS));
    }
    let backoff = 2u64.saturating_pow(attempt).min(MAX_BACKOFF_SECS);
    // Jitter from the clock: no rng dependency for something this rough.
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u64 % 500)
        .unwrap_or(0);
    Duration::from_millis(backoff * 1000 + jitter)
}

/// True for a status worth trying again rather than reporting.
fn is_transient(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Run a request, waiting out rate limits.
fn with_retry<F>(mut send: F) -> Result<reqwest::blocking::Response, String>
where
    F: FnMut() -> Result<reqwest::blocking::Response, String>,
{
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match send() {
            Ok(response) => {
                let status = response.status();
                if !is_transient(status) || attempt + 1 == MAX_ATTEMPTS {
                    return Ok(response);
                }
                let advised = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                last = format!("the server is busy ({status})");
                std::thread::sleep(retry_delay(attempt, advised.as_deref()));
            }
            Err(e) => {
                last = e;
                if attempt + 1 == MAX_ATTEMPTS {
                    return Err(format!("Could not reach the server: {last}"));
                }
                std::thread::sleep(retry_delay(attempt, None));
            }
        }
    }
    Err(format!("Gave up after {MAX_ATTEMPTS} attempts: {last}"))
}
const CORE: &str = "urn:ietf:params:jmap:core";
const FILENODE: &str = "urn:ietf:params:jmap:filenode";

/// The first method-level error in a JMAP response, if any.
///
/// A failed call comes back as `["error", {"type": "..."}, "callId"]` rather
/// than as a transport failure, so nothing above this notices unless it looks.
fn method_error(responses: &[Value]) -> Option<String> {
    for response in responses {
        let Some(entry) = response.as_array() else { continue };
        if entry.first().and_then(Value::as_str) == Some("error") {
            let kind = entry
                .get(1)
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Some(kind.to_string());
        }
    }
    None
}

fn created_id(data: &Value, key: &str) -> Option<String> {
    data.get("methodResponses")?
        .as_array()?
        .first()?
        .get(1)?
        .get("created")?
        .get(key)?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

fn not_updated(data: &Value) -> bool {
    data.get("methodResponses")
        .and_then(Value::as_array)
        .and_then(|r| r.first())
        .and_then(|r| r.get(1))
        .and_then(|r| r.get("notUpdated"))
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty())
}

/// The suffix a download is written under until it is complete.
pub const PART_SUFFIX: &str = "mecloud-part";
/// The suffix a "same file?" comparison downloads under.
pub const VERIFY_SUFFIX: &str = "mecloud-verify";

/// A sibling of `path` named `<file name>.<suffix>`.
pub fn scratch_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{suffix}"))
}

/// Nodes per page.
const PAGE: i64 = 256;

/// Pages before giving up: about a million nodes. Reaching it is an error, not
/// the end of the listing — the old cap of 200 pages returned the first 51,200
/// files as if they were all of them, and everything after looked deleted.
const MAX_PAGES: usize = 4096;

/// The account to sync with.
///
/// The server's declared primary file-storage account if there is one. If not,
/// the only account that has file storage. Anything else — no account, or
/// several and no primary — is refused rather than guessed: syncing a folder
/// into somebody's shared account is not a recoverable mistake.
pub fn pick_account(session: &Value) -> Result<String, String> {
    if let Some(id) = session
        .get("primaryAccounts")
        .and_then(|a| a.get(FILENODE))
        .and_then(Value::as_str)
    {
        return Ok(id.to_string());
    }
    let with_files: Vec<&String> = session
        .get("accounts")
        .and_then(Value::as_object)
        .map(|accounts| {
            accounts
                .iter()
                .filter(|(_, a)| {
                    a.get("accountCapabilities")
                        .and_then(Value::as_object)
                        .is_some_and(|c| c.contains_key(FILENODE))
                })
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default();
    match with_files.as_slice() {
        [only] => Ok((*only).clone()),
        [] => Err("This account has no file storage.".into()),
        _ => Err("The server offers several file storage accounts and does not say which is yours, \
                  so nothing will be synced rather than guessing."
            .into()),
    }
}

/// What one page of a listing said.
#[derive(Debug, Default)]
struct Page {
    ids: usize,
    nodes: Vec<Node>,
    not_found: usize,
    query_state: Option<String>,
    total: Option<u64>,
}

/// Read one query+get response, refusing anything short of a complete page.
///
/// Every refusal here is the difference between "the server failed to tell us"
/// and "these files were deleted", and only the second may lead to a deletion.
fn read_page(data: &Value) -> Result<Page, String> {
    let responses = data.get("methodResponses").and_then(Value::as_array).cloned().unwrap_or_default();

    // JMAP reports a failed call by replacing the response name with "error"
    // (RFC 8620 section 3.6.2). An error has no `ids`, and reading that as an
    // empty drive is the bug that once planned to delete every local copy.
    if let Some(problem) = method_error(&responses) {
        return Err(format!("The server could not list your files: {problem}"));
    }
    const UNREADABLE: &str = "The server returned a file listing this client could not read.";

    let query = responses.first().and_then(|r| r.get(1)).ok_or(UNREADABLE)?;
    let ids = query.get("ids").and_then(Value::as_array).ok_or(UNREADABLE)?;
    let mut page = Page {
        ids: ids.len(),
        query_state: query.get("queryState").and_then(Value::as_str).map(str::to_string),
        total: query.get("total").and_then(Value::as_u64),
        ..Default::default()
    };
    if ids.is_empty() {
        return Ok(page);
    }

    let get = responses.get(1).and_then(|r| r.get(1)).ok_or(UNREADABLE)?;
    let list = get.get("list").and_then(Value::as_array).ok_or(UNREADABLE)?;
    page.not_found = get.get("notFound").and_then(Value::as_array).map(Vec::len).unwrap_or(0);

    for item in list {
        let id = item.get("id").and_then(Value::as_str).ok_or(UNREADABLE)?;
        let blob_id = item.get("blobId").and_then(Value::as_str).map(str::to_string);
        page.nodes.push(Node {
            id: id.to_string(),
            name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            parent_id: item.get("parentId").and_then(Value::as_str).map(str::to_string),
            is_folder: blob_id.is_none(),
            blob_id,
            size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
        });
    }

    // `notFound` is a node deleted between the query and the get, which is a
    // real deletion. Anything else missing is the server not telling us.
    if page.nodes.len() + page.not_found < page.ids {
        return Err(format!(
            "The server listed {} files but described only {}; the listing is incomplete.",
            page.ids,
            page.nodes.len() + page.not_found
        ));
    }
    Ok(page)
}

/// Page through a whole listing, or fail. Never returns a partial answer.
///
/// `fetch(position)` runs one query+get. Kept apart from the HTTP so the rules
/// about what counts as complete can be tested on canned responses.
pub(crate) fn list_all<F>(mut fetch: F) -> Result<Vec<Node>, String>
where
    F: FnMut(i64) -> Result<Value, String>,
{
    const CHANGED: &str = "Your drive changed while it was being listed; trying again next pass.";
    let mut out: Vec<Node> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut position = 0i64;
    let mut not_found = 0usize;
    let mut first_state: Option<String> = None;
    let mut total: Option<u64> = None;
    let mut finished = false;

    for _ in 0..MAX_PAGES {
        let page = read_page(&fetch(position)?)?;

        // Paging by position over a list that changes between pages skips or
        // repeats entries, and a skipped file looks deleted. The query state
        // is the server's own word on whether the list moved.
        if let Some(state) = &page.query_state {
            match &first_state {
                None => first_state = Some(state.clone()),
                Some(first) if first != state => return Err(CHANGED.into()),
                _ => {}
            }
        }
        if total.is_none() {
            total = page.total;
        }

        if page.ids == 0 {
            finished = true;
            break;
        }
        for node in page.nodes {
            // A server that ignores `position` hands back page one forever.
            if !seen.insert(node.id.clone()) {
                return Err("The server repeated part of the file listing; it cannot be trusted.".into());
            }
            out.push(node);
        }
        not_found += page.not_found;
        position += page.ids as i64;

        if total.is_some_and(|t| position as u64 >= t) {
            finished = true;
            break;
        }
    }

    if !finished {
        return Err("This drive has more files than the client can list in one pass.".into());
    }
    if let Some(expected) = total {
        if (out.len() + not_found) as u64 != expected {
            return Err(format!(
                "The server said the drive holds {expected} items but listed {}; the listing is incomplete.",
                out.len() + not_found
            ));
        }
    }
    Ok(out)
}

/// The server's tree, keyed by path.
#[derive(Debug, Default)]
pub struct Listing {
    /// Files: path → version and size.
    pub files: Tree,
    /// Files: path → node id.
    pub file_ids: BTreeMap<String, String>,
    /// Folders: path → node id.
    pub folder_ids: BTreeMap<String, String>,
    /// Paths more than one node claims. The first by id is kept; the rest are
    /// reported, never silently dropped.
    pub duplicates: Vec<String>,
}

/// Build paths from a flat node list. The one parent walk in the crate.
///
/// The server stores a tree by parent id; the reconciler compares paths. A
/// node whose parent is missing, a loop, or a name that is not a single path
/// component makes the whole listing an error. Dropping such a node used to
/// make it, and everything under it, look deleted on the server.
pub fn to_tree(nodes: &[Node]) -> Result<Listing, String> {
    let by_id: BTreeMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut sorted: Vec<&Node> = nodes.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut listing = Listing::default();

    for node in sorted {
        let mut parts = Vec::new();
        let mut cursor = Some(node);
        while let Some(current) = cursor {
            if parts.len() > 256 {
                return Err(format!("The server's folders loop back on themselves near \"{}\".", node.name));
            }
            let name = current.name.as_str();
            if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
                return Err(format!("The server has an item with an unusable name: \"{name}\"."));
            }
            parts.push(name);
            cursor = match current.parent_id.as_deref() {
                None => None,
                Some(id) => Some(*by_id.get(id).ok_or_else(|| {
                    format!(
                        "\"{}\" is in a folder the server did not list; the listing is incomplete.",
                        node.name
                    )
                })?),
            };
        }
        parts.reverse();
        let path = parts.join("/");

        if listing.files.contains_key(&path) || listing.folder_ids.contains_key(&path) {
            listing.duplicates.push(path);
            continue;
        }
        if node.is_folder {
            listing.folder_ids.insert(path, node.id.clone());
        } else {
            // The blob id is the remote version marker: JMAP exposes no content
            // hash, and a node's blob id changes exactly when its contents do.
            let version = node.blob_id.clone().unwrap_or_default();
            listing.files.insert(path.clone(), Entry { version, size: node.size });
            listing.file_ids.insert(path, node.id.clone());
        }
    }
    Ok(listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, parent: Option<&str>) -> Node {
        Node { id: id.into(), name: name.into(), parent_id: parent.map(Into::into),
               blob_id: None, size: 0, is_folder: true }
    }
    fn file(id: &str, name: &str, parent: Option<&str>, size: u64) -> Node {
        Node { id: id.into(), name: name.into(), parent_id: parent.map(Into::into),
               blob_id: Some(format!("blob-{id}")), size, is_folder: false }
    }

    #[test]
    fn builds_paths_from_parent_ids() {
        let nodes = vec![
            folder("f1", "Photos", None),
            folder("f2", "2026", Some("f1")),
            file("n1", "a.png", Some("f2"), 10),
            file("n2", "root.txt", None, 3),
        ];
        let listing = to_tree(&nodes).unwrap();
        assert!(listing.files.contains_key("Photos/2026/a.png"));
        assert!(listing.files.contains_key("root.txt"));
        assert_eq!(listing.file_ids["Photos/2026/a.png"], "n1");
        assert_eq!(listing.folder_ids["Photos"], "f1");
        assert_eq!(listing.folder_ids["Photos/2026"], "f2");
    }

    #[test]
    fn folders_are_not_file_entries() {
        let listing = to_tree(&[folder("f1", "Photos", None)]).unwrap();
        assert!(listing.files.is_empty());
        assert_eq!(listing.folder_ids.len(), 1);
    }

    #[test]
    fn a_node_with_a_missing_parent_fails_the_listing() {
        // Dropping it made the file, and everything else in that folder, look
        // deleted on the server; re-homing it put it somewhere never created.
        assert!(to_tree(&[file("n1", "orphan.txt", Some("gone"), 1)]).is_err());
        assert!(to_tree(&[folder("f9", "Orphan", Some("gone"))]).is_err());
    }

    #[test]
    fn a_parent_cycle_fails_rather_than_hangs() {
        let nodes = vec![
            folder("a", "a", Some("b")),
            folder("b", "b", Some("a")),
            file("n1", "x.txt", Some("a"), 1),
        ];
        assert!(to_tree(&nodes).is_err());
    }

    #[test]
    fn a_name_that_is_not_one_path_component_fails_the_listing() {
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(to_tree(&[file("n1", bad, None, 1)]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn two_nodes_claiming_one_path_are_reported() {
        let listing = to_tree(&[file("n1", "a.txt", None, 1), file("n2", "a.txt", None, 2)]).unwrap();
        assert_eq!(listing.file_ids["a.txt"], "n1");
        assert_eq!(listing.duplicates, vec!["a.txt".to_string()]);
    }

    // ── list_all ────────────────────────────────────────────────────────

    fn page(ids: &[&str], listed: &[&str], total: Option<u64>, state: &str) -> Value {
        let list: Vec<Value> = listed
            .iter()
            .map(|id| json!({ "id": id, "name": format!("{id}.txt"), "blobId": format!("b{id}"), "size": 1 }))
            .collect();
        let mut query = json!({ "ids": ids, "queryState": state });
        if let Some(t) = total {
            query["total"] = json!(t);
        }
        json!({ "methodResponses": [
            ["FileNode/query", query, "q"],
            ["FileNode/get", { "list": list, "notFound": [] }, "g"]
        ]})
    }

    #[test]
    fn a_complete_paged_listing_is_returned_in_full() {
        let pages = [page(&["a", "b"], &["a", "b"], Some(3), "s1"), page(&["c"], &["c"], Some(3), "s1")];
        let mut calls = 0;
        let nodes = list_all(|position| {
            calls += 1;
            Ok(pages[if position == 0 { 0 } else { 1 }].clone())
        })
        .unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(calls, 2);
    }

    #[test]
    fn a_short_page_is_an_error_not_a_smaller_drive() {
        // The query named two ids; the get described one. Treating that as a
        // one-file page is how a hiccup turned into local deletions.
        let result = list_all(|_| Ok(page(&["a", "b"], &["a"], Some(2), "s1")));
        assert!(result.is_err());
    }

    #[test]
    fn a_missing_list_is_an_error() {
        let result = list_all(|_| {
            Ok(json!({ "methodResponses": [
                ["FileNode/query", { "ids": ["a"] }, "q"],
                ["FileNode/get", {}, "g"]
            ]}))
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_total_that_does_not_match_is_an_error() {
        let result = list_all(|position| {
            Ok(if position == 0 { page(&["a"], &["a"], Some(5), "s1") } else { page(&[], &[], Some(5), "s1") })
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_listing_that_changes_mid_way_is_an_error() {
        let result = list_all(|position| {
            Ok(if position == 0 { page(&["a"], &["a"], None, "s1") } else { page(&["b"], &["b"], None, "s2") })
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_server_that_ignores_position_is_caught() {
        let result = list_all(|_| Ok(page(&["a"], &["a"], None, "s1")));
        assert!(result.is_err());
    }

    #[test]
    fn a_node_reported_not_found_is_a_real_deletion() {
        let mut data = page(&["a", "b"], &["a"], Some(2), "s1");
        data["methodResponses"][1][1]["notFound"] = json!(["b"]);
        let nodes = list_all(|_| Ok(data.clone())).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn an_empty_drive_is_an_empty_listing() {
        assert!(list_all(|_| Ok(page(&[], &[], Some(0), "s1"))).unwrap().is_empty());
    }

    // ── pick_account ────────────────────────────────────────────────────

    #[test]
    fn the_primary_file_account_wins() {
        let session = json!({
            "primaryAccounts": { FILENODE: "me" },
            "accounts": { "me": {}, "shared": {} }
        });
        assert_eq!(pick_account(&session).unwrap(), "me");
    }

    #[test]
    fn a_single_file_account_is_used_without_a_primary() {
        let session = json!({ "accounts": {
            "mail-only": { "accountCapabilities": { CORE: {} } },
            "files": { "accountCapabilities": { FILENODE: {} } }
        }});
        assert_eq!(pick_account(&session).unwrap(), "files");
    }

    #[test]
    fn several_file_accounts_and_no_primary_are_refused() {
        // The old fallback took whichever key came first.
        let session = json!({ "accounts": {
            "a": { "accountCapabilities": { FILENODE: {} } },
            "b": { "accountCapabilities": { FILENODE: {} } }
        }});
        assert!(pick_account(&session).is_err());
        assert!(pick_account(&json!({ "accounts": {} })).is_err());
    }

    #[test]
    fn scratch_names_append_rather_than_replace_the_extension() {
        let a = scratch_path(Path::new("/s/report.txt"), PART_SUFFIX);
        let b = scratch_path(Path::new("/s/report.pdf"), PART_SUFFIX);
        assert_eq!(a, Path::new("/s/report.txt.mecloud-part"));
        assert_ne!(a, b);
    }

    #[test]
    fn a_server_supplied_retry_after_is_honoured_in_full() {
        assert_eq!(retry_delay(0, Some("7")), Duration::from_secs(7));
        // The real case: a per-minute limit answers with 60. Shortening that
        // to our own 30-second ceiling retried into a closed window and failed
        // every attempt.
        assert_eq!(retry_delay(0, Some("60")), Duration::from_secs(60));
        assert!(retry_delay(0, Some("60")) > Duration::from_secs(MAX_BACKOFF_SECS));
        // Still bounded, so a hostile value cannot park the client for a day.
        assert_eq!(retry_delay(0, Some("99999")), Duration::from_secs(MAX_ADVISED_WAIT_SECS));
    }

    #[test]
    fn without_advice_the_wait_grows_and_is_capped() {
        let first = retry_delay(0, None).as_millis();
        let later = retry_delay(3, None).as_millis();
        assert!(later > first, "backoff should grow: {first} then {later}");
        assert!(retry_delay(20, None) <= Duration::from_millis(MAX_BACKOFF_SECS * 1000 + 500));
    }

    #[test]
    fn nonsense_retry_after_falls_back_to_backoff() {
        // An HTTP-date Retry-After is legal and not parsed here; it must not
        // become a zero-second wait.
        assert!(retry_delay(1, Some("Wed, 21 Oct 2026 07:28:00 GMT")) >= Duration::from_secs(2));
        assert!(retry_delay(1, Some("")) >= Duration::from_secs(2));
    }

    #[test]
    fn rate_limiting_and_server_errors_are_worth_retrying_but_refusals_are_not() {
        use reqwest::StatusCode;
        assert!(is_transient(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient(StatusCode::BAD_GATEWAY));
        // A 401 means the pairing is gone; retrying it just delays the truth.
        assert!(!is_transient(StatusCode::UNAUTHORIZED));
        assert!(!is_transient(StatusCode::NOT_FOUND));
    }

    #[test]
    fn a_method_error_is_recognised_rather_than_read_as_an_empty_drive() {
        // The bug that emptied an account: JMAP signals a failed call by
        // replacing the response name, and an error carries no `ids`.
        let responses = vec![serde_json::json!(["error", {"type": "accountNotFound"}, "q"])];
        assert_eq!(method_error(&responses).as_deref(), Some("accountNotFound"));

        let ok = vec![serde_json::json!(["FileNode/query", {"ids": []}, "q"])];
        assert_eq!(method_error(&ok), None);
    }

    #[test]
    fn an_error_anywhere_in_the_batch_counts() {
        let responses = vec![
            serde_json::json!(["FileNode/query", {"ids": ["a"]}, "q"]),
            serde_json::json!(["error", {"type": "requestTooLarge"}, "g"]),
        ];
        assert_eq!(method_error(&responses).as_deref(), Some("requestTooLarge"));
    }
}
