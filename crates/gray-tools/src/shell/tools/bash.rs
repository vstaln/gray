//! shell/tools/bash.rs — foreground bash on the new contract (brief 1D).
//!
//! Always logs (`~/.gray/shell/<session>/t{n}.log` from the first chunk),
//! one middle-out view, honest header, fenced body. `is_error` only for
//! harness failures (spawn error, guard deny, bad args); any exit status,
//! signal death, timeout or cancel returns `ToolOutput::ok`.
//!
//! Task ids come from a per-process `AtomicU32` fallback until brief 2B
//! replaces it with the session-scoped registry (ids may therefore repeat
//! across sessions until then).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{PermissionMode, Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS, PumpSummary, TaskId, TaskInfo, TaskState,
    VIEW_BUDGET_BYTES, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::guard;
use crate::shell::pump::Pump;
use crate::shell::spawn::spawn;
use crate::shell::view::{format_elapsed, header, middle_out, resume_hint};
use crate::{fail, get_opt_bool, get_opt_u64, get_str};

pub const BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
pub const BASH_GUIDELINES: &[&str] = &[];

/// Per-process task counter; 2B replaces this with the registry.
static NEXT_TASK_ID: AtomicU32 = AtomicU32::new(1);

/// Runs a command through the shell (`sh -c`).
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via `sh -c` and capture stdout/stderr. \
             Times out after `timeout` seconds (default 30, max 600). \
             Non-zero exits are data, not tool errors — read the header.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"},
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default 30, capped at 600)"
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Not yet available (lands in Phase 2); when true, runs in the foreground and says so"
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

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let command = match get_str(&args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };
        let requested = match get_opt_u64(&args, "timeout") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let secs = requested
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);
        let background = match get_opt_bool(&args, "background") {
            Ok(b) => b.unwrap_or(false),
            Err(e) => return e,
        };

        match guard::classify(&command) {
            guard::Decision::Allow => {}
            guard::Decision::Deny(msg) => return fail(msg),
            guard::Decision::Prompt { rule, why, alt } => {
                // Auto mode (print/-p, GRAY_PERMISSION=auto) skips the ask;
                // Deny verdicts still block above regardless of mode.
                if ctx.permission != PermissionMode::Auto {
                    if !guard::prompt_allowance(rule) {
                        return fail(format!(
                            "Blocked by destructive-command guard ({rule}): already asked twice this session — have the user run it manually. {why}"
                        ));
                    }
                    if !guard::ask_allow_once(ctx, &command, rule, &why, &alt).await {
                        return fail(format!(
                            "Blocked by destructive-command guard ({rule}): user did not approve. {why} Safe alternative: {alt}."
                        ));
                    }
                }
            }
        }

        let id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
        let session = ctx
            .session_id
            .clone()
            .unwrap_or_else(|| "nosession".into());
        let log_path = shell_dir().join(session).join(format!("{id}.log"));
        // Fresh task → fresh log. Ids restart every process (registry lands
        // in 2B) while logs persist, so a stale file may sit at this path;
        // the pump opens O_APPEND and would mix runs. A live task in this
        // process can never hold this id (just issued), so removal is safe.
        let _ = std::fs::remove_file(&log_path);
        let start = Instant::now();

        let spawned = match spawn(&command, &ctx.cwd, id) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };
        let mut child = spawned.child;
        let (bytes_tx, _bytes_rx) = watch::channel(0u64);
        let pump = Pump::start(
            id,
            child.stdout.take(),
            child.stderr.take(),
            log_path.clone(),
            bytes_tx,
            None,
            None,
        );

        #[derive(PartialEq)]
        enum Cause {
            Exit,
            Timeout,
            Cancel,
        }
        let (status, cause) = tokio::select! {
            s = child.wait() => match s {
                Ok(st) => (st, Cause::Exit),
                Err(e) => return fail(format!("failed to wait for command: {e}")),
            },
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                term_then_kill_throwaway(&mut child, spawned.pgid).await;
                match child.wait().await {
                    Ok(st) => (st, Cause::Timeout),
                    Err(e) => return fail(format!("failed to wait for command: {e}")),
                }
            }
            _ = ctx.cancel.cancelled() => {
                term_then_kill_throwaway(&mut child, spawned.pgid).await;
                match child.wait().await {
                    Ok(st) => (st, Cause::Cancel),
                    Err(e) => return fail(format!("failed to wait for command: {e}")),
                }
            }
        };
        let summary = match pump.await {
            Ok(s) => s,
            Err(e) => return fail(format!("output pump failed: {e}")),
        };

        let elapsed = start.elapsed();
        let report = exit_report(status, &command);
        let view = build_view(id, &log_path, &summary);
        let task = TaskInfo {
            id,
            pid: spawned.pid,
            pgid: spawned.pgid,
            command: command.clone(),
            started: start,
            log_path: log_path.clone(),
            bytes: summary.total_bytes,
            state: TaskState::Exited {
                report: report.clone(),
                at: Instant::now(),
            },
        };
        let head = header(&task, Some(&report), Some(&view), elapsed);
        let mut out = match cause {
            Cause::Exit => head,
            Cause::Timeout => {
                format!("killed after {secs}s (timeout) — output preserved\n{head}")
            }
            Cause::Cancel => {
                format!("cancelled by user after {}\n{head}", format_elapsed(elapsed))
            }
        };
        if !view.body.is_empty() {
            out.push('\n');
            out.push_str(&fence(id, &view.body));
        }
        if background {
            out.push_str("\n(background not yet available — ran in foreground)");
        }
        ToolOutput::ok(out)
    }
}

fn shell_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    home.join(".gray/shell")
}

/// Bounded view: the whole log read back from disk when it fits the byte
/// budget (≤50 KiB read), else one middle-out pass over head ++ tail.
/// `{{MARKER}}` is replaced only when bytes were actually omitted, so user
/// text can never collide with the slot.
fn build_view(id: TaskId, log_path: &Path, summary: &PumpSummary) -> View {
    let raw: Vec<u8> = if summary.total_bytes <= VIEW_BUDGET_BYTES as u64 {
        match std::fs::read(log_path) {
            Ok(bytes) => bytes,
            Err(_) => {
                let mut cat = summary.head.clone();
                cat.extend_from_slice(&summary.tail);
                cat
            }
        }
    } else {
        let mut cat = summary.head.clone();
        cat.extend_from_slice(&summary.tail);
        cat
    };
    let mut view = middle_out(&raw, VIEW_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
    if view.omitted_range.is_some() {
        let hint = resume_hint(id, &view);
        view.body = view.body.replace("{{MARKER}}", &hint);
    }
    view
}

// THROWAWAY (Phase 1 only): SIGTERM to our own pgid, 2 s grace, SIGKILL.
// Brief 2B replaces the timeout arm with promotion (no kill at all) and 2D
// replaces this with kill::term_then_kill. Kept minimal on purpose.
async fn term_then_kill_throwaway(child: &mut tokio::process::Child, pgid: i32) {
    if pgid <= 1 || pgid == std::process::id() as i32 {
        return; // never broadcast: refuse degenerate or own groups
    }
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    let grace = Duration::from_secs(2);
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }
        if t0.elapsed() >= grace {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if matches!(child.try_wait(), Ok(None)) {
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}
