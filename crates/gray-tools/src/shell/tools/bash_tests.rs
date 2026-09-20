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
        b"a\r\n".repeat(20000),
        b"z".repeat(INLINE_BUDGET_BYTES + 4096),
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
        assert!(out.stdout.len() <= READ_CHUNK as usize);
        // The marker in the body already names the byte window, so the extra
        // hint only has to say where the next page starts — and only when
        // there is more than one page left.
        let next = result
            .content
            .lines()
            .find_map(|l| l.strip_prefix("Then skip="));
        if raw.len() > INLINE_BUDGET_BYTES + READ_CHUNK as usize {
            assert!(
                next.is_some(),
                "multi-page window must name the next offset"
            );
        } else {
            assert!(next.is_none(), "single page needs no next offset");
        }
        if raw[0] == b'z' {
            assert_eq!(
                out.stdout,
                vec![b'z'; (raw.len() - INLINE_BUDGET_BYTES).min(READ_CHUNK as usize)]
            );
        } else {
            // Run 35229845737 captured the recovered bytes: Git Bash's sed
            // pipes CRLF text to native readers through a text-mode MSYS
            // pipe, folding CRLF to LF (`a\r\n` -> `a\n`). Whether that
            // folding happens is a property of the local MSYS mount, not of
            // gray: the same command returned raw CRLF on windows-runtime
            // (run 35517869669). The disk log stays byte-verbatim either way
            // (asserted in
            // progress_is_line_safe_but_log_retains_carriage_returns), so
            // the invariant that matters here is that recovery starts at the
            // truncated marker byte and stays bounded — not which EOL the
            // local pipe hands back. Emit the bytes in hex on failure.
            assert!(
                out.stdout.starts_with(b"a\r\n") || out.stdout.starts_with(b"a\n"),
                "expected 610d0a (or EOL-folded 610a) recovery, got {:02x?} (command: {command})",
                out.stdout
            );
        }
    }
}

#[tokio::test]
async fn explicit_timeout_says_how_to_rerun() {
    // A killed command must say how to let it finish: the benchmark retro
    // showed agents reading a timeout kill as "the command failed".
    let session = sess("timeout-msg");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "sleep 5", "timeout": 1}))
        .await;
    // A kill is data, not a tool error (non-zero exits are data by design),
    // but the note must be the first thing the model reads.
    assert!(!r.is_error, "killed command stays a result: {}", r.content);
    assert!(
        r.content.starts_with("timed out after 1s"),
        "the timeout note leads the output: {}",
        r.content
    );
    assert!(
        r.content.contains("rerun without `timeout`"),
        "the kill must say how to let it finish: {}",
        r.content
    );
}

#[test]
fn bash_schema_promises_no_default_timeout() {
    // Regression guard for a real mismatch: the tool text promised a 120s
    // default while the code killed at 30s. Agents lost long suites to it.
    assert!(
        DEFAULT_TIMEOUT_SECS.is_none(),
        "commands run until they exit unless a timeout is passed"
    );
    let def = BashTool::default().def();
    let desc = def.parameters["properties"]["timeout"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(desc.contains("omitted = no limit"), "{desc}");
    assert!(
        def.description.contains("no default"),
        "tool description must not promise a default: {}",
        def.description
    );
}

#[tokio::test]
async fn missing_command_gets_an_actionable_hint() {
    let session = sess("not-found");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(
            &ctx,
            json!({"command": "gray-definitely-not-a-tool --help"}),
        )
        .await;
    assert!(
        r.content
            .contains("`gray-definitely-not-a-tool` is not installed here"),
        "hint names the tool: {}",
        r.content
    );
    assert!(
        r.content.contains("command -v gray-definitely-not-a-tool"),
        "hint says how to confirm: {}",
        r.content
    );
}

#[tokio::test]
async fn ordinary_failures_get_no_not_found_hint() {
    let session = sess("no-hint");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "grep -q nomatch /etc/hostname"}))
        .await;
    assert!(
        !r.content.contains("is not installed here"),
        "ordinary non-zero exit stays clean: {}",
        r.content
    );
}

#[test]
fn not_found_subject_reads_every_shell_wording() {
    for (line, want) in [
        ("bash: line 1: rg: command not found", "rg"),
        ("sh: 1: xxd: not found", "xxd"),
        ("zsh: command not found: goyacc", "goyacc"),
        ("-bash: ruff: command not found", "ruff"),
    ] {
        assert_eq!(not_found_subject(line).as_deref(), Some(want), "{line}");
    }
    assert!(missing_command_hint("exit 0\nall good").is_none());
    assert!(missing_command_hint("curl: (22) 404 not found").is_none());
}
