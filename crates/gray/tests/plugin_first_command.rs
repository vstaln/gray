//! Real REPL regression: a plugin command must work before the first model turn.
#[test]
fn plugin_command_initializes_lazy_agent_without_provider_call() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let echo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/echo/echo.sh");
    std::fs::write(
        dir.path().join("gray.yml"),
        format!(
            "plugins:\n  - builtin: tools-minimal\n  - sidecar: [sh, {}]\n",
            echo.display()
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gray"))
        .args([
            "--model",
            "test/local",
            "--base-url",
            "http://127.0.0.1:1/v1",
            "--context-window",
            "32000",
        ])
        .env("GRAY_HOME", dir.path().join("home"))
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/echo FIRST_PLUGIN_COMMAND\n/quit\n")
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
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines()
            .any(|line| line.trim() == "FIRST_PLUGIN_COMMAND"),
        "{text}"
    );
    assert!(!text.contains("unknown command"), "{text}");
}
