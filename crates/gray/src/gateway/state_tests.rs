use super::*;

#[test]
fn write_read_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let rec = record(STATE_RUNNING, None, 42);
    write(home.path(), &rec).unwrap();
    assert_eq!(read(home.path()), Some(rec));
}

#[test]
fn corrupt_state_reads_as_none() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(path(home.path()), "not json").unwrap();
    assert!(read(home.path()).is_none());
}
