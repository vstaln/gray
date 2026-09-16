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

#[path = "dedup_tests.rs"]
#[cfg(test)]
mod tests;
