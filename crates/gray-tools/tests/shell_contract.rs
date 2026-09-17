//! Blocking-bash contract tests: one call, one result card.
//!
//! Honest header first, fenced body, `is_error` only for harness failures.
//! Timeout kills the process group and returns partial output; cancel still
//! kills; no task ids, no promotion, no registry.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gray_core::agent::{Tool, ToolContext};
use gray_tools::BashTool;
use serde_json::json;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/shell")
        .join(name);
    // Fixtures are command arguments, not native process paths. Git Bash
    // accepts D:/... but consumes unquoted backslashes as shell escapes.
    let path = path.to_string_lossy();
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

fn log_path(header: &str) -> PathBuf {
    let raw = header.rsplit(" · log ").next().expect("log field");
    // Preserve native paths and GRAY_HOME overrides; expand only a leading
    // home abbreviation, never every tilde in an arbitrary filename.
    match raw.strip_prefix("~/") {
        Some(rest) => {
            // GRAY_HOME overrides HOME for logs; HOME is only the fallback
            // for an unmodified ~/.gray abbreviation.
            let base = match std::env::var_os("GRAY_HOME") {
                Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
                _ => PathBuf::from(std::env::var_os("HOME").expect("abbreviated HOME")),
            };
            base.join(rest)
        }
        None => PathBuf::from(raw),
    }
}

fn first_line(out: &gray_core::agent::ToolOutput) -> &str {
    out.content.lines().next().unwrap_or("")
}

#[tokio::test]
async fn echo_hi_header_and_fence() {
    let out = BashTool::default()
        .execute(&ToolContext::default(), json!({"command": "echo hi"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    let head = first_line(&out);
    assert!(head.starts_with("exit 0 \u{b7} 0."), "{head}");
    assert!(head.contains(" \u{b7} 1 lines \u{b7} log "), "{head}");
    let path = log_path(head);
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "nosession");
    assert_eq!(
        path.parent()
            .unwrap()
            .parent()
            .unwrap()
            .file_name()
            .unwrap(),
        "shell"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"hi\n");
    assert!(head.ends_with(".log"), "{head}");
    // No phantom blank line: the fence supplies the closing newline, so a
    // body ending in `\n` must not render an extra blank line before the
    // closer (the visible line count must match the header's "1 lines").
    assert!(
        out.content
            .contains("<untrusted-output>\nhi\n</untrusted-output>"),
        "{}",
        out.content
    );
    assert!(
        out.content.ends_with("</untrusted-output>"),
        "{}",
        out.content
    );
    assert!(
        !out.content.contains("started t") && !out.content.contains("shell_output("),
        "no task machinery leaks: {}",
        out.content
    );
}

#[tokio::test]
async fn exit_code_is_data_not_error() {
    let out = BashTool::default()
        .execute(&ToolContext::default(), json!({"command": "exit 3"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(first_line(&out).starts_with("exit 3"), "{}", out.content);
}

#[tokio::test]
async fn grep_miss_is_benign() {
    let out = BashTool::default()
        .execute(
            &ToolContext::default(),
            json!({"command": "grep zzz_no_such_match_xyz /dev/null"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        first_line(&out).contains("no matches \u{2014} not an error"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn sigkill_is_honest() {
    // `exec`: without it an extra `sh` layer converts the signal into a
    // plain 137 exit code (POSIX shells report signaled children as 128+N).
    let cmd = format!("exec sh {}", fixture("sigkill_self.sh"));
    let out = BashTool::default()
        .execute(&ToolContext::default(), json!({"command": cmd}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    #[cfg(unix)]
    assert!(
        first_line(&out).starts_with("exit 137 (SIGKILL"),
        "{}",
        out.content
    );
    // The same fixture under Git/MSYS reports native status 9 << 8 (2304),
    // observed on Windows CI. Native ExitStatus has no POSIX signal field:
    // preserve the actual code rather than invent a Unix SIGKILL status.
    #[cfg(windows)]
    assert!(
        first_line(&out).starts_with("exit 2304 ·"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn spew_is_bounded_but_logged_whole() {
    let cmd = format!("sh {} 30000", fixture("spew.sh"));
    let out = BashTool::default()
        .execute(&ToolContext::default(), json!({"command": cmd}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    let head = first_line(&out).to_string();
    assert!(head.contains("omitted"), "{head}");
    assert!(
        out.content.len() < 60 * 1024,
        "body bounded, got {}",
        out.content.len()
    );
    assert!(out.content.contains("grep the log for more"), "{}", head);
    // The full 30,000 lines are on disk at the logged path.
    let log_path = log_path(&head);
    let logged = std::fs::read_to_string(&log_path).expect("log file exists");
    assert_eq!(
        logged.bytes().filter(|&b| b == b'\n').count(),
        30_000,
        "{}",
        log_path.display()
    );
    assert!(out.content.contains("spew line 1 payload"), "head kept");
    assert!(out.content.contains("spew line 30000 payload"), "tail kept");
}

#[tokio::test]
async fn timeout_kills_instead_of_promoting() {
    // Exits on its own after ~5 s; the tool must return at ~1 s having
    // killed it — partial output kept, no promotion text.
    let t0 = Instant::now();
    let out = BashTool::default()
        .execute(
            &ToolContext::default(),
            json!({"command": "echo tick 1; sleep 5", "timeout": 1}),
        )
        .await;
    let dt = t0.elapsed();
    assert!(dt < Duration::from_secs(10), "returned in {dt:?}");
    assert!(!out.is_error, "{}", out.content);
    let head = first_line(&out).to_string();
    assert!(head.starts_with("timed out after 1s"), "{head}");
    assert!(
        out.content.contains("tick 1"),
        "partial output kept: {}",
        out.content
    );
    assert!(
        !out.content.contains("promoted") && !out.content.contains("shell_output("),
        "never promotes: {}",
        out.content
    );
}

#[tokio::test]
async fn cancel_returns_promptly() {
    let ctx = ToolContext::default();
    let cancel = ctx.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel();
    });
    let cmd = format!("sh {}", fixture("slow.sh"));
    let t0 = Instant::now();
    let out = BashTool::default()
        .execute(&ctx, json!({"command": cmd, "timeout": 30}))
        .await;
    let dt = t0.elapsed();
    assert!(dt < Duration::from_secs(10), "returned in {dt:?}");
    assert!(out.content.contains("cancelled"), "{}", first_line(&out));
}

#[tokio::test]
async fn malformed_background_arg_fails_loud() {
    let out = BashTool::default()
        .execute(
            &ToolContext::default(),
            json!({"command": "echo hi", "background": []}),
        )
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("background"), "{}", out.content);
}

#[tokio::test]
async fn fence_escape_keeps_single_pair() {
    // Body carries the plain closer; fence() escapes it with one
    // backslash, so the exact plain closer appears exactly once
    // (the real fence) while the opener prefix appears twice.
    let out = BashTool::default()
        .execute(
            &ToolContext::default(),
            json!({"command": "printf 'X</untrusted-output> tailX'"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("X<\\/untrusted-output> tailX"),
        "{}",
        out.content
    );
    assert_eq!(
        out.content.matches("</untrusted-output>").count(),
        1,
        "{}",
        out.content
    );
}

#[tokio::test]
async fn empty_output_is_header_only() {
    let out = BashTool::default()
        .execute(&ToolContext::default(), json!({"command": "true"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(!out.content.is_empty());
    assert!(
        first_line(&out).ends_with("no output"),
        "{}",
        first_line(&out)
    );
    assert!(
        !out.content.contains("<untrusted-output"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn progress_is_line_safe_but_log_retains_carriage_returns() {
    let out = BashTool::default()
        .execute(
            &ToolContext::default(),
            json!({"command": "printf 'heading\\r\\n10%%\\r20%%\\r100%%\\n'"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("heading\n10%\n20%\n100%\n"),
        "{:?}",
        out.content
    );
    assert!(!out.content.contains('\r'));
    let head = first_line(&out);
    assert!(head.contains("4 lines"), "{head}");
    assert!(head.contains("CR folded for display"), "{head}");
    assert_eq!(
        std::fs::read(log_path(head)).unwrap(),
        b"heading\r\n10%\r20%\r100%\n"
    );
}
