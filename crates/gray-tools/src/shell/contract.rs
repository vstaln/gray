//! shell/contract.rs — shared shell-task types (Phase 0, brief 0).
//!
//! Every public type the shell briefs share. Behavior lives with the
//! owners: exit→exit.rs, view/header→view.rs, fence→fence.rs,
//! pump→pump.rs, spawn→spawn.rs, registry→registry.rs, kill→kill.rs,
//! wake/sleep→wake.rs.

#![allow(dead_code, unused_variables)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

// NOTE: `regex` backs `NotifyPattern` (brief 3C, 1 MiB size limit).
use tokio::process::Child;

// ── budgets & limits ──────────────────────────────────────────────

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const MAX_TIMEOUT_SECS: u64 = 600;
pub const VIEW_BUDGET_BYTES: usize = 50 * 1024;
pub const VIEW_BUDGET_LINES: usize = 2000;
pub const VIEW_HEAD_FRACTION: f32 = 0.25; // head 25%, tail 75%
pub const PROMOTION_TAIL_BYTES: usize = 2048;
pub const MEM_HEAD_BYTES: usize = 16 * 1024;
pub const MEM_TAIL_BYTES: usize = 48 * 1024;
pub const EXITED_TASK_TTL: Duration = Duration::from_secs(30 * 60);
pub const EXITED_TASK_KEEP: usize = 20;

// ── core types ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TaskId(pub u32); // Display: "t{n}"

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// Compiled `notify_on` regex (brief 3C): size-limited to 1 MiB.
/// Invalid expressions fail here so the tool can report the regex error
/// plus an example. Empty never matches (preserved stub rule — a bare
/// empty regex would match every line).
#[derive(Clone, Debug)]
pub struct NotifyPattern {
    expr: String,
    regex: regex::Regex,
}

impl NotifyPattern {
    pub fn new(expr: &str) -> Result<Self, regex::Error> {
        let regex = regex::RegexBuilder::new(expr).size_limit(1 << 20).build()?;
        Ok(Self {
            expr: expr.to_string(),
            regex,
        })
    }
    pub fn as_str(&self) -> &str {
        &self.expr
    }
    pub fn matches(&self, line: &str) -> bool {
        !self.expr.is_empty() && self.regex.is_match(line)
    }
}

#[derive(Clone, Debug)]
pub struct ExitReport {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub effective: i32,
    pub label: String,
    pub note: Option<String>,
    pub benign: bool,
}

pub struct View {
    pub body: String,
    pub shown_lines: (usize, usize),
    pub omitted_lines: usize,
    pub omitted_bytes: usize,
    pub omitted_range: Option<(u64, u64)>,
    pub total_lines: usize,
    pub total_bytes: u64,
}

pub enum TaskState {
    Running,
    Exited { report: ExitReport, at: Instant },
}

pub struct TaskInfo {
    pub id: TaskId,
    pub pid: u32,
    pub pgid: i32,
    pub command: String,
    pub started: Instant,
    pub log_path: PathBuf,
    pub bytes: u64,
    pub state: TaskState,
}

pub enum WaitMode {
    None,
    Output,
    Exit,
}

#[derive(Clone, Debug)] // Clone: broadcast::Sender<WakeEvent> requires it (P1D wiring fix)
pub enum WakeEvent {
    Exited { id: TaskId, report: ExitReport },
    PatternMatched { id: TaskId, line: String },
    UserInput,
}

pub enum KillTarget {
    Task(TaskId),
    Pid(u32),
    Port(u16),
}

pub struct KillReport {
    pub pid: u32,
    pub method: KillMethod,
    pub report: Option<ExitReport>,
    pub describe: String,
}

pub enum KillMethod {
    TermAnswered(Duration),
    TermIgnoredThenKill(Duration),
    AlreadyExited,
}

pub struct PumpSummary {
    pub total_bytes: u64,
    pub total_lines: usize,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    /// Set when the log dir/file could not be created or a write failed.
    /// The memory view stays alive either way; never panics.
    pub log_write_failed: bool,
}

// spawn.rs (brief 1D; P1D ruling: `task` param added — the 2-arg form
// cannot set the brief-mandated `GRAY_TASK_ID` env on the child)
pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    pub pgid: i32,
    pub start_ticks: Option<u64>,
}
