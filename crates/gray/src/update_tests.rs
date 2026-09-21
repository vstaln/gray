use super::*;

#[test]
fn semver_compare() {
    assert!(is_newer("0.2.0", "0.1.9"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.1.0", "0.2.0"));
}

#[test]
fn bad_versions_never_newer() {
    assert!(!is_newer("garbage", "0.1.0"));
    assert!(!is_newer("0.1.0-beta.1", "0.1.0"));
    assert!(!is_newer(" 1.2.3 ", "1.2.3"));
    assert!(is_newer(" 1.2.3 ", "1.2.2"));
}

#[test]
fn auto_update_refuses_beta_channel() {
    assert!(auto_update_allowed("stable", Some("1")));
    assert!(!auto_update_allowed("beta", Some("1")));
    assert!(!auto_update_allowed("stable", Some("0")));
    assert!(!auto_update_allowed("stable", None));
}

#[test]
fn install_command_carries_channel() {
    assert!(install_command().contains("gray.alignment.id/install.sh"));
    if CHANNEL == "beta" {
        assert!(install_command().ends_with("beta'"));
    }
}

#[test]
fn check_due_logic() {
    assert!(update_check_due(None, 1_000_000));
    assert!(!update_check_due(Some(1_000_000), 1_000_000 + 3600));
    assert!(update_check_due(Some(1_000_000), 1_000_000 + 24 * 3600));
    assert!(update_check_due(Some(2_000_000), 1_000_000)); // clock skew never blocks
}

#[test]
fn update_lock_is_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update.lock");
    let guard = acquire_update_lock_at(&path).unwrap();
    let probe = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(
        probe.try_lock().is_err(),
        "second exclusive lock must fail while held"
    );
    drop(guard);
    assert!(probe.try_lock().is_ok());
    let _ = probe.unlock();
}

#[test]
fn beta_uses_build_identity_not_cargo_version() {
    assert!(update_available(
        "beta",
        "new-commit",
        "0.1.0",
        "old-commit"
    ));
    assert!(!update_available("beta", "same", "0.1.0", "same"));
    assert!(!update_available("beta", "", "0.1.0", "same"));
    assert!(is_newer("0.2.0-beta.1", "0.1.0"));
    assert!(is_newer("0.2.0", "0.2.0-beta.1"));
}

#[test]
fn update_lock_release_is_not_delayed_by_an_inherited_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update.lock");
    let guard = acquire_update_lock_at(&path).unwrap();
    // A concurrent fork can retain this descriptor until its exec/exit. Model
    // that lifetime deterministically rather than depending on spawn timing.
    let inherited = guard.0.try_clone().unwrap();
    let probe = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(probe.try_lock().is_err());
    drop(guard);
    assert!(
        probe.try_lock().is_ok(),
        "guard must explicitly release the shared lock"
    );
    probe.unlock().unwrap();
    drop(inherited);
}

#[test]
fn shadow_warning_names_the_stale_binary_and_its_fix() {
    let w = shadow_warning(
        Path::new("/home/u/.local/bin/gray"),
        "0.1.1",
        Path::new("/home/u/.cargo/bin/gray"),
        Some("0.1.0"),
    )
    .expect("an older gray first on PATH must be reported");
    assert!(w.contains("/home/u/.cargo/bin/gray"), "{w}");
    assert!(w.contains("gray 0.1.0"), "{w}");
    assert!(w.contains("0.1.1"), "{w}");
    assert!(w.contains("rm /home/u/.cargo/bin/gray"), "{w}");
}

#[test]
fn shadow_warning_stays_quiet_when_path_resolves_the_updated_build() {
    let installed = Path::new("/home/u/.local/bin/gray");
    assert!(shadow_warning(installed, "0.1.1", installed, Some("0.1.1")).is_none());
}

#[test]
fn shadow_warning_ignores_a_path_binary_that_is_not_older() {
    let installed = Path::new("/home/u/.local/bin/gray");
    // Equal or newer build elsewhere is a legitimate choice, not a shadow.
    assert!(shadow_warning(installed, "0.1.1", Path::new("/opt/gray"), Some("0.1.1")).is_none());
    assert!(shadow_warning(installed, "0.1.1", Path::new("/opt/gray"), Some("0.2.0")).is_none());
    // Unreadable version: cannot prove it is current, so say so.
    assert!(shadow_warning(installed, "0.1.1", Path::new("/opt/gray"), None).is_some());
}

#[test]
fn first_gray_in_prefers_the_earliest_path_entry() {
    let dir = tempfile::tempdir().unwrap();
    let early = dir.path().join("early");
    let late = dir.path().join("late");
    std::fs::create_dir_all(&early).unwrap();
    std::fs::create_dir_all(&late).unwrap();
    std::fs::write(late.join("gray"), b"stub").unwrap();

    assert_eq!(
        first_gray_in(&[early.clone(), late.clone()]),
        Some(late.join("gray"))
    );

    std::fs::write(early.join("gray"), b"stub").unwrap();
    assert_eq!(
        first_gray_in(&[early.clone(), late.clone()]),
        Some(early.join("gray")),
        "the first PATH entry wins, like a shell"
    );
    assert_eq!(first_gray_in(&[dir.path().join("nowhere")]), None);
}

#[test]
#[cfg(windows)] // the launcher is gray.exe there, not gray
fn first_gray_in_finds_gray_exe() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gray.exe"), b"stub").unwrap();

    assert_eq!(
        first_gray_in(&[dir.path().to_path_buf()]),
        Some(dir.path().join("gray.exe")),
    );
    assert_eq!(first_gray_in(&[dir.path().join("nowhere")]), None);
}

#[test]
#[cfg(unix)] // symlink farms are the Unix case; Windows needs privileges for this
fn same_binary_sees_symlinked_install_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let link = dir.path().join("link");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("gray"), b"stub").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert!(same_binary(&real.join("gray"), &link.join("gray")));
    assert!(same_binary(&real.join("gray"), &real.join("gray")));
    assert!(!same_binary(
        &real.join("gray"),
        &dir.path().join("other/gray")
    ));
}

/// Serializes the env-mutating shadow tests below (same pattern as the
/// compact switch tests).
static SHADOW_ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_fake_gray(path: &Path, version: &str) {
    std::fs::write(path, format!("#!/bin/sh\necho 'gray {version}'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Runs `body` with `GRAY_INSTALL_DIR`/`HOME`/`PATH` replaced, then restores.
fn with_update_env(vars: &[(&str, Option<&Path>)], body: impl FnOnce()) {
    let _serial = SHADOW_ENV_SERIAL.lock().unwrap();
    let saved: Vec<(String, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(k, _)| ((*k).to_string(), std::env::var_os(k)))
        .collect();
    for (k, v) in vars {
        match v {
            Some(p) => unsafe { std::env::set_var(k, p) },
            None => unsafe { std::env::remove_var(k) },
        }
    }
    body();
    for (k, v) in saved {
        match v {
            Some(v) => unsafe { std::env::set_var(&k, v) },
            None => unsafe { std::env::remove_var(&k) },
        }
    }
}

#[test]
fn shadow_guard_catches_a_stale_copy_that_wins_path() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh");
    let stale = dir.path().join("stale");
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    write_fake_gray(&fresh.join("gray"), "0.1.1");
    write_fake_gray(&stale.join("gray"), "0.1.0");

    let shadowed_path = std::env::join_paths([&stale, &fresh]).unwrap();
    let clean_path = std::env::join_paths([&fresh, &stale]).unwrap();

    with_update_env(
        &[
            ("GRAY_INSTALL_DIR", Some(fresh.as_path())),
            ("PATH", Some(shadowed_path.as_os_str().as_ref())),
        ],
        || {
            let w = post_update_shadow_warning().expect("stale gray first on PATH must warn");
            assert!(w.contains("0.1.0"), "{w}");
            assert!(w.contains("0.1.1"), "{w}");
            assert!(w.contains(&*stale.join("gray").to_string_lossy()), "{w}");
        },
    );

    with_update_env(
        &[
            ("GRAY_INSTALL_DIR", Some(fresh.as_path())),
            ("PATH", Some(clean_path.as_os_str().as_ref())),
        ],
        || {
            assert!(
                post_update_shadow_warning().is_none(),
                "resolving the freshly installed build is not a shadow"
            );
        },
    );
}

#[test]
fn shadow_guard_uses_the_default_local_bin_destination() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let stale = dir.path().join("stale");
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    write_fake_gray(&home.join(".local/bin/gray"), "0.1.1");
    write_fake_gray(&stale.join("gray"), "0.1.0");

    let path = std::env::join_paths([&stale, &home.join(".local/bin")]).unwrap();
    with_update_env(
        &[
            ("GRAY_INSTALL_DIR", None),
            ("HOME", Some(home.as_path())),
            ("PATH", Some(path.as_os_str().as_ref())),
        ],
        || {
            assert_eq!(
                installer_dest(),
                Some(home.join(".local/bin")),
                "the installer default must match dist/install.sh"
            );
            assert!(post_update_shadow_warning().is_some());
        },
    );
}

#[test]
fn divergence_warning_names_the_newer_build_path_resolves() {
    let w = divergence_warning(
        Path::new("/home/u/.cargo/bin/gray"),
        "0.1.0",
        Path::new("/home/u/.local/bin/gray"),
        "0.1.1",
    )
    .expect("a newer gray first on PATH must be reported");
    assert!(w.contains("/home/u/.local/bin/gray"), "{w}");
    assert!(w.contains("gray 0.1.1"), "{w}");
    assert!(w.contains("gray 0.1.0"), "{w}");
    assert!(w.contains("relaunch `gray`"), "{w}");
}

#[test]
fn divergence_warning_ignores_the_binary_already_running() {
    let current = Path::new("/home/u/.local/bin/gray");
    assert!(divergence_warning(current, "0.1.1", current, "0.1.1").is_none());
}

#[test]
fn divergence_warning_ignores_an_older_or_equal_path_build() {
    let current = Path::new("/home/u/.cargo/bin/gray");
    // Deliberately running an older build is the user's call, not a hazard.
    assert!(divergence_warning(current, "0.1.1", Path::new("/opt/gray"), "0.1.0").is_none());
    assert!(divergence_warning(current, "0.1.1", Path::new("/opt/gray"), "0.1.1").is_none());
}

#[test]
fn divergence_guard_fires_when_a_newer_gray_wins_path() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("stale");
    let fresh = dir.path().join("fresh");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    write_fake_gray(&fresh.join("gray"), "9.9.9");
    let path = std::env::join_paths([&fresh]).unwrap();

    with_update_env(&[("PATH", Some(path.as_os_str().as_ref()))], || {
        let w = path_divergence_warning().expect("newer gray first on PATH must warn");
        assert!(w.contains("9.9.9"), "{w}");
        assert!(w.contains(&*fresh.join("gray").to_string_lossy()), "{w}");
    });

    // Same layout, nothing newer on PATH: silent.
    write_fake_gray(&fresh.join("gray"), "0.0.1");
    with_update_env(&[("PATH", Some(path.as_os_str().as_ref()))], || {
        assert!(path_divergence_warning().is_none());
    });
}
