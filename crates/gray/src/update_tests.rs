use super::*;

#[test]
fn semver_compare() {
    assert!(is_newer("0.2.0", "0.1.9"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.1.0", "0.2.0"));
}

#[test]
fn semver_prereleases_order_by_full_precedence() {
    assert!(is_newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
    assert!(!is_newer("1.0.0-alpha.1", "1.0.0-alpha.2"));
    assert!(is_newer("1.0.0-alpha.10", "1.0.0-alpha.9"));

    assert!(is_newer("1.0.0-alpha.beta", "1.0.0-alpha.1"));

    assert!(is_newer("1.0.0-alpha.1.1", "1.0.0-alpha.1"));

    assert!(is_newer("1.0.0", "1.0.0-beta.1"));
    assert!(!is_newer("1.0.0-beta.1", "1.0.0"));

    assert!(!is_newer("1.2.3+build.1", "1.2.3"));
    assert!(is_newer("1.2.4+build.1", "1.2.3+build.9"));
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
    assert!(update_check_due(Some(2_000_000), 1_000_000));
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
fn shadow_warning_names_the_shadowing_path_and_the_fix() {
    let w = shadow_warning(
        Path::new("/home/u/.local/bin/gray"),
        "0.1.1",
        Path::new("/home/u/.cargo/bin/gray"),
    )
    .expect("a differing gray first on PATH must be reported");
    assert!(w.contains("/home/u/.cargo/bin/gray"), "{w}");
    assert!(w.contains("0.1.1"), "{w}");
    assert!(w.contains("rm /home/u/.cargo/bin/gray"), "{w}");
}

#[test]
fn shadow_warning_stays_quiet_when_path_resolves_the_updated_build() {
    let installed = Path::new("/home/u/.local/bin/gray");
    assert!(shadow_warning(installed, "0.1.1", installed).is_none());
}

#[test]
fn shadow_warning_reports_any_differing_path_without_running_it() {
    let installed = Path::new("/home/u/.local/bin/gray");

    for other in ["/opt/gray", "/home/u/.cargo/bin/gray"] {
        let w = shadow_warning(installed, "0.1.1", Path::new(other))
            .expect("a differing path must be reported");
        assert!(w.contains(other), "{w}");
        assert!(w.contains("rm "), "{w}");
    }
}

#[test]
fn first_gray_in_prefers_the_earliest_path_entry() {
    let dir = tempfile::tempdir().unwrap();
    let early = dir.path().join("early");
    let late = dir.path().join("late");
    std::fs::create_dir_all(&early).unwrap();
    std::fs::create_dir_all(&late).unwrap();
    write_launcher(&late.join("gray"));

    assert_eq!(
        first_gray_in(&[early.clone(), late.clone()]),
        Some(late.join("gray"))
    );

    write_launcher(&early.join("gray"));
    assert_eq!(
        first_gray_in(&[early.clone(), late.clone()]),
        Some(early.join("gray")),
        "the first PATH entry wins, like a shell"
    );
    assert_eq!(first_gray_in(&[dir.path().join("nowhere")]), None);
}

fn write_launcher(path: &Path) {
    std::fs::write(path, b"stub").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
#[cfg(unix)]
fn first_gray_in_skips_a_gray_the_shell_could_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let real = dir.path().join("real");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&real).unwrap();

    std::fs::write(data.join("gray"), b"not a program").unwrap();
    write_launcher(&real.join("gray"));

    assert_eq!(
        first_gray_in(&[data.clone(), real.clone()]),
        Some(real.join("gray")),
        "only a runnable launcher counts"
    );
    assert_eq!(first_gray_in(std::slice::from_ref(&data)), None);
}

#[test]
#[cfg(windows)]
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
#[cfg(unix)]
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

static SHADOW_ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_fake_gray(path: &Path, version: &str) {
    std::fs::write(path, format!("#!/bin/sh\necho 'gray {version}'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

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
#[cfg(unix)]
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

            assert!(w.contains("0.1.1"), "{w}");
            assert!(!w.contains("0.1.0"), "{w}");
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
#[cfg(unix)]
fn shadow_guard_uses_the_default_local_bin_destination() {
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
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
#[cfg(windows)]
fn shadow_guard_uses_the_windows_default_destination() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("AppData").join("Local");
    with_update_env(
        &[
            ("GRAY_INSTALL_DIR", None),
            ("LOCALAPPDATA", Some(local.as_path())),
        ],
        || {
            assert_eq!(
                installer_dest(),
                Some(local.join("Programs").join("gray").join("bin")),
                "the installer default must match dist/install-native.ps1"
            );
        },
    );
}

#[test]
fn divergence_warning_names_the_other_path_and_the_relaunch() {
    let w = divergence_warning(
        Path::new("/home/u/.cargo/bin/gray"),
        "0.1.0",
        Path::new("/home/u/.local/bin/gray"),
    )
    .expect("a differing gray first on PATH must be reported");
    assert!(w.contains("/home/u/.local/bin/gray"), "{w}");
    assert!(w.contains("gray 0.1.0"), "{w}");
    assert!(w.contains("relaunch `gray`"), "{w}");
}

#[test]
fn divergence_warning_ignores_the_binary_already_running() {
    let current = Path::new("/home/u/.local/bin/gray");
    assert!(divergence_warning(current, "0.1.1", current).is_none());
}

#[test]
fn divergence_warning_reports_an_older_candidate_too() {
    let current = Path::new("/home/u/.cargo/bin/gray");

    let w = divergence_warning(current, "0.1.1", Path::new("/opt/gray"))
        .expect("a differing path must be reported");
    assert!(w.contains("/opt/gray"), "{w}");
}

#[test]
#[cfg(unix)]
fn divergence_guard_fires_when_a_different_gray_wins_path() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).unwrap();
    write_fake_gray(&first.join("gray"), "9.9.9");
    let path = std::env::join_paths([&first]).unwrap();

    with_update_env(&[("PATH", Some(path.as_os_str().as_ref()))], || {
        let w = path_divergence_warning().expect("a different gray first on PATH must warn");
        assert!(w.contains(&*first.join("gray").to_string_lossy()), "{w}");

        assert!(!w.contains("9.9.9"), "{w}");
        assert!(w.contains("relaunch `gray`"), "{w}");
    });
}

#[test]
#[cfg(unix)]
fn path_guards_never_execute_the_path_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let installed = dir.path().join("installed");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&installed).unwrap();
    let sentinel = dir.path().join("ran");
    std::fs::write(
        first.join("gray"),
        format!(
            "#!/bin/sh\ntouch {}\necho 'gray 9.9.9'\n",
            sentinel.display()
        ),
    )
    .unwrap();
    write_launcher(&first.join("gray"));
    write_fake_gray(&installed.join("gray"), "0.1.1");
    let path = std::env::join_paths([&first]).unwrap();

    with_update_env(
        &[
            ("PATH", Some(path.as_os_str().as_ref())),
            ("GRAY_INSTALL_DIR", Some(installed.as_path())),
        ],
        || {
            assert!(
                path_divergence_warning().is_some(),
                "a differing candidate must still be reported"
            );
            assert!(
                post_update_shadow_warning().is_some(),
                "a shadowing candidate must still be reported"
            );
            assert!(
                !sentinel.exists(),
                "the PATH candidate was executed; it must only ever be named"
            );
        },
    );
}

#[test]
#[cfg(unix)]
fn a_pin_mismatch_leaves_no_scratch_dir() {
    use std::fs;
    let scratch = tempfile::tempdir().unwrap();
    let cdn = scratch.path().join("cdn");
    fs::create_dir_all(cdn.join("dl")).unwrap();
    fs::write(cdn.join("install.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let temp_root = std::env::temp_dir();

    let err = run_pinned_installer(
        &"0".repeat(64),
        &format!("file://{}/dl", cdn.display()),
        false,
    )
    .expect_err("a wrong pin must refuse to run the script");
    assert!(format!("{err}").contains("checksum mismatch"), "{err}");

    let leftovers: Vec<_> = fs::read_dir(&temp_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with("gray-installer"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "installer scratch left behind: {leftovers:?}"
    );
}

#[test]
#[cfg(unix)]
fn cargo_shadow_is_removed_not_warned_about() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let fresh = home.join(".local/bin");
    let stale = home.join(".cargo/bin");
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    write_fake_gray(&fresh.join("gray"), "0.1.1");
    write_fake_gray(&stale.join("gray"), "0.1.0");

    let path = std::env::join_paths([&stale, &fresh]).unwrap();
    with_update_env(
        &[
            ("GRAY_INSTALL_DIR", None),
            ("HOME", Some(home.as_path())),
            ("PATH", Some(path.as_os_str().as_ref())),
        ],
        || {
            assert!(
                post_update_shadow_warning().is_none(),
                "the cargo-install shadow is deleted, leaving the updated build first on PATH"
            );
            assert!(!stale.join("gray").exists(), "the stray must be gone");
            assert!(fresh.join("gray").exists(), "the real install is untouched");
        },
    );
}
