use super::*;

fn entry_for(path: &std::path::Path, full_view: bool, content_hash: Option<u64>) -> LedgerEntry {
    let meta = std::fs::metadata(path).unwrap();
    LedgerEntry {
        mtime: meta.modified().unwrap(),
        size: meta.len(),
        content_hash,
        full_view,
        window: (1, None),
        first_line: 1,
        last_line: 2,
        dedup_armed: true,
        read_at: std::time::Instant::now(),
    }
}

fn fixture(content: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.txt");
    std::fs::write(&p, content).unwrap();
    let old = String::from_utf8(content.to_vec()).unwrap();
    (dir, p, old)
}

#[test]
fn new_file_is_always_allowed() {
    assert!(WriteTool::decide(None, None, b"", false, "new.txt").is_none());
}

#[test]
fn unread_existing_file_is_refused_naming_read_and_force() {
    let (_dir, p, old) = fixture(b"one\ntwo\n");
    let meta = std::fs::metadata(&p).unwrap();
    let msg = WriteTool::decide(None, Some(&meta), old.as_bytes(), false, "f.txt")
        .expect("unread write must be refused");
    assert!(msg.contains("has not been read"), "{msg}");
    assert!(msg.contains("read f.txt"), "{msg}");
    assert!(msg.contains("force=true"), "{msg}");
}

#[test]
fn force_bypasses_unread_and_partial_but_not_stale() {
    let (_dir, p, old) = fixture(b"one\ntwo\n");
    let meta = std::fs::metadata(&p).unwrap();
    assert!(WriteTool::decide(None, Some(&meta), old.as_bytes(), true, "f.txt").is_none());
    let partial = entry_for(&p, false, None);
    assert!(
        WriteTool::decide(Some(&partial), Some(&meta), old.as_bytes(), true, "f.txt").is_none()
    );
    // Staleness still refuses even with force.
    std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
    let grown = std::fs::metadata(&p).unwrap();
    let current = std::fs::read(&p).unwrap();
    assert!(WriteTool::decide(Some(&partial), Some(&grown), &current, true, "f.txt").is_some());
}

#[test]
fn full_view_writes_are_allowed() {
    let (_dir, p, old) = fixture(b"one\ntwo\n");
    let meta = std::fs::metadata(&p).unwrap();
    let full = entry_for(&p, true, None);
    assert!(WriteTool::decide(Some(&full), Some(&meta), old.as_bytes(), false, "f.txt").is_none());
}

#[test]
fn partial_with_matching_hash_is_still_refused_hash_is_freshness_only() {
    // Old lossy rule allowed a partial view when the hash matched disk.
    // New contract: hash gates freshness only; delivered coverage gates
    // overwrite, so a partial view is refused even with a matching hash.
    let (_dir, p, old) = fixture(b"one\ntwo\n");
    let meta = std::fs::metadata(&p).unwrap();
    let partial = entry_for(&p, false, FileLedger::hash_bytes(old.as_bytes()));
    let msg = WriteTool::decide(Some(&partial), Some(&meta), old.as_bytes(), false, "f.txt")
        .expect("partial write must be refused even with matching hash");
    assert!(msg.contains("only part of"), "{msg}");
}

#[test]
fn hash_mismatch_with_same_mtime_shape_is_stale() {
    // Same size, forged-old mtime in entry is already stale via mtime;
    // here the bytes differ while mtime/size match (entry forged to match
    // disk meta): hash mismatch must still refuse as changed.
    let (_dir, p, _old) = fixture(b"one\ntwo\n");
    let meta = std::fs::metadata(&p).unwrap();
    let entry = LedgerEntry {
        mtime: meta.modified().unwrap(),
        size: meta.len(),
        content_hash: FileLedger::hash_bytes(b"different bytes same len!!"),
        full_view: true,
        window: (1, None),
        first_line: 1,
        last_line: 2,
        dedup_armed: true,
        read_at: std::time::Instant::now(),
    };
    let current = std::fs::read(&p).unwrap();
    // Sanity: same length so only the hash can catch it.
    assert_eq!(current.len() as u64, meta.len());
    let msg = WriteTool::decide(Some(&entry), Some(&meta), &current, false, "f.txt")
        .expect("hash mismatch must be stale");
    assert!(msg.contains("changed on disk"), "{msg}");
}

#[test]
fn changed_on_disk_is_refused() {
    let (_dir, p, _old) = fixture(b"one\ntwo\n");
    let stale = entry_for(&p, true, None);
    std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
    let meta = std::fs::metadata(&p).unwrap();
    let current = std::fs::read(&p).unwrap();
    let msg = WriteTool::decide(Some(&stale), Some(&meta), &current, false, "f.txt")
        .expect("stale write must be refused");
    assert!(msg.contains("changed on disk"), "{msg}");
}

#[test]
fn partial_view_is_refused_with_resume_offset_not_unread_wording() {
    let (_dir, p, old) = fixture(b"l1\nl2\nl3\nl4\nl5\n");
    let meta = std::fs::metadata(&p).unwrap();
    let mut partial = entry_for(&p, false, None);
    partial.last_line = 2;
    let msg = WriteTool::decide(Some(&partial), Some(&meta), old.as_bytes(), false, "f.txt")
        .expect("partial write must be refused");
    assert!(msg.contains("only part of f.txt"), "{msg}");
    assert!(msg.contains("lines 1-2 of 5"), "{msg}");
    assert!(msg.contains("offset=3"), "{msg}");
    assert!(!msg.contains("has not been read"), "{msg}");
}
