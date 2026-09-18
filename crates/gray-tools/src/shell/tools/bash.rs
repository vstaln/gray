//! Shell execution: ordinary blocking calls and session-owned background jobs.
//! Both modes share timeout, cancellation, redaction, bounded output and reaping.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS, MAX_YIELD_MS, MEM_HEAD_BYTES,
    MEM_TAIL_BYTES, MIN_YIELD_MS, PUMP_DRAIN_TIMEOUT, PumpSummary, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::Pump;
use crate::shell::spawn::spawn;
use crate::shell::view::{format_elapsed, header, middle_out, resume_hint};
use crate::{fail, get_opt_bool, get_opt_u64, get_str};

mod jobs;

/// One registry-owned job collection. Dropping the tool cancels its jobs.
#[derive(Default)]
pub struct BashTool {
    jobs: jobs::Jobs,
}

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via sh -c. Default: wait for exit. For independent long work, \
             use background:true to return immediately, or yield_ms to return a job ID if still \
             running after that window. Multiple jobs can run concurrently; continue other work \
             instead of polling. Completion notices arrive between model rounds (or on the next \
             user turn when idle). Use action:list/status/output/cancel with job_id to manage jobs; \
             output/status accept wait_ms (bounded blocking wait, clamped 0-30000ms) so one call \
             can await a job instead of polling. Jobs belong to this session and stop when Gray exits. \
             timeout is the total runtime limit (default 30s, capped at 600s), NOT the yield window. \
             Non-zero exits are data, not tool errors. Full output is logged; inline output is bounded.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command; required for action:run (default)"},
                    "action": {"type": "string", "enum": ["run", "list", "status", "output", "cancel"]},
                    "job_id": {"type": "string", "description": "Job ID returned by bash; required for status/output/cancel"},
                    "background": {"type": "boolean", "description": "Return immediately; run independently in this session"},
                    "timeout": {"type": "integer", "description": "Total runtime limit in seconds (default 30, clamped 1-600)"},
                    "yield_ms": {"type": "integer", "description": "Wait at most this many milliseconds before returning a running job (clamped 100-10000); omitted means wait for exit"},
                    "wait_ms": {"type": "integer", "description": "Bounded blocking wait on action:output/status only: await the job's exit up to this many ms (clamped 0-30000) instead of polling; omitted means return immediately"}
                }
            }),
        )
    }

    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.jobs.notifications(ctx)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        if !args.is_object() {
            return fail("bash arguments must be an object".into());
        }
        let action = match args.get("action") {
            None => "run",
            Some(Value::String(s)) => s.as_str(),
            _ => return fail("action must be a string".into()),
        };
        if args.get("wait_ms").is_some_and(|v| !v.is_null()) && action == "run" {
            return fail("wait_ms is only valid for action:output/status".into());
        }
        if action != "run" {
            // `wait` left the run surface (rejected loudly on the run path
            // below); the async wait path takes `wait_ms` on output/status.
            return self.jobs.action(ctx, action, &args).await;
        }
        // job_id on a plain run carries no extra intent (nothing is dropped),
        // and real models echo it back from the schema: ignore it. The
        // removed-API family below would silently lose intent, so it still
        // fails loudly — with removal as the first instruction, never a
        // suggestion that re-triggers the same failure.
        // `wait` left the run surface with the async wait path: it only rides
        // output/status as `wait_ms` now, so a run carrying it fails loudly
        // instead of silently dropping the intent.
        if args.get("wait").is_some_and(|v| !v.is_null()) {
            return fail(
                "`wait` is not a run argument; remove it. To await a job use action:output/status with wait_ms; for background work use background:true or yield_ms".into(),
            );
        }
        for key in ["task_id", "from_offset", "notify_on", "run_in_background"] {
            if args.get(key).is_some_and(|v| !v.is_null()) {
                return fail(format!(
                    "`{key}` is not a run argument; remove it. For background work use background:true or yield_ms; job_id only pairs with action:status/output/cancel"
                ));
            }
        }
        let command = match get_str(&args, "command") {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return fail("command must be non-empty".into()),
            Err(e) => return e,
        };
        let secs = match get_opt_u64(&args, "timeout") {
            Ok(v) => v.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS),
            Err(e) => return e,
        };
        let background = match get_opt_bool(&args, "background") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };
        let window = match get_opt_u64(&args, "yield_ms") {
            Ok(v) => v.map(|ms| Duration::from_millis(ms.clamp(MIN_YIELD_MS, MAX_YIELD_MS))),
            Err(e) => return e,
        };
        if ctx.cancel.is_cancelled() {
            return fail("command not started: cancelled".into());
        }
        if background || window.is_some() {
            return self
                .jobs
                .start(
                    ctx,
                    command,
                    secs,
                    if background {
                        Duration::ZERO
                    } else {
                        window.unwrap()
                    },
                )
                .await;
        }
        let log_path = log_path(ctx);
        let start = Instant::now();
        let spawned = match spawn(&command, &ctx.cwd) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };
        #[cfg(not(windows))]
        let guard = crate::shell::kill::GroupGuard::new(spawned.pgid);
        run_command(
            command,
            log_path,
            secs,
            start,
            ctx.clone(),
            spawned,
            #[cfg(not(windows))]
            guard,
        )
        .await
    }
}

fn log_path(ctx: &ToolContext) -> PathBuf {
    // Session IDs are host-supplied, not path components trusted from a tool call.
    let session = ctx.session_id.as_deref().unwrap_or("nosession");
    let safe: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    shell_dir()
        .join(safe)
        .join(format!("bash-{}.log", uuid::Uuid::new_v4().as_simple()))
}

async fn run_command(
    command: String,
    log_path: PathBuf,
    secs: u64,
    start: Instant,
    ctx: ToolContext,
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] mut guard: crate::shell::kill::GroupGuard,
) -> ToolOutput {
    #[cfg(not(windows))]
    let target = spawned.pgid;
    #[cfg(windows)]
    let target = &spawned.job;
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
                let _ = term_then_kill(target, Duration::from_secs(2)).await;
                let _ = child.start_kill();
                abort_pump(pump).await;
                return fail(format!("failed to wait for command: {e}"));
            }
        },
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(start + Duration::from_secs(secs))) => {
            // Raced exit between the timer and now: report it, don't kill.
            match child.try_wait() {
                Ok(Some(st)) => { exited = Some(st); Cause::Exit }
                _ => Cause::Timeout,
            }
        }
        _ = ctx.cancel.cancelled() => Cause::Cancel,
    };
    // Unix escalates SIGTERM → SIGKILL; Windows terminates the owned job.
    // Failed termination is a harness error, never a successful timeout.
    let first_line: Option<String> = match cause {
        Cause::Exit => None,
        Cause::Timeout => {
            // Reap while Unix escalation polls: macOS can return EPERM
            // when only an unreaped zombie remains in the process group.
            // try_join also stops waiting if termination fails; never hide
            // that failure or block forever waiting for an unkillable child.
            match tokio::try_join!(term_then_kill(target, Duration::from_secs(2)), async {
                child
                    .wait()
                    .await
                    .map_err(|e| format!("failed to wait for command: {e}"))
            }) {
                Ok(((), st)) => exited = Some(st),
                Err(e) => {
                    let _ = child.start_kill();
                    abort_pump(pump).await;
                    return fail(e);
                }
            }
            Some(format!("timed out after {secs}s (process group killed)"))
        }
        Cause::Cancel => {
            // Reap while Unix escalation polls: macOS can return EPERM
            // when only an unreaped zombie remains in the process group.
            // try_join also stops waiting if termination fails; never hide
            // that failure or block forever waiting for an unkillable child.
            match tokio::try_join!(term_then_kill(target, Duration::from_secs(2)), async {
                child
                    .wait()
                    .await
                    .map_err(|e| format!("failed to wait for command: {e}"))
            }) {
                Ok(((), st)) => exited = Some(st),
                Err(e) => {
                    let _ = child.start_kill();
                    abort_pump(pump).await;
                    return fail(e);
                }
            }
            Some(format!(
                "cancelled by user after {}",
                format_elapsed(start.elapsed())
            ))
        }
    };
    // A Windows shell may exit before background descendants release the
    // pipes. End this call's job before draining, not after a 30s pipe wait.
    // Native Windows shell background jobs therefore never outlive a call.
    #[cfg(windows)]
    drop(spawned.job);
    #[cfg(not(windows))]
    guard.disarm();
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
fn gray_home() -> PathBuf {
    gray_core::paths::gray_home().unwrap_or_else(|| std::env::temp_dir().join(".gray"))
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
    // Best effort: only the sampled head+tail are in hand (never read the
    // whole file here); the live pump path above tracks this exactly.
    let has_cr = head.iter().chain(tail.iter()).any(|&b| b == b'\r');
    PumpSummary {
        total_bytes,
        total_lines,
        head,
        tail,
        has_cr,
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
    if let Some((start, end)) = view.omitted_range {
        // Absolute, shell-quoted path: never expand the display-only ~/
        // shorthand. Git Bash consumes the command string, so the path must
        // use forward slashes; backslashes are shell escapes there.
        let path = log_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\"'\"'");
        let command = if small_log_on_disk(log_path, summary) {
            // middle_out offsets are sanitized bytes. Recover by line instead,
            // including the last shown line in case it was cut mid-line.
            let first = view.shown_lines.0.max(1);
            let last = first.saturating_add(199);
            format!("sed -n '{first},{last}p;{last}q' '{path}' | head -c 4096")
        } else {
            // Large-log sampling tracks raw offsets, not sanitized ones.
            let count = end.saturating_sub(start).min(4096);
            format!("dd if='{path}' bs=1 skip={start} count={count} 2>/dev/null")
        };
        out.push_str(&format!("\nRead more: {command}"));
    }
    ToolOutput::ok(out)
}

fn small_log_on_disk(log_path: &std::path::Path, summary: &PumpSummary) -> bool {
    let file_len = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    summary.total_bytes <= INLINE_BUDGET_BYTES as u64 && file_len <= INLINE_BUDGET_BYTES as u64
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
    let use_disk = small_log_on_disk(log_path, summary);
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
        has_cr: summary.has_cr,
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

#[path = "bash_tests.rs"]
#[cfg(test)]
mod tests;
