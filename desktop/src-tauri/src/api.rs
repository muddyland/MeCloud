//! Talking to the MeCloud server as a paired device.
//!
//! Deliberately goes through the server's `/api/jmap` and `/api/files/blob`
//! endpoints — the same ones the web UI uses — rather than at the mail server
//! directly. The client therefore never holds a mail-server credential, never
//! needs to know where the mail server is, and inherits the proxy's limits and
//! validation for free. The device token is the only secret it carries.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::reconcile::{Entry, Tree};

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
    #[serde(default, rename = "type")]
    media_type: Option<String>,
}

impl Api {
    pub fn new(base: &str, token: &str) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            // Generous: a sync can move large files over a slow link. The
            // connect timeout is what catches an unreachable server quickly.
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(600))
            .user_agent(concat!("MeCloud-Desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token: token.to_string(), http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// One JMAP request, with the device token as a bearer credential.
    fn jmap(&self, using: &[&str], calls: Value) -> Result<Value, String> {
        let response = self
            .http
            .post(self.url("/api/jmap"))
            .bearer_auth(&self.token)
            .json(&json!({ "using": using, "methodCalls": calls }))
            .send()
            .map_err(|e| format!("Could not reach the server: {e}"))?;

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
        session
            .get("primaryAccounts")
            .and_then(|a| a.get(FILENODE))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                session
                    .get("accounts")
                    .and_then(Value::as_object)
                    .and_then(|a| a.keys().next().cloned())
            })
            .ok_or_else(|| "This account has no file storage.".to_string())
    }

    /// Every node in the drive.
    ///
    /// Paged with query + a back-reference, the way the web client does it:
    /// `ids: null` is only "everything" while the account stays under the
    /// server's maxObjectsInGet, and past that some servers truncate silently
    /// rather than refusing — which would read here as "those files were
    /// deleted".
    pub fn nodes(&self, account_id: &str) -> Result<Vec<Node>, String> {
        let mut out = Vec::new();
        let mut position = 0i64;

        for _ in 0..200 {
            let data = self.jmap(
                &[CORE, FILENODE],
                json!([
                    ["FileNode/query", { "accountId": account_id, "position": position, "limit": 256 }, "q"],
                    ["FileNode/get", {
                        "accountId": account_id,
                        "#ids": { "resultOf": "q", "name": "FileNode/query", "path": "/ids" }
                    }, "g"]
                ]),
            )?;

            let responses = data.get("methodResponses").and_then(Value::as_array).cloned().unwrap_or_default();
            let ids_len = responses
                .first()
                .and_then(|r| r.get(1))
                .and_then(|r| r.get("ids"))
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            if ids_len == 0 {
                break;
            }

            let list = responses
                .get(1)
                .and_then(|r| r.get(1))
                .and_then(|r| r.get("list"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let before = out.len();
            for item in list {
                let Some(id) = item.get("id").and_then(Value::as_str) else { continue };
                let blob_id = item.get("blobId").and_then(Value::as_str).map(str::to_string);
                out.push(Node {
                    id: id.to_string(),
                    name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    parent_id: item.get("parentId").and_then(Value::as_str).map(str::to_string),
                    is_folder: blob_id.is_none(),
                    blob_id,
                    size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                });
            }
            // A server that ignores `position` would otherwise loop forever.
            if out.len() == before {
                break;
            }
            position += ids_len as i64;
        }

        Ok(out)
    }

    pub fn upload_blob(&self, path: &Path) -> Result<(String, u64), String> {
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut body = Vec::new();
        file.read_to_end(&mut body).map_err(|e| format!("{}: {e}", path.display()))?;

        let response = self
            .http
            .post(self.url("/api/files/blob"))
            .bearer_auth(&self.token)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .send()
            .map_err(|e| format!("Upload failed: {e}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UNPAIRED.to_string());
        }
        if !status.is_success() {
            return Err(format!("Upload refused ({status})."));
        }
        let blob: BlobUpload = response.json().map_err(|e| e.to_string())?;
        let size = blob.size.unwrap_or(0);
        let _ = blob.media_type;
        Ok((blob.blob_id, size))
    }

    pub fn download_blob(&self, blob_id: &str, name: &str, to: &Path) -> Result<(), String> {
        let url = format!(
            "{}/api/files/blob/{}?name={}",
            self.base,
            urlencode(blob_id),
            urlencode(name)
        );
        let mut response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("Download failed: {e}"))?;

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
        // download never leaves a half file that the next scan would hash and
        // treat as the real thing.
        let temp = to.with_extension("mecloud-part");
        {
            let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
            response.copy_to(&mut file).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&temp, to).map_err(|e| e.to_string())
    }

    pub fn create_folder(&self, account_id: &str, name: &str, parent: Option<&str>) -> Result<String, String> {
        let data = self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id,
                "create": { "f": { "name": name, "parentId": parent } }
            }, "s"]]),
        )?;
        created_id(&data, "f").ok_or_else(|| format!("The server would not create the folder \"{name}\"."))
    }

    pub fn create_file(
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

    pub fn update_file(&self, account_id: &str, node_id: &str, blob_id: &str, size: u64) -> Result<(), String> {
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
    pub fn node_blob(&self, account_id: &str, node_id: &str) -> Result<Option<String>, String> {
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

    pub fn destroy(&self, account_id: &str, node_id: &str) -> Result<(), String> {
        self.jmap(
            &[CORE, FILENODE],
            json!([["FileNode/set", {
                "accountId": account_id, "destroy": [node_id], "onDestroyRemoveChildren": true
            }, "s"]]),
        )?;
        Ok(())
    }
}

pub const UNPAIRED: &str = "This computer is no longer connected to your account.";
const CORE: &str = "urn:ietf:params:jmap:core";
const FILENODE: &str = "urn:ietf:params:jmap:filenode";

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

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Build path → entry and path → node id from a flat node list.
///
/// The server stores a tree by parent id; the reconciler compares paths. A
/// node whose parent is missing is dropped rather than being attached to the
/// root — inventing a location for it is how files end up in the wrong folder.
pub fn to_tree(nodes: &[Node]) -> (Tree, BTreeMap<String, String>) {
    let by_id: BTreeMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut tree = Tree::new();
    let mut ids = BTreeMap::new();

    'nodes: for node in nodes {
        if node.is_folder {
            continue;
        }
        let mut parts = vec![node.name.as_str()];
        let mut cursor = node.parent_id.as_deref();
        let mut guard = 0;
        while let Some(id) = cursor {
            guard += 1;
            if guard > 64 {
                continue 'nodes;      // defensive: a cycle would hang here
            }
            let Some(parent) = by_id.get(id) else { continue 'nodes };
            parts.push(parent.name.as_str());
            cursor = parent.parent_id.as_deref();
        }
        parts.reverse();
        let path = parts.join("/");
        if path.is_empty() {
            continue;
        }
        // The blob id is the remote version marker: JMAP exposes no content
        // hash, and a node's blob id changes exactly when its contents do.
        let version = node.blob_id.clone().unwrap_or_default();
        tree.insert(path.clone(), Entry { version, size: node.size });
        ids.insert(path, node.id.clone());
    }

    (tree, ids)
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
        let (tree, ids) = to_tree(&nodes);
        assert!(tree.contains_key("Photos/2026/a.png"));
        assert!(tree.contains_key("root.txt"));
        assert_eq!(ids["Photos/2026/a.png"], "n1");
    }

    #[test]
    fn folders_are_not_entries_in_their_own_right() {
        let (tree, _) = to_tree(&[folder("f1", "Photos", None)]);
        assert!(tree.is_empty());
    }

    #[test]
    fn a_node_with_a_missing_parent_is_dropped_not_rehomed() {
        // Attaching it to the root would put the file somewhere the user never
        // created, and the next sync would happily upload it there.
        let (tree, _) = to_tree(&[file("n1", "orphan.txt", Some("gone"), 1)]);
        assert!(tree.is_empty());
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        let nodes = vec![
            Node { id: "a".into(), name: "a".into(), parent_id: Some("b".into()),
                   blob_id: None, size: 0, is_folder: true },
            Node { id: "b".into(), name: "b".into(), parent_id: Some("a".into()),
                   blob_id: None, size: 0, is_folder: true },
            file("n1", "x.txt", Some("a"), 1),
        ];
        let (tree, _) = to_tree(&nodes);
        assert!(tree.is_empty());
    }

    #[test]
    fn url_encoding_escapes_what_would_break_a_query() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urlencode("plain-name_1.txt"), "plain-name_1.txt");
    }
}
