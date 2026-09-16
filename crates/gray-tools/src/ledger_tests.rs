use super::*;

fn entry(full_view: bool) -> LedgerEntry {
    LedgerEntry {
        mtime: SystemTime::now(),
        size: 3,
        content_hash: FileLedger::hash_bytes(b"abc"),
        full_view,
        window: (1, None),
        first_line: 1,
        last_line: 1,
        dedup_armed: true,
        read_at: Instant::now(),
    }
}

#[test]
fn record_get_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, b"abc").unwrap();
    let ledger = FileLedger::new();
    assert!(ledger.get(&p).is_none());
    ledger.record_read(&p, entry(true));
    let got = ledger.get(&p).expect("recorded entry must be found");
    assert!(got.full_view);
    assert!(got.dedup_armed);
    assert_eq!(got.window, (1, None));
    assert_eq!(got.content_hash, FileLedger::hash_bytes(b"abc"));
}

#[test]
fn canonical_key_unifies_dot_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, b"abc").unwrap();
    let dotted = dir.path().join(".").join("a.rs");
    assert_ne!(
        p.as_os_str(),
        dotted.as_os_str(),
        "precondition: spellings differ lexically"
    );
    let ledger = FileLedger::new();
    ledger.record_read(&p, entry(false));
    assert!(
        ledger.get(&dotted).is_some(),
        "./a.rs and /abs/a.rs must map to one key"
    );
}

#[test]
fn full_view_flag_is_stored_verbatim() {
    let ledger = FileLedger::new();
    // The ledger stores the flag verbatim; the caller computes it. Under
    // the current contract only fully delivered reads set true — clamped
    // or cut reads set false.
    ledger.record_read(Path::new("full-small.js"), entry(true));
    ledger.record_read(Path::new("cut-big.log"), entry(false));
    assert!(ledger.get(Path::new("full-small.js")).unwrap().full_view);
    assert!(!ledger.get(Path::new("cut-big.log")).unwrap().full_view);
}

#[test]
fn hash_is_stable_and_skipped_past_64mib() {
    assert_eq!(
        FileLedger::hash_bytes(b"hello"),
        FileLedger::hash_bytes(b"hello")
    );
    assert_ne!(
        FileLedger::hash_bytes(b"hello"),
        FileLedger::hash_bytes(b"world")
    );
    let big = vec![0u8; (MAX_HASH_BYTES + 1) as usize];
    assert_eq!(FileLedger::hash_bytes(&big), None);
}

#[test]
fn mark_written_is_full_view_and_disarms_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("out.txt");
    std::fs::write(&p, b"hi\nthere\n").unwrap();
    let ledger = FileLedger::new();
    ledger.mark_written(&p, b"hi\nthere\n");
    let got = ledger.get(&p).expect("written entry must be found");
    assert!(got.full_view);
    assert!(!got.dedup_armed, "next read must return full content");
    assert_eq!(got.size, 9);
    assert_eq!(got.last_line, 2);
    // Missing file: falls back to the given bytes (no I/O to trust).
    ledger.mark_written(Path::new("no-such-file.txt"), b"ab");
    let missing = ledger.get(Path::new("no-such-file.txt")).unwrap();
    assert_eq!(missing.size, 2);
    assert!(missing.full_view);
}

#[test]
fn disarm_keeps_entries_but_clears_armed() {
    let ledger = FileLedger::new();
    ledger.record_read(Path::new("a"), entry(true));
    ledger.record_read(Path::new("b"), entry(true));
    ledger.disarm_all_dedup();
    for name in ["a", "b"] {
        let got = ledger.get(Path::new(name)).unwrap();
        assert!(!got.dedup_armed, "{name} must be disarmed");
        assert!(got.full_view, "{name} entry must survive for the guard");
    }
}

#[test]
fn clear_forgets_everything() {
    let ledger = FileLedger::new();
    ledger.record_read(Path::new("a"), entry(true));
    ledger.clear();
    assert!(ledger.get(Path::new("a")).is_none());
}
