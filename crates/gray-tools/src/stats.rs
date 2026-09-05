//! Per-call tool-output stats (T0.2): a token meter, not a behavior change.
//!
//! `est_tokens = bytes / 4` is a documented approximation; swap for a real
//! tokenizer later. Wiring into `Registry::execute` lands with the read
//! module split (T1.1); this module is the shared helper it will call.

/// Divisor for the documented bytes/4 token approximation.
pub const EST_TOKENS_DIVISOR: u64 = 4;

/// Values for [`ToolStats::truncated_by`].
pub const CUT_LINES: &str = "lines";
pub const CUT_BYTES: &str = "bytes";
pub const CUT_CLAMP: &str = "clamp";
pub const CUT_NONE: &str = "none";

/// Approximate token count for `bytes` of tool-output text.
pub fn est_tokens(bytes: u64) -> u64 {
    bytes / EST_TOKENS_DIVISOR
}

/// Stats gate: only emit when `GRAY_TOOL_STATS=1`.
pub fn enabled() -> bool {
    matches!(std::env::var("GRAY_TOOL_STATS").as_deref(), Ok("1"))
}

/// One tool-call record. `notice` is the short kind (`empty`, `eof`, …) or `none`.
pub struct ToolStats<'a> {
    pub tool: &'a str,
    pub path: &'a str,
    pub bytes: u64,
    pub lines: u64,
    pub truncated_by: &'a str,
    pub notice: &'a str,
}

impl ToolStats<'_> {
    /// `tool=read path=… bytes=… lines=… est_tokens=… truncated_by=… notice=…`
    pub fn line(&self) -> String {
        format!(
            "tool={} path={} bytes={} lines={} est_tokens={} truncated_by={} notice={}",
            self.tool,
            self.path,
            self.bytes,
            self.lines,
            est_tokens(self.bytes),
            self.truncated_by,
            self.notice,
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
            "tool": self.tool, "path": self.path,
            "bytes": self.bytes, "lines": self.lines,
            "est_tokens": est_tokens(self.bytes),
            "truncated_by": self.truncated_by, "notice": self.notice,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_bytes_over_four() {
        assert_eq!(est_tokens(51_200), 12_800);
        assert_eq!(est_tokens(7), 1);
    }

    #[test]
    fn line_format_matches_spec() {
        let s = ToolStats {
            tool: "read",
            path: "long.txt",
            bytes: 51_200,
            lines: 1846,
            truncated_by: CUT_BYTES,
            notice: "none",
        };
        assert_eq!(
            s.line(),
            "tool=read path=long.txt bytes=51200 lines=1846 \
             est_tokens=12800 truncated_by=bytes notice=none"
        );
    }
}
