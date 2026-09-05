//! T3.3 consume-on-hit dedup for the `read` tool.
//!
//! A repeated identical read of an unchanged file returns a ~40-token stub
//! instead of the content — exactly once — then the arm is consumed and the
//! next identical read returns full content again.
//!
//! Spec: plan.ts T3.3 ("Unchanged-read dedup, consume-on-hit"). Wired in
//! `read/mod.rs` (`mod dedup;`): the check runs after the device/dir gates,
//! before any content I/O; every content return re-arms via `record_read`.
//!
//! Hit condition: same canonical path, same `(offset, limit)` window, disk
//! mtime AND size equal to the ledger entry, `dedup_armed`, and `enable`
//! (the `GRAY_READ_DEDUP=0` kill switch — read by the caller via [`enabled`]
//! so this module never touches process env and unit tests stay race-free).
//! Relational guard: a read from offset 0/1 (or absent, normalized to 1 by
//! the caller) is never stubbed unless the entry is a `full_view` — a bare
//! `read <path>` must not hide unseen lines.

use std::path::Path;

use gray_core::agent::ToolOutput;

use crate::ledger::{FileLedger, LedgerEntry};

/// Kill switch: `GRAY_READ_DEDUP=0` disables stubbing (kill switch documented
/// in the T7.1 README env table).
pub fn enabled() -> bool {
    std::env::var("GRAY_READ_DEDUP").as_deref() != Ok("0")
}

/// Returns the stub when this exact window was already shown and the file is
/// unchanged, consuming the arm; `None` on any miss (the caller reads
/// normally and re-arms). Never errors: an unreadable file is a miss.
pub fn check(
    ledger: &FileLedger,
    path: &Path,
    display: &str,
    offset: i64,
    limit: Option<u64>,
    enable: bool,
) -> Option<ToolOutput> {
    if !enable {
        return None;
    }
    let entry = ledger.get(path)?;
    if entry.window != (offset, limit) || !entry.dedup_armed {
        return None;
    }
    // A bare `read <path>` (offset 0/1/absent) claims the whole file: only
    // stub it when the previous read really covered lines 1..=T. Explicit
    // windows (offset>1, tails) name their lines in the stub, so they may hit.
    if (0..=1).contains(&offset) && !entry.full_view {
        return None;
    }
    // Empty-file records (last < first) carry no lines to name; re-show them.
    if entry.last_line < entry.first_line {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if meta.modified().ok()? != entry.mtime || meta.len() != entry.size {
        return None;
    }
    let stub = super::notices::dedup_stub(display, entry.first_line, entry.last_line);
    ledger.record_read(
        path,
        LedgerEntry {
            dedup_armed: false,
            ..entry
        },
    );
    Some(ToolOutput::ok(stub))
}

#[cfg(test)]
mod tests {
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
}
