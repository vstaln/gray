//! Delegation config + state helpers — steal hermes tools/delegate_tool.py + async_delegation.py
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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

/// Shared delegation state — ponytail: global semaphore, per-process.
pub struct DelegationState {
    pub sem: Arc<Semaphore>,
    pub active: RwLock<HashMap<String, ActiveRecord>>,
    pub paused: AtomicBool,
    pub completion_tx: tokio::sync::mpsc::UnboundedSender<CompletionEvent>,
    pub completion_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<CompletionEvent>>>,
    // ponytail: SQLite durability stub — path is ~/.gray/state.db, but we keep in-memory for now
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
}

impl Default for DelegationState {
    fn default() -> Self { Self::new(10).as_ref().clone() }
}

// clone for Default impl — not used directly
impl Clone for DelegationState {
    fn clone(&self) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self { sem: self.sem.clone(), active: RwLock::new(HashMap::new()), paused: AtomicBool::new(self.paused.load(Ordering::Relaxed)), completion_tx: tx, completion_rx: Mutex::new(Some(rx)) }
    }
}
