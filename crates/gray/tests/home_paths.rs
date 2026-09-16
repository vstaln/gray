//! Subprocess isolation: never mutate HOME while sibling tests run threads.
use std::process::Command;

#[test]
fn native_home_without_shell_environment() {
    if std::env::var_os("GRAY_PATH_TEST_CHILD").is_some() {
        let profile = std::path::PathBuf::from(std::env::var_os("GRAY_PATH_TEST_PROFILE").unwrap());
        let expected = std::env::var_os("GRAY_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| profile.join(".gray"));
        assert_eq!(gray::setup::gray_home().unwrap(), expected);
        assert_eq!(gray::sys_prompt_path().unwrap(), expected.join("AGENTS.md"));
        assert_eq!(
            gray::session_store::default_root().unwrap(),
            expected.join("sessions")
        );
        return;
    }
    let dir = tempfile::Builder::new()
        .prefix("gray profile café ")
        .tempdir()
        .unwrap();
    for override_home in [false, true] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "native_home_without_shell_environment",
                "--nocapture",
            ])
            .env("GRAY_PATH_TEST_CHILD", "1")
            .env("GRAY_PATH_TEST_PROFILE", dir.path())
            .env("USERPROFILE", dir.path())
            .env_remove("GRAY_HOME");
        // A Windows installation launched from PowerShell has no Unix HOME.
        #[cfg(windows)]
        child.env_remove("HOME");
        #[cfg(not(windows))]
        child.env("HOME", dir.path());
        if override_home {
            child.env("GRAY_HOME", dir.path().join("custom root"));
        }
        assert!(
            child.status().unwrap().success(),
            "override={override_home}"
        );
    }
}
