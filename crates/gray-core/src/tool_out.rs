//! Shared tool-output policy: truncation caps and arg-validation helpers.
//!
//! Truncation policy (applied to every tool output): results are capped at
//! 2000 lines / 50 KiB, keeping head + tail with a `[truncated ...]`
//! annotation; error outputs are additionally hard-capped at 2 KiB.
//! (Moved from `gray-tools` so non-core tool crates like `gray-cron` share
//! the policy without depending on the whole builtin toolset.)

use serde_json::Value;

use crate::agent::ToolOutput;

/// Maximum number of lines kept in a successful tool output.
pub const MAX_LINES: usize = 2000;
/// Maximum size in bytes of a successful tool output.
pub const MAX_BYTES: usize = 50 * 1024;
/// Hard cap for error outputs (applied after the general truncation).
pub const MAX_ERROR_BYTES: usize = 2048;

/// Truncates a successful output: 2000-line / 50 KiB cap, head + tail kept,
/// with a `[truncated N lines / M bytes]` annotation in the middle.
pub fn truncate_output(text: &str) -> String {
    let mut notes: Vec<String> = Vec::new();

    // Line cap: keep first half + last half of the allowed budget.
    let total_lines = text.lines().count();
    let body = if total_lines > MAX_LINES {
        let dropped = total_lines - MAX_LINES;
        notes.push(format!("{dropped} lines"));
        let keep = MAX_LINES / 2;
        let all: Vec<&str> = text.lines().collect();
        let mut parts = all[..keep].to_vec();
        parts.extend_from_slice(&all[all.len() - keep..]);
        parts.join("\n")
    } else if text.ends_with('\n') {
        // `lines()` drops the trailing newline; preserve it verbatim.
        text.to_string()
    } else {
        text.to_string()
    };

    // Byte cap on what remains (head + tail around the annotation).
    if body.len() > MAX_BYTES {
        let dropped_bytes = body.len() - MAX_BYTES;
        notes.push(format!("{dropped_bytes} bytes"));
        let half = MAX_BYTES / 2;
        let head_end = floor_char_boundary(&body, half);
        let tail_start = ceil_char_boundary(&body, body.len() - half);
        format!(
            "{}\n{}\n{}",
            &body[..head_end],
            annotation(&notes),
            &body[tail_start..]
        )
    } else if notes.is_empty() {
        body
    } else {
        // Line-truncated but within byte budget: insert the annotation in
        // the middle without touching the rest of the content.
        let lines: Vec<&str> = body.lines().collect();
        let mid = lines.len() / 2;
        let mut out = lines[..mid].join("\n");
        out.push('\n');
        out.push_str(&annotation(&notes));
        out.push('\n');
        out.push_str(&lines[mid..].join("\n"));
        if body.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// Error outputs: general truncation, then a hard 2 KiB head cap.
pub fn truncate_error(text: &str) -> String {
    let truncated = truncate_output(text);
    if truncated.len() > MAX_ERROR_BYTES {
        let cut = floor_char_boundary(&truncated, MAX_ERROR_BYTES);
        format!("{}\n[error truncated to 2KiB]", &truncated[..cut])
    } else {
        truncated
    }
}

/// Wraps raw stdout-like text into a successful [`ToolOutput`].
pub fn finish(raw: String) -> ToolOutput {
    ToolOutput::ok(truncate_output(&raw))
}

/// Wraps raw failure text into an error [`ToolOutput`] (capped at 2 KiB).
pub fn fail(raw: String) -> ToolOutput {
    ToolOutput::error(truncate_error(&raw))
}

fn annotation(notes: &[String]) -> String {
    format!("[truncated {}]", notes.join(" / "))
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Required string argument.
pub fn get_str(args: &Value, key: &str) -> Result<String, ToolOutput> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected string"))),
        None => Err(fail(format!("missing required argument '{key}'"))),
    }
}

/// Optional unsigned integer argument (`null`/absent -> `None`).
pub fn get_opt_u64(args: &Value, key: &str) -> Result<Option<u64>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(|| {
            fail(format!(
                "invalid argument '{key}': expected non-negative integer"
            ))
        }),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected integer"))),
    }
}

/// Optional boolean argument (`null`/absent -> `None`).
pub fn get_opt_bool(args: &Value, key: &str) -> Result<Option<bool>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected boolean"))),
    }
}

/// Resolves a user-supplied path against the execution cwd; absolute paths
/// are used verbatim.
pub fn resolve_path(cwd: &std::path::Path, p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}
