//! shell/contract.rs: shared shell types.
//!
//! Blocking `bash` only: budgets, exit reports, bounded views. No task
//! registry, no background tasks, no wake events: a command runs, the tool
//! waits (killing the process group on timeout/cancel), and the full log
//! stays on disk for `grep`.

use std::time::Duration;

use tokio::process::Child;

// budgets and limits

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const MAX_TIMEOUT_SECS: u64 = 600;
pub const VIEW_BUDGET_BYTES: usize = 50 * 1024;
pub const VIEW_BUDGET_LINES: usize = 2000;
pub const VIEW_HEAD_FRACTION: f32 = 0.25; // head 25%, tail 75%
pub const MEM_HEAD_BYTES: usize = 6 * 1024;
pub const MEM_TAIL_BYTES: usize = 6 * 1024;
/// Inline budget for bash results: head + tail, ~12 KiB (~3k tokens).
/// The full log always persists on disk; `grep` it instead of rerunning.
pub const INLINE_BUDGET_BYTES: usize = MEM_HEAD_BYTES + MEM_TAIL_BYTES;
/// How long the tool waits for the output pump after the child exits.
/// A grandchild inheriting the pipes keeps the pump alive forever, so the
/// result must never wait past this.
pub const PUMP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

// core types

#[derive(Clone, Debug)]
pub struct ExitReport {
    pub effective: i32,
    pub label: String,
    pub note: Option<String>,
}

pub struct View {
    pub body: String,
    pub shown_lines: (usize, usize),
    pub omitted_lines: usize,
    pub omitted_bytes: usize,
    pub omitted_range: Option<(u64, u64)>,
    pub total_lines: usize,
    pub total_bytes: u64,
    /// Raw bytes contain `\r`: the rendered body folds CRLF for display, so
    /// the header must say so (a CRLF file and an LF file render identically).
    pub has_cr: bool,
}

pub struct PumpSummary {
    pub total_bytes: u64,
    pub total_lines: usize,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    /// Any streamed chunk contained `\r` (exact: tracked while pumping, so
    /// CRs in the omitted middle still count).
    pub has_cr: bool,
    /// Set when the log dir/file could not be created or a write failed.
    /// The memory view stays alive either way; never panics.
    pub log_write_failed: bool,
}

// spawn.rs: the detached child plus its group id (setsid: pgid == pid).
pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    pub pgid: i32,
}
