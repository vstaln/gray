#[cfg(unix)]
use super::*;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
#[test]
fn setsid_failure_is_an_error() {
    // The pre_exec closure maps -1 to Err so a failed setsid can never
    // record a fictitious pgid == pid.
    assert!(check_setsid(-1).is_err());
    assert!(check_setsid(1234).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_records_real_process_group() {
    let spawned = spawn("true", &PathBuf::from("/tmp"), None, None).expect("sh -c true must spawn");
    assert_eq!(spawned.pgid, spawned.pid as i32);
}

#[test]
fn prefix_splits_on_whitespace() {
    assert_eq!(
        split_prefix("docker exec -i dev sh -s").unwrap(),
        vec!["docker", "exec", "-i", "dev", "sh", "-s"]
    );
    assert!(split_prefix("   ").unwrap().is_empty());
}

#[test]
fn prefix_honours_quotes_so_a_box_name_may_contain_a_space() {
    assert_eq!(
        split_prefix(r#"ssh -p 2222 'my box' sh -s"#).unwrap(),
        vec!["ssh", "-p", "2222", "my box", "sh", "-s"]
    );
    assert_eq!(split_prefix(r#"sh -s"#).unwrap(), vec!["sh", "-s"]);
    assert_eq!(split_prefix(r#"echo "a b""#).unwrap(), vec!["echo", "a b"]);
}

#[test]
fn an_unterminated_quote_is_an_error_not_a_truncated_prefix() {
    // Truncating here would run the command somewhere the user did not name.
    assert!(split_prefix("ssh 'my box").is_err());
    assert!(split_prefix("ssh box\\").is_err());
}

#[test]
fn exported_values_are_posix_quoted() {
    assert_eq!(shell_quote("/tmp/gray report"), "'/tmp/gray report'");
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
}

/// The prefix path's whole point: the far side's shell runs the command, and
/// gray's environment still reaches it. `sh -s` stands in for
/// `docker exec -i dev sh -s` / `ssh box sh -s` — a second shell, reached
/// through a program boundary, reading the script from stdin.
#[cfg(unix)]
#[tokio::test]
async fn exec_prefix_runs_the_command_on_the_far_shell() {
    let spawned = spawn_with_prefix(
        "echo hello-from-prefix",
        &PathBuf::from("/tmp"),
        None,
        None,
        Some(vec!["sh".into(), "-s".into()]),
    )
    .expect("prefixed spawn must succeed");
    let out = spawned.child.wait_with_output().await.expect("child");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello-from-prefix"),
        "prefixed run produced {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exec_prefix_carries_gray_env_across_the_boundary() {
    // `docker exec` and `ssh` do not forward the client environment, so the
    // export preamble is the only thing keeping GRAY_CWD_REPORT alive.
    let report = std::env::temp_dir().join("gray-exec-env-test.txt");
    let spawned = spawn_with_prefix(
        r#"case "$GRAY_CWD_REPORT" in /*) echo report-ok;; *) echo report-missing;; esac"#,
        &PathBuf::from("/tmp"),
        Some("sess-1"),
        Some(&report),
        Some(vec!["sh".into(), "-s".into()]),
    )
    .expect("prefixed spawn must succeed");
    let out = spawned.child.wait_with_output().await.expect("child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("report-ok"), "prefixed env: {stdout:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_heredoc_survives_the_prefix_boundary_intact() {
    // The one thing argv-passing would mangle: a heredoc terminator is a bare
    // word, so any re-quoting of the command breaks it.
    let dir = std::env::temp_dir().join(format!("gray-exec-heredoc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("body.txt");
    let file = file.display();
    let script = format!("cat > {file} <<'EOF'\nline one\nEOF\nwc -l < {file}");
    let spawned = spawn_with_prefix(
        &script,
        &dir,
        None,
        None,
        Some(vec!["sh".into(), "-s".into()]),
    )
    .expect("prefixed spawn must succeed");
    let out = spawned.child.wait_with_output().await.expect("child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('1'), "heredoc via prefix: {stdout:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
