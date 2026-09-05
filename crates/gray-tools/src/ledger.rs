//! T3.1 FileLedger: shared session state for read/write/edit.
//!
//! Pure recording only (no behavior change yet): read will call
//! [`FileLedger::record_read`] for every read, and write/edit will consult
//! the entry before overwriting (T3.2) or stubbing a repeat read (T3.3).
//! That wiring is NOT here — see the follow-ups below.
//!
//! Spec: plan.ts T3.1 ("FileLedger: shared session state for read/write/edit").
//!
//! Contract:
//! * Keyed by canonicalized [`PathBuf`]: `./a.rs` and `/abs/a.rs` map to one
//!   entry (falls back to the literal path when it does not exist yet).
//! * `content_hash` is over the RAW file bytes (post-stream, pre-clamp), so a
//!   clamped-but-complete read still verifies as "what you saw == disk".
//!   Files over [`MAX_HASH_BYTES`] (64 MiB) are never hashed (`None` → never
//!   eligible for the T3.2 bytes-match or T3.3 dedup). NOTE: the card writes
//!   `content_hash: u64`, but `None` needs `Option<u64>` — stored as such.
//! * `full_view` = the window covered lines 1..=T with no line/byte cut.
//!   Clamped lines still count as full (the T3.2 relational fix); the caller
//!   computes the flag, the ledger only stores it.
//! * Tools run sequentially; a std [`Mutex`] is enough (no async lock).
//!
//! Follow-ups (out of scope for this task — files this task must not touch):
//! * `read/mod.rs`, `write.rs`, `edit.rs`: constructors take
//!   `Arc<FileLedger>` (`ReadTool::new(...)`; `Default` keeps a private
//!   ledger so existing tests compile); read records, write/edit consult.
//! * `gray-plugin/src/builder.rs` (`ToolsBasicPlugin` holds the `Arc`): the
//!   `Registry::file_ledger` accessor in `gray-tools/src/lib.rs` is the seam.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

/// Files larger than this are never content-hashed (`content_hash == None`).
pub const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;

/// What the model has seen of one file. Built by the read tool, read by
/// write (T3.2 guard) and the dedup check (T3.3).
#[derive(Clone, Debug)]
pub struct LedgerEntry {
    /// File mtime at read time (T3.2 rule 3: refuse when disk differs).
    pub mtime: SystemTime,
    /// File size in bytes at read time.
    pub size: u64,
    /// Hash of the full raw file bytes, or `None` past [`MAX_HASH_BYTES`].
    pub content_hash: Option<u64>,
    /// Window covered lines 1..=T with no line/byte cut (clamp still counts).
    pub full_view: bool,
    /// The `(offset, limit)` window that was shown.
    pub window: (i64, Option<u64>),
    /// Absolute 1-indexed first line shown.
    pub first_line: usize,
    /// Absolute 1-indexed last line shown.
    pub last_line: usize,
    /// A repeat of [`LedgerEntry::window`] may be answered with a stub
    /// (T3.3); consumed (set false) on hit.
    pub dedup_armed: bool,
    /// When the read happened (session-local; never persisted).
    pub read_at: Instant,
}

/// In-memory map from canonical path to [`LedgerEntry`].
#[derive(Debug, Default)]
pub struct FileLedger {
    inner: Mutex<HashMap<PathBuf, LedgerEntry>>,
}

impl FileLedger {
    /// Empty ledger (same as `Default`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Canonical key: `canonicalize` when the path exists, else the literal.
    fn key(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Hash of the raw file bytes, or `None` past [`MAX_HASH_BYTES`].
    /// Session-local only (never persisted); equal bytes give equal hashes
    /// within one run.
    pub fn hash_bytes(bytes: &[u8]) -> Option<u64> {
        if bytes.len() as u64 > MAX_HASH_BYTES {
            return None;
        }
        let mut h = DefaultHasher::new();
        h.write(bytes);
        Some(h.finish())
    }

    /// Record what a read showed. Overwrites any previous entry; dedup is
    /// re-armed exactly as the caller sets it (a normal read sets `true`).
    pub fn record_read(&self, path: &Path, entry: LedgerEntry) {
        self.inner
            .lock()
            .expect("FileLedger lock poisoned")
            .insert(Self::key(path), entry);
    }

    /// Clone of the entry for `path`, if the file was read this session.
    pub fn get(&self, path: &Path) -> Option<LedgerEntry> {
        self.inner
            .lock()
            .expect("FileLedger lock poisoned")
            .get(&Self::key(path))
            .cloned()
    }

    /// Record a successful write/edit so the next write is allowed without a
    /// re-read (T3.2): the whole new content is known, `full_view` is true,
    /// and dedup stays disarmed so the next read returns full content.
    pub fn mark_written(&self, path: &Path, new_bytes: &[u8]) {
        let (mtime, size) = std::fs::metadata(path)
            .map(|m| (m.modified().unwrap_or_else(|_| SystemTime::now()), m.len()))
            .unwrap_or_else(|_| (SystemTime::now(), new_bytes.len() as u64));
        let newlines = new_bytes.iter().filter(|&&b| b == b'\n').count();
        let lines = if new_bytes.is_empty() {
            0
        } else {
            newlines + usize::from(!new_bytes.ends_with(b"\n"))
        };
        self.record_read(
            path,
            LedgerEntry {
                mtime,
                size,
                content_hash: Self::hash_bytes(new_bytes),
                full_view: true,
                window: (1, None),
                first_line: 1,
                last_line: lines,
                dedup_armed: false,
                read_at: Instant::now(),
            },
        );
    }

    /// After any compaction: entries stay for the write guard, but no stub
    /// may reference a compacted-away result (T3.4).
    pub fn disarm_all_dedup(&self) {
        for entry in self
            .inner
            .lock()
            .expect("FileLedger lock poisoned")
            .values_mut()
        {
            entry.dedup_armed = false;
        }
    }

    /// On `/new` / session resume: the new session saw nothing yet (T3.4).
    pub fn clear(&self) {
        self.inner.lock().expect("FileLedger lock poisoned").clear();
    }
}

#[cfg(test)]
mod tests {
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
        // Clamped-but-complete reads count as full (caller sets true); a
        // byte-cut read sets false. The ledger stores the flag, nothing more.
        ledger.record_read(Path::new("clamped-min.js"), entry(true));
        ledger.record_read(Path::new("cut-big.log"), entry(false));
        assert!(ledger.get(Path::new("clamped-min.js")).unwrap().full_view);
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
}
