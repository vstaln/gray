//! Preview limitations are explicit errors, never a silent WSL install/service.
#![cfg(windows)]
use std::time::Duration;

#[tokio::test]
async fn unsupported_operations_fail_without_shell_or_service_side_effects() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["update"],
        vec!["gateway", "status"],
        vec!["gateway", "install"],
        vec!["cron", "tick"],
        vec!["cron", "serve"],
        vec!["cron", "run", "missing"],
    ] {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_gray"))
                .args(&args)
                .env("GRAY_HOME", home.path())
                .env("GRAY_NO_UPDATE_CHECK", "1")
                .env("PATH", "")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("unsupported operation must return promptly")
        .unwrap();
        assert!(!output.status.success(), "{args:?} pretended to succeed");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("native Windows"), "{args:?}: {error}");
    }
    assert!(!home.path().join("gateway.pid").exists());
    assert!(
        !home.path().join("cron").exists(),
        "execution rejected before claiming jobs"
    );
}
