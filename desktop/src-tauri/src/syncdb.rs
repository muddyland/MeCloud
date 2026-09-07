//! What was true at the end of the last successful sync.
//!
//! The reconciler needs a third input beyond the two live trees (see
//! `reconcile`), and this is where it lives. SQLite rather than a JSON file
//! because it is written after *every* transferred file: a crash halfway
//! through a large sync must leave a coherent record of what did land, not a
//! truncated document.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::reconcile::{Baseline, Baselines};

pub struct SyncDb {
    conn: Connection,
}

/// A file too large to upload without the user agreeing to it first.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingUpload {
    pub path: String,
    pub size: u64,
    pub approved: bool,
    pub days_waiting: i64,
}

/// A file deleted here, still present on the server.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingDeletion {
    pub path: String,
    pub node_id: Option<String>,
    pub size: u64,
    pub marked_at: i64,
    pub days_waiting: i64,
}

impl SyncDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        Self::from_connection(conn)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        Self::from_connection(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn from_connection(conn: Connection) -> Result<Self, String> {
        // WAL so a reader (the UI asking for status) never blocks the writer.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS baseline (
                 path         TEXT PRIMARY KEY,
                 local_hash   TEXT NOT NULL,
                 remote_blob  TEXT NOT NULL,
                 size         INTEGER NOT NULL,
                 node_id      TEXT,
                 mtime        INTEGER NOT NULL DEFAULT 0,
                 synced_at    INTEGER NOT NULL
             );
             -- Files deleted locally, not yet deleted on the server. A local
             -- deletion is only evidence that something happened here; it is
             -- not proof the user wants the only other copy destroyed. These
             -- wait, visibly, until the user says so or the delay expires.
             CREATE TABLE IF NOT EXISTS pending_deletion (
                 path      TEXT PRIMARY KEY,
                 node_id   TEXT,
                 size      INTEGER NOT NULL DEFAULT 0,
                 marked_at INTEGER NOT NULL
             );
             -- Uploads held back for being larger than the user's threshold.
             -- Not an exclusion: the file stays on disk and stays tracked, and
             -- the row exists so the settings screen can ask about it once
             -- rather than the engine asking on every pass.
             CREATE TABLE IF NOT EXISTS pending_upload (
                 path      TEXT PRIMARY KEY,
                 size      INTEGER NOT NULL DEFAULT 0,
                 approved  INTEGER NOT NULL DEFAULT 0,
                 marked_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;

        // `CREATE TABLE IF NOT EXISTS` does nothing to a table that already
        // exists, so a column added in a later version never appears on a
        // database made by an earlier one. Every upgrade hit this: the client
        // ran, and every single write failed with "no column named mtime".
        //
        // Adding it here, tolerating the error when it is already present, is
        // the whole migration — the default makes existing rows behave as
        // "timestamp unknown", which the scanner already treats as "must hash".
        let already_there = matches!(
            conn.execute("ALTER TABLE baseline ADD COLUMN mtime INTEGER NOT NULL DEFAULT 0", []),
            Err(rusqlite::Error::SqliteFailure(_, _)),
        );
        let _ = already_there;

        Ok(Self { conn })
    }

    pub fn baselines(&self) -> Result<Baselines, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, local_hash, remote_blob FROM baseline")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Baseline { local: row.get::<_, String>(1)?, remote: row.get::<_, String>(2)? },
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut out = Baselines::new();
        for row in rows {
            let (path, baseline) = row.map_err(|e| e.to_string())?;
            out.insert(path, baseline);
        }
        Ok(out)
    }

    /// Record a path as synced. Written per file, not per run.
    ///
    /// Both markers are stored: the local content hash and the remote blob id.
    /// They are never comparable with each other, only with what the same side
    /// reports next time — which is exactly what the reconciler asks.
    pub fn record(
        &self, path: &str, local_hash: &str, remote_blob: &str, size: u64, node_id: Option<&str>,
    ) -> Result<(), String> {
        self.record_with_mtime(path, local_hash, remote_blob, size, node_id, 0)
    }

    /// As `record`, also storing the file's modification time so the next scan
    /// can tell at a glance whether it needs re-reading.
    pub fn record_with_mtime(
        &self, path: &str, local_hash: &str, remote_blob: &str, size: u64,
        node_id: Option<&str>, mtime: i64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO baseline (path, local_hash, remote_blob, size, node_id, mtime, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'))
                 ON CONFLICT(path) DO UPDATE SET
                   local_hash = excluded.local_hash, remote_blob = excluded.remote_blob,
                   size = excluded.size, node_id = excluded.node_id,
                   mtime = excluded.mtime, synced_at = excluded.synced_at",
                params![path, local_hash, remote_blob, size as i64, node_id, mtime],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn forget(&self, path: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM baseline WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Whether one path is tracked.
    ///
    /// A single indexed lookup, because this is what the file manager asks —
    /// once per visible file, on its own main thread. It used to read the
    /// whole baseline table into a map to test one key, which is O(tracked)
    /// per file and made opening a folder of 200 files take eight seconds
    /// with 20,000 files tracked.
    pub fn is_tracked(&self, path: &str) -> Result<bool, String> {
        self.conn
            .query_row("SELECT 1 FROM baseline WHERE path = ?1", params![path], |_| Ok(()))
            .map(|_| true)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                other => Err(other.to_string()),
            })
    }

    /// The recorded size and mtime for a path, for deciding whether to re-hash.
    pub fn stamp(&self, path: &str) -> Option<(u64, i64, String)> {
        self.conn
            .query_row(
                "SELECT size, mtime, local_hash FROM baseline WHERE path = ?1",
                params![path],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .ok()
    }

    /// Every path's (size, mtime, hash), for a scan that wants to skip
    /// re-reading files nothing has touched.
    pub fn stamps(&self) -> Result<std::collections::HashMap<String, (u64, i64, String)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, size, mtime, local_hash FROM baseline")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)?, row.get::<_, String>(3)?),
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (path, stamp) = row.map_err(|e| e.to_string())?;
            out.insert(path, stamp);
        }
        Ok(out)
    }

    pub fn node_id(&self, path: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT node_id FROM baseline WHERE path = ?1", params![path], |r| r.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.to_string()),
            })
    }

    // ── Deferred server-side deletions ──────────────────────────────────────

    /// Mark a path as deleted locally and awaiting deletion on the server.
    ///
    /// The baseline row is deliberately kept. Dropping it would make the next
    /// pass see "on the server, absent here, no history" and download the file
    /// straight back, undoing the deletion the user actually made.
    /// Note that a file is too large to upload without being asked about.
    ///
    /// Deliberately does not disturb an existing row: re-noting it must not
    /// clear an approval the user has already given, or an approved upload
    /// that failed once would need approving again on every pass.
    /// Every tracked path with its size — what the selective-sync picker needs
    /// to say how much a folder is costing.
    pub fn sizes(&self) -> Result<Vec<(String, u64)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, size FROM baseline")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn hold_upload(&self, path: &str, size: u64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO pending_upload (path, size, approved, marked_at)
                 VALUES (?1, ?2, 0, strftime('%s','now'))
                 ON CONFLICT(path) DO UPDATE SET size = ?2",
                params![path, size as i64],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Whether this path may be uploaded despite exceeding the threshold.
    pub fn upload_approved(&self, path: &str) -> bool {
        self.conn
            .query_row(
                "SELECT approved FROM pending_upload WHERE path = ?1",
                params![path],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v == 1)
            .unwrap_or(false)
    }

    pub fn approve_upload(&self, path: &str) -> Result<(), String> {
        self.conn
            .execute("UPDATE pending_upload SET approved = 1 WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_upload(&self, path: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM pending_upload WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn held_uploads(&self) -> Result<Vec<PendingUpload>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, size, approved,
                        (strftime('%s','now') - marked_at) / 86400
                 FROM pending_upload WHERE approved = 0 ORDER BY size DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PendingUpload {
                    path: r.get(0)?,
                    size: r.get::<_, i64>(1)? as u64,
                    approved: r.get::<_, i64>(2)? == 1,
                    days_waiting: r.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn mark_pending(&self, path: &str, node_id: Option<&str>, size: u64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO pending_deletion (path, node_id, size, marked_at)
                 VALUES (?1, ?2, ?3, strftime('%s','now'))
                 ON CONFLICT(path) DO NOTHING",
                params![path, node_id, size as i64],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn is_pending(&self, path: &str) -> Result<bool, String> {
        self.conn
            .query_row("SELECT 1 FROM pending_deletion WHERE path = ?1", params![path], |_| Ok(()))
            .map(|_| true)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                other => Err(other.to_string()),
            })
    }

    pub fn pending_paths(&self) -> Result<std::collections::HashSet<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM pending_deletion")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// Everything waiting, newest first, with how long it has been waiting.
    pub fn pending(&self) -> Result<Vec<PendingDeletion>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, node_id, size, marked_at,
                        CAST((strftime('%s','now') - marked_at) / 86400 AS INTEGER)
                 FROM pending_deletion ORDER BY marked_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PendingDeletion {
                    path: row.get(0)?,
                    node_id: row.get(1)?,
                    size: row.get::<_, i64>(2)? as u64,
                    marked_at: row.get(3)?,
                    days_waiting: row.get::<_, i64>(4)?.max(0),
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// Those that have waited longer than `days`.
    pub fn expired_pending(&self, days: i64) -> Result<Vec<PendingDeletion>, String> {
        Ok(self.pending()?.into_iter().filter(|p| p.days_waiting >= days).collect())
    }

    pub fn clear_pending(&self, path: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM pending_deletion WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn count(&self) -> Result<i64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM baseline", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_from_an_older_version_gains_the_new_column() {
        // The upgrade path: a table created before `mtime` existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE baseline (
                 path TEXT PRIMARY KEY, local_hash TEXT NOT NULL,
                 remote_blob TEXT NOT NULL, size INTEGER NOT NULL,
                 node_id TEXT, synced_at INTEGER NOT NULL);
             INSERT INTO baseline VALUES ('old.txt','L1','R1',5,'n1',0);",
        )
        .unwrap();

        let db = SyncDb::from_connection(conn).expect("should migrate, not fail");
        // The existing row survives...
        assert!(db.is_tracked("old.txt").unwrap());
        // ...and writes that use the new column now work.
        db.record_with_mtime("new.txt", "L2", "R2", 9, None, 4242).unwrap();
        assert_eq!(db.stamp("new.txt"), Some((9, 4242, "L2".into())));
        // The pre-existing row reads as "timestamp unknown", which the scanner
        // treats as "must hash" rather than trusting a value it never had.
        assert_eq!(db.stamp("old.txt"), Some((5, 0, "L1".into())));
    }

    #[test]
    fn opening_the_same_database_twice_is_harmless() {
        let conn = Connection::open_in_memory().unwrap();
        let db = SyncDb::from_connection(conn).unwrap();
        db.record_with_mtime("a.txt", "L", "R", 1, None, 7).unwrap();
        assert_eq!(db.stamp("a.txt").unwrap().1, 7);
    }

    #[test]
    fn a_pending_deletion_keeps_its_baseline() {
        // Dropping the baseline would make the next pass treat the file as new
        // on the server and download it straight back.
        let db = SyncDb::in_memory().unwrap();
        db.record("a.txt", "L1", "R1", 5, Some("n1")).unwrap();
        db.mark_pending("a.txt", Some("n1"), 5).unwrap();
        assert!(db.is_tracked("a.txt").unwrap(), "baseline must survive");
        assert!(db.is_pending("a.txt").unwrap());
        assert_eq!(db.pending().unwrap().len(), 1);
        assert_eq!(db.pending_paths().unwrap().len(), 1);
    }

    #[test]
    fn marking_the_same_path_twice_does_not_restart_the_clock() {
        // Otherwise a file could sit in the trash forever, re-marked by every
        // pass and never reaching its expiry.
        let db = SyncDb::in_memory().unwrap();
        db.mark_pending("a.txt", None, 1).unwrap();
        let first = db.pending().unwrap()[0].marked_at;
        db.mark_pending("a.txt", None, 1).unwrap();
        assert_eq!(db.pending().unwrap().len(), 1);
        assert_eq!(db.pending().unwrap()[0].marked_at, first);
    }

    #[test]
    fn nothing_is_expired_before_its_time() {
        let db = SyncDb::in_memory().unwrap();
        db.mark_pending("a.txt", None, 1).unwrap();
        assert!(db.expired_pending(30).unwrap().is_empty());
        // Everything qualifies at zero days, which is what "delete now" uses.
        assert_eq!(db.expired_pending(0).unwrap().len(), 1);
    }

    #[test]
    fn clearing_a_pending_deletion_removes_only_that_one() {
        let db = SyncDb::in_memory().unwrap();
        db.mark_pending("a.txt", None, 1).unwrap();
        db.mark_pending("b.txt", None, 1).unwrap();
        db.clear_pending("a.txt").unwrap();
        let left: Vec<String> = db.pending().unwrap().into_iter().map(|p| p.path).collect();
        assert_eq!(left, vec!["b.txt"]);
    }

    #[test]
    fn a_fresh_database_has_no_baselines() {
        let db = SyncDb::in_memory().unwrap();
        assert!(db.baselines().unwrap().is_empty());
        assert_eq!(db.count().unwrap(), 0);
    }

    #[test]
    fn recording_then_reading_round_trips() {
        let db = SyncDb::in_memory().unwrap();
        db.record("a/b.txt", "L1", "R1", 12, Some("node-1")).unwrap();
        let baselines = db.baselines().unwrap();
        assert_eq!(baselines["a/b.txt"].local, "L1");
        assert_eq!(baselines["a/b.txt"].remote, "R1");
        assert_eq!(db.node_id("a/b.txt").unwrap().as_deref(), Some("node-1"));
    }

    #[test]
    fn recording_the_same_path_twice_updates_rather_than_duplicating() {
        let db = SyncDb::in_memory().unwrap();
        db.record("a.txt", "L1", "R1", 1, None).unwrap();
        db.record("a.txt", "L2", "R2", 2, Some("n")).unwrap();
        assert_eq!(db.count().unwrap(), 1);
        assert_eq!(db.baselines().unwrap()["a.txt"].local, "L2");
        assert_eq!(db.baselines().unwrap()["a.txt"].remote, "R2");
    }

    #[test]
    fn forgetting_a_path_removes_its_baseline() {
        // A deleted file must lose its baseline, or the next run sees a
        // baseline with no file on either side and tries to delete again.
        let db = SyncDb::in_memory().unwrap();
        db.record("a.txt", "L1", "R1", 1, None).unwrap();
        db.forget("a.txt").unwrap();
        assert!(db.baselines().unwrap().is_empty());
    }

    #[test]
    fn tracking_one_path_is_a_single_lookup() {
        let db = SyncDb::in_memory().unwrap();
        db.record("a.txt", "L1", "R1", 1, None).unwrap();
        assert!(db.is_tracked("a.txt").unwrap());
        assert!(!db.is_tracked("b.txt").unwrap());
    }

    #[test]
    fn a_stamp_carries_what_a_scan_needs_to_skip_a_file() {
        let db = SyncDb::in_memory().unwrap();
        db.record_with_mtime("a.txt", "L1", "R1", 42, None, 1234).unwrap();
        assert_eq!(db.stamp("a.txt"), Some((42, 1234, "L1".into())));
        assert_eq!(db.stamp("missing.txt"), None);
        assert_eq!(db.stamps().unwrap()["a.txt"], (42, 1234, "L1".into()));
    }

    #[test]
    fn an_unknown_path_has_no_node_id() {
        let db = SyncDb::in_memory().unwrap();
        assert_eq!(db.node_id("nope.txt").unwrap(), None);
    }

    #[test]
    fn a_held_upload_is_listed_until_it_is_approved() {
        let db = SyncDb::in_memory().unwrap();
        db.hold_upload("big.iso", 4_000_000_000).unwrap();

        let held = db.held_uploads().unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].size, 4_000_000_000);
        assert!(!db.upload_approved("big.iso"));

        db.approve_upload("big.iso").unwrap();
        assert!(db.upload_approved("big.iso"));
        assert!(db.held_uploads().unwrap().is_empty(), "approved is no longer waiting");
    }

    #[test]
    fn re_noting_a_held_upload_does_not_revoke_its_approval() {
        // The engine notes the file on every pass until it is actually
        // uploaded. If that cleared the approval, an upload that failed once
        // for any reason could never succeed.
        let db = SyncDb::in_memory().unwrap();
        db.hold_upload("big.iso", 100).unwrap();
        db.approve_upload("big.iso").unwrap();
        db.hold_upload("big.iso", 120).unwrap();

        assert!(db.upload_approved("big.iso"));
    }

    #[test]
    fn clearing_a_held_upload_forgets_it_entirely() {
        let db = SyncDb::in_memory().unwrap();
        db.hold_upload("big.iso", 100).unwrap();
        db.clear_upload("big.iso").unwrap();
        assert!(db.held_uploads().unwrap().is_empty());
        assert!(!db.upload_approved("big.iso"));
    }
}
