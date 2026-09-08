//! shell/tools/shell_output.rs — cursor reads, wait modes, list (brief 2C).
//!
//! One tool that reads only new bytes from a task's log by offset, can block
//! until the next write or until exit, lists tasks when no id is given, and
//! never returns silence. Registration in `Registry`/`plugin.rs` is 2E's job;
//! until then this module is reachable as
//! `gray_tools::shell::tools::shell_output::ShellOutputTool` (same unwired
//! pattern as 1A/1B/1C before 1D).
//!
//! Reads use `File::seek` + a bounded `take` — never `read_to_string` a
//! possibly multi-GB log. Waits park on the registry's watch channels, with
//! a log-length check for pre-subscribe writes; no busy-wait. Offsets are file bytes; the `middle_out` marker's absolute
//! offsets are sanitized-space (same drift as 1D's `build_view` — clean ASCII
//! logs are unaffected).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

use crate::shell::contract::{
    MAX_TIMEOUT_SECS, TaskId, TaskInfo, TaskState, VIEW_BUDGET_BYTES, VIEW_BUDGET_LINES,
};
use crate::shell::fence::fence;
use crate::shell::registry::registry;
use crate::shell::view::{fmt_num, format_elapsed, home_relative, middle_out, resume_hint};
use crate::{fail, get_opt_u64};

/// Default/max window for one read (brief: default 16384, cap 51200).
pub const DEFAULT_MAX_BYTES: u64 = 16_384;
pub const MAX_READ_BYTES: u64 = 51_200;
/// Default `timeout` for the `wait` arms (brief: 30, range 1..=600).
pub const DEFAULT_WAIT_SECS: u64 = 30;

/// Reads task output by byte offset (`shell_output`).
pub struct ShellOutputTool;

#[async_trait]
impl Tool for ShellOutputTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "shell_output",
            "Read new output from a background shell task by byte offset, optionally blocking \
             until more output arrives or the task exits. With no task_id, lists this session's tasks.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id (e.g. \"t2\"). Omit to list this session's tasks"},
                    "from_offset": {"type": "integer", "description": "Byte offset to read from (default 0); use next_offset from the last read"},
                    "wait": {"type": "string", "description": "\"none\" (default), \"output\" (block until the next write), or \"exit\" (block until the task exits)"},
                    "timeout": {"type": "integer", "description": "Seconds to block for wait (default 30, capped at 600)"},
                    "max_bytes": {"type": "integer", "description": "Max bytes to return in one read (default 16384, capped at 51200)"}
                },
                "required": []
            }),
        )
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let task_id = match get_opt_str(&args, "task_id") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let from_offset = match get_opt_u64(&args, "from_offset") {
            Ok(o) => o.unwrap_or(0),
            Err(e) => return e,
        };
        let wait = match get_opt_str(&args, "wait") {
            Ok(w) => w.unwrap_or_else(|| "none".to_string()),
            Err(e) => return e,
        };
        if !matches!(wait.as_str(), "none" | "output" | "exit") {
            return fail(format!(
                "invalid argument 'wait': expected \"none\", \"output\" or \"exit\", got \"{wait}\""
            ));
        }
        let timeout = match get_opt_u64(&args, "timeout") {
            Ok(t) => t.unwrap_or(DEFAULT_WAIT_SECS).clamp(1, MAX_TIMEOUT_SECS),
            Err(e) => return e,
        };
        let max_bytes = match get_opt_u64(&args, "max_bytes") {
            Ok(m) => m.unwrap_or(DEFAULT_MAX_BYTES).clamp(1, MAX_READ_BYTES),
            Err(e) => return e,
        };

        let session = ctx.session_id.clone().unwrap_or_else(|| "nosession".into());
        let Some(task_id) = task_id else {
            return ToolOutput::ok(list_tasks(&session));
        };
        let Some(id) = parse_task_id(&task_id) else {
            return fail(unknown_task(&session, &task_id));
        };
        if registry().get(&session, id).is_none() {
            return fail(unknown_task(&session, &task_id));
        }

        // Park on the watch channels; re-read the registry after (the task
        // may have exited while we waited).
        let mut cancelled = false;
        if wait == "output" {
            cancelled = wait_for_output(&session, id, from_offset, timeout, &ctx.cancel).await;
        } else if wait == "exit" {
            cancelled = wait_for_exit(&session, id, timeout, &ctx.cancel).await;
        }
        let Some(info) = registry().get(&session, id) else {
            return fail(unknown_task(&session, &task_id));
        };

        let len = log_len(&info.log_path);
        if from_offset > len {
            let head = header(&info, len, len, len);
            return ToolOutput::ok(format!(
                "{head}\noffset {} is past the end (log is {} bytes). Retry with from_offset={len} or 0.",
                fmt_num(from_offset as usize),
                fmt_num(len as usize),
            ));
        }

        let (window, a, b, more) = read_window(&info.log_path, from_offset, len, max_bytes);
        let exited = matches!(info.state, TaskState::Exited { .. });
        let mut out = header(&info, a, b, len);

        if b == a {
            // Nothing new. Never silence: name the recovery.
            out.push('\n');
            out.push_str(&nothing_new(&info, &wait, len, timeout, cancelled, exited));
            return ToolOutput::ok(out);
        }

        let mut view = middle_out(&window, VIEW_BUDGET_BYTES, VIEW_BUDGET_LINES, a);
        if view.omitted_range.is_some() {
            let hint = resume_hint(id, &view);
            view.body = view.body.replace("{{MARKER}}", &hint);
        }
        if !view.body.is_empty() {
            out.push('\n');
            out.push_str(&fence(id, &view.body));
        }
        if more {
            out.push_str(&format!(
                "\n…more available, next_offset={b}. shell_output(task_id=\"{id}\", from_offset={b}) for the rest."
            ));
        } else if !exited && !window.ends_with(b"\n") {
            out.push_str("\n(last line incomplete — the task is still writing it)");
        }
        if cancelled {
            out.push_str("\nwait cancelled — call again to keep reading");
        } else if wait == "exit" && !exited {
            out.push_str(&format!(
                "\nstill running after {timeout}s; call again with wait=exit"
            ));
        }
        ToolOutput::ok(out)
    }
}

/// Optional string argument (`null`/absent -> `None`). Local until 4A
/// centralizes arg parsing.
fn get_opt_str(args: &Value, key: &str) -> Result<Option<String>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected string"))),
    }
}

/// Accept `t12` or bare `12` (whitespace-tolerant). Anything else is an
/// unknown task, not an arg error — the caller names what exists.
fn parse_task_id(raw: &str) -> Option<TaskId> {
    let s = raw.trim().strip_prefix(['t', 'T']).unwrap_or(raw.trim());
    s.parse::<u32>().ok().map(TaskId)
}

fn unknown_task(session: &str, raw: &str) -> String {
    // Chosen fix for session-scoped tasks: keep scoping (frozen contract),
    // make errors unambiguous by naming sessions and where the id lives.
    let ids: Vec<String> = registry()
        .list(session)
        .iter()
        .map(|t| t.id.to_string())
        .collect();
    let mut out = if ids.is_empty() {
        format!(
            "unknown task \"{raw}\" in session \"{session}\". No tasks this session yet — bash(background=true) starts one."
        )
    } else {
        format!(
            "unknown task \"{raw}\" in session \"{session}\". Known tasks: {}.",
            ids.join(", ")
        )
    };
    // If the numeric id parses and lives in other session(s), say where —
    // tasks are session-scoped, so cross-session refs need the owning session.
    let num = raw.trim().strip_prefix(['t', 'T']).unwrap_or(raw.trim());
    if let Ok(n) = num.parse::<u32>() {
        let others: Vec<String> = registry()
            .sessions_with_task(crate::shell::contract::TaskId(n))
            .into_iter()
            .filter(|s| s != session)
            .collect();
        if !others.is_empty() {
            out.push_str(&format!(
                " Exists in session-scoped session(s): {}. Tasks are session-scoped.",
                others.join(", ")
            ));
        }
    }
    out
}

/// No task_id: one line per task (id-sorted; `list` already sorts) plus a
/// summary line. Empty registry names the way out.
fn list_tasks(session: &str) -> String {
    let tasks = registry().list(session);
    if tasks.is_empty() {
        return "no tasks this session. bash(background=true) starts one.".to_string();
    }
    let mut out: Vec<String> = tasks.iter().map(list_line).collect();
    let last = tasks.last().map(|t| t.id.to_string()).unwrap_or_default();
    let n = tasks.len();
    out.push(format!(
        "{n} task{} this session. shell_output(task_id=\"{last}\") to read output.",
        if n == 1 { "" } else { "s" }
    ));
    out.join("\n")
}

fn list_line(t: &TaskInfo) -> String {
    match &t.state {
        TaskState::Running => format!(
            "{} · running · pid {} · {} · log {}",
            t.id,
            t.pid,
            format_elapsed(t.started.elapsed()),
            home_relative(&t.log_path),
        ),
        TaskState::Exited { report, at } => format!(
            "{}{} · ran {} · finished {} ago · log {}",
            t.id,
            report_line(report),
            format_elapsed(at.saturating_duration_since(t.started)),
            format_elapsed(Instant::now().saturating_duration_since(*at)),
            home_relative(&t.log_path),
        ),
    }
}

/// `exit 0` plus the benign/OOM note in parens, mirroring `view::header`.
fn report_line(r: &crate::shell::contract::ExitReport) -> String {
    match &r.note {
        Some(n) => format!(" · {} ({n})", r.label),
        None => format!(" · {}", r.label),
    }
}

/// Read header: running vs exited shape from the brief.
fn header(info: &TaskInfo, a: u64, b: u64, len: u64) -> String {
    let bytes = format!(
        "bytes {}–{} of {} · next_offset={b}",
        fmt_num(a as usize),
        fmt_num(b as usize),
        fmt_num(len as usize),
    );
    match &info.state {
        TaskState::Running => format!(
            "{} · running · pid {} · {} · {bytes}",
            info.id,
            info.pid,
            format_elapsed(info.started.elapsed()),
        ),
        TaskState::Exited { report, at } => format!(
            "{}{} · ran {} · finished {} ago · {bytes}",
            info.id,
            report_line(report),
            format_elapsed(at.saturating_duration_since(info.started)),
            format_elapsed(Instant::now().saturating_duration_since(*at)),
        ),
    }
}

/// Nothing new after the wait: the anti-polling string (wait=none),
/// the timeout restatement (wait=output/exit + still running), or the
/// cancel note. Exited tasks say the log is complete.
fn nothing_new(
    info: &TaskInfo,
    wait: &str,
    len: u64,
    timeout: u64,
    cancelled: bool,
    exited: bool,
) -> String {
    if cancelled {
        return format!(
            "wait cancelled after {}",
            format_elapsed(info.started.elapsed())
        );
    }
    let done = if exited {
        format!(" {} has exited — this is all the output there is.", info.id)
    } else {
        String::new()
    };
    match wait {
        "output" => format!(
            "no new output after {timeout}s (log is {} bytes, next_offset={len}). Call again with wait=\"output\" to keep waiting.{done}",
            fmt_num(len as usize),
        ),
        "exit" if !exited => {
            format!("still running after {timeout}s; call again with wait=exit")
        }
        _ => format!(
            "no new output for {} (log is {} bytes, next_offset={len}). Do not poll: call again with wait=\"output\" to block until the next write, wait=\"exit\" to block until exit, or sleep(seconds) and continue working.{done}",
            info.id,
            fmt_num(len as usize),
        ),
    }
}

/// Block until the log grows past `from` or the task exits (brief: the watch
/// channels are the only wait primitive). True when `cancel` fired.
async fn wait_for_output(
    session: &str,
    id: TaskId,
    from: u64,
    timeout: u64,
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    let (mut bytes_rx, mut exit_rx) = match (
        registry().bytes_rx(session, id),
        registry().exit_rx(session, id),
    ) {
        (Some(b), Some(e)) => (b, e),
        _ => return false,
    };
    // Ground truth for the file: the pump writes the log before it sends the
    // watch, so bytes may sit on disk while the watch is still stale.
    let log_path = registry().get(session, id).map(|t| t.log_path);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        // Final borrow on the exit path: the waiter drains the pump before
        // marking, so an exited task's tail is already on disk.
        if *bytes_rx.borrow() > from || exit_rx.borrow().is_some() {
            return false;
        }
        // Pre-subscribe writes: bytes already on disk satisfy the wait even
        // when the watch hasn't published them yet.
        if let Some(p) = log_path.as_ref()
            && log_len(p) > from
        {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline - now;
        tokio::select! {
            _ = bytes_rx.changed() => {}
            _ = exit_rx.changed() => {}
            _ = tokio::time::sleep(remaining) => {}
            _ = cancel.cancelled() => return true,
        }
    }
}

/// Block until the exit watch resolves. True when `cancel` fired.
async fn wait_for_exit(
    session: &str,
    id: TaskId,
    timeout: u64,
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    let mut exit_rx = match registry().exit_rx(session, id) {
        Some(rx) => rx,
        None => return false,
    };
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        if exit_rx.borrow().is_some() {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline - now;
        tokio::select! {
            _ = exit_rx.changed() => {}
            _ = tokio::time::sleep(remaining) => {}
            _ = cancel.cancelled() => return true,
        }
    }
}

fn log_len(log_path: &Path) -> u64 {
    std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0)
}

/// Bounded window `[from, b)`: at most `max_bytes`, cut back to the last
/// `\n` when more remains. Returns `(bytes, a, b, more)`.
fn read_window(log_path: &Path, from: u64, len: u64, max_bytes: u64) -> (Vec<u8>, u64, u64, bool) {
    let want = (len - from).min(max_bytes) as usize;
    let mut buf = vec![0u8; want];
    let got = std::fs::File::open(log_path)
        .and_then(|mut f| {
            f.seek(SeekFrom::Start(from))?;
            let mut read = 0;
            while read < want {
                match f.read(&mut buf[read..])? {
                    0 => break,
                    n => read += n,
                }
            }
            Ok(read)
        })
        .unwrap_or(0);
    buf.truncate(got);
    let end = from + got as u64;
    if end < len {
        // More remains: cut back to the last newline so we never hand out a
        // partial line when the caller will page the rest anyway.
        if let Some(i) = buf.iter().rposition(|&c| c == b'\n') {
            let b = from + i as u64 + 1;
            buf.truncate(i + 1);
            return (buf, from, b, true);
        }
        return (buf, from, end, true);
    }
    (buf, from, end, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use gray_core::agent::Tool;
    use serde_json::json;

    use crate::shell::tools::bash::BashTool;

    static SESS_N: AtomicU64 = AtomicU64::new(0);

    fn sess(tag: &str) -> String {
        format!(
            "out-2c-{tag}-{}-{}",
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

    fn task_n(content: &str, prefix: &str) -> u32 {
        content
            .lines()
            .next()
            .and_then(|h| h.split(prefix).nth(1))
            .and_then(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .expect("header names the task")
    }

    fn next_offset(content: &str) -> u64 {
        content
            .split("next_offset=")
            .nth(1)
            .and_then(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .expect("result names next_offset")
    }

    #[test]
    fn parse_task_id_shapes() {
        assert_eq!(parse_task_id("t4").map(|t| t.0), Some(4));
        assert_eq!(parse_task_id("T12").map(|t| t.0), Some(12));
        assert_eq!(parse_task_id("7").map(|t| t.0), Some(7));
        assert_eq!(parse_task_id("  t9  ").map(|t| t.0), Some(9));
        assert!(parse_task_id("tx").is_none());
        assert!(parse_task_id("").is_none());
        assert!(parse_task_id("t").is_none());
    }

    #[test]
    fn bad_wait_is_an_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let out = ShellOutputTool
                .execute(
                    &ctx_for(&sess("wait")),
                    json!({"task_id": "t1", "wait": "soon"}),
                )
                .await;
            assert!(out.is_error, "{}", out.content);
            assert!(out.content.contains("'wait'"), "{}", out.content);
        });
    }

    #[tokio::test]
    async fn empty_list_names_the_way_out() {
        let out = ShellOutputTool
            .execute(&ctx_for(&sess("empty")), json!({}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("no tasks this session"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("bash(background=true)"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn unknown_id_fails_and_lists_ids() {
        let session = sess("unknown");
        let ctx = ctx_for(&session);
        let bg = BashTool
            .execute(&ctx, json!({"command": "echo hi", "background": true}))
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        // Let the waiter reap it so the registry holds an Exited task.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let out = ShellOutputTool
            .execute(&ctx, json!({"task_id": "t999"}))
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("unknown task"), "{}", out.content);
        assert!(out.content.contains("Known tasks: t1"), "{}", out.content);
    }

    #[tokio::test]
    async fn cursor_read_then_no_new_output_then_wait_output() {
        let session = sess("cursor");
        let ctx = ctx_for(&session);
        // One burst, 5 s of silence, one more line: every step is deterministic.
        let bg = BashTool
            .execute(
                &ctx,
                json!({"command": "echo first; sleep 5; echo second; sleep 1", "background": true}),
            )
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        let n = task_n(&bg.content, "started t");
        let id = format!("t{n}");

        // First read blocks for the burst: bytes + next_offset.
        let first = ShellOutputTool
            .execute(
                &ctx,
                json!({"task_id": id, "wait": "output", "timeout": 10}),
            )
            .await;
        assert!(!first.is_error, "{}", first.content);
        let at = next_offset(&first.content);
        assert_eq!(at, 6, "exactly 'first\\n': {}", first.content);
        assert!(first.content.contains("running"), "{}", first.content);

        // Immediate re-read at the cursor: the anti-polling string (5 s quiet).
        let again = ShellOutputTool
            .execute(&ctx, json!({"task_id": id, "from_offset": at}))
            .await;
        assert!(!again.is_error, "{}", again.content);
        assert!(again.content.contains("no new output"), "{}", again.content);
        assert!(
            again.content.contains("wait=\"output\""),
            "{}",
            again.content
        );

        // wait=output: the second line lands at ~5 s; generous ceiling.
        let t0 = Instant::now();
        let waited = ShellOutputTool
            .execute(
                &ctx,
                json!({"task_id": id, "from_offset": at, "wait": "output", "timeout": 10}),
            )
            .await;
        let dt = t0.elapsed();
        assert!(!waited.is_error, "{}", waited.content);
        assert!(
            next_offset(&waited.content) > at,
            "new bytes: {}",
            waited.content
        );
        assert!(waited.content.contains("second"), "{}", waited.content);
        assert!(
            dt < Duration::from_secs(9),
            "woke on the write, not the timeout: {dt:?}"
        );

        // Past-the-end offset is a note, not an error.
        let past = ShellOutputTool
            .execute(&ctx, json!({"task_id": id, "from_offset": 999_999_999_u64}))
            .await;
        assert!(!past.is_error, "{}", past.content);
        assert!(past.content.contains("past the end"), "{}", past.content);
    }

    #[tokio::test]
    async fn wait_exit_returns_on_exit_and_on_timeout() {
        let session = sess("waitexit");
        let ctx = ctx_for(&session);
        let bg = BashTool
            .execute(&ctx, json!({"command": "sleep 2", "background": true}))
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        let id = format!("t{}", task_n(&bg.content, "started t"));

        // wait=exit resolves at ~2 s with the exit header.
        let t0 = Instant::now();
        let done = ShellOutputTool
            .execute(&ctx, json!({"task_id": id, "wait": "exit", "timeout": 10}))
            .await;
        let dt = t0.elapsed();
        assert!(!done.is_error, "{}", done.content);
        assert!(done.content.contains("exit 0"), "{}", done.content);
        assert!(
            dt >= Duration::from_millis(1500) && dt < Duration::from_secs(8),
            "{dt:?}"
        );

        // A longer sleeper with a 1 s wait: still running + call-again hint.
        let bg2 = BashTool
            .execute(&ctx, json!({"command": "sleep 30", "background": true}))
            .await;
        let id2 = format!("t{}", task_n(&bg2.content, "started t"));
        let t0 = Instant::now();
        let early = ShellOutputTool
            .execute(&ctx, json!({"task_id": id2, "wait": "exit", "timeout": 1}))
            .await;
        let dt = t0.elapsed();
        assert!(!early.is_error, "{}", early.content);
        assert!(
            early.content.contains("still running after 1s"),
            "{}",
            early.content
        );
        assert!(
            early.content.contains("call again with wait=exit"),
            "{}",
            early.content
        );
        assert!(
            dt < Duration::from_secs(5),
            "returned at the timeout: {dt:?}"
        );
        // Cleanup behind 2D's kill: SIGTERM our own group child directly.
        // The sleeper exits on its own in 30 s; the suite does not wait.
    }

    #[tokio::test]
    async fn list_shows_tasks_sorted_with_summary() {
        let session = sess("list");
        let ctx = ctx_for(&session);
        for i in 1..=3 {
            let out = BashTool
                .execute(
                    &ctx,
                    json!({"command": format!("echo list-{i}"), "background": true}),
                )
                .await;
            assert!(!out.is_error, "{}", out.content);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let out = ShellOutputTool.execute(&ctx, json!({})).await;
        assert!(!out.is_error, "{}", out.content);
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 4, "3 tasks + summary: {}", out.content);
        assert!(lines[0].starts_with("t1 · "), "{}", lines[0]);
        assert!(lines[1].starts_with("t2 · "), "{}", lines[1]);
        assert!(lines[2].starts_with("t3 · "), "{}", lines[2]);
        assert!(lines[3].contains("3 tasks this session"), "{}", lines[3]);
    }

    #[tokio::test]
    async fn big_window_gets_absolute_middle_out_marker() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let spew = manifest.join("tests/fixtures/shell/spew.sh");
        let session = sess("bigwin");
        let ctx = ctx_for(&session);
        let fg = BashTool
            .execute(
                &ctx,
                json!({"command": format!("sh {} 30000", spew.display())}),
            )
            .await;
        assert!(!fg.is_error, "{}", fg.content);
        let n = task_n(&fg.content, "/t");
        let id = format!("t{n}");

        // Whole 1.2 MiB log through a 50 KiB window: line budget forces the marker.
        let out = ShellOutputTool
            .execute(
                &ctx,
                json!({"task_id": id, "from_offset": 0, "max_bytes": 51200}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("…more available"),
            "{}",
            &out.content[..400]
        );
        let at = next_offset(&out.content);
        assert!(at > 0 && at <= 51200, "next_offset in-window: {at}");
        // … and paging at the returned cursor keeps working.
        let page2 = ShellOutputTool
            .execute(
                &ctx,
                json!({"task_id": id, "from_offset": at, "max_bytes": 51200}),
            )
            .await;
        assert!(!page2.is_error, "{}", page2.content);
        assert!(page2.content.contains("…more available"), "pages chain");
    }

    #[tokio::test]
    async fn cross_session_read_error_names_other_session() {
        // Bug 3: session-scoped by design; error must say where tN actually lives.
        let sess_a = sess("cross-a");
        let sess_b = sess("cross-b");
        let ctx_a = ctx_for(&sess_a);
        let bg = BashTool
            .execute(&ctx_a, json!({"command": "echo hi", "background": true}))
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ctx_b = ctx_for(&sess_b);
        let out = ShellOutputTool
            .execute(&ctx_b, json!({"task_id": "t1"}))
            .await;
        assert!(
            out.is_error,
            "cross-session read must stay scoped, got {}",
            out.content
        );
        assert!(out.content.contains("unknown task"), "{}", out.content);
        assert!(
            out.content.contains(&sess_a) || out.content.contains("session-scoped"),
            "error must name where t1 lives (session {sess_a}), got {}",
            out.content
        );
    }

    #[tokio::test]
    async fn cancel_during_wait_returns_promptly() {
        let session = sess("cancel");
        let ctx = ctx_for(&session);
        let bg = BashTool
            .execute(&ctx, json!({"command": "sleep 30", "background": true}))
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        let id = format!("t{}", task_n(&bg.content, "started t"));
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            cancel.cancel();
        });
        let t0 = Instant::now();
        let out = ShellOutputTool
            .execute(&ctx, json!({"task_id": id, "wait": "exit", "timeout": 30}))
            .await;
        let dt = t0.elapsed();
        assert!(dt < Duration::from_secs(5), "prompt return: {dt:?}");
        assert!(out.content.contains("cancelled"), "{}", out.content);
    }

    // UNRUN (cargo test banned under X).
    #[tokio::test]
    async fn wait_output_sees_pre_subscribe_file_bytes() {
        // Bytes already on disk before we subscribe must satisfy wait=output
        // at once, even when the watch hasn't published them yet.
        use std::io::Write as _;

        let session = sess("presub");
        let ctx = ctx_for(&session);
        // `echo` makes the pump create the log file; `sleep` keeps the task
        // alive for the wait below. (A bare `sleep` never produces output,
        // so no log file ever appears and the open below cannot succeed.)
        let bg = BashTool
            .execute(
                &ctx,
                json!({"command": "echo ready && sleep 30", "background": true}),
            )
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        let n = task_n(&bg.content, "started t");
        let id = format!("t{n}");
        // A write that bypassed the pump's watch sender entirely.
        let info = registry()
            .get(&session, TaskId(n))
            .expect("task just started");
        // The pump creates the log file asynchronously; wait for the echo
        // to land (CI runners are slow) so the append below can't lose a
        // creation race. tokio sleep (not thread sleep): this suite runs on
        // tokio's current_thread runtime, where blocking the thread would
        // starve the very pump future we're waiting for.
        let mut waited = 0;
        while !info.log_path.exists() && waited < 100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            waited += 1;
        }
        assert!(info.log_path.exists(), "pump never created log file");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&info.log_path)
            .expect("open log")
            .write_all(b"pre-subscribed\n")
            .expect("append");
        let t0 = Instant::now();
        let out = ShellOutputTool
            .execute(
                &ctx,
                json!({"task_id": id, "from_offset": 0, "wait": "output", "timeout": 10}),
            )
            .await;
        let dt = t0.elapsed();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("pre-subscribed"), "{}", out.content);
        assert!(
            dt < Duration::from_secs(5),
            "pre-subscribe bytes must not wait out the timeout: {dt:?}"
        );
        // The sleeper exits on its own in 30 s; the suite does not wait.
    }
}
