use super::*;

#[test]
fn first_boot_is_silent_then_a_crash_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(boot(dir.path()), Previous::FirstBoot);
    // Never marked stopped: the run died.
    let answer = boot_json(dir.path(), 2);
    assert_eq!(answer["previous"], "crashed");
    assert!(
        answer["notice"]
            .as_str()
            .unwrap()
            .contains("2 turns were cut short")
    );
    assert_eq!(startup_notice(Previous::FirstBoot, 3), None);
}

#[test]
fn a_requested_restart_reads_as_restart_once() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path());
    request_restart(dir.path()).unwrap();
    let stopped = stop_json(dir.path());
    assert_eq!(stopped["restart"], true);
    assert_eq!(stopped["active_notice"], active_notice(true));
    assert_eq!(boot_json(dir.path(), 0)["previous"], "restart");
    assert!(!restart_requested(dir.path()));
    assert_eq!(stop_json(dir.path())["restart"], false);
    assert_eq!(boot(dir.path()), Previous::Clean { restart: false });
}

#[test]
fn a_plain_stop_clears_a_stale_restart_marker() {
    let dir = tempfile::tempdir().unwrap();
    request_restart(dir.path()).unwrap();
    clear_restart(dir.path());
    assert!(!mark_stopped(dir.path()));
}
