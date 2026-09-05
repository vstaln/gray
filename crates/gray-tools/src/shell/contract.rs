//! shell/contract.rs — V2 frozen contract (Phase 0, brief 0).
//!
//! Every public type + signature the later briefs fill in. Bodies are
//! `todo!()` until the owning brief lands. Parallel implementers may add
//! private items freely; changing a `pub` signature requires a message back
//! to the orchestrator, never a silent edit.
//!
//! Status: NOT wired into the crate yet (`pub mod shell` + `bash.rs`
//! shrink land with brief 1D). This file is the compile target, not yet
//! part of the build. Plan source: `/tmp/opencode/read-eff/src/data/`.
//!
//! Owners: exit→1A, view/header→1B, fence→1B, pump→1C, spawn→1D,
//! registry→2A, kill→2D, wake/sleep→3A–3C.

#![allow(dead_code, unused_variables)]

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// NOTE: `regex` is not a gray-tools dep yet; brief 3C adds it via Cargo
// (orchestrator approves deps). `tokio` sync/task/process come from the
// existing workspace dep.
use gray_core::agent::ToolContext;
use regex::Regex;
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

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

// exit.rs (brief 1A)
pub fn exit_report(status: std::process::ExitStatus, command: &str) -> ExitReport {
    todo!()
}

// view.rs (brief 1B)
pub fn middle_out(log: &[u8], budget_bytes: usize, budget_lines: usize, base_offset: u64) -> View {
    todo!()
}
pub fn header(
    task: &TaskInfo,
    report: Option<&ExitReport>,
    view: Option<&View>,
    elapsed: Duration,
) -> String {
    todo!()
}
pub fn resume_hint(task: TaskId, view: &View) -> String {
    todo!()
}

// fence.rs (brief 1B)
pub fn fence(task: TaskId, body: &str) -> String {
    // escapes "</untrusted-output" inside body
    todo!()
}

// pump.rs (brief 1C)
pub struct Pump;
impl Pump {
    pub fn start(
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
        log_path: PathBuf,
        bytes_tx: watch::Sender<u64>,
        pattern: Option<Regex>,
        wake: Option<broadcast::Sender<WakeEvent>>,
    ) -> JoinHandle<PumpSummary> {
        todo!()
    }
}
pub struct PumpSummary {
    pub total_bytes: u64,
    pub total_lines: usize,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
}

// registry.rs (brief 2A)
pub struct ProcessRegistry;
pub fn registry() -> &'static ProcessRegistry {
    todo!()
}
impl ProcessRegistry {
    pub fn register(
        &self,
        session: &str,
        child: &Child,
        command: &str,
        log_path: PathBuf,
    ) -> TaskId {
        todo!()
    }
    pub fn get(&self, session: &str, id: TaskId) -> Option<TaskInfo> {
        todo!()
    }
    pub fn list(&self, session: &str) -> Vec<TaskInfo> {
        todo!()
    }
    pub fn mark_exited(&self, session: &str, id: TaskId, report: ExitReport) {
        todo!()
    }
    pub fn bytes_rx(&self, session: &str, id: TaskId) -> Option<watch::Receiver<u64>> {
        todo!()
    }
    pub fn exit_rx(
        &self,
        session: &str,
        id: TaskId,
    ) -> Option<watch::Receiver<Option<ExitReport>>> {
        todo!()
    }
    pub fn wake_tx(&self) -> broadcast::Sender<WakeEvent> {
        todo!()
    }
    pub fn notify_user_input(&self) {
        todo!()
    }
    pub fn gc(&self, session: &str) {
        todo!()
    }
    pub async fn shutdown_session(&self, session: &str) {
        todo!()
    }
}

// spawn.rs (brief 1D)
pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    pub pgid: i32,
    pub start_ticks: Option<u64>,
}
pub fn spawn(command: &str, cwd: &Path) -> io::Result<Spawned> {
    todo!()
}

// kill.rs (brief 2D)
pub async fn kill(target: KillTarget, session: &str, ctx: &ToolContext) -> Result<KillReport, String> {
    todo!()
}
pub fn pid_for_port(port: u16) -> io::Result<Option<(u32, String)>> {
    todo!()
}
pub fn still_same_process(pid: u32, start_ticks: Option<u64>) -> bool {
    todo!()
}
