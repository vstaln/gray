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
//! * `content_hash` is over the RAW file bytes. It gates freshness only
//!   (unchanged on disk), never overwrite authorization — delivered coverage
//!   does that. Files over [`MAX_HASH_BYTES`] (64 MiB) are never hashed
//!   (`None`). NOTE: the card writes `content_hash: u64`, but `None` needs
//!   `Option<u64>` — stored as such.
//! * `full_view` = the entire byte representation was delivered: lines 1..=T
//!   with no line/byte cut and no clamped (shortened) lines. The caller
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

    /// Forget one entry (bulk rollback for bodies never delivered).
    pub fn remove(&self, path: &Path) {
        self.inner
            .lock()
            .expect("FileLedger lock poisoned")
            .remove(&Self::key(path));
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

#[path = "ledger_tests.rs"]
#[cfg(test)]
mod tests;
