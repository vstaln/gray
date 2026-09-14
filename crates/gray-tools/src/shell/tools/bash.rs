//! shell/tools/bash.rs — blocking-only bash.
//!
//! Runs `sh -c`, waits for exit, kills the process group on timeout or
//! cancel. No background mode, no task ids, no registry: one call, one
//! result card. `is_error` only for harness failures (spawn error, bad
//! args); any exit status, signal death, timeout or cancel returns
//! `ToolOutput::ok` with the verdict on the first line.
//!
//! Always logs (`$GRAY_HOME/shell/<session>/bash-<uuid>.log`), one
//! middle-out view, honest header, fenced body. The full log stays on disk;
//! the header names its path — grep it instead of rerunning.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS, MEM_HEAD_BYTES, MEM_TAIL_BYTES,
    PUMP_DRAIN_TIMEOUT, PumpSummary, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::Pump;
use crate::shell::spawn::spawn;
use crate::shell::view::{format_elapsed, header, middle_out, resume_hint};
use crate::{fail, get_opt_u64, get_str};

pub const BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
/// Usage guidelines, ≤ 6 bullets by contract (brief 3D — every word here
/// ships on every request, so cut adjectives, never add).
pub const BASH_GUIDELINES: &[&str] = &[
    "bash output: first line is the verdict (exit, duration, size, log path). Non-zero exit is data, not a tool error; read the header.",
    "bash blocks until the command exits; timeout (default 30s, max 600s) kills it and returns partial output.",
    "Need background? `cmd > /tmp/out.log 2>&1 & echo $!`, poll with `tail`, stop with `kill`.",
    "Truncated output names the log path; grep the log instead of rerunning.",
];

/// Runs a command through the shell (`sh -c`), blocking until it exits.
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via `sh -c` and capture stdout/stderr. \
             Blocks until the command exits. Times out after `timeout` seconds \
             (default 30, capped at 600): the process group is killed and \
             partial output is returned. \
             Non-zero exits are data, not tool errors — read the header.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"},
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default 30, capped at 600). On expiry the process group is killed and partial output returned."
                    }
                },
                "required": ["command"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(BASH_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(BASH_GUIDELINES)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let command = match get_str(&args, "command") {
            Ok(c) if !c.trim().is_empty() => c,
            _ => {
                return match get_str(&args, "command") {
                    Ok(_) => fail(
                        "missing required argument 'command': expected a non-empty string"
                            .to_string(),
                    ),
                    Err(e) => e,
                };
            }
        };
        // Loud rejection for the removed background family: silent ignore
        // would fake success while dropping the model's intent.
        if let Some(obj) = args.as_object() {
            for k in ["background", "run_in_background", "notify_on"] {
                if obj.contains_key(k) {
                    return fail(format!(
                        "bash is blocking-only: `{k}` was removed. \
                         Long commands block until exit; for shell-native background use \
                         `cmd > /tmp/out.log 2>&1 & echo $!`, poll with `tail`, stop with `kill`"
                    ));
                }
            }
            for k in ["task_id", "from_offset", "wait"] {
                if obj.contains_key(k) {
                    return fail(format!(
                        "bash is blocking-only: `{k}` belongs to the removed shell_output tool. \
                         The full log path is in the header — grep it instead"
                    ));
                }
            }
        }
        let requested = match get_opt_u64(&args, "timeout") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let secs = requested
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        let session = ctx.session_id.clone().unwrap_or_else(|| "nosession".into());
        let log_path = shell_dir()
            .join(&session)
            .join(format!("bash-{}.log", uuid::Uuid::new_v4().as_simple()));
        let start = Instant::now();

        let spawned = match spawn(&command, &ctx.cwd) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };
        let mut child = spawned.child;
        let pump = Pump::start(child.stdout.take(), child.stderr.take(), log_path.clone());

        enum Cause {
            Exit,
            Timeout,
            Cancel,
        }
        let mut exited: Option<std::process::ExitStatus> = None;
        let cause = tokio::select! {
            s = child.wait() => match s {
                Ok(st) => { exited = Some(st); Cause::Exit }
                Err(e) => {
                    let _ = term_then_kill(spawned.pgid, Duration::from_secs(2)).await;
                    let _ = child.wait().await;
                    abort_pump(pump).await;
                    return fail(format!("failed to wait for command: {e}"));
                }
            },
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                // Raced exit between the timer and now: report it, don't kill.
                match child.try_wait() {
                    Ok(Some(st)) => { exited = Some(st); Cause::Exit }
                    _ => Cause::Timeout,
                }
            }
            _ = ctx.cancel.cancelled() => Cause::Cancel,
        };
        // Timeout and cancel both escalate SIGTERM → SIGKILL on our own
        // group, then reap. Refusal (degenerate pgid) still falls through
        // to wait — the child is ours, wait reaps it.
        let first_line: Option<String> = match cause {
            Cause::Exit => None,
            Cause::Timeout => {
                let _ = term_then_kill(spawned.pgid, Duration::from_secs(2)).await;
                match child.wait().await {
                    Ok(st) => {
                        exited = Some(st);
                    }
                    Err(e) => {
                        abort_pump(pump).await;
                        return fail(format!("failed to wait for command: {e}"));
                    }
                }
                Some(format!("timed out after {secs}s (process group killed)"))
            }
            Cause::Cancel => {
                let _ = term_then_kill(spawned.pgid, Duration::from_secs(2)).await;
                match child.wait().await {
                    Ok(st) => {
                        exited = Some(st);
                    }
                    Err(e) => {
                        abort_pump(pump).await;
                        return fail(format!("failed to wait for command: {e}"));
                    }
                }
                Some(format!(
                    "cancelled by user after {}",
                    format_elapsed(start.elapsed())
                ))
            }
        };
        let status = exited.expect("every cause resolves a status");
        let (summary, drain_truncated) = match drain_pump(pump, &log_path).await {
            Ok(v) => v,
            Err(e) => return fail(format!("output pump failed: {e}")),
        };
        let mut first = first_line;
        if drain_truncated {
            let note = format!(
                "output truncated: pump drain timed out after {}s",
                PUMP_DRAIN_TIMEOUT.as_secs()
            );
            first = Some(match first {
                Some(f) => format!("{f} ({note})"),
                None => note,
            });
        }
        if summary.log_write_failed {
            let note = "log write failed; view is memory-only";
            first = Some(match first {
                Some(f) => format!("{f} ({note})"),
                None => note.to_string(),
            });
        }
        finish_inline(&command, &log_path, status, &summary, start, first)
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

async fn abort_pump(pump: JoinHandle<PumpSummary>) {
    pump.abort();
    let _ = pump.await;
}

/// Bounded pump drain after the child exits: a grandchild inheriting the
/// pipes keeps the pump alive forever, so the result must never wait past
/// [`PUMP_DRAIN_TIMEOUT`]. On timeout the handle is aborted and
/// `truncated=true` so the caller reports truncated cleanup. Happy path
/// (drain completes in time) is byte-identical to a bare `pump.await`.
async fn drain_pump(
    mut pump: JoinHandle<PumpSummary>,
    log_path: &std::path::Path,
) -> Result<(PumpSummary, bool), tokio::task::JoinError> {
    match tokio::time::timeout(PUMP_DRAIN_TIMEOUT, &mut pump).await {
        Ok(res) => res.map(|s| (s, false)),
        Err(_) => {
            pump.abort();
            let _ = (&mut pump).await;
            log::warn!("shell: pump drain timed out; aborted, reporting truncated");
            Ok((truncated_summary_from_disk(log_path), true))
        }
    }
}

/// Fallback summary when the drain times out: rebuilt from the log file with
/// bounded reads (head + tail budgets only, never the whole file) so the
/// rendered view stays middle-out with an `omitted` marker and the header
/// size stays honest.
fn truncated_summary_from_disk(log_path: &std::path::Path) -> PumpSummary {
    use std::io::{Read, Seek, SeekFrom};
    let total_bytes = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    let mut head = Vec::new();
    let mut tail = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log_path) {
        let _ = f
            .by_ref()
            .take(MEM_HEAD_BYTES as u64)
            .read_to_end(&mut head);
        let tail_len = (MEM_TAIL_BYTES as u64).min(total_bytes);
        if total_bytes > head.len() as u64
            && f.seek(SeekFrom::Start(total_bytes.saturating_sub(tail_len)))
                .is_ok()
        {
            let _ = f.by_ref().take(tail_len).read_to_end(&mut tail);
        }
    }
    let total_lines = head
        .iter()
        .chain(tail.iter())
        .filter(|&&b| b == b'\n')
        .count();
    PumpSummary {
        total_bytes,
        total_lines,
        head,
        tail,
        log_write_failed: false,
    }
}

/// Render a reaped command: header + fenced body. `first_line` prefixes the
/// header (timeout/cancel/truncation arms) or is absent (plain exit).
fn finish_inline(
    command: &str,
    log_path: &std::path::Path,
    status: std::process::ExitStatus,
    summary: &PumpSummary,
    start: Instant,
    first_line: Option<String>,
) -> ToolOutput {
    let elapsed = start.elapsed();
    let report = exit_report(status, command);
    let view = build_view(log_path, summary);
    let head = header(&report, Some(&view), elapsed, log_path);
    let mut out = match first_line {
        Some(first) => format!("{first}\n{head}"),
        None => head,
    };
    if !view.body.is_empty() {
        out.push('\n');
        out.push_str(&fence(&view.body));
    }
    ToolOutput::ok(out)
}

/// Bounded inline view (≤ ~12 KiB): the whole log read back from disk when
/// it fits the inline budget, else head ++ tail from the bounded memory
/// sample with the elided shape imposed explicitly.
/// `{{MARKER}}` is replaced only when bytes were actually omitted, so user
/// text can never collide with the slot.
fn build_view(log_path: &std::path::Path, summary: &PumpSummary) -> View {
    // Byte-bounded: the small path reads the log back from disk — never read
    // an unbounded log on a stale/small summary. Check the real length first
    // and fall back to the bounded in-memory head ++ tail.
    let file_len = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    let use_disk =
        summary.total_bytes <= INLINE_BUDGET_BYTES as u64 && file_len <= INLINE_BUDGET_BYTES as u64;
    // On read error fall through to the memory sample below.
    if use_disk && let Ok(bytes) = std::fs::read(log_path) {
        let mut view = middle_out(&bytes, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
        if view.omitted_range.is_some() {
            let hint = resume_hint(&view);
            view.body = view.body.replace("{{MARKER}}", &hint);
        }
        return view;
    }
    // Large logs: the ≤12 KiB memory sample (first MEM_HEAD_BYTES verbatim ++
    // ring of last MEM_TAIL_BYTES) always fits the inline budget, so a plain
    // middle_out pass would report nothing omitted. Sanitize head and tail
    // separately instead — a whole-path middle_out returns its sanitized
    // input verbatim with exact line counts — and impose the elided shape
    // with honest totals from the summary. The full log stays on disk.
    let head_part = middle_out(&summary.head, usize::MAX, usize::MAX, 0);
    let tail_part = middle_out(&summary.tail, usize::MAX, usize::MAX, 0);
    let shown = (head_part.total_lines, tail_part.total_lines);
    let omitted_lines = summary.total_lines.saturating_sub(shown.0 + shown.1);
    let omitted_bytes = usize::try_from(
        summary
            .total_bytes
            .saturating_sub((summary.head.len() + summary.tail.len()) as u64),
    )
    .unwrap_or(usize::MAX);
    if omitted_lines == 0 && omitted_bytes == 0 {
        // Degenerate (summary disagrees with the log length, e.g. the drain
        // fallback undercounted): render the sample whole, no marker.
        let mut cat = summary.head.clone();
        cat.extend_from_slice(&summary.tail);
        return middle_out(&cat, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
    }
    // Omitted middle in raw-log space: the head is the first bytes verbatim
    // and the tail ring the last.
    let omitted_range = Some((
        summary.head.len() as u64,
        summary
            .total_bytes
            .saturating_sub(summary.tail.len() as u64),
    ));
    let mut view = View {
        body: String::new(),
        shown_lines: shown,
        omitted_lines,
        omitted_bytes,
        omitted_range,
        total_lines: summary.total_lines,
        total_bytes: summary.total_bytes,
    };
    let hint = resume_hint(&view);
    // trim_end on the head is count-preserving (a trailing newline never
    // starts a new line), so shown counts match the displayed lines exactly.
    view.body = format!(
        "{}\n{}\n{}",
        head_part.body.trim_end_matches('\n'),
        hint,
        tail_part.body
    );
    view
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
            "bash-1b-{tag}-{}-{}",
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
        // Isolated GRAY_HOME must own shell logs, not real HOME.
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
    async fn echo_returns_exit_zero_with_output() {
        let session = sess("echo");
        let ctx = ctx_for(&session);
        let r = BashTool
            .execute(&ctx, json!({"command": "echo hello"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let head = r.content.lines().next().unwrap_or("");
        assert!(head.starts_with("exit 0"), "{head}");
        assert!(r.content.contains("hello"), "{}", r.content);
        assert!(r.content.contains("log "), "{}", r.content);
        assert!(
            !r.content.contains("started t"),
            "no task ids anymore: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn removed_background_arg_fails_loud() {
        let session = sess("bgone");
        let ctx = ctx_for(&session);
        let r = BashTool
            .execute(&ctx, json!({"command": "echo hi", "background": true}))
            .await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("blocking-only"), "{}", r.content);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_and_returns_partial_output() {
        // `echo out; sleep 30` with timeout 1: SIGTERM lands, partial
        // output survives, no promotion text.
        let session = sess("timeout");
        let ctx = ctx_for(&session);
        let t0 = Instant::now();
        let r = BashTool
            .execute(&ctx, json!({"command": "echo out; sleep 30", "timeout": 1}))
            .await;
        let dt = t0.elapsed();
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("timed out after 1s"), "{}", r.content);
        assert!(r.content.contains("out"), "{}", r.content);
        assert!(
            !r.content.contains("promoted"),
            "never promotes: {}",
            r.content
        );
        assert!(
            dt < Duration::from_secs(15),
            "timeout must kill, not wait: {dt:?}"
        );
    }

    #[tokio::test]
    async fn empty_command_is_an_error() {
        let session = sess("empty");
        let ctx = ctx_for(&session);
        let r = BashTool.execute(&ctx, json!({"command": "   "})).await;
        assert!(r.is_error, "{}", r.content);
    }
}
