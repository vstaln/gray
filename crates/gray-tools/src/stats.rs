//! Per-call tool-output stats (T0.2): a token meter, not a behavior change.
//!
//! `est_tokens = bytes / 4` is a documented approximation; swap for a real
//! tokenizer later.
//!
//! Every tool call is classified retrieval / action / other and metered at
//! `Registry::execute`, so `GRAY_TOOL_STATS=1` yields the
//! tokens-to-success accounting the literature asks for: coding agents spend
//! most of their context budget on retrieval (arXiv:2608.13568, "Does a
//! Language Server Save Tokens for Coding Agents?"), and resource use belongs
//! in the score rather than beside it (arXiv:2608.05519, EcoAgent-Bench).
//! Classification is by tool name and honest about its limit: gray's default
//! surface is `bash` alone, where retrieval and action share one tool, so
//! `bash` meters as `other`. The JSONL sink lands in `$GRAY_HOME/logs/`, so
//! set both vars (`GRAY_TOOL_STATS=1 GRAY_HOME=~/.gray`) to collect it.

/// Divisor for the documented bytes/4 token approximation.
pub const EST_TOKENS_DIVISOR: u64 = 4;

/// Values for [`ToolStats::truncated_by`].
pub const CUT_LINES: &str = "lines";
pub const CUT_BYTES: &str = "bytes";
pub const CUT_NONE: &str = "none";

/// Approximate token count for `bytes` of tool-output text.
pub fn est_tokens(bytes: u64) -> u64 {
    bytes / EST_TOKENS_DIVISOR
}

/// Tool classes for the retrieval/action split (arXiv:2608.13568 regimes).
pub const CLASS_RETRIEVAL: &str = "retrieval";
pub const CLASS_ACTION: &str = "action";
pub const CLASS_OTHER: &str = "other";

/// Tools that meter their own call with richer detail (read knows its
/// truncation); `Registry::execute` skips these so one call is never counted
/// twice. Anything added here must self-report via [`ToolStats::report`].
pub const SELF_REPORTING: &[&str] = &["read"];

/// Classify a tool by name: read-only lookup tools are `retrieval` (lexical
/// retrieval in 2608.13568's terms), file-mutating tools are `action`,
/// everything else (bash, plugins) is `other`.
pub fn classify(tool: &str) -> &'static str {
    match tool {
        "read" | "grep" | "find" | "ls" => CLASS_RETRIEVAL,
        "edit" | "write" => CLASS_ACTION,
        _ => CLASS_OTHER,
    }
}

/// Stats gate: only emit when `GRAY_TOOL_STATS=1`.
pub fn enabled() -> bool {
    matches!(std::env::var("GRAY_TOOL_STATS").as_deref(), Ok("1"))
}

/// One tool-call record.
pub struct ToolStats<'a> {
    pub tool: &'a str,
    /// retrieval / action / other (see [`classify`]).
    pub class: &'a str,
    pub path: &'a str,
    pub bytes: u64,
    pub lines: u64,
    pub truncated_by: &'a str,
}

impl ToolStats<'_> {
    /// `tool=read class=retrieval path=… bytes=… lines=… est_tokens=… truncated_by=…`
    pub fn line(&self) -> String {
        format!(
            "tool={} class={} path={} bytes={} lines={} est_tokens={} truncated_by={}",
            self.tool,
            self.class,
            self.path,
            self.bytes,
            self.lines,
            est_tokens(self.bytes),
            self.truncated_by,
        )
    }

    /// Log + append JSON to `$GRAY_HOME/logs/tool-stats.jsonl`. No-op unless
    /// [`enabled`]; file I/O is best-effort and never panics.
    pub fn report(&self) {
        if !enabled() {
            return;
        }
        log::info!(target: "gray_tools", "{}", self.line());
        let Ok(home) = std::env::var("GRAY_HOME") else {
            return;
        };
        let dir = std::path::Path::new(&home).join("logs");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let rec = serde_json::json!({
            "tool": self.tool, "class": self.class, "path": self.path,
            "bytes": self.bytes, "lines": self.lines,
            "est_tokens": est_tokens(self.bytes),
            "truncated_by": self.truncated_by,
        });
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("tool-stats.jsonl"))
            && let Err(e) = writeln!(f, "{rec}")
        {
            log::debug!(target: "gray_tools", "tool-stats append failed: {e}");
        }
    }
}

#[path = "stats_tests.rs"]
#[cfg(test)]
mod tests;
