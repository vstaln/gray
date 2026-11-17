use std::time::Duration;

use tokio::process::Child;

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

pub const MAX_TIMEOUT_SECS: u64 = 3600;

pub const SLEEP_SLACK_SECS: u64 = 30;
pub const VIEW_BUDGET_LINES: usize = 2000;
pub const VIEW_HEAD_FRACTION: f32 = 0.25;
pub fn mem_head_bytes() -> usize {
    gray_core::tool_out::inline_bytes() / 2
}

pub fn mem_tail_bytes() -> usize {
    gray_core::tool_out::inline_bytes() - mem_head_bytes()
}

pub fn inline_budget_bytes() -> usize {
    gray_core::tool_out::inline_bytes()
}

pub const PUMP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

pub const DETACHED_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub const TERM_KILL_GRACE: Duration = Duration::from_secs(2);

pub const EXIT_STDIO_GRACE_MS: u64 = 100;

#[derive(Clone, Debug)]
pub struct ExitReport {
    pub label: String,
    pub note: Option<String>,
    pub code: i32,
}

pub struct View {
    pub body: String,
    pub shown_lines: (usize, usize),
    pub omitted_lines: usize,
    pub omitted_bytes: usize,
    pub omitted_range: Option<(u64, u64)>,
    pub total_lines: usize,

    pub has_cr: bool,
}

pub struct PumpSummary {
    pub total_bytes: u64,
    pub total_lines: usize,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,

    pub has_cr: bool,

    pub log_write_failed: bool,
}

pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    #[cfg(not(windows))]
    pub pgid: i32,
    #[cfg(windows)]
    pub job: super::windows::Job,
}
