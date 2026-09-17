use super::*;

#[test]
fn claim_read_and_remove_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let rec = claim(home.path()).unwrap();
    assert_eq!(rec.pid, std::process::id());
    let seen = read(home.path()).unwrap();
    assert_eq!(seen.pid, rec.pid);
    assert!(alive(&seen));
    assert!(running(home.path()).is_some());
    remove_owned(home.path(), rec.pid);
    assert!(read(home.path()).is_none());
}

#[test]
fn live_record_refuses_a_second_claim() {
    let home = tempfile::tempdir().unwrap();
    claim(home.path()).unwrap(); // ours, and it is alive
    let err = claim(home.path()).unwrap_err().to_string();
    assert!(err.contains("already running"), "{err}");
}

#[cfg(target_os = "linux")]
#[test]
fn recycled_pid_reads_as_stale_and_is_replaced() {
    let home = tempfile::tempdir().unwrap();
    let mut rec = claim(home.path()).unwrap();
    // Same pid, different start time: the pid was recycled since.
    rec.start_time = Some(rec.start_time.unwrap_or(1) + 1_000_000);
    std::fs::write(
        record_path(home.path()),
        serde_json::to_string(&rec).unwrap(),
    )
    .unwrap();
    assert!(running(home.path()).is_none());
    let fresh = claim(home.path()).unwrap();
    assert_eq!(fresh.pid, std::process::id());
    assert!(alive(&read(home.path()).unwrap()));
}

#[test]
fn corrupt_record_is_replaced_on_claim() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(record_path(home.path()), "{not json").unwrap();
    assert!(read(home.path()).is_none());
    assert!(claim(home.path()).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn proc_start_time_matches_our_own_process() {
    assert!(proc_start_time(std::process::id()).is_some());
}

#[test]
fn other_process_is_live_and_blocks_claim() {
    #[cfg(unix)]
    let mut child = std::process::Command::new("sleep")
        .arg("20")
        .spawn()
        .unwrap();
    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "ping -n 20 127.0.0.1 >NUL"])
        .spawn()
        .unwrap();
    let pid = child.id();
    let home = tempfile::tempdir().unwrap();
    let result = std::panic::catch_unwind(|| {
        assert!(pid_alive(pid));
        let mut record = claim(home.path()).unwrap();
        record.pid = pid;
        record.start_time = proc_start_time(pid);
        std::fs::write(
            record_path(home.path()),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(running(home.path()).is_some());
        assert!(claim(home.path()).is_err());
    });
    let _ = child.kill();
    let _ = child.wait();
    result.unwrap();
}
