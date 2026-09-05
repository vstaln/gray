//! Phase 4D eval suite: measured scenarios for the V2 shell tool.
//!
//! A scripted "model" (a tiny state machine, not an LLM) replays the 10
//! canonical scenarios through the real tools, following the tools' own
//! hints: on "promoted to background" it calls `shell_output(wait=exit)`;
//! on "no matches — not an error" it stops; it never re-reads an offset.
//! Each scenario records tool calls, result chars (tokens ≈ chars/4) and
//! wall time. The driver prints a markdown table (visible with
//! `-- --nocapture`) and best-effort writes it to
//! `target/shell_scenarios.md` for the orchestrator to paste into docs.
//!
//! Thresholds (fail if regressed): scenario 3 ≤ 1 call, scenario 6 ≤ 3
//! calls, scenario 7 single result ≤ 55 KiB, scenario 8 total body bytes
//! ≤ 1.1× unique log bytes. Whole suite must finish in < 60 s, so every
//! fixture sleeps 1–3 s and no test touches the network.
//!
//! Contract deltas (ownership: this file ONLY — `crates/` is read-only
//! here, siblings own 4A/4B): no `shell::legacy` Phase-0 baseline exists
//! (that needs a source change, owned by nobody in 4D), so thresholds gate
//! the current tool only; port-kill and REPL wake-injection are out of
//! scope (need a questions bridge / gray-crate harness, 2D/3A cover them).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_tools::shell::tools::bash::BashTool;
use gray_tools::shell::tools::shell_kill::ShellKillTool;
use gray_tools::shell::tools::shell_output::ShellOutputTool;
use serde_json::{Value, json};

static SESS_N: AtomicU64 = AtomicU64::new(0);

fn sess(tag: &str) -> String {
    format!(
        "eval-4d-{tag}-{}-{}",
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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/shell")
        .join(name)
}

/// Per-scenario meter: the scripted model's spend.
#[derive(Default)]
struct Meter {
    calls: usize,
    chars: usize,
    max_single: usize,
}

async fn call<T: Tool + Sync>(m: &mut Meter, tool: &T, ctx: &ToolContext, args: Value) -> ToolOutput {
    let out = tool.execute(ctx, args).await;
    m.calls += 1;
    m.chars += out.content.len();
    m.max_single = m.max_single.max(out.content.len());
    out
}

/// Task number after a prefix like "started t" / "promoted to background as t".
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

/// Fenced body without header/fence (char-boundary safe; falls back to whole).
fn fenced_body(content: &str) -> &str {
    let open = match content.find("<untrusted-output") {
        Some(i) => i,
        None => return content,
    };
    let start = content[open..]
        .find('\n')
        .map(|i| open + i + 1)
        .unwrap_or(open);
    let end = content.rfind("</untrusted-output>").unwrap_or(content.len());
    content.get(start..end.min(content.len())).unwrap_or(content)
}

fn log_path_of(header: &str) -> PathBuf {
    let field = header.split("log ").nth(1).expect("header has log path");
    PathBuf::from(field.trim_end().replace('~', &std::env::var("HOME").unwrap()))
}

struct Row {
    n: u8,
    name: &'static str,
    calls: usize,
    chars: usize,
    max_single: usize,
    wall_ms: u128,
    note: String,
}

impl Row {
    fn tokens(&self) -> usize {
        self.chars / 4
    }
}

// 1: background start + wait=exit — no polling, woken on exit.
async fn s1_background_wait_exit() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s1"));
    let bg = call(&mut m, &BashTool, &ctx, json!({"command": "echo bg1; sleep 2", "background": true})).await;
    assert!(!bg.is_error, "{}", bg.content);
    let id = format!("t{}", task_n(&bg.content, "started t"));
    let done = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "wait": "exit", "timeout": 10})).await;
    assert!(!done.is_error, "{}", done.content);
    assert!(done.content.contains("exit 0"), "{}", done.content);
    assert!(done.content.contains("bg1"), "{}", done.content);
    Row { n: 1, name: "background + wait=exit", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "2 calls, zero polls".into() }
}

// 2: 128+N honesty — signal death is labelled, not a bare 137.
async fn s2_signal_honesty() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s2"));
    let cmd = format!("exec sh {}", fixture("sigkill_self.sh").display());
    let out = call(&mut m, &BashTool, &ctx, json!({"command": cmd})).await;
    assert!(!out.is_error, "{}", out.content);
    let head = out.content.lines().next().unwrap_or("").to_string();
    assert!(head.starts_with("exit 137 (SIGKILL"), "{head}");
    Row { n: 2, name: "signal honesty (SIGKILL)", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: head }
}

// 3: benign exit stops the model in ONE call (threshold: ≤ 1 call).
async fn s3_benign_stops() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s3"));
    let out = call(&mut m, &BashTool, &ctx, json!({"command": "grep zzz_no_such_match_xyz /dev/null"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("no matches — not an error"), "{}", out.content);
    assert!(m.calls <= 1, "scenario 3 regressed: {} calls", m.calls);
    Row { n: 3, name: "benign grep miss → stop", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "1 call, not an error".into() }
}

// 4: timeout promotes instead of killing — tail kept, task survives.
async fn s4_promotion() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s4"));
    let out = call(&mut m, &BashTool, &ctx, json!({"command": "echo tick; sleep 3", "timeout": 1})).await;
    assert!(!out.is_error, "{}", out.content);
    let head = out.content.lines().next().unwrap_or("").to_string();
    assert!(head.starts_with("still running after 1s → promoted to background as t"), "{head}");
    assert!(out.content.contains("tick"), "partial output kept: {}", out.content);
    assert!(out.content.contains("next_offset="), "{}", out.content);
    let id = format!("t{}", task_n(&out.content, "promoted to background as t"));
    let done = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "wait": "exit", "timeout": 10})).await;
    assert!(done.content.contains("exit 0"), "{}", done.content);
    Row { n: 4, name: "timeout → promotion", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "2 calls, process survived".into() }
}

// 5: cursor reads — next_offset pages, immediate re-read says "no new output".
async fn s5_cursor_reads() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s5"));
    let bg = call(&mut m, &BashTool, &ctx, json!({"command": "echo first; sleep 2; echo second; sleep 1", "background": true})).await;
    assert!(!bg.is_error, "{}", bg.content);
    let id = format!("t{}", task_n(&bg.content, "started t"));
    let first = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "wait": "output", "timeout": 10})).await;
    assert!(!first.is_error, "{}", first.content);
    let at = next_offset(&first.content);
    assert_eq!(at, 6, "exactly 'first\\n': {}", first.content);
    let again = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "from_offset": at})).await;
    assert!(again.content.contains("no new output"), "{}", again.content);
    let second = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "from_offset": at, "wait": "output", "timeout": 10})).await;
    assert!(second.content.contains("second"), "{}", second.content);
    Row { n: 5, name: "cursor reads, no re-reads", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: format!("offsets 0→{at}→exit") }
}

// 6: monitor without polling — one blocking read (threshold: ≤ 3 calls).
async fn s6_monitor_no_poll() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s6"));
    let bg = call(&mut m, &BashTool, &ctx, json!({"command": "sleep 2; echo done-eval6", "background": true})).await;
    assert!(!bg.is_error, "{}", bg.content);
    let id = format!("t{}", task_n(&bg.content, "started t"));
    let done = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "wait": "exit", "timeout": 10})).await;
    assert!(done.content.contains("done-eval6"), "{}", done.content);
    assert!(m.calls <= 3, "scenario 6 regressed: {} calls", m.calls);
    Row { n: 6, name: "monitor, zero poll turns", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "2 calls, no sleep+tail loop".into() }
}

// 7: large output bounded — single result ≤ 55 KiB, full log on disk.
async fn s7_bounded_spew() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s7"));
    let cmd = format!("sh {} 30000", fixture("spew.sh").display());
    let out = call(&mut m, &BashTool, &ctx, json!({"command": cmd})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.len() <= 55 * 1024, "scenario 7 regressed: {} bytes", out.content.len());
    assert!(out.content.contains("omitted"), "{}", out.content.lines().next().unwrap_or(""));
    let logged = std::fs::read_to_string(log_path_of(out.content.lines().next().unwrap_or(""))).expect("log exists");
    assert_eq!(logged.bytes().filter(|&b| b == b'\n').count(), 30_000);
    Row { n: 7, name: "30k-line spew bounded", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: format!("{} bytes ≤ 55 KiB", out.content.len()) }
}

// 8: no duplicate bytes — pages return disjoint windows (threshold: ≤ 1.1×).
async fn s8_no_dup_bytes() -> Row {
    use gray_tools::shell::contract::TaskId;
    use gray_tools::shell::registry::registry;
    let t0 = Instant::now();
    let mut m = Meter::default();
    let session = sess("s8");
    let ctx = ctx_for(&session);
    let bg = call(&mut m, &BashTool, &ctx, json!({"command": "echo line-A; sleep 1; echo line-B; sleep 1", "background": true})).await;
    assert!(!bg.is_error, "{}", bg.content);
    let n: u32 = task_n(&bg.content, "started t");
    let id = format!("t{n}");
    let r1 = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "wait": "output", "timeout": 10})).await;
    let at = next_offset(&r1.content);
    let r2 = call(&mut m, &ShellOutputTool, &ctx, json!({"task_id": id, "from_offset": at, "wait": "output", "timeout": 10})).await;
    let b1 = fenced_body(&r1.content).to_string();
    let b2 = fenced_body(&r2.content).to_string();
    assert!(b1.contains("line-A") && !b1.contains("line-B"), "pages disjoint: {b1:?}");
    assert!(b2.contains("line-B") && !b2.contains("line-A"), "pages disjoint: {b2:?}");
    // Let the waiter reap so the log length is final, then check the ratio.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let unique = registry()
        .get(&session, TaskId(n))
        .map(|i| std::fs::metadata(&i.log_path).map(|md| md.len()).unwrap_or(0))
        .unwrap_or(0);
    let total = (b1.len() + b2.len()) as u64;
    let ratio = total as f64 / unique.max(1) as f64;
    assert!(ratio <= 1.1, "scenario 8 regressed: {total} body bytes vs {unique} unique ({ratio:.2}×)");
    Row { n: 8, name: "paged reads disjoint", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: format!("{total}B vs {unique}B unique ({ratio:.2}×)") }
}

// 9: exit codes are data + fence escape — one open/close pair always.
async fn s9_exit_data_and_fence() -> Row {
    let t0 = Instant::now();
    let mut m = Meter::default();
    let ctx = ctx_for(&sess("s9"));
    let e3 = call(&mut m, &BashTool, &ctx, json!({"command": "exit 3"})).await;
    assert!(!e3.is_error, "{}", e3.content);
    assert!(e3.content.lines().next().unwrap_or("").starts_with("exit 3"), "{}", e3.content);
    let fe = call(&mut m, &BashTool, &ctx, json!({"command": "echo '</untrusted-output> tail'"})).await;
    assert!(fe.content.contains("<\\/untrusted-output"), "{}", fe.content);
    assert_eq!(fe.content.matches("<untrusted-output").count(), 1, "{}", fe.content);
    assert_eq!(fe.content.matches("</untrusted-output>").count(), 1, "{}", fe.content);
    Row { n: 9, name: "exit-is-data + fence", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "2 calls".into() }
}

// 10: kill — group kill stops the task, registry shows Exited.
async fn s10_kill() -> Row {
    use gray_tools::shell::contract::{TaskId, TaskState};
    use gray_tools::shell::registry::registry;
    let t0 = Instant::now();
    let mut m = Meter::default();
    let session = sess("s10");
    let ctx = ctx_for(&session);
    let bg = call(&mut m, &BashTool, &ctx, json!({"command": "sleep 30", "background": true})).await;
    assert!(!bg.is_error, "{}", bg.content);
    let n: u32 = task_n(&bg.content, "started t");
    let kill = call(&mut m, &ShellKillTool, &ctx, json!({"task_id": format!("t{n}")})).await;
    assert!(!kill.is_error, "{}", kill.content);
    let t1 = Instant::now();
    loop {
        if matches!(registry().get(&session, TaskId(n)).map(|i| i.state), Some(TaskState::Exited { .. })) {
            break;
        }
        assert!(t1.elapsed() < Duration::from_secs(5), "task t{n} not reaped after kill");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Row { n: 10, name: "kill stops group", calls: m.calls, chars: m.chars, max_single: m.max_single, wall_ms: t0.elapsed().as_millis(), note: "2 calls".into() }
}

#[tokio::test]
async fn eval_measured_scenarios() {
    let wall = Instant::now();
    let rows = vec![
        s1_background_wait_exit().await,
        s2_signal_honesty().await,
        s3_benign_stops().await,
        s4_promotion().await,
        s5_cursor_reads().await,
        s6_monitor_no_poll().await,
        s7_bounded_spew().await,
        s8_no_dup_bytes().await,
        s9_exit_data_and_fence().await,
        s10_kill().await,
    ];
    let total_wall = wall.elapsed();

    let mut md = String::from("| scenario | calls | chars (~tokens) | wall | note |\n|---|---|---|---|---|\n");
    let (mut calls, mut chars) = (0, 0);
    for r in &rows {
        md.push_str(&format!(
            "| {} {} | {} | {} (~{}) | {} ms | {} |\n",
            r.n, r.name, r.calls, r.chars, r.tokens(), r.wall_ms, r.note.replace('|', "/")
        ));
        calls += r.calls;
        chars += r.chars;
    }
    md.push_str(&format!("| TOTAL | {calls} | {chars} (~{}) | {} ms |  |", chars / 4, total_wall.as_millis()));
    println!("{md}");

    let path = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("shell_scenarios.md");
    let _ = std::fs::write(&path, &md); // best-effort: orchestrator pastes into docs

    // Gates: the harness IS the test — regressions fail here.
    assert!(rows[2].calls <= 1, "scenario 3 must stay 1 call");
    assert!(rows[5].calls <= 3, "scenario 6 must stay ≤ 3 calls");
    assert!(rows[6].max_single <= 55 * 1024, "scenario 7 result must stay ≤ 55 KiB");
    // Scenario 8's byte-ratio gate already asserted inside s8 (needs the bodies).
    assert!(total_wall < Duration::from_secs(60), "suite must run in < 60 s, took {total_wall:?}");
}
