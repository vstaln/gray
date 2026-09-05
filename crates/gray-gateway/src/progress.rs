//! Hermes-style progress bubbles (port of `hermes-gateway/src/turn.rs`
//! `on_tool_event` composition + `drain_progress_messages` grouping).
//!
//! Pure state machine — no IO. The daemon task in [`crate::daemon`] owns
//! sending, editing and deleting the bubble; this only decides WHAT text
//! goes in it.

/// Max preview chars for tool args in one bubble line.
pub const PROGRESS_PREVIEW_CAP: usize = 80;

/// `⏳ {name}…` — a tool just started.
pub fn tool_start_line(name: &str) -> String {
    format!("⏳ {name}…")
}

/// `🔧 {name}: "{compact args}"`, or `🔧 {name}…` when there are no args.
pub fn tool_end_line(name: &str, args: &serde_json::Value) -> String {
    match summarize_args(args) {
        Some(preview) => format!("🔧 {name}: \"{preview}\""),
        None => format!("🔧 {name}…"),
    }
}

fn summarize_args(args: &serde_json::Value) -> Option<String> {
    if args.is_null() || args == &serde_json::Value::Object(Default::default()) {
        return None;
    }
    let compact = serde_json::to_string(args).unwrap_or_else(|_| "…".into());
    Some(truncate_chars(&compact, PROGRESS_PREVIEW_CAP))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Accumulated bubble lines with Hermes `(×N)` dedup.
#[derive(Debug, Default)]
pub struct ProgressLines {
    lines: Vec<String>,
}

impl ProgressLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn push_start(&mut self, name: &str) {
        self.push_dedup(tool_start_line(name));
    }

    /// Replaces the last line when it is this tool's still-open `⏳` line,
    /// otherwise appends (dedup still applies).
    pub fn push_end(&mut self, name: &str, args: &serde_json::Value) {
        let start = tool_start_line(name);
        if self.lines.last().is_some_and(|l| *l == start) {
            self.lines.pop();
        }
        self.push_dedup(tool_end_line(name, args));
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Hermes `split_overflow`: group lines so each joined group fits in
    /// `max_utf16` UTF-16 units. One overlong line forms its own group.
    pub fn split_groups(&self, max_utf16: usize) -> Vec<String> {
        let mut groups: Vec<String> = Vec::new();
        let mut cur: Vec<&str> = Vec::new();
        let mut cur_len = 0usize; // joined length incl. newlines
        for line in &self.lines {
            let lw = utf16_len(line);
            let add = if cur.is_empty() { lw } else { lw + 1 };
            if !cur.is_empty() && cur_len + add > max_utf16 {
                groups.push(cur.join("\n"));
                cur.clear();
                cur_len = 0;
            }
            cur.push(line);
            cur_len += if cur.len() == 1 { lw } else { lw + 1 };
        }
        if !cur.is_empty() {
            groups.push(cur.join("\n"));
        }
        groups
    }

    /// Append, collapsing consecutive identical lines into `{line} (×N)`.
    fn push_dedup(&mut self, msg: String) {
        let base = strip_count_suffix(self.lines.last().map(String::as_str).unwrap_or(""));
        if base == msg {
            let n = count_suffix(self.lines.last().map(String::as_str).unwrap_or("")) + 1;
            *self.lines.last_mut().unwrap() = format!("{msg} (×{n})");
        } else {
            self.lines.push(msg);
        }
    }
}

/// `"foo (×3)"` → `"foo"`; anything else unchanged.
fn strip_count_suffix(line: &str) -> &str {
    match line.rfind(" (×") {
        Some(i)
            if line.ends_with(')')
                && line[i + 4..line.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit()) =>
        {
            &line[..i]
        }
        _ => line,
    }
}

/// `"foo (×3)"` → `3`; anything else → `1`.
fn count_suffix(line: &str) -> usize {
    match line.rfind(" (×") {
        Some(i) if line.ends_with(')') => line[i + 4..line.len() - 1].parse().unwrap_or(1),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn start_line_format() {
        assert_eq!(tool_start_line("terminal"), "⏳ terminal…");
    }

    #[test]
    fn end_line_without_args() {
        assert_eq!(tool_end_line("read", &json!(null)), "🔧 read…");
        assert_eq!(tool_end_line("read", &json!({})), "🔧 read…");
    }

    #[test]
    fn end_line_with_args_preview() {
        let l = tool_end_line("terminal", &json!({"command": "ls -la"}));
        assert_eq!(l, "🔧 terminal: \"{\"command\":\"ls -la\"}\"");
    }

    #[test]
    fn end_line_truncates_long_args_to_80_chars() {
        let big = "x".repeat(200);
        let l = tool_end_line("exec", &json!({"command": big}));
        assert!(l.chars().count() <= "🔧 exec: \"\"".chars().count() + 80);
    }

    #[test]
    fn end_replaces_matching_start_line() {
        let mut p = ProgressLines::new();
        p.push_start("terminal");
        p.push_end("terminal", &json!({"command": "ls"}));
        assert_eq!(p.text(), "🔧 terminal: \"{\"command\":\"ls\"}\"");
    }

    #[test]
    fn end_pushes_when_last_line_is_different_tool() {
        let mut p = ProgressLines::new();
        p.push_start("read");
        p.push_end("terminal", &json!(null));
        assert_eq!(p.text(), "⏳ read…\n🔧 terminal…");
    }

    #[test]
    fn consecutive_identical_lines_dedup_with_count() {
        let mut p = ProgressLines::new();
        p.push_start("execute_code");
        p.push_start("execute_code");
        p.push_start("execute_code");
        assert_eq!(p.text(), "⏳ execute_code… (×3)");
    }

    #[test]
    fn dedup_resets_after_different_line() {
        let mut p = ProgressLines::new();
        p.push_start("a");
        p.push_start("a");
        p.push_start("b");
        p.push_start("b");
        assert_eq!(p.text(), "⏳ a… (×2)\n⏳ b… (×2)");
    }

    #[test]
    fn split_groups_keeps_short_lines_together() {
        let mut p = ProgressLines::new();
        p.push_start("a");
        p.push_start("b");
        assert_eq!(p.split_groups(2000), vec!["⏳ a…\n⏳ b…".to_string()]);
    }

    #[test]
    fn split_groups_rolls_overflow_into_new_group() {
        let mut p = ProgressLines::new();
        p.push_start("aaaa");
        p.push_start("bbbb");
        // "⏳ aaaa…" is 7 UTF-16 units; cap 10 forces a split.
        assert_eq!(
            p.split_groups(10),
            vec!["⏳ aaaa…".to_string(), "⏳ bbbb…".to_string()]
        );
    }
}
