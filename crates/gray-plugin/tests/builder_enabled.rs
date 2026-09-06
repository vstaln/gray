use std::collections::BTreeMap;
use std::sync::Arc;

use gray_plugin::Plugin;
use gray_plugin::builder::{active_plugins, default_plugins, take_builder_warnings};
use gray_plugin::lock::{LockEntry, LockFile, lock_path, project_lock_path};

// Serializes the process-global mutation below (GRAY_HOME + cwd) within
// this test binary; every other suite runs in its own process.
static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_entry(argv: Vec<String>, enabled: bool) -> LockEntry {
    LockEntry {
        ecosystem: "test".to_string(),
        version: "1.0.0".to_string(),
        hash: "abc123".to_string(),
        source: "test-source".to_string(),
        argv,
        adapter_version: "1".to_string(),
        installed_at: "2026-09-05T00:00:00Z".to_string(),
        scope: "test".to_string(),
        enabled,
    }
}

fn manifest_names(plugins: &[Arc<dyn Plugin>]) -> Vec<String> {
    plugins.iter().map(|p| p.manifest().name).collect()
}

#[tokio::test]
async fn disabled_profile_sidecars_warn_and_skip_before_spawn() {
    let _guard = GUARD.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let prev_home = std::env::var("GRAY_HOME").ok();
    let prev_cwd = std::env::current_dir().unwrap();
    // SAFETY: serialized by GUARD; all other suites are separate processes.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_current_dir(work.path()).unwrap();
    }

    // Absolute argv so the profile entry and the lock entry match exactly
    // regardless of cwd (the filter compares argv lists verbatim).
    let echo = format!("{}/testdata/echo_plugin.sh", env!("CARGO_MANIFEST_DIR"));
    let dead = "definitely-not-a-real-binary-xyz".to_string();
    let profile = work.path().join("gray.yml");
    std::fs::write(
        &profile,
        format!("plugins:\n  - sidecar: [{dead}]\n  - sidecar: [{echo}]\n"),
    )
    .unwrap();
    let profile_str = profile.to_string_lossy().into_owned();
    let save = |echo_on: bool, dead_on: bool| {
        LockFile {
            schema: 1,
            plugins: BTreeMap::from([
                (
                    "dead-plugin".to_string(),
                    lock_entry(vec![dead.clone()], dead_on),
                ),
                (
                    "echo-fixture".to_string(),
                    lock_entry(vec![echo.clone()], echo_on),
                ),
            ]),
        }
        .save(&lock_path(home.path()))
        .unwrap();
    };

    // Both disabled: skipped before spawning. abort=true would Err on any
    // spawn attempt, so Ok proves the dead binary was never spawned.
    save(false, false);
    let (plugins, fallback) = active_plugins(default_plugins(), profile_str.as_str(), None, true)
        .await
        .unwrap();
    let warnings = take_builder_warnings();
    assert!(fallback);
    assert!(manifest_names(&plugins).contains(&"tools-basic".to_string()));
    assert!(
        !manifest_names(&plugins).contains(&"echo".to_string()),
        "{:?}",
        manifest_names(&plugins)
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("disabled in plugin lock")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().all(|w| !w.contains("failed to spawn")),
        "nothing was spawned: {warnings:?}"
    );

    // Re-enable echo: present; dead entry still warns + skips.
    save(true, false);
    let (plugins, fallback) = active_plugins(default_plugins(), profile_str.as_str(), None, true)
        .await
        .unwrap();
    let warnings = take_builder_warnings();
    assert!(!fallback);
    assert!(manifest_names(&plugins).contains(&"echo".to_string()));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("sidecar[0]"), "{warnings:?}");
    assert!(
        warnings[0].contains("disabled in plugin lock"),
        "{warnings:?}"
    );

    // Re-enable dead too (no abort): the spawn is re-armed, fails, and the
    // daemon-style path warns + skips while echo stays present.
    save(true, true);
    let (plugins, _) = active_plugins(default_plugins(), profile_str.as_str(), None, false)
        .await
        .unwrap();
    let warnings = take_builder_warnings();
    assert!(manifest_names(&plugins).contains(&"echo".to_string()));
    assert!(
        warnings.iter().any(|w| w.contains("failed to spawn")),
        "{warnings:?}"
    );

    // SAFETY: same serialization note as above.
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("GRAY_HOME", v),
            None => std::env::remove_var("GRAY_HOME"),
        }
        std::env::set_current_dir(prev_cwd).unwrap();
    }
}

/// Real-install shape (empty argv + executable dir under `$GRAY_HOME/plugins`)
/// activates through the builder: enabled shows in the manifest, disable
/// removes it, re-enable restores it — including the project overlay.
#[tokio::test]
async fn lock_install_dir_activates_and_respects_enabled_flag() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = GUARD.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let prev_home = std::env::var("GRAY_HOME").ok();
    let prev_cwd = std::env::current_dir().unwrap();
    // SAFETY: serialized by GUARD; all other suites are separate processes.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_current_dir(work.path()).unwrap();
    }

    // Fake install: executable dir like a real `install` unpack.
    let fixture = std::fs::read(format!(
        "{}/testdata/echo_plugin.sh",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let dir = home.path().join("plugins").join("demo-echo");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("plugin.sh");
    std::fs::write(&script, &fixture).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Missing profile: lock installs still activate (no gray.yml needed).
    let missing_profile = work
        .path()
        .join("no-such-gray.yml")
        .to_string_lossy()
        .into_owned();
    let save_user = |enabled: bool| {
        LockFile {
            schema: 1,
            plugins: BTreeMap::from([("demo-echo".to_string(), lock_entry(vec![], enabled))]),
        }
        .save(&lock_path(home.path()))
        .unwrap();
    };

    save_user(true);
    let (plugins, fallback) =
        active_plugins(default_plugins(), missing_profile.as_str(), None, false)
            .await
            .unwrap();
    assert!(!fallback, "{:?}", manifest_names(&plugins));
    assert!(
        manifest_names(&plugins).contains(&"echo".to_string()),
        "{:?}",
        manifest_names(&plugins)
    );
    let _ = take_builder_warnings();

    // Disable → absent (back to builtin fallback).
    save_user(false);
    let (plugins, fallback) =
        active_plugins(default_plugins(), missing_profile.as_str(), None, false)
            .await
            .unwrap();
    assert!(fallback, "{:?}", manifest_names(&plugins));
    assert!(
        !manifest_names(&plugins).contains(&"echo".to_string()),
        "{:?}",
        manifest_names(&plugins)
    );
    let _ = take_builder_warnings();

    // Re-enable → present again.
    save_user(true);
    let (plugins, _) = active_plugins(default_plugins(), missing_profile.as_str(), None, false)
        .await
        .unwrap();
    assert!(
        manifest_names(&plugins).contains(&"echo".to_string()),
        "{:?}",
        manifest_names(&plugins)
    );
    let _ = take_builder_warnings();

    // Project overlay disables without touching the user lock.
    LockFile {
        schema: 1,
        plugins: BTreeMap::from([("demo-echo".to_string(), lock_entry(vec![], false))]),
    }
    .save(&project_lock_path(work.path()))
    .unwrap();
    let (plugins, _) = active_plugins(default_plugins(), missing_profile.as_str(), None, false)
        .await
        .unwrap();
    assert!(
        !manifest_names(&plugins).contains(&"echo".to_string()),
        "project overlay must win: {:?}",
        manifest_names(&plugins)
    );
    let _ = take_builder_warnings();
    std::fs::remove_file(project_lock_path(work.path())).unwrap();

    // Profile sidecar pointing at the install dir correlates by resolved
    // path (not argv contents): disabled lock skips it before spawning.
    save_user(false);
    let profile = work.path().join("gray.yml");
    std::fs::write(
        &profile,
        format!("plugins:\n  - sidecar: [{}]\n", dir.display()),
    )
    .unwrap();
    let profile_str = profile.to_string_lossy().into_owned();
    let (plugins, fallback) = active_plugins(default_plugins(), profile_str.as_str(), None, true)
        .await
        .unwrap();
    assert!(fallback, "{:?}", manifest_names(&plugins));
    let warnings = take_builder_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("disabled in plugin lock")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().all(|w| !w.contains("failed to spawn")),
        "disabled install must never spawn: {warnings:?}"
    );

    // SAFETY: same serialization note as above.
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("GRAY_HOME", v),
            None => std::env::remove_var("GRAY_HOME"),
        }
        std::env::set_current_dir(prev_cwd).unwrap();
    }
}
