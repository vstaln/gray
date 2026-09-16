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
        assert_eq!(gray_core::paths::user_home().unwrap(), profile);
        eprintln!(
            "resolved profile={:?}, gray root={:?}, expected={expected:?}",
            gray_core::paths::user_home(),
            gray::setup::gray_home()
        );
        // Exercise the package reader's real plugins/lock.json layout. The
        // plugin host's separate plugins.json format is not this fixture.
        std::fs::create_dir_all(expected.join("plugins")).unwrap();
        std::fs::write(
            expected.join("plugins/lock.json"),
            r#"{"schema":1,"plugins":{"native-fixture":{"version":"1"}}}"#,
        )
        .unwrap();
        assert!(
            gray_pkg::ops::list()
                .unwrap()
                .contains_key("native-fixture")
        );
        let config = profile.join("profile.yml");
        std::fs::write(&config, "plugins:\n  - sidecar: ~/plugin.exe\n").unwrap();
        let entries = gray_plugin::profile::load_entries(config.to_str().unwrap()).unwrap();
        assert_eq!(
            entries,
            vec![gray_plugin::profile::PluginEntry::Sidecar(
                gray_plugin::profile::SidecarSpec(vec![
                    profile.join("plugin.exe").to_string_lossy().into_owned()
                ])
            )]
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
