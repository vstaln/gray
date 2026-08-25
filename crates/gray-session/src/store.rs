//! SQLite state store — Wave 0-1 Task 14 phase 1
//!
//! Mirrors `hermes_state.py` v1 columns (derived from `hermes_state_common.py`
//! `SCHEMA_SQL`): `sessions` + `messages` + FTS5 index, WAL mode, `source` tag
//! and `parent_session_id` chain. Anything beyond (compression-chain helpers,
//! batch tables, trigram/CJK indexes, deferred indexes, extended session
//! columns) is intentionally deferred — see ledger PARTIAL rows.
//!
//! Column names are taken verbatim from Python `CREATE TABLE` statements; no
//! invented columns.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

/// Unified error for the store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Metadata for `sessions` row — minimal v1 projection.
/// Column names match Python `SCHEMA_SQL` exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub title: Option<String>,
    pub source: String,
    pub model: Option<String>,
    pub parent_session_id: Option<String>,
}

/// FTS5 hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub session_id: String,
    pub message_id: i64,
    pub snippet: String,
}

/// SQLite-backed session store.
///
/// Design: stores the DB path, opens a fresh `Connection` per operation
/// (WAL allows concurrent readers; `busy_timeout` handles writer contention).
/// This keeps `StateStore` `Send + Sync` without requiring `Connection: Send`.
#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    /// Open or create the DB at `path`, enable WAL, and run the v1 migration
    /// idempotently. WAL is enforced via `PRAGMA journal_mode=WAL`.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::set_pragmas(&conn)?;
        Self::migrate(&conn)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    fn connect(&self) -> Result<Connection, Error> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::set_pragmas(&conn)?;
        Ok(conn)
    }

    /// WAL mode — mirrors hermes_state.py apply_wal_with_fallback.
    /// `journal_mode=WAL` RETURNS the resulting mode, so it must go through
    /// `query_row`; rusqlite's `pragma_update` rejects pragmas with results
    /// (ExecuteReturnedResults).
    fn set_pragmas(conn: &Connection) -> Result<(), Error> {
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        Ok(())
    }

    fn migrate(conn: &Connection) -> Result<(), Error> {
        // v1 DDL derived from hermes_state_common.SCHEMA_SQL + FTS_SQL.
        // Sessions: id, source, title, model, parent_session_id, started_at, ended_at
        // Messages: id, session_id, role, content, timestamp
        // FTS5: external-content on messages(content) with triggers.
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                title TEXT,
                model TEXT,
                parent_session_id TEXT,
                started_at REAL NOT NULL,
                ended_at REAL,
                FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_source ON sessions(source);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT,
                timestamp REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);

            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content,
                content='messages',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.id, old.content);
                INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
            END;
            "#,
        )?;
        Ok(())
    }

    /// Insert a new session row.
    pub fn create_session(&self, meta: SessionMeta) -> Result<(), Error> {
        let conn = self.connect()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        conn.execute(
            "INSERT INTO sessions (id, source, title, model, parent_session_id, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                meta.id,
                meta.source,
                meta.title,
                meta.model,
                meta.parent_session_id,
                now
            ],
        )?;
        Ok(())
    }

    /// Append a message to `session_id`, returning the `messages.id` rowid.
    pub fn append_message(&self, session_id: &str, role: &str, content: &str) -> Result<i64, Error> {
        let conn = self.connect()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, content, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// FTS5 search: `MATCH` query, capped at `limit` hits.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, Error> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        // Use FTS5 MATCH via the virtual table; join to messages for session_id.
        // Snippet uses the message content directly (ponytail: shortest that works,
        // full snippet() highlighting deferred until search UX needs it).
        let mut stmt = conn.prepare(
            "SELECT m.session_id, m.id, COALESCE(m.content, '') \
             FROM messages_fts \
             JOIN messages m ON m.id = messages_fts.rowid \
             WHERE messages_fts MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SearchHit {
                session_id: row.get(0)?,
                message_id: row.get(1)?,
                snippet: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// List sessions, optionally filtered by `source`.
    pub fn list_sessions(&self, source: Option<&str>) -> Result<Vec<SessionMeta>, Error> {
        let conn = self.connect()?;
        let mut out = Vec::new();
        if let Some(src) = source {
            let mut stmt = conn.prepare(
                "SELECT id, title, source, model, parent_session_id FROM sessions WHERE source = ?1 ORDER BY started_at DESC",
            )?;
            let rows = stmt.query_map(params![src], |row| {
                Ok(SessionMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    source: row.get(2)?,
                    model: row.get(3)?,
                    parent_session_id: row.get(4)?,
                })
            })?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, title, source, model, parent_session_id FROM sessions ORDER BY started_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(SessionMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    source: row.get(2)?,
                    model: row.get(3)?,
                    parent_session_id: row.get(4)?,
                })
            })?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_wal_schema_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        drop(StateStore::open(&db).unwrap());
        let again = StateStore::open(&db).unwrap(); // second open migrates cleanly
        // Verify WAL mode is active and tables exist
        let conn = again.connect().unwrap();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn append_then_fts_search_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let store = StateStore::open(&db).unwrap();

        let meta = SessionMeta {
            id: "sess-1".to_string(),
            title: Some("test".to_string()),
            source: "cli".to_string(),
            model: Some("test-model".to_string()),
            parent_session_id: None,
        };
        store.create_session(meta).unwrap();
        let rowid = store
            .append_message("sess-1", "user", "the quick brown fox")
            .unwrap();
        assert!(rowid > 0);

        let hits = store.search("quick", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "sess-1");
        assert_eq!(hits[0].message_id, rowid);
        assert!(hits[0].snippet.contains("quick"));

        let empty = store.search("zebra", 10).unwrap();
        assert!(empty.is_empty());

        let telegram = store.list_sessions(Some("telegram")).unwrap();
        assert!(telegram.is_empty());

        let cli = store.list_sessions(Some("cli")).unwrap();
        assert_eq!(cli.len(), 1);
        assert_eq!(cli[0].id, "sess-1");

        let all = store.list_sessions(None).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn parent_session_id_chain() {
        // Covers parent_session_id column derived from Python SCHEMA_SQL
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let store = StateStore::open(&db).unwrap();
        store
            .create_session(SessionMeta {
                id: "parent".to_string(),
                title: None,
                source: "cli".to_string(),
                model: None,
                parent_session_id: None,
            })
            .unwrap();
        store
            .create_session(SessionMeta {
                id: "child".to_string(),
                title: None,
                source: "cli".to_string(),
                model: None,
                parent_session_id: Some("parent".to_string()),
            })
            .unwrap();
        let all = store.list_sessions(None).unwrap();
        assert_eq!(all.len(), 2);
        let child = all.iter().find(|s| s.id == "child").unwrap();
        assert_eq!(child.parent_session_id.as_deref(), Some("parent"));
    }
}
