//! shell/contract.rs: shared shell types.
//!
//! Blocking and managed-background commands share execution budgets and views.

use std::time::Duration;

use tokio::process::Child;

// budgets and limits

/// How long one bash call blocks before the command moves to a background
/// job, when the model passes no `timeout`. Reaching it NEVER kills: the
/// command keeps running as a job, the call returns its log and pgid, and
/// the session is woken when it finishes. (A killing 30s, then 120s,
/// default once cut real builds short, and the agent read the kill as a
/// failed command.) `GRAY_BASH_TIMEOUT_SECS` overrides it per process.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Cap for an explicitly requested `timeout` (one hour).
pub const MAX_TIMEOUT_SECS: u64 = 3600;
/// Slack added to a bare leading `sleep N` when no `timeout` was passed, so
/// an in-band wait (`sleep 300 && tail log`) blocks as written instead of
/// itself turning into a background job.
pub const SLEEP_SLACK_SECS: u64 = 30;
pub const VIEW_BUDGET_LINES: usize = 2000;
pub const VIEW_HEAD_FRACTION: f32 = 0.25; // head 25%, tail 75%
pub const MEM_HEAD_BYTES: usize = 6 * 1024;
pub const MEM_TAIL_BYTES: usize = 6 * 1024;
/// Inline budget for bash results: head + tail, 12 KiB (~3k tokens).
/// The full log always persists on disk; `grep` it instead of rerunning.
/// Cut back from 24+24 KiB: exploration spends most of its tokens on tool
/// output, and a result re-sent in every later request is re-billed every
/// later request (cost grows with the square of the turn count). The
/// `dd`/`sed` resume hint below names the exact byte window, so an
/// elided middle costs one paged read instead of a rerun.
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

// Unix owns a detached process group; Windows owns a non-inheritable job.
// Keep the job alive until termination/reaping; a PID cannot replace it.
pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    #[cfg(not(windows))]
    pub pgid: i32,
    #[cfg(windows)]
    pub job: super::windows::Job,
}
