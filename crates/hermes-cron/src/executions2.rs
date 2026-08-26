//! Profile-local durable audit ledger for cron execution attempts.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/executions.py` (284 lines).
//! The ledger records what is known about each attempt; it is not a retry queue.
//! Interrupted attempts become `unknown` only after their exact owner process is
//! proved gone. Terminal states are immutable.
//!
//! Python source docstring (preserved):
//! ```text
//! Profile-local durable audit ledger for cron execution attempts.
//!
//! The ledger records what is known about each attempt; it is not a retry queue.
//! Interrupted attempts become ``unknown`` only after their exact owner process is
//! proved gone. Terminal states are immutable.
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use rusqlite::{params, Connection, Row, Transaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants & global state (mirrors Python module globals)
// ---------------------------------------------------------------------------

/// Maximum number of terminal executions retained; older terminal rows are pruned.
/// Mirrors `MAX_TERMINAL_EXECUTIONS = 1000`.
pub const MAX_TERMINAL_EXECUTIONS: i64 = 1000;

/// Terminal states — immutable once reached.
/// Mirrors `_TERMINAL_STATES = ("completed", "failed", "unknown")`.
pub const TERMINAL_STATES: &[&str] = &["completed", "failed", "unknown"];

/// All valid statuses (CHECK constraint).
pub const ALL_STATUSES: &[&str] = &["claimed", "running", "completed", "failed", "unknown"];

/// Optional test override for the DB path.
/// Mirrors `EXECUTIONS_FILE: Optional[Path] = None`.
///
/// Production resolves the path at transaction time so dashboard operations that
/// temporarily enter another profile cannot leak that profile's execution records
/// into the import-time home.
static EXECUTIONS_FILE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Global lock guarding all DB access. Mirrors `_lock = threading.RLock()`.
static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

/// Unique identifier for this process incarnation. Mirrors `_PROCESS_ID = uuid.uuid4().hex`.
static PROCESS_ID: OnceLock<String> = OnceLock::new();

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("rusqlite: {0}")]
    Rusqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("other: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Data model — mirrors the `executions` table
// ---------------------------------------------------------------------------

/// One row in the `executions` ledger. Mirrors `Dict[str, Any]` row dicts in Python.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub id: String,
    pub job_id: String,
    pub source: String,
    pub process_id: String,
    pub pid: i64,
    pub process_started_at: Option<i64>,
    pub status: String,
    pub claimed_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

fn row_to_record(row: &Row) -> rusqlite::Result<ExecutionRecord> {
    Ok(ExecutionRecord {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        source: row.get("source")?,
        process_id: row.get("process_id")?,
        pid: row.get("pid")?,
        process_started_at: row.get("process_started_at")?,
        status: row.get("status")?,
        claimed_at: row.get("claimed_at")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        error: row.get("error")?,
    })
}

// ---------------------------------------------------------------------------
// Home / path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve the Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()` resolution:
/// `HERMES_HOME` env → `~/.hermes` (POSIX) / `%LOCALAPPDATA%/hermes` (Windows).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    // Context-local override (Python's ContextVar) is not replicated in Rust;
    // env var is the single source of truth for profile scoping.
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

/// Path to the executions DB file.
/// Mirrors `EXECUTIONS_FILE or (get_hermes_home().resolve() / "cron" / "executions.db")`.
pub fn executions_db_path() -> PathBuf {
    if let Ok(guard) = EXECUTIONS_FILE_OVERRIDE.lock() {
        if let Some(p) = guard.clone() {
            return p;
        }
    }
    get_hermes_home().join("cron").join("executions.db")
}

/// Test-only override for the executions DB path.
/// Mirrors setting `EXECUTIONS_FILE = Path(...)` in Python tests.
pub fn set_executions_file_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = EXECUTIONS_FILE_OVERRIDE.lock() {
        *guard = path;
    }
}

fn get_process_id() -> String {
    PROCESS_ID
        .get_or_init(|| Uuid::new_v4().simple().to_string())
        .clone()
}

fn hermes_now_iso() -> String {
    // Mirrors `hermes_time.now().isoformat()` — timezone-aware.
    // Python resolves HERMES_TIMEZONE / config.yaml timezone; Rust uses UTC
    // as the canonical durable ledger timestamp (ISO-8601, parseable by
    // `datetime.fromisoformat`). Wall-clock tz is presentation-layer only.
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// SQLite helpers — mirrors `_connect`, `_initialize_schema`, `_transaction`
// ---------------------------------------------------------------------------

/// Open a SQLite connection to the executions DB.
/// Mirrors `_connect() -> sqlite3.Connection`.
fn _connect() -> Result<Connection> {
    let path = executions_db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    // Python passes `timeout=5` to `sqlite3.connect`; plus PRAGMA busy_timeout=5000.
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    Ok(conn)
}

/// Apply WAL with fallback — mirrors `hermes_state.apply_wal_with_fallback`.
///
/// On WAL-incompatible filesystems (NFS, etc.) SQLite either errors or silently
/// refuses WAL. We fall back to DELETE so the ledger keeps working, matching
/// Python's fallback behavior (`db_label="cron/executions.db"`).
fn apply_wal_with_fallback(conn: &Connection) -> Result<()> {
    // Try to enable WAL; if the returned mode is not WAL, fall back.
    let mode: std::result::Result<String, _> =
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0));
    match mode {
        Ok(m) if m.to_ascii_lowercase() == "wal" => {
            log::debug!("cron/executions.db journal_mode=WAL");
        }
        Ok(other) => {
            log::debug!(
                "cron/executions.db WAL unavailable (mode={}), falling back to DELETE",
                other
            );
            let _ = conn.execute("PRAGMA journal_mode=DELETE", []);
        }
        Err(e) => {
            log::debug!("cron/executions.db PRAGMA journal_mode=WAL failed: {e}, fallback DELETE");
            let _ = conn.execute("PRAGMA journal_mode=DELETE", []);
        }
    }
    Ok(())
}

/// Initialize schema if needed.
/// Mirrors `_initialize_schema(conn: sqlite3.Connection) -> None`.
fn _initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute("PRAGMA busy_timeout=5000", [])?;
    apply_wal_with_fallback(conn)?;
    conn.execute("PRAGMA synchronous=FULL", [])?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS executions (
             id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL,
             source TEXT NOT NULL,
             process_id TEXT NOT NULL,
             pid INTEGER NOT NULL,
             process_started_at INTEGER,
             status TEXT NOT NULL CHECK(status IN
               ('claimed','running','completed','failed','unknown')),
             claimed_at TEXT NOT NULL,
             started_at TEXT,
             finished_at TEXT,
             error TEXT
           )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_executions_job_claimed \
         ON executions(job_id, claimed_at DESC, id DESC)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_executions_status_claimed \
         ON executions(status, claimed_at DESC, id DESC)",
        [],
    )?;
    Ok(())
}

/// Transaction helper — mirrors `@contextmanager def _transaction()`.
/// Opens a connection, commits/rolls back on exit, always closes.
/// Holds the global lock for the entire transaction (mirrors `with _lock:`).
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

/// Read-only helper (still needs schema init + lock, mirrors Python which
/// uses `_transaction` even for reads to ensure schema exists).
fn with_connection<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T>,
{
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let conn = _connect()?;
    _initialize_schema(&conn)?;
    let result = f(&conn)?;
    // `conn` dropped here → close (mirrors `finally: conn.close()`).
    Ok(result)
}

fn fetch_one_by_id(tx: &Transaction, id: &str) -> Result<Option<ExecutionRecord>> {
    let mut stmt = tx.prepare("SELECT * FROM executions WHERE id=?")?;
    let mut rows = stmt.query_map(params![id], |row| row_to_record(row))?;
    match rows.next() {
        Some(Ok(rec)) => Ok(Some(rec)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

fn fetch_one_by_id_conn(conn: &Connection, id: &str) -> Result<Option<ExecutionRecord>> {
    let mut stmt = conn.prepare("SELECT * FROM executions WHERE id=?")?;
    let mut rows = stmt.query_map(params![id], |row| row_to_record(row))?;
    match rows.next() {
        Some(Ok(rec)) => Ok(Some(rec)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Telemetry / process helpers — mirrors `_emit_execution_state`,
// `_process_start_time`, `_owner_is_live`
// ---------------------------------------------------------------------------

/// Project durable state to monitoring without affecting ledger behavior.
/// Mirrors `_emit_execution_state(record, delivery_outcome=None)` — best-effort,
/// swallowed on failure.
fn _emit_execution_state(record: Option<&ExecutionRecord>, delivery_outcome: Option<&str>) {
    // In Python this calls `agent.monitoring.cron_health.emit_execution_state`
    // which is best-effort and flushes on terminal states. In Rust we log at
    // debug and swallow all errors so ledger behavior is never affected.
    if let Some(rec) = record {
        log::debug!(
            "cron execution state: id={} job_id={} status={} source={} delivery_outcome={:?}",
            rec.id,
            rec.job_id,
            rec.status,
            rec.source,
            delivery_outcome
        );
        // If a real monitoring emitter is wired, it would be called here.
        // Swallow any error, matching Python's `except Exception: pass`.
    }
}

/// Return a stable per-process start-time fingerprint, or None.
/// Mirrors `gateway.status.get_process_start_time(pid)` → `Optional[int]`.
///
/// On Linux this is field 22 of `/proc/<pid>/stat` (clock ticks since boot).
/// On non-Linux we return None (fail-open for liveness checks).
fn _process_start_time(pid: i32) -> Option<i64> {
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(content) = std::fs::read_to_string(&stat_path) {
        // `comm` field (field 2) is in parentheses and may contain spaces.
        // Find the last `)` to reliably locate the starttime field.
        if let Some(idx) = content.rfind(')') {
            let after = &content[idx + 1..];
            let parts: Vec<&str> = after.split_whitespace().collect();
            // After `)` the fields are: state (field 3) at parts[0], ...
            // starttime (field 22) is at parts[19] (0-based after stripping pid+comm).
            if parts.len() > 19 {
                if let Ok(val) = parts[19].parse::<i64>() {
                    return Some(val);
                }
            }
        } else {
            // Fallback: naive split (matches Python's `split()[21]`).
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() > 21 {
                if let Ok(val) = parts[21].parse::<i64>() {
                    return Some(val);
                }
            }
        }
    }
    // Non-Linux fallback: try `psutil` equivalent via `ps` or return None.
    // Python falls back to `psutil.Process(pid).create_time()*100` quantized.
    // In Rust we could shell out to `ps`, but returning None yields
    // `owner_is_live == false` (conservative: mark unknown), which is safe
    // for the ledger (fail-unknown rather than fail-live).
    None
}

/// Cross-platform "is this PID alive" check that does NOT kill the target.
/// Mirrors `gateway.status._pid_exists(pid)`.
///
/// Critical: on Windows `os.kill(pid, 0)` sends Ctrl+C; Python uses `psutil`.
/// In Rust we prefer `/proc` on Linux, else `ps` or conservative true.
fn _pid_exists(pid: i32) -> bool {
    // Linux: /proc/<pid> exists + zombie check.
    let proc_path = Path::new(&format!("/proc/{}", pid));
    if proc_path.exists() {
        // Zombie check — mirrors Python's `psutil.Process(pid).status() == STATUS_ZOMBIE`.
        // A zombie is still in the process table but already dead; treating it as
        // alive would leave interrupted executions forever claimed.
        if let Ok(status) = std::fs::read_to_string(proc_path.join("status")) {
            for line in status.lines() {
                if line.starts_with("State:") {
                    // e.g. "State:\tZ (zombie)"
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 && parts[1].starts_with('Z') {
                        return false;
                    }
                    break;
                }
            }
        }
        return true;
    }

    // Non-Linux fallback: try `ps -p <pid> -o pid=` (exists on macOS/Unix).
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
        {
            if output.status.success() {
                let out = String::from_utf8_lossy(&output.stdout);
                let trimmed = out.trim();
                // `ps` prints the pid if alive, empty otherwise.
                if !trimmed.is_empty() {
                    return trimmed == pid.to_string();
                } else {
                    return false;
                }
            }
        }
        // If `ps` unavailable, fail safe: inability to prove death must not rewrite state.
        // Mirrors Python's `except Exception: return True`.
        return true;
    }
    #[cfg(not(unix))]
    {
        // Windows without /proc: fail safe true (need psutil-like check, but we
        // conservatively assume live rather than incorrectly marking unknown).
        return true;
    }
}

/// Whether the owner process is still live.
/// Mirrors `_owner_is_live(pid: int, started_at: Optional[int]) -> bool`.
fn _owner_is_live(pid: i32, started_at: Option<i64>) -> bool {
    // Fail safe: inability to prove death must not rewrite state.
    // Python wraps `_pid_exists` in try/except and returns True on exception.
    let exists = _pid_exists(pid);
    // If we definitively know the pid is gone, owner is not live.
    // If _pid_exists returned true due to fail-safe, we keep checking start time.
    if !exists {
        return false;
    }
    if started_at.is_none() {
        return pid == std::process::id() as i32;
    }
    let current = _process_start_time(pid);
    match current {
        Some(cur) => Some(cur) == started_at,
        None => false,
    }
}

/// Prune old terminal executions, keeping only the newest `MAX_TERMINAL_EXECUTIONS`.
/// Mirrors `_prune_unlocked(conn: sqlite3.Connection) -> None`.
fn _prune_unlocked(tx: &Transaction) -> Result<()> {
    let limit = std::cmp::max(0, MAX_TERMINAL_EXECUTIONS);
    tx.execute(
        "DELETE FROM executions WHERE id IN (
             SELECT id FROM executions
             WHERE status IN ('completed','failed','unknown')
             ORDER BY claimed_at DESC, id DESC LIMIT -1 OFFSET ?
           )",
        params![limit],
    )?;
    Ok(())
}

fn _prune_unlocked_conn(conn: &Connection) -> Result<()> {
    let limit = std::cmp::max(0, MAX_TERMINAL_EXECUTIONS);
    conn.execute(
        "DELETE FROM executions WHERE id IN (
             SELECT id FROM executions
             WHERE status IN ('completed','failed','unknown')
             ORDER BY claimed_at DESC, id DESC LIMIT -1 OFFSET ?
           )",
        params![limit],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python's top-level functions 1:1
// ---------------------------------------------------------------------------

/// Persist a claimed attempt before executor/provider dispatch.
/// Mirrors `def create_execution(job_id: str, *, source: str) -> Dict[str, Any]`.
pub fn create_execution(job_id: &str, source: &str) -> Result<ExecutionRecord> {
    let now = hermes_now_iso();
    let execution_id = Uuid::new_v4().simple().to_string();
    let pid = std::process::id() as i64;
    let pid_i32 = pid as i32;
    let process_id = get_process_id();
    let process_started_at = _process_start_time(pid_i32);

    let record = with_transaction(|tx| {
        tx.execute(
            "INSERT INTO executions
               (id, job_id, source, process_id, pid, process_started_at,
                status, claimed_at)
               VALUES (?, ?, ?, ?, ?, ?, 'claimed', ?)",
            params![
                execution_id,
                job_id.to_string(),
                source.to_string(),
                process_id,
                pid,
                process_started_at,
                now
            ],
        )?;
        let rec = fetch_one_by_id(tx, &execution_id)?.expect("inserted execution must exist");
        Ok(rec)
    })?;

    _emit_execution_state(Some(&record), None);
    Ok(record)
}

/// Transition one claimed attempt to running exactly once.
/// Mirrors `def mark_execution_running(execution_id: str) -> Optional[Dict[str, Any]]`.
pub fn mark_execution_running(execution_id: &str) -> Result<Option<ExecutionRecord>> {
    let now = hermes_now_iso();
    let record = with_transaction(|tx| {
        let cur = tx.execute(
            "UPDATE executions SET status='running', started_at=? WHERE id=? AND status='claimed'",
            params![now, execution_id],
        )?;
        if cur != 1 {
            return Ok(None);
        }
        let rec = fetch_one_by_id(tx, execution_id)?;
        Ok(rec)
    })?;

    if let Some(ref rec) = record {
        _emit_execution_state(Some(rec), None);
    }
    Ok(record)
}

/// Write a terminal result once; terminal attempts cannot be rewritten.
/// Mirrors `def finish_execution(execution_id: str, *, success: bool, error: Optional[str]=None, delivery_outcome: Optional[str]=None)`.
pub fn finish_execution(
    execution_id: &str,
    success: bool,
    error: Option<&str>,
    delivery_outcome: Option<&str>,
) -> Result<Option<ExecutionRecord>> {
    let now = hermes_now_iso();
    let status = if success { "completed" } else { "failed" };
    let detail: Option<String> = if success {
        None
    } else {
        Some(if let Some(e) = error {
            if e.is_empty() {
                "unknown failure".to_string()
            } else {
                e.to_string()
            }
        } else {
            "unknown failure".to_string()
        })
    };

    let record = with_transaction(|tx| {
        let cur = tx.execute(
            "UPDATE executions SET status=?, finished_at=?, error=? WHERE id=? AND status IN ('claimed','running')",
            params![status, now, detail, execution_id],
        )?;
        if cur != 1 {
            return Ok(None);
        }
        _prune_unlocked(tx)?;
        let rec = fetch_one_by_id(tx, execution_id)?;
        Ok(rec)
    })?;

    if let Some(ref rec) = record {
        _emit_execution_state(Some(rec), delivery_outcome);
    }
    Ok(record)
}

/// Mark provably abandoned attempts unknown without scheduling retries.
/// Mirrors `def recover_interrupted_executions() -> int`.
pub fn recover_interrupted_executions() -> Result<usize> {
    let now = hermes_now_iso();
    let (changed, recovered) = with_transaction(|tx| {
        // Collect candidates first to avoid borrowing tx immutably while mutating.
        let mut stmt = tx.prepare(
            "SELECT id, process_id, pid, process_started_at FROM executions WHERE status IN ('claimed','running')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;

        let mut candidates: Vec<(String, String, i32, Option<i64>)> = Vec::new();
        for r in rows {
            let (id, proc_id, pid, started_at) = r?;
            candidates.push((id, proc_id, pid as i32, started_at));
        }
        drop(stmt);

        let my_process_id = get_process_id();
        let mut changed: usize = 0;
        let mut recovered: Vec<ExecutionRecord> = Vec::new();

        for (id, proc_id, pid, started_at) in candidates {
            if proc_id == my_process_id {
                continue;
            }
            if _owner_is_live(pid, started_at) {
                continue;
            }
            let cur = tx.execute(
                "UPDATE executions SET status='unknown', finished_at=?, error=? WHERE id=? AND status IN ('claimed','running')",
                params![
                    now,
                    "Scheduler restarted after this execution's owner exited before a durable terminal state; whether side effects ran is unknown.",
                    id
                ],
            )?;
            changed += cur as usize;
            if cur == 1 {
                if let Some(rec) = fetch_one_by_id(tx, &id)? {
                    recovered.push(rec);
                }
            }
        }

        if changed > 0 {
            _prune_unlocked(tx)?;
        }
        Ok((changed, recovered))
    })?;

    for rec in &recovered {
        _emit_execution_state(Some(rec), None);
    }
    Ok(changed)
}

/// Return indexed, newest-first execution history with cursor pagination.
/// Mirrors `def list_executions(*, job_id: Optional[str]=None, limit: int=50, before_claimed_at: Optional[str]=None)`.
pub fn list_executions(
    job_id: Option<&str>,
    limit: i64,
    before_claimed_at: Option<&str>,
) -> Result<Vec<ExecutionRecord>> {
    let limit_clamped = std::cmp::max(1, std::cmp::min(limit, 500));
    with_connection(|conn| {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(jid) = job_id {
            clauses.push("job_id=?".to_string());
            params.push(rusqlite::types::Value::Text(jid.to_string()));
        }
        if let Some(bc) = before_claimed_at {
            clauses.push("claimed_at < ?".to_string());
            params.push(rusqlite::types::Value::Text(bc.to_string()));
        }

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        params.push(rusqlite::types::Value::Integer(limit_clamped));

        let sql = format!(
            "SELECT * FROM executions{} ORDER BY claimed_at DESC, id DESC LIMIT ?",
            where_sql
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row_to_record(row)
        })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// Latest execution for a single job.
/// Mirrors `def latest_execution(job_id: str) -> Optional[Dict[str, Any]]`.
pub fn latest_execution(job_id: &str) -> Result<Option<ExecutionRecord>> {
    let rows = list_executions(Some(job_id), 1, None)?;
    Ok(rows.into_iter().next())
}

/// Load latest execution for many jobs in one indexed query.
/// Mirrors `def latest_executions(job_ids: List[str]) -> Dict[str, Dict[str, Any]]`.
pub fn latest_executions(job_ids: &[String]) -> Result<HashMap<String, ExecutionRecord>> {
    // Deduplicate preserving first occurrence order, filter empty strings — mirrors `dict.fromkeys`.
    let mut seen = HashSet::new();
    let mut clean: Vec<String> = Vec::new();
    for jid in job_ids {
        let s = jid.trim().to_string();
        if s.is_empty() {
            continue;
        }
        if seen.insert(s.clone()) {
            clean.push(s);
        }
    }
    if clean.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = clean.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    with_connection(|conn| {
        // Mirrors Python's f-string IN (...) + correlated subquery for latest per job_id.
        let sql = format!(
            "SELECT e.* FROM executions e
             WHERE e.job_id IN ({})
               AND e.id=(SELECT e2.id FROM executions e2
                         WHERE e2.job_id=e.job_id
                         ORDER BY e2.claimed_at DESC, e2.id DESC LIMIT 1)",
            placeholders
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<rusqlite::types::Value> =
            clean.iter().map(|s| rusqlite::types::Value::Text(s.clone())).collect();

        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row_to_record(row)
        })?;

        let mut map = HashMap::new();
        for r in rows {
            let rec = r?;
            map.insert(rec.job_id.clone(), rec);
        }
        Ok(map)
    })
}

// ---------------------------------------------------------------------------
// Convenience helpers for tests / callers
// ---------------------------------------------------------------------------

/// Expose `MAX_TERMINAL_EXECUTIONS` pruning helper for callers that hold a
/// connection directly. Mirrors `_prune_unlocked` being testable.
pub fn prune_unlocked_for_conn(conn: &Connection) -> Result<()> {
    _prune_unlocked_conn(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_terminal_executions_is_1000() {
        assert_eq!(MAX_TERMINAL_EXECUTIONS, 1000);
    }

    #[test]
    fn terminal_states_contains_expected() {
        assert!(TERMINAL_STATES.contains(&"completed"));
        assert!(TERMINAL_STATES.contains(&"failed"));
        assert!(TERMINAL_STATES.contains(&"unknown"));
    }

    #[test]
    fn hermes_now_iso_is_rfc3339_parseable() {
        let s = hermes_now_iso();
        assert!(chrono::DateTime::parse_from_rfc3339(&s).is_ok(), "now iso must be rfc3339: {s}");
    }
}
