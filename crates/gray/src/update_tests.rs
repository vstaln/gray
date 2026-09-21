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
    // Same triple: identifiers order numerically, not lexically.
    assert!(is_newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
    assert!(!is_newer("1.0.0-alpha.1", "1.0.0-alpha.2"));
    assert!(is_newer("1.0.0-alpha.10", "1.0.0-alpha.9"));
    // Numeric identifiers sort below alphanumeric ones.
    assert!(is_newer("1.0.0-alpha.beta", "1.0.0-alpha.1"));
    // Fewer fields sort lower.
    assert!(is_newer("1.0.0-alpha.1.1", "1.0.0-alpha.1"));
    // A release outranks every one of its prereleases.
    assert!(is_newer("1.0.0", "1.0.0-beta.1"));
    assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
    // Build metadata is ignored entirely.
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
    // The candidate is never executed, so its version is unknown and every
    // differing path is reported rather than silently cleared.
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

/// A runnable stub, which on Unix means the executable bit.
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
    // A plain data file named `gray` cannot shadow anything.
    std::fs::write(data.join("gray"), b"not a program").unwrap();
    write_launcher(&real.join("gray"));

    assert_eq!(
        first_gray_in(&[data.clone(), real.clone()]),
        Some(real.join("gray")),
        "only a runnable launcher counts"
    );
    assert_eq!(first_gray_in(&[data.clone()]), None);
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
#[cfg(unix)] // spawns `#!/bin/sh` fakes; Windows cannot execute them
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
            // The installed build's version is read from the installer's own
            // file; the candidate's never is, because it is never executed.
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
#[cfg(unix)] // asserts dist/install.sh's default; spawns `#!/bin/sh` fakes
fn shadow_guard_uses_the_default_local_bin_destination() {
    // A root process installs to /usr/local/bin and never consults HOME, so
    // the default this test pins exists only on a non-root runner.
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
#[cfg(windows)] // asserts dist/install-native.ps1's default; spawn-free
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
    // The candidate is never executed, so "older" cannot be established and
    // the differing path is reported either way.
    let w = divergence_warning(current, "0.1.1", Path::new("/opt/gray"))
        .expect("a differing path must be reported");
    assert!(w.contains("/opt/gray"), "{w}");
}

#[test]
#[cfg(unix)] // spawns `#!/bin/sh` fakes; Windows cannot execute them
fn divergence_guard_fires_when_a_different_gray_wins_path() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).unwrap();
    write_fake_gray(&first.join("gray"), "9.9.9");
    let path = std::env::join_paths([&first]).unwrap();

    with_update_env(&[("PATH", Some(path.as_os_str().as_ref()))], || {
        let w = path_divergence_warning().expect("a different gray first on PATH must warn");
        assert!(w.contains(&*first.join("gray").to_string_lossy()), "{w}");
        // Path-only: the candidate's version is never read, so it cannot appear.
        assert!(!w.contains("9.9.9"), "{w}");
        assert!(w.contains("relaunch `gray`"), "{w}");
    });
}

/// The security contract: a PATH-resolved candidate is untrusted search-path
/// output. Both guards must report it by path and never run it (CWE-426).
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
