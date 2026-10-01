use super::*;
#[test]
fn oversized_log_rotates_and_caps() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gray.log");
    std::fs::write(&log, vec![b'x'; (LOG_MAX_BYTES + 1) as usize]).unwrap();
    std::fs::write(dir.path().join("gray.log.1"), b"old1").unwrap();
    std::fs::write(dir.path().join("gray.log.2"), b"old2").unwrap();
    rotate_if_needed(&log);
    assert!(std::fs::metadata(&log).unwrap().len() < LOG_MAX_BYTES);
    assert!(!dir.path().join("gray.log.3").exists(), "must cap at .2");
}
#[test]
fn small_log_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gray.log");
    std::fs::write(&log, b"tiny").unwrap();
    rotate_if_needed(&log);
    assert_eq!(std::fs::read(&log).unwrap(), b"tiny");
}

/// Unix-only: drift detection compares (dev, ino), and the non-Unix
/// implementation is deliberately a `false` stub (logging must not crash
/// boot on missing file-id semantics).
#[cfg(unix)]
#[test]
fn handle_drifted_detects_a_log_rotated_away() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gray.log");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "one").unwrap();
    assert!(
        !super::handle_drifted(&path, &f),
        "a handle on its own path is not drifted"
    );
    // Another process rotates the log: our handle now writes into the
    // renamed-away inode until the next log line re-anchors it.
    std::fs::rename(&path, dir.path().join("gray.log.1")).unwrap();
    std::fs::File::create(&path).unwrap();
    assert!(
        super::handle_drifted(&path, &f),
        "a handle on a rotated-away inode must read as drifted"
    );
}

#[test]
fn rotation_takes_the_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gray.log");
    std::fs::write(&path, vec![b'x'; super::LOG_MAX_BYTES as usize + 1]).unwrap();
    super::rotate_if_needed(&path);
    assert!(path.with_extension("log.1").exists());
    assert!(path.exists(), "a fresh file follows the rotate");
    assert!(path.with_extension("log.lock").exists());
    // Oversize again: .1 shifts to .2 rather than being lost.
    std::fs::write(&path, vec![b'y'; super::LOG_MAX_BYTES as usize + 1]).unwrap();
    super::rotate_if_needed(&path);
    assert!(path.with_extension("log.2").exists());
}
