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
