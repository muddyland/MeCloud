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
}
