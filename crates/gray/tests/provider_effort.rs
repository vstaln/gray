//! Exercise the actual startup path, not just the already-working clamp helper.
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn fresh_session_normalizes_saved_deepseek_effort_before_display() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.json"),
        r#"{"model":"deepseek-v4.1-flash","base_url":"http://127.0.0.1:1/v1","thinking_effort":"xhigh"}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gray"))
        .current_dir(home.path())
        .env("GRAY_HOME", home.path())
        .env_remove("GRAY_THINKING_EFFORT")
        .args([
            "--model",
            "deepseek-v4.1-flash",
            "--base-url",
            "http://127.0.0.1:1/v1",
            "--api-key",
            "test",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/thinking\n/quit\n")
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("REPL did not exit");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("thinking effort: max — levels: off, low, medium, high, max"),
        "{stdout}"
    );
    assert!(!stdout.contains("thinking effort: xhigh"), "{stdout}");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["thinking_effort"], "max");
}
