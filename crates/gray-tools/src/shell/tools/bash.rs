//! shell/tools/bash.rs — bash with background mode + timeout promotion (brief 2B).
//!
//! Always logs (`$GRAY_HOME/shell/<session>/t{n}.log` from the first chunk),
//! one middle-out view, honest header, fenced body. `is_error` only for
//! harness failures (spawn error, guard deny, bad args); any exit status,
//! signal death, promotion or cancel returns `ToolOutput::ok`.
//!
//! Every spawn registers a registry task (foreground included). A foreground
//! command that outlives its timeout is promoted to background — never
//! killed. Cancel (Ctrl-C) still kills via `kill::term_then_kill`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{PermissionMode, Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::process::Child;
use tokio::task::JoinHandle;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, ExitReport, MAX_TIMEOUT_SECS, NotifyPattern, PROMOTION_TAIL_BYTES,
    PumpSummary, TaskId, TaskInfo, TaskState, VIEW_BUDGET_BYTES, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::guard;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::Pump;
use crate::shell::registry::registry;
use crate::shell::spawn::spawn;
use crate::shell::view::{format_elapsed, header, home_relative, middle_out, resume_hint};
use crate::{fail, get_opt_bool, get_opt_u64, get_str};

pub const BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
/// Usage guidelines, ≤ 6 bullets by contract (brief 3D — every word here
/// ships on every request, so cut adjectives, never add).
pub const BASH_GUIDELINES: &[&str] = &[
    "bash output: first line is the verdict (exit, duration, size, log path). Non-zero exit is data, not a tool error; read the header.",
    "Long or server-like commands: background=true. You are woken when they exit; do not poll.",
    "Read new output with shell_output(task_id, from_offset=<next_offset>, wait='output'|'exit'). Never re-read the same bytes.",
    "Port in use: shell_kill(port=N). Stop a task: shell_kill(task_id).",
    "Waiting on something: sleep(seconds) — it ends early when anything happens.",
    "Truncated output names the log path; grep the log instead of rerunning.",
];

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
                        "description": "Return immediately with a task id; read output with shell_output(task_id). A foreground command that outlives `timeout` is promoted to background the same way instead of being killed"
                    },
                    "notify_on": {
                        "type": "string",
                        "description": "Regex (e.g. \"error|ready\"): wake a sleeping agent when a log line matches. Rate-limited (10s, max 5 per task, then disabled). Only meaningful with background=true or after promotion"
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
        // Empty = absent (an empty regex would match every line). Invalid →
        // fail with the regex error plus an example (brief 3C wording).
        let notify_pattern = match get_opt_str(&args, "notify_on") {
            Ok(Some(s)) if !s.is_empty() => match NotifyPattern::new(&s) {
                Ok(p) => Some(p),
                Err(e) => {
                    return fail(format!(
                        "invalid notify_on regex {s:?}: {e}. Example: notify_on=\"error|ready\""
                    ));
                }
            },
            Ok(_) => None,
            Err(e) => return e,
        };

        match guard::classify(&command) {
            guard::Decision::Allow => {}
            guard::Decision::Deny(msg) => return fail(msg),
            guard::Decision::Prompt { rule, why, alt } => {
                // Fail closed without an interactive user: -p/auto mode has
                // no TTY to ask on, so a Prompt verdict denies here instead
                // of auto-executing (Deny verdicts block above regardless).
                if ctx.permission == PermissionMode::Auto {
                    return fail(format!(
                        "Blocked by destructive-command guard ({rule}): non-interactive mode cannot approve risky commands — have the user run it manually. {why} Safe alternative: {alt}."
                    ));
                }
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

        let session = ctx.session_id.clone().unwrap_or_else(|| "nosession".into());
        let id = registry().reserve(&session);
        let log_path = shell_dir().join(&session).join(format!("{id}.log"));
        // Fresh task → fresh log. Ids restart every process (cross-process
        // ids are REPL-owned, 2E) while logs persist, so a stale file may sit
        // at this path; the pump opens O_APPEND and would mix runs. A live
        // task in this process can never hold this id (just reserved), so
        // removal is safe.
        let _ = std::fs::remove_file(&log_path);
        let start = Instant::now();

        let spawned = match spawn(&command, &ctx.cwd, id) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };
        let reg = registry();
        let mut child = spawned.child;
        reg.bind(&session, id, &child, &command, log_path.clone());
        let bytes_tx = match reg.bytes_tx(&session, id) {
            Some(tx) => tx,
            None => return fail("registry lost the task just bound".to_string()),
        };
        let pump = Pump::start(
            id,
            child.stdout.take(),
            child.stderr.take(),
            log_path.clone(),
            bytes_tx,
            notify_pattern,
            Some(reg.wake_tx()),
        );

        if background {
            let pid = spawned.pid;
            tokio::spawn(waiter(child, pump, session, id, command.clone()));
            return ToolOutput::ok(background_start(id, pid, &log_path));
        }

        enum Cause {
            Exit,
            Promote,
            Cancel,
        }
        // The exit status travels out of the select in `exited`; Promote and
        // Cancel resolve it after (waiter reap / kill reap).
        let mut exited: Option<std::process::ExitStatus> = None;
        let cause = tokio::select! {
            s = child.wait() => match s {
                Ok(st) => { exited = Some(st); Cause::Exit }
                Err(e) => {
                    // Nothing to render from; hand the child to the waiter
                    // for reaping so the task cannot leak as Running.
                    tokio::spawn(waiter(child, pump, session, id, command.clone()));
                    return fail(format!("failed to wait for command: {e}"));
                }
            },
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                // Raced exit between the timer and now: report it, don't promote.
                match child.try_wait() {
                    Ok(Some(st)) => { exited = Some(st); Cause::Exit }
                    _ => Cause::Promote,
                }
            }
            _ = ctx.cancel.cancelled() => Cause::Cancel,
        };
        match cause {
            Cause::Exit => {
                let status = exited.expect("Exit always carries a status");
                let summary = match pump.await {
                    Ok(s) => s,
                    Err(e) => return fail(format!("output pump failed: {e}")),
                };
                finish_inline(
                    &session,
                    spawned.pid,
                    spawned.pgid,
                    id,
                    &command,
                    &log_path,
                    status,
                    &summary,
                    start,
                    None,
                )
            }
            Cause::Promote => {
                // Do NOT signal the child: the same waiter as background
                // mode owns it from here on.
                tokio::spawn(waiter(child, pump, session, id, command.clone()));
                ToolOutput::ok(promotion_string(id, spawned.pid, &log_path, secs))
            }
            Cause::Cancel => {
                // User pressed Ctrl-C: escalate SIGTERM → SIGKILL on our
                // group, then reap. Refusal (degenerate pgid) still falls
                // through to wait — the child is ours, wait reaps it.
                let _ = term_then_kill(spawned.pgid, Duration::from_secs(2)).await;
                let status = match child.wait().await {
                    Ok(st) => st,
                    Err(e) => return fail(format!("failed to wait for command: {e}")),
                };
                let summary = match pump.await {
                    Ok(s) => s,
                    Err(e) => return fail(format!("output pump failed: {e}")),
                };
                let first = format!(
                    "cancelled by user after {}",
                    format_elapsed(start.elapsed())
                );
                finish_inline(
                    &session,
                    spawned.pid,
                    spawned.pgid,
                    id,
                    &command,
                    &log_path,
                    status,
                    &summary,
                    start,
                    Some(first),
                )
            }
        }
    }
}

/// Optional string argument (`null`/absent -> `None`). Local until 4A
/// centralizes arg parsing (same helper lives in shell_output.rs).
fn get_opt_str(args: &Value, key: &str) -> Result<Option<String>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected string"))),
    }
}

fn gray_home() -> PathBuf {
    std::env::var("GRAY_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".gray"))
                .unwrap_or_else(|_| std::env::temp_dir().join(".gray"))
        })
}

fn shell_dir() -> PathBuf {
    gray_home().join("shell")
}

/// Shared waiter: background starts and promoted timeouts differ only in
/// who awaits. Reaps the child, drains the pump, marks the task exited.
async fn waiter(
    mut child: Child,
    pump: JoinHandle<PumpSummary>,
    session: String,
    id: TaskId,
    command: String,
) {
    let status = match child.wait().await {
        Ok(st) => st,
        Err(e) => {
            log::warn!("shell {id}: wait failed after detach: {e}");
            registry().mark_exited(
                &session,
                id,
                ExitReport {
                    code: None,
                    signal: None,
                    effective: 1,
                    label: "exit 1".to_string(),
                    note: Some(format!("failed to wait for process: {e}")),
                    benign: false,
                },
            );
            return;
        }
    };
    // Drain the pump before marking so the log tail is complete for 2C reads.
    let _ = pump.await;
    registry().mark_exited(&session, id, exit_report(status, &command));
}

/// Render a reaped foreground command: mark exited, header + fence.
/// `first_line` prefixes the header (cancel arm) or is absent (plain exit).
#[allow(clippy::too_many_arguments)]
fn finish_inline(
    session: &str,
    pid: u32,
    pgid: i32,
    id: TaskId,
    command: &str,
    log_path: &Path,
    status: std::process::ExitStatus,
    summary: &PumpSummary,
    start: Instant,
    first_line: Option<String>,
) -> ToolOutput {
    let elapsed = start.elapsed();
    let report = exit_report(status, command);
    registry().mark_exited(session, id, report.clone());
    let view = build_view(id, log_path, summary);
    let task = TaskInfo {
        id,
        pid,
        pgid,
        command: command.to_string(),
        started: start,
        log_path: log_path.to_path_buf(),
        bytes: summary.total_bytes,
        state: TaskState::Exited {
            report: report.clone(),
            at: Instant::now(),
        },
    };
    let head = header(&task, Some(&report), Some(&view), elapsed);
    let mut out = match first_line {
        Some(first) => format!("{first}\n{head}"),
        None => head,
    };
    if !view.body.is_empty() {
        out.push('\n');
        out.push_str(&fence(id, &view.body));
    }
    ToolOutput::ok(out)
}

fn background_start(id: TaskId, pid: u32, log_path: &Path) -> String {
    format!(
        "started {id} · pid {pid} · log {}\nshell_output(task_id=\"{id}\", from_offset=0) to read output",
        home_relative(log_path)
    )
}

fn promotion_string(id: TaskId, pid: u32, log_path: &Path, secs: u64) -> String {
    let (tail, len) = read_log_tail(log_path);
    let view = middle_out(
        &tail,
        VIEW_BUDGET_BYTES,
        VIEW_BUDGET_LINES,
        len.saturating_sub(tail.len() as u64),
    );
    let mut out = format!(
        "still running after {secs}s → promoted to background as {id} · pid {pid} · log {}\n",
        home_relative(log_path)
    );
    if !view.body.is_empty() {
        out.push_str(&fence(id, &view.body));
        out.push('\n');
    }
    out.push_str(&format!(
        "shell_output(task_id=\"{id}\", from_offset={len}) for the rest · next_offset={len}"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::Tool;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SESS_N: AtomicU64 = AtomicU64::new(0);

    fn sess(tag: &str) -> String {
        format!(
            "bash-2b-{tag}-{}-{}",
            std::process::id(),
            SESS_N.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn ctx_for(session: &str) -> ToolContext {
        ToolContext {
            session_id: Some(session.to_string()),
            ..ToolContext::default()
        }
    }

    #[test]
    fn shell_dir_respects_gray_home() {
        // Bug 1: isolated GRAY_HOME must own shell logs, not real HOME.
        let dir = tempfile::tempdir().expect("tempdir");
        let gray = dir.path().to_string_lossy().into_owned();
        let prev = std::env::var("GRAY_HOME").ok();
        unsafe { std::env::set_var("GRAY_HOME", &gray) };
        let d = shell_dir();
        match prev {
            Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
            None => unsafe { std::env::remove_var("GRAY_HOME") },
        }
        assert!(
            d.starts_with(dir.path()),
            "shell_dir must live under GRAY_HOME, got {}",
            d.display()
        );
        assert_eq!(d.file_name().and_then(|s| s.to_str()), Some("shell"));
    }

    #[tokio::test]
    async fn prompt_verdicts_fail_closed_in_auto_mode() {
        // -p/auto has no TTY to ask on: a Prompt-verdict command must be
        // denied, never auto-executed.
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: Some(sess("failclosed")),
            permission: PermissionMode::Auto,
            ..ToolContext::default()
        };
        let tool = BashTool;
        // Prompt verdict (git-reset-hard), harmless to attempt: the bare
        // tempdir is not a git repo, so even execution would just fail as data.
        let r = tool
            .execute(&ctx, json!({"command": "git reset --hard"}))
            .await;
        assert!(
            r.is_error,
            "Prompt must fail closed in auto mode: {}",
            r.content
        );
        assert!(
            r.content.contains("Blocked by destructive-command guard"),
            "{}",
            r.content
        );
    }

    #[tokio::test]
    async fn prompt_verdicts_still_ask_in_interactive_mode() {
        // Ask mode without a reachable user fails closed too (no bridge),
        // via the ask path — interactive behavior unchanged.
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: Some(sess("askclosed")),
            permission: PermissionMode::Ask,
            ..ToolContext::default()
        };
        let tool = BashTool;
        let r = tool
            .execute(&ctx, json!({"command": "git reset --hard"}))
            .await;
        assert!(r.is_error, "expected denial without a user: {}", r.content);
        assert!(
            r.content.contains("Blocked by destructive-command guard"),
            "{}",
            r.content
        );
    }

    #[tokio::test]
    async fn two_background_spawns_are_t1_then_t2() {
        // Bug 2: monotonic per-session ids, never reuse within a session.
        let session = sess("mono");
        let ctx = ctx_for(&session);
        let tool = BashTool;
        let r1 = tool
            .execute(&ctx, json!({"command": "echo one", "background": true}))
            .await;
        assert!(!r1.is_error, "{}", r1.content);
        let r2 = tool
            .execute(&ctx, json!({"command": "echo two", "background": true}))
            .await;
        assert!(!r2.is_error, "{}", r2.content);
        let n1: u32 = r1
            .content
            .lines()
            .next()
            .and_then(|h| h.split("started t").nth(1))
            .and_then(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .expect("t1 header");
        let n2: u32 = r2
            .content
            .lines()
            .next()
            .and_then(|h| h.split("started t").nth(1))
            .and_then(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .expect("t2 header");
        assert_eq!(
            (n1, n2),
            (1, 2),
            "expected t1 then t2, got t{n1} then t{n2}: {:?} / {:?}",
            r1.content,
            r2.content
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Last PROMOTION_TAIL_BYTES of the log + its total length (bounded seek —
/// never read_to_string a possibly multi-GB log).
fn read_log_tail(log_path: &Path) -> (Vec<u8>, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(PROMOTION_TAIL_BYTES as u64);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_ok() {
        let _ = f.take(PROMOTION_TAIL_BYTES as u64).read_to_end(&mut buf);
    }
    (buf, len)
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
