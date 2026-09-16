use super::*;
use std::time::Instant;

fn record(
    ledger: &FileLedger,
    path: &Path,
    full_view: bool,
    window: (i64, Option<u64>),
    armed: bool,
) {
    let meta = std::fs::metadata(path).unwrap();
    let bytes = std::fs::read(path).unwrap();
    ledger.record_read(
        path,
        LedgerEntry {
            mtime: meta.modified().unwrap(),
            size: meta.len(),
            content_hash: FileLedger::hash_bytes(&bytes),
            full_view,
            window,
            first_line: 1,
            last_line: 2,
            dedup_armed: armed,
            read_at: Instant::now(),
        },
    );
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.txt");
    std::fs::write(&p, b"one\ntwo\n").unwrap();
    (dir, p)
}

#[test]
fn hit_stubs_once_then_misses_until_rearmed() {
    let (_dir, p) = fixture();
    let ledger = FileLedger::new();
    record(&ledger, &p, true, (1, None), true);
    let hit = check(&ledger, &p, "a.txt", 1, None, true).expect("first repeat must stub");
    assert!(
        hit.content.contains("unchanged since your previous read"),
        "{}",
        hit.content
    );
    assert!(!hit.is_error);
    assert!(!ledger.get(&p).unwrap().dedup_armed, "hit consumes the arm");
    assert!(
        check(&ledger, &p, "a.txt", 1, None, true).is_none(),
        "consumed arm misses"
    );
    // A normal read re-arms (what read/mod.rs does on the miss path)…
    record(&ledger, &p, true, (1, None), true);
    assert!(
        check(&ledger, &p, "a.txt", 1, None, true).is_some(),
        "re-armed hit stubs again"
    );
}

#[test]
fn partial_offset_one_read_is_never_stubbed() {
    let (_dir, p) = fixture();
    let ledger = FileLedger::new();
    // lockfile shape: lines 1-2000 shown, cut before the end.
    record(&ledger, &p, false, (1, None), true);
    assert!(check(&ledger, &p, "a.txt", 1, None, true).is_none());
    assert!(
        ledger.get(&p).unwrap().dedup_armed,
        "miss leaves the arm alone"
    );
}

#[test]
fn window_mismatch_and_kill_switch_miss() {
    let (_dir, p) = fixture();
    let ledger = FileLedger::new();
    record(&ledger, &p, true, (1, None), true);
    assert!(check(&ledger, &p, "a.txt", 1, Some(100), true).is_none());
    assert!(check(&ledger, &p, "a.txt", 2, None, true).is_none());
    assert!(check(&ledger, &p, "a.txt", 1, None, false).is_none());
}

#[test]
fn changed_mtime_or_size_misses() {
    let (_dir, p) = fixture();
    let ledger = FileLedger::new();
    // Same size on disk, forged-old mtime in the entry: stale.
    let meta = std::fs::metadata(&p).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    ledger.record_read(
        &p,
        LedgerEntry {
            mtime: std::time::SystemTime::UNIX_EPOCH,
            size: meta.len(),
            content_hash: FileLedger::hash_bytes(&bytes),
            full_view: true,
            window: (1, None),
            first_line: 1,
            last_line: 2,
            dedup_armed: true,
            read_at: Instant::now(),
        },
    );
    assert!(check(&ledger, &p, "a.txt", 1, None, true).is_none());
    // Bigger file, fresh entry, then grow again: stale by size.
    std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
    record(&ledger, &p, true, (1, None), true);
    std::fs::write(&p, b"one\ntwo\nthree\nfour\n").unwrap();
    assert!(check(&ledger, &p, "a.txt", 1, None, true).is_none());
}
