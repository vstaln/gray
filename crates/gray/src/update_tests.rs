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
