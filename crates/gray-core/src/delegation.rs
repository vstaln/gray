//! Delegation config + state helpers — steal hermes tools/delegate_tool.py + async_delegation.py
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, Semaphore};

/// Config mirror of hermes delegation: max_concurrent_children=10, max_spawn_depth=1
#[derive(Debug, Clone)]
pub struct DelegateConfig {
    pub max_concurrent_children: usize,
    pub max_spawn_depth: usize,
    pub orchestrator_enabled: bool,
    pub child_timeout: Option<Duration>,
    pub max_iterations: usize,
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self { max_concurrent_children: 10, max_spawn_depth: 1, orchestrator_enabled: true, child_timeout: None, max_iterations: 250 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegateRole { Leaf, Orchestrator }

pub fn normalize_role(s: Option<&str>) -> DelegateRole {
    match s.map(|v| v.to_ascii_lowercase()).as_deref() {
        Some("orchestrator") => DelegateRole::Orchestrator,
        _ => DelegateRole::Leaf,
    }
}

pub const DELEGATE_BLOCKED: &[&str] = &["delegate_task"];

#[derive(Debug, Clone)]
pub struct ActiveRecord {
    pub subagent_id: String,
    pub delegation_id: String,
    pub goal: String,
    pub started_at: Instant,
    pub depth: usize,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct CompletionEvent {
    pub delegation_id: String,
    pub subagent_id: String,
    pub goal: String,
    pub output: String,
    pub is_error: bool,
}

pub fn state_db_path() -> PathBuf {
    if let Ok(gh) = std::env::var("GRAY_HOME") {
        return PathBuf::from(gh).join("state.db");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".gray/state.db");
    }
    PathBuf::from(".gray/state.db")
}

pub fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS async_delegations (delegation_id TEXT PRIMARY KEY, subagent_id TEXT, goal TEXT, status TEXT, created_at INTEGER)",
        [],
    )?;
    Ok(())
}

pub fn persist_dispatch(delegation_id: &str, subagent_id: &str, goal: &str, status: &str) -> anyhow::Result<()> {
    let path = state_db_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let conn = rusqlite::Connection::open(&path)?;
    init_schema(&conn)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    conn.execute(
        "INSERT OR REPLACE INTO async_delegations (delegation_id, subagent_id, goal, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![delegation_id, subagent_id, goal, status, now],
    )?;
    Ok(())
}

pub fn persist_completion(delegation_id: &str, status: &str) -> anyhow::Result<()> {
    let path = state_db_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let conn = rusqlite::Connection::open(&path)?;
    init_schema(&conn)?;
    conn.execute(
        "UPDATE async_delegations SET status=?1 WHERE delegation_id=?2",
        rusqlite::params![status, delegation_id],
    )?;
    Ok(())
}

/// Load pending delegations and re-queue them onto the global completion channel.
/// Returns number restored. Stale rows older than 48h are marked dropped.
pub fn restore_undelivered(state: &DelegationState) -> usize {
    let path = state_db_path();
    let conn = match rusqlite::Connection::open(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    if init_schema(&conn).is_err() { return 0; }
    let mut stmt = match conn.prepare("SELECT delegation_id, subagent_id, goal, status, created_at FROM async_delegations WHERE status='dispatched' ORDER BY created_at") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let rows = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?))
    }) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut restored = 0;
    for row in rows.flatten() {
        let (delegation_id, subagent_id, goal, _status, created_at) = row;
        // stale 48h drop
        if now - created_at > 48 * 3600 {
            let _ = conn.execute("UPDATE async_delegations SET status='dropped' WHERE delegation_id=?1", rusqlite::params![delegation_id]);
            continue;
        }
        let ev = CompletionEvent { delegation_id: delegation_id.clone(), subagent_id: subagent_id.clone(), goal: goal.clone(), output: format!("[restored] {goal}"), is_error: false };
        if state.completion_tx.send(ev).is_ok() { restored += 1; }
    }
    restored
}

// --- DelegationState ---

/// Shared delegation state (global, per-process).
pub struct DelegationState {
    pub sem: Arc<Semaphore>,
    pub active: RwLock<HashMap<String, ActiveRecord>>,
    pub paused: AtomicBool,
    pub completion_tx: tokio::sync::mpsc::UnboundedSender<CompletionEvent>,
    pub completion_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<CompletionEvent>>>,
}

impl DelegationState {
    pub fn new(max_concurrent: usize) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            sem: Arc::new(Semaphore::new(max_concurrent.max(1))),
            active: RwLock::new(HashMap::new()),
            paused: AtomicBool::new(false),
            completion_tx: tx,
            completion_rx: Mutex::new(Some(rx)),
        })
    }
    pub fn is_paused(&self) -> bool { self.paused.load(Ordering::Relaxed) }
    pub fn set_paused(&self, v: bool) { self.paused.store(v, Ordering::Relaxed); }

    /// Drain all pending completions without blocking (for repl idle poll).
    pub fn try_drain(&self) -> Vec<CompletionEvent> {
        let mut out = Vec::new();
        if let Ok(mut guard) = self.completion_rx.lock() {
            if let Some(rx) = guard.as_mut() {
                while let Ok(ev) = rx.try_recv() { out.push(ev); }
            }
        }
        out
    }
}

impl Default for DelegationState {
    fn default() -> Self { Self::new(10).as_ref().clone() }
}

impl Clone for DelegationState {
    fn clone(&self) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self { sem: self.sem.clone(), active: RwLock::new(HashMap::new()), paused: AtomicBool::new(self.paused.load(Ordering::Relaxed)), completion_tx: tx, completion_rx: Mutex::new(Some(rx)) }
    }
}

// Global singleton for REPL drain (process-wide completion_queue)
static GLOBAL_STATE: OnceLock<Arc<DelegationState>> = OnceLock::new();

pub fn global_delegation_state() -> Arc<DelegationState> {
    GLOBAL_STATE.get_or_init(|| {
        let s = DelegationState::new(10);
        // best-effort restore on first access
        let _ = restore_undelivered(&s);
        s
    }).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_creates() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let cnt: i64 = conn.query_row("SELECT count(*) FROM async_delegations", [], |r| r.get(0)).unwrap();
        assert_eq!(cnt, 0);
    }
}
