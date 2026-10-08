//! `ChildRunner` against `#!/bin/sh` fake-grays: the NDJSON contract is the
//! real one (`print.rs`), and the process bits (pipes, pgroup, kill) run for
//! real. Shell scripts make every test unix-only.
#![cfg(unix)]

use super::*;

/// An executable `#!/bin/sh` script in `dir` named `name`.
fn fake_gray(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn runner(bin: PathBuf, home: &std::path::Path) -> ChildRunner {
    ChildRunner {
        gray_bin: bin,
        home: home.to_path_buf(),
    }
}

fn req() -> TurnRequest {
    TurnRequest {
        key: "main".to_string(),
        session_id: None,
        prompt: "hi".to_string(),
        route: None,
        kind: Kind::User,
        cwd: std::env::temp_dir(),
        timeout: Duration::from_secs(30),
    }
}

#[tokio::test]
async fn result_row_gives_text_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-ok",
        r#"printf '%s\n' '{"type":"progress","phase":"generating","session_id":"s-1"}'
printf '%s\n' '{"type":"result","text":"all done","session_id":"s-1"}'
"#,
    );
    let out = runner(bin, dir.path()).run(req()).await;
    assert_eq!(out.error, None);
    assert_eq!(out.text, "all done");
    assert_eq!(out.session_id.as_deref(), Some("s-1"));
}

#[tokio::test]
async fn session_id_goes_through_as_flag() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-args",
        r#"printf '%s\n' "{\"type\":\"result\",\"text\":\"$*\",\"session_id\":\"s-2\"}"
"#,
    );
    let mut req = req();
    req.session_id = Some("sess-99".to_string());
    let out = runner(bin, dir.path()).run(req).await;
    assert_eq!(out.error, None);
    assert!(out.text.contains("--session sess-99"), "{}", out.text);
    assert!(out.text.contains("--max-requests 200"), "{}", out.text);
}

#[tokio::test]
async fn error_row_becomes_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-err",
        r#"printf '%s\n' '{"type":"error","code":"provider_dead","message":"upstream 500","session_id":"s-3"}'
"#,
    );
    let out = runner(bin, dir.path()).run(req()).await;
    let e = out.error.unwrap();
    assert!(e.contains("provider_dead"), "{e}");
    assert!(e.contains("upstream 500"), "{e}");
    assert_eq!(out.session_id.as_deref(), Some("s-3"));
}

#[tokio::test]
async fn nonzero_exit_reports_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-die",
        r#"echo "boom: no api key" >&2
exit 3
"#,
    );
    let out = runner(bin, dir.path()).run(req()).await;
    let e = out.error.unwrap();
    assert!(e.contains("status 3"), "{e}");
    assert!(e.contains("boom: no api key"), "{e}");
}

#[tokio::test]
async fn timeout_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(dir.path(), "gray-hang", "exec sleep 30");
    let mut req = req();
    req.timeout = Duration::from_secs(1);
    let start = std::time::Instant::now();
    let out = runner(bin, dir.path()).run(req).await;
    let elapsed = start.elapsed();
    let e = out.error.unwrap();
    assert!(e.contains("timed out"), "{e}");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

#[tokio::test]
async fn turn_env_reaches_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-env",
        r#"esc=$(printf '%s' "$GRAY_CRON_ORIGIN" | sed 's/["\\]/\\&/g')
printf '%s\n' "{\"type\":\"result\",\"text\":\"cron=$esc|kind=$GRAY_TURN_ORIGIN|wall=$GRAY_MAX_WALL_SECS|reason=$GRAY_SHOW_REASONING\",\"session_id\":\"s-5\"}"
"#,
    );
    let mut req = req();
    req.route = Some(Route {
        platform: "discord".to_string(),
        chat: "42".to_string(),
        thread: None,
        route: None,
    });
    let out = runner(bin, dir.path()).run(req).await;
    assert_eq!(out.error, None);
    assert!(out.text.contains(r#""platform":"discord""#), "{}", out.text);
    assert!(out.text.contains("kind=user"), "{}", out.text);
    assert!(out.text.contains("wall=30"), "{}", out.text);
    assert!(out.text.contains("reason=0"), "{}", out.text);
}

#[tokio::test]
async fn no_route_removes_cron_origin() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_gray(
        dir.path(),
        "gray-env2",
        r#"printf '%s\n' "{\"type\":\"result\",\"text\":\"cron=${GRAY_CRON_ORIGIN:-unset}\",\"session_id\":\"s-6\"}"
"#,
    );
    let out = runner(bin, dir.path()).run(req()).await;
    assert_eq!(out.error, None);
    assert_eq!(out.text, "cron=unset");
}

#[tokio::test]
async fn spawn_failure_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let out = runner(dir.path().join("missing"), dir.path())
        .run(req())
        .await;
    let e = out.error.unwrap();
    assert!(e.contains("spawn failed"), "{e}");
}
