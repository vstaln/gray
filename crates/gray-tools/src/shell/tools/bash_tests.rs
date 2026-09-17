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
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "echo hello"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let head = r.content.lines().next().unwrap_or("");
    assert!(head.starts_with("exit 0"), "{head}");
    assert!(r.content.contains("hello"), "{}", r.content);
    assert!(r.content.contains("log "), "{}", r.content);
    assert!(!r.content.contains("Read more:"));
    assert!(
        !r.content.contains("started t"),
        "no task ids anymore: {}",
        r.content
    );
}

#[tokio::test]
async fn malformed_background_arg_fails_loud() {
    let session = sess("bgone");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "echo hi", "background": []}))
        .await;
    assert!(r.is_error, "{}", r.content);
    assert!(r.content.contains("background"), "{}", r.content);
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_and_returns_partial_output() {
    // `echo out; sleep 30` with timeout 1: SIGTERM lands, partial
    // output survives, no promotion text.
    let session = sess("timeout");
    let ctx = ctx_for(&session);
    let t0 = Instant::now();
    let r = BashTool::default()
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
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "   "}))
        .await;
    assert!(r.is_error, "{}", r.content);
}

#[test]
fn truncated_log_has_executable_bounded_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log ' $(false) file");
    for raw in [
        b"a\r\n".repeat(4000),
        b"z".repeat(30000),
        b"z".repeat(INLINE_BUDGET_BYTES + 1),
    ] {
        std::fs::write(&path, &raw).unwrap();
        let summary = PumpSummary {
            total_bytes: raw.len() as u64,
            total_lines: raw.iter().filter(|&&b| b == b'\n').count(),
            head: raw[..raw.len().min(MEM_HEAD_BYTES)].to_vec(),
            tail: raw[raw.len().saturating_sub(MEM_TAIL_BYTES)..].to_vec(),
            has_cr: raw.contains(&b'\r'),
            log_write_failed: false,
        };
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .status()
            .unwrap();
        let result = finish_inline("probe", &path, status, &summary, Instant::now(), None);
        let command = result
            .content
            .lines()
            .find_map(|l| l.strip_prefix("Read more: "))
            .expect("copyable command");
        let out = std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}", out.stderr);
        assert!(!out.stdout.is_empty());
        assert!(out.stdout.len() <= 4096);
        if raw[0] == b'z' {
            assert_eq!(
                out.stdout,
                vec![b'z'; (raw.len() - INLINE_BUDGET_BYTES).min(4096)]
            );
        } else {
            // Run 35229845737 captured the recovered bytes: Git Bash's sed
            // pipes CRLF text to native readers through a text-mode MSYS
            // pipe, folding CRLF to LF (`a\r\n` -> `a\n`). The disk log
            // stays byte-verbatim (asserted in
            // progress_is_line_safe_but_log_retains_carriage_returns);
            // recovery output is byte-exact on Unix, EOL-folded on Windows.
            // Emit both sides in hex on failure for direct comparison.
            #[cfg(windows)]
            assert!(
                out.stdout.starts_with(b"a\n"),
                "expected 610a-prefixed folded recovery, got {:02x?} (command: {command})",
                out.stdout
            );
            #[cfg(not(windows))]
            assert!(
                out.stdout.starts_with(b"a\r\n"),
                "expected raw 610d0a recovery, got {:02x?} (command: {command})",
                out.stdout
            );
        }
    }
}
