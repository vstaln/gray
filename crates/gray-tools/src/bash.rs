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

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Hard upper bound for the timeout argument.
const MAX_TIMEOUT_SECS: u64 = 300;

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
        let stdout = join_drain(stdout_task).await;
        let stderr = join_drain(stderr_task).await;

        let mut combined = String::new();
        for stream in [&stdout, &stderr] {
            if !stream.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(stream);
            }
        }

        match status.code() {
            Some(0) => finish(combined),
            Some(code) => fail(format!("command exited with code {code}\n{combined}")),
            None => fail(format!("command terminated by signal\n{combined}")),
        }
    }
}

/// Spawns a task reading a pipe fully into a String.
fn spawn_drain(pipe: Option<impl AsyncReadExt + Unpin + Send + 'static>) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

async fn join_drain(handle: tokio::task::JoinHandle<String>) -> String {
    // The drain task only fails if the runtime is shutting down; treat that
    // as empty output rather than panicking.
    handle.await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ctx() -> ToolContext {
        ToolContext::default()
    }

    #[tokio::test]
    async fn captures_stdout_and_stderr() {
        let out = BashTool
            .execute(&ctx(), json!({"command": "echo out; echo err >&2"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("out"), "{}", out.content);
        assert!(out.content.contains("err"), "{}", out.content);
    }

    #[tokio::test]
    async fn nonzero_exit_is_error_with_code() {
        let out = BashTool.execute(&ctx(), json!({"command": "exit 7"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("code 7"), "{}", out.content);
    }

    #[tokio::test]
    async fn runs_in_context_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx();
        c.cwd = dir.path().to_path_buf();
        let out = BashTool.execute(&c, json!({"command": "pwd"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content.trim(), dir.path().to_str().unwrap());
    }

    #[tokio::test]
    async fn timeout_kills_sleeping_child_quickly() {
        let start = Instant::now();
        let out = BashTool
            .execute(&ctx(), json!({"command": "sleep 30", "timeout": 1}))
            .await;
        let elapsed = start.elapsed();
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("timed out after 1s"), "{}", out.content);
        assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
    }

    #[tokio::test]
    async fn timeout_argument_is_clamped_to_cap() {
        // Ask for 100000s; the clamp must cap it at 300s. We only verify the
        // argument is accepted and the command still runs.
        let out = BashTool
            .execute(&ctx(), json!({"command": "echo ok", "timeout": 100_000}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content.trim(), "ok");
    }

    #[tokio::test]
    async fn cancel_token_kills_child_quickly() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut c = ctx();
        c.cancel = cancel.clone();

        let exec = tokio::spawn(async move {
            BashTool.execute(&c, json!({"command": "sleep 30"})).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let start = Instant::now();
        cancel.cancel();
        let out = exec.await.unwrap();
        let elapsed = start.elapsed();

        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("cancelled"), "{}", out.content);
        assert!(elapsed < Duration::from_secs(5), "cancel took {elapsed:?}");
    }

    #[tokio::test]
    async fn missing_command_argument_is_error() {
        let out = BashTool.execute(&ctx(), json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("missing required argument 'command'"));
    }

    #[test]
    fn bash_is_never_concurrency_safe() {
        assert!(!BashTool.is_concurrency_safe(&json!({"command": "ls"})));
    }
}
