use super::*;

#[test]
fn stale_only_when_mtime_or_size_drift() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.txt");
    std::fs::write(&p, b"one\ntwo\n").unwrap();
    let meta = std::fs::metadata(&p).unwrap();
    let entry = LedgerEntry {
        mtime: meta.modified().unwrap(),
        size: meta.len(),
        content_hash: FileLedger::hash_bytes(b"one\ntwo\n"),
        full_view: true,
        window: (1, None),
        first_line: 1,
        last_line: 2,
        dedup_armed: true,
        read_at: std::time::Instant::now(),
    };
    assert!(!EditTool::is_stale(&entry, &meta));
    // Same size, bumped mtime: stale by mtime alone (the name's other
    // half). Explicit timestamp, no filesystem-granularity gamble.
    let later = meta.modified().unwrap() + std::time::Duration::from_secs(2);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let touched = std::fs::metadata(&p).unwrap();
    assert_eq!(touched.len(), meta.len());
    assert!(EditTool::is_stale(&entry, &touched));
    std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
    let grown = std::fs::metadata(&p).unwrap();
    assert!(EditTool::is_stale(&entry, &grown));
}
