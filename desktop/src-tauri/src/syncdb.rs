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
                 synced_at    INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
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
        self.conn
            .execute(
                "INSERT INTO baseline (path, local_hash, remote_blob, size, node_id, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now'))
                 ON CONFLICT(path) DO UPDATE SET
                   local_hash = excluded.local_hash, remote_blob = excluded.remote_blob,
                   size = excluded.size, node_id = excluded.node_id,
                   synced_at = excluded.synced_at",
                params![path, local_hash, remote_blob, size as i64, node_id],
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
    fn an_unknown_path_has_no_node_id() {
        let db = SyncDb::in_memory().unwrap();
        assert_eq!(db.node_id("nope.txt").unwrap(), None);
    }
}
