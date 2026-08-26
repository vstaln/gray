//! Per-job durable notepad for cron jobs.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/notepad.py` (187 lines).
//! A tiny KV scratchpad each cron job can use to carry state across scheduled
//! wake-ups (cursors, watermarks, watchlists). Stored in its own profile-local
//! SQLite file next to the executions ledger, following the same
//! connection/pragma pattern as `cron/executions.py`.
//!
//! Size caps (documented contract):
//! - `MAX_VALUE_BYTES` (16 KB): per-key value cap, measured in UTF-8 bytes.
//! - `MAX_JOB_TOTAL_BYTES` (64 KB): per-job cap over the sum of key+value bytes.
//!   Oversized writes raise `ValueError` (mapped to `NotepadError::Validation`) and
//!   leave the store untouched — the notepad is prompt-injected each run, so
//!   unbounded growth would bloat every wake-up's prompt.
//!
//! Write path is the CLI (`hermes cron notepad <job_id> set <key> <value>`),
//! which the running agent invokes via its terminal tool; no model tool is added.
//!
//! Python source docstring (preserved):
//! ```text
//! Per-job durable notepad for cron jobs.
//!
//! A tiny KV scratchpad each cron job can use to carry state across scheduled
//! wake-ups (cursors, watermarks, watchlists). Stored in its own profile-local
//! SQLite file next to the executions ledger, following the same
//! connection/pragma pattern as ``cron/executions.py``.
//! ```

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{params, Connection, Row, Transaction};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants & global state (mirrors Python module globals)
// ---------------------------------------------------------------------------

pub const MAX_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_KEY_CHARS: usize = 128;
pub const MAX_JOB_TOTAL_BYTES: usize = 64 * 1024;

/// Optional test override for the DB path.
/// Mirrors `NOTEPAD_FILE` reassignment in Python tests.
static NOTEPAD_FILE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Global lock guarding all DB access. Mirrors `_lock = threading.RLock()`.
static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum NotepadError {
    #[error("{0}")]
    Validation(String),
    #[error("rusqlite: {0}")]
    Rusqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, NotepadError>;

// ---------------------------------------------------------------------------
// Data model — mirrors `cron_notepad` table rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRecord {
    pub job_id: String,
    pub key: String,
    pub value: String,
    pub updated_at: String,
}

fn row_to_record(row: &Row) -> rusqlite::Result<NoteRecord> {
    Ok(NoteRecord {
        job_id: row.get("job_id")?,
        key: row.get("key")?,
        value: row.get("value")?,
        updated_at: row.get("updated_at")?,
    })
}

// ---------------------------------------------------------------------------
// Home / path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve the Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()`:
/// `HERMES_HOME` env → `~/.hermes` (POSIX) / `%LOCALAPPDATA%/hermes` (Windows).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(".hermes");
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        if !userprofile.trim().is_empty() {
            return PathBuf::from(userprofile).join(".hermes");
        }
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        if !localappdata.trim().is_empty() {
            return PathBuf::from(localappdata).join("hermes");
        }
    }
    PathBuf::from(".hermes")
}

/// Path to the notepad DB file.
/// Mirrors `NOTEPAD_FILE = get_hermes_home().resolve() / "cron" / "notepad.db"`.
pub fn notepad_db_path() -> PathBuf {
    if let Ok(guard) = NOTEPAD_FILE_OVERRIDE.lock() {
        if let Some(p) = guard.clone() {
            return p;
        }
    }
    get_hermes_home().join("cron").join("notepad.db")
}

/// Test-only override for the notepad DB path.
/// Mirrors setting `NOTEPAD_FILE = Path(...)` in Python tests.
pub fn set_notepad_file_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = NOTEPAD_FILE_OVERRIDE.lock() {
        *guard = path;
    }
}

fn hermes_now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// SQLite helpers — mirrors `_connect`, `_initialize_schema`, `_transaction`
// ---------------------------------------------------------------------------

fn _connect() -> Result<Connection> {
    let path = notepad_db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    Ok(conn)
}

fn apply_wal_with_fallback(conn: &Connection) -> Result<()> {
    let mode: std::result::Result<String, _> =
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0));
    match mode {
        Ok(m) if m.to_ascii_lowercase() == "wal" => {
            log::debug!("cron/notepad.db journal_mode=WAL");
        }
        Ok(other) => {
            log::debug!(
                "cron/notepad.db WAL unavailable (mode={}), falling back to DELETE",
                other
            );
            let _ = conn.execute("PRAGMA journal_mode=DELETE", []);
        }
        Err(e) => {
            log::debug!("cron/notepad.db PRAGMA journal_mode=WAL failed: {e}, fallback DELETE");
            let _ = conn.execute("PRAGMA journal_mode=DELETE", []);
        }
    }
    Ok(())
}

fn _initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute("PRAGMA busy_timeout=5000", [])?;
    apply_wal_with_fallback(conn)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS cron_notepad (
             job_id TEXT NOT NULL,
             key TEXT NOT NULL,
             value TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             PRIMARY KEY (job_id, key)
           )",
        [],
    )?;
    Ok(())
}

fn with_transaction<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&Transaction) -> Result<T>,
{
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let mut conn = _connect()?;
    _initialize_schema(&conn)?;
    let tx = conn.transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

fn with_connection<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T>,
{
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let conn = _connect()?;
    _initialize_schema(&conn)?;
    let result = f(&conn)?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Validation — mirrors `def _validate`
// ---------------------------------------------------------------------------

fn _validate(job_id: &str, key: &str, value: &str) -> Result<()> {
    if job_id.is_empty() {
        return Err(NotepadError::Validation("job_id must be non-empty".to_string()));
    }
    if key.is_empty() {
        return Err(NotepadError::Validation("key must be non-empty".to_string()));
    }
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(NotepadError::Validation(format!(
            "key too long (max {MAX_KEY_CHARS} characters)"
        )));
    }
    if value.as_bytes().len() > MAX_VALUE_BYTES {
        return Err(NotepadError::Validation(format!(
            "value too large (max {MAX_VALUE_BYTES} bytes per key)"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions 1:1
// ---------------------------------------------------------------------------

/// Upsert one key. Raises `NotepadError::Validation` when a size cap would be exceeded.
/// Mirrors `def set_note(job_id: str, key: str, value: str) -> Dict[str, Any]`.
pub fn set_note(job_id: &str, key: &str, value: &str) -> Result<NoteRecord> {
    let job_id = job_id.to_string();
    let key = key.to_string();
    let value = value.to_string();
    _validate(&job_id, &key, &value)?;
    let now = hermes_now_iso();
    with_transaction(|conn| {
        let other_bytes: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(CAST(key AS BLOB)) + LENGTH(CAST(value AS BLOB))), 0) \
                 FROM cron_notepad WHERE job_id=? AND key<>?",
                params![job_id, key],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let other_bytes = other_bytes as usize;
        let entry_bytes = key.as_bytes().len() + value.as_bytes().len();
        if other_bytes + entry_bytes > MAX_JOB_TOTAL_BYTES {
            return Err(NotepadError::Validation(format!(
                "notepad full: job '{job_id}' would exceed {MAX_JOB_TOTAL_BYTES} bytes total; delete unused keys first"
            )));
        }
        conn.execute(
            "INSERT INTO cron_notepad (job_id, key, value, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(job_id, key) \
             DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![job_id, key, value, now],
        )?;
        Ok(())
    })?;
    Ok(NoteRecord {
        job_id,
        key,
        value,
        updated_at: now,
    })
}

/// Mirrors `def get_note(job_id: str, key: str) -> Optional[str]`.
pub fn get_note(job_id: &str, key: &str) -> Result<Option<String>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare("SELECT value FROM cron_notepad WHERE job_id=? AND key=?")?;
        let mut rows = stmt.query(params![job_id, key])?;
        if let Some(row) = rows.next()? {
            let val: String = row.get(0)?;
            Ok(Some(val))
        } else {
            Ok(None)
        }
    })
}

/// Mirrors `def delete_note(job_id: str, key: str) -> bool`.
pub fn delete_note(job_id: &str, key: &str) -> Result<bool> {
    with_transaction(|conn| {
        let n = conn.execute(
            "DELETE FROM cron_notepad WHERE job_id=? AND key=?",
            params![job_id, key],
        )?;
        Ok(n > 0)
    })
}

/// All entries for one job, sorted by key.
/// Mirrors `def list_notes(job_id: str) -> List[Dict[str, Any]]`.
pub fn list_notes(job_id: &str) -> Result<Vec<NoteRecord>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT job_id, key, value, updated_at FROM cron_notepad \
             WHERE job_id=? ORDER BY key",
        )?;
        let rows = stmt.query_map(params![job_id], |row| row_to_record(row))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// Delete every key for one job (e.g. on job removal). Returns row count.
/// Called from `cron.jobs.remove_job` so deleted jobs don't orphan rows.
/// No-ops without creating the DB when no notepad file exists yet.
/// Mirrors `def clear_notepad(job_id: str) -> int`.
pub fn clear_notepad(job_id: &str) -> Result<usize> {
    if !notepad_db_path().exists() {
        return Ok(0);
    }
    with_transaction(|conn| {
        let n = conn.execute(
            "DELETE FROM cron_notepad WHERE job_id=?",
            params![job_id],
        )?;
        Ok(n as usize)
    })
}

/// Render a job's notepad as a prompt section, or "" when empty/unavailable.
/// Empty notepad MUST return the empty string so jobs that never use the feature
/// get a byte-identical prompt (prompt-cache + drift safety).
/// Mirrors `def render_notepad_section(job_id: str) -> str`.
pub fn render_notepad_section(job_id: &str) -> String {
    let notes = match list_notes(job_id) {
        Ok(n) => n,
        Err(_) => return String::new(),
    };
    if notes.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = notes
        .iter()
        .map(|note| format!("- {}: {}", note.key, note.value))
        .collect();
    format!(
        "## Job notepad (persistent across runs)\n\
This durable scratchpad survives between scheduled runs of this \
job. Update it via the CLI, e.g.:\n\
`hermes cron notepad {job_id} set <key> <value>` \
(also: get/delete/list; `hermes cron notepad {job_id} delete \
<key>` removes an entry).\n\n\
{}\n\n",
        lines.join("\n")
    )
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_match_python() {
        assert_eq!(MAX_VALUE_BYTES, 16 * 1024);
        assert_eq!(MAX_KEY_CHARS, 128);
        assert_eq!(MAX_JOB_TOTAL_BYTES, 64 * 1024);
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(matches!(_validate("", "k", "v"), Err(NotepadError::Validation(_))));
        assert!(matches!(_validate("j", "", "v"), Err(NotepadError::Validation(_))));
    }

    #[test]
    fn validate_rejects_long_key() {
        let long = "k".repeat(129);
        assert!(matches!(_validate("j", &long, "v"), Err(NotepadError::Validation(_))));
    }

    #[test]
    fn validate_rejects_large_value() {
        let large = "a".repeat(16 * 1024 + 1);
        assert!(matches!(_validate("j", "k", &large), Err(NotepadError::Validation(_))));
    }

    #[test]
    fn hermes_now_iso_is_rfc3339() {
        let s = hermes_now_iso();
        assert!(chrono::DateTime::parse_from_rfc3339(&s).is_ok(), "now iso must be rfc3339: {s}");
    }

    #[test]
    fn render_empty_is_empty() {
        // Uses temp HERMES_HOME so list_notes hits empty/non-existent DB and returns "".
        let dir = std::env::temp_dir().join(format!("hermes-test-notepad-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let orig = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", &dir) };
        set_notepad_file_override(None);
        let s = render_notepad_section("no-such-job");
        assert_eq!(s, "");
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            match orig {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
        set_notepad_file_override(None);
    }
}
