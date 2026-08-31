//! The `bash` tool: runs a shell command via `sh -c` with timeout + cancel.

use std::time::Duration;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{fail, finish, get_opt_u64, get_str, Tool};

/// Sanitize binary output: filter 0x00-0x1f except 0x09,0x0A,0x0D, trim to last valid UTF-8
fn sanitize_binary_output(bytes: &[u8]) -> String {
    let filtered: Vec<u8> = bytes
        .iter()
        .filter(|&&b| b == 0x09 || b == 0x0A || b == 0x0D || b >= 0x20 || b >= 0x80)
        .copied()
        .collect();
    // Trim to last valid UTF-8 boundary and convert lossily
    String::from_utf8_lossy(&filtered).into_owned()
}

/// Tail truncation: keep last MAX_LINES / MAX_BYTES, return (truncated_content, Option<temp_path>)
fn truncate_bash_tail(text: &str) -> (String, Option<String>) {
    use crate::{MAX_BYTES, MAX_LINES};
    let total_lines = text.lines().count();
    let needs_truncate = total_lines > MAX_LINES || text.len() > MAX_BYTES;
    if !needs_truncate {
        return (text.to_string(), None);
    }
    // Keep tail: last MAX_LINES and last MAX_BYTES
    let lines: Vec<&str> = text.lines().collect();
    let tail_start_line = if total_lines > MAX_LINES { total_lines - MAX_LINES } else { 0 };
    let mut tail_text = lines[tail_start_line..].join("\n");
    if text.ends_with('\n') {
        tail_text.push('\n');
    }
    // Byte cap on tail — keep last MAX_BYTES bytes at char boundary
    let mut truncated = tail_text;
    if truncated.len() > MAX_BYTES {
        let half = MAX_BYTES;
        let raw_start = truncated.len().saturating_sub(half);
        // Find char boundary for start
        let mut start = raw_start;
        while start < truncated.len() && !truncated.is_char_boundary(start) {
            start += 1;
        }
        if start < truncated.len() {
            truncated = truncated[start..].to_string();
        } else {
            truncated = String::new();
        }
        // Handle partial first line: if we cut mid-line, drop it
        if !truncated.starts_with('\n') && tail_start_line > 0 {
            if let Some(nl) = truncated.find('\n') {
                truncated = truncated[nl + 1..].to_string();
                truncated = format!("[truncated ... showing last {} lines / {} bytes]\n{}", MAX_LINES, MAX_BYTES, truncated);
            }
        }
    } else if total_lines > MAX_LINES {
        truncated = format!(
            "[truncated {} lines, showing last {} lines]\n{}",
            total_lines - MAX_LINES,
            MAX_LINES,
            truncated
        );
    }
    // Create temp file for full output (tail truncation keeps only last lines, full is in temp file)
    let temp_path = {
        let mut path = std::env::temp_dir().join(format!(
            "bash-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // Ensure unique if collision
        let mut counter = 0;
        while path.exists() {
            counter += 1;
            path = std::env::temp_dir().join(format!(
                "bash-{}-{}-{}.log",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                counter
            ));
        }
        if std::fs::write(&path, text.as_bytes()).is_ok() {
            Some(path.to_string_lossy().to_string())
        } else {
            None
        }
    };
    let mut out = truncated;
    if let Some(ref p) = temp_path {
        out.push_str(&format!("\n[full output: {}]", p));
    }
    (out, temp_path)
}

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Hard upper bound for the timeout argument.
const MAX_TIMEOUT_SECS: u64 = 300;

pub const BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
pub const BASH_GUIDELINES: &[&str] = &["You can inspect PI_* environment variables for current model and session details."];

/// Runs a command through the shell (`sh -c`).
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via `sh -c` and capture stdout/stderr. \
             Times out after `timeout` seconds (default 60, max 300).",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"},
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default 60, capped at 300)"
                    }
                },
                "required": ["command"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(BASH_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(BASH_GUIDELINES)
    }

    // Shell commands have arbitrary side effects: serialize them.
    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        false
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let command = match get_str(&args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };
        let requested = match get_opt_u64(&args, "timeout") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let secs = requested.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS);

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&ctx.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            let _ = cmd.process_group(0); // detach from our signal group
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };

        // Drain pipes on separate tasks so a chatty child cannot deadlock us.
        let stdout_task = spawn_drain(child.stdout.take());
        let stderr_task = spawn_drain(child.stderr.take());

        let status = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => status,
                Err(e) => return fail(format!("failed to wait for command: {e}")),
            },
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return fail(format!(
                    "command timed out after {secs}s and was killed: {command}"
                ));
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return fail(format!("command cancelled and killed: {command}"));
            }
        };

        // Child exited: pipes are closed, drain tasks finish promptly.
        let stdout_bytes = join_drain_bytes(stdout_task).await;
        let stderr_bytes = join_drain_bytes(stderr_task).await;
        let stdout = sanitize_binary_output(&stdout_bytes);
        let stderr = sanitize_binary_output(&stderr_bytes);

        let mut combined = String::new();
        for stream in [&stdout, &stderr] {
            if !stream.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(stream);
            }
        }

        // Tail truncation: keep last lines/bytes, write full to temp file
        let (truncated, _temp_path) = truncate_bash_tail(&combined);
        match status.code() {
            Some(0) => finish(truncated),
            Some(code) => fail(format!("command exited with code {code}\n{truncated}")),
            None => fail(format!("command terminated by signal\n{truncated}")),
        }
    }
}

/// Spawns a task reading a pipe fully into bytes (for binary sanitization).
fn spawn_drain(pipe: Option<impl AsyncReadExt + Unpin + Send + 'static>) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    })
}

async fn join_drain_bytes(handle: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.await.unwrap_or_default()
}

