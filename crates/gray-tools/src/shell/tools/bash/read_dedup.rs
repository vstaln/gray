//! Read dedup on the surface that is actually loaded: `cat`/`sed`/`head`/
//! `tail` through `bash`.
//!
//! The `read` tool has had consume-on-hit dedup for a while, but the default
//! profile is bash-only — so the repeated read that costs the tokens was the
//! one with no dedup. This is the same rule keyed on the command instead of
//! the tool: an exact repeat of a window whose file has not changed returns a
//! citation stub, once, and the repeat after that reads again (consume-on-hit,
//! so the model can always get the bytes back by asking a second time).
//!
//! Deliberately narrow. Only a command that is *nothing but* one read of one
//! existing regular file qualifies: a pipe, a redirect, a second operand, a
//! glob, a subshell or a `cat -n` produces different bytes, and stubbing one
//! of those would hide output the model has never seen. Runs whose output
//! overflowed the inline budget are never recorded either — a record the model
//! only half saw is not a record it can be told "unchanged since you read it"
//! about.
//!
//! One entry per file, as the shared ledger already is: the repeat that stubs
//! is the repeat of the read that came last, and any other window runs again.
//!
//! Shared ledger: the host builds this with the same [`FileLedger`] the
//! `read`/`write`/`edit` tools use, so the same file read through either
//! surface dedups against the other one, and `/new` + compaction lifecycle
//! (`clear` / `disarm_all_dedup`) covers this state too.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gray_core::agent::ToolOutput;

use crate::ledger::{FileLedger, LedgerEntry};
use crate::shell::contract::INLINE_BUDGET_BYTES;
use crate::{finish, resolve_path};

/// Kill switch, the read tool's own: `GRAY_READ_DEDUP=0` disables stubbing.
/// The caller reads the env and passes the flag, so this module never touches
/// process env and unit tests stay race-free.
pub fn enabled() -> bool {
    std::env::var("GRAY_READ_DEDUP").as_deref() != Ok("0")
}

/// One file, one window, as a single read command asks for it.
#[derive(Debug, PartialEq)]
pub struct PlainRead {
    /// Absolute path of an existing regular file.
    pub path: PathBuf,
    /// The command as written, quoted into the stub so the model can rerun it.
    pub command: String,
    /// `(first_line, line_count)` in the ledger's window shape: `(1, None)`
    /// for a whole file, `(0, Some(n))` for a tail (its start moves with the
    /// file), `(first, Some(count))` for a head or a sed range.
    pub window: (i64, Option<u64>),
    /// The command asks for the whole file (`cat`, `sed -n '1,$p'`).
    pub whole_file: bool,
}

/// The single read `command` performs, or `None` when it is anything else.
///
/// Conservative by construction: an unrecognized shape is a normal run, so a
/// parse that is too clever can only cost tokens, never hide output.
pub fn plain_read(command: &str, cwd: &Path) -> Option<PlainRead> {
    let command = command.trim();
    // Any of these can produce something other than one file's bytes.
    if command.is_empty() || command.contains(['|', ';', '&', '>', '<', '`', '*', '?', '\n']) {
        return None;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    let (path, window, whole_file) = match words.as_slice() {
        ["cat", path] => (*path, (1, None), true),
        // Byte windows (`-c`) are a different unit; declining them is cheaper
        // than pretending a byte count is a line count.
        ["head", "-n", n, path] => (*path, (1, Some(line_count(n)?)), false),
        ["tail", "-n", n, path] => (*path, (0, Some(line_count(n)?)), false),
        // The program arrives quoted: `sed -n '1,40p' file`.
        ["sed", "-n", program, path] => sed_window(program.trim_matches('\''), path)?,
        _ => return None,
    };
    if !literal_operand(path) {
        return None;
    }
    let full = resolve_path(cwd, path);
    // Only a real regular file: a directory, a fifo or a device has no
    // byte-identical repeat to reason about.
    if !std::fs::metadata(&full).is_ok_and(|m| m.is_file()) {
        return None;
    }
    Some(PlainRead {
        path: full,
        command: command.to_string(),
        window,
        whole_file,
    })
}

/// Characters an operand must not contain. The path is resolved *literally*
/// here while the shell would have rewritten it, so a tilde, a variable or a
/// quote means the two disagree about which file this is — decline rather than
/// stub one read with another's bytes. A leading `-` is a flag, not a path.
const REWRITTEN_BY_SHELL: [char; 12] =
    ['~', '$', '"', '\'', '\\', '(', ')', '{', '}', '[', ']', '#'];

fn literal_operand(path: &str) -> bool {
    !path.starts_with('-') && !path.contains(REWRITTEN_BY_SHELL)
}

/// A plain decimal count (`20`). `0` is declined: `head -n 0` prints nothing,
/// which is not a window anybody can be cited for.
fn line_count(n: &str) -> Option<u64> {
    let n: u64 = n.parse().ok()?;
    (n > 0).then_some(n)
}

/// The `sed` window a dedup-eligible command reads.
type SedWindow<'a> = (&'a str, (i64, Option<u64>), bool);

/// `A,Bp` → the window it prints; `1,$p` is the whole file.
#[allow(clippy::type_complexity)] // (path, (first, last), unbounded) is clearer spelled out
fn sed_window<'a>(program: &'a str, path: &'a str) -> Option<SedWindow<'a>> {
    let range = program.strip_suffix('p')?;
    let (from, to) = range.split_once(',')?;
    if from.is_empty() || to.is_empty() || !from.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if to == "$" && from == "1" {
        return Some((path, (1, None), true));
    }
    if !to.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (from, to): (i64, i64) = (from.parse().ok()?, to.parse().ok()?);
    (from >= 1 && to > from).then_some((path, (from, Some((to - from + 1) as u64)), false))
}

/// The stub for an exact repeat, consuming the arm; `None` on any miss
/// (different window, changed file, or nothing recorded). Never errors: an
/// unreadable file is a miss, so the command runs as written.
pub fn check(ledger: &FileLedger, read: &PlainRead, enable: bool) -> Option<ToolOutput> {
    if !enable {
        return None;
    }
    let entry = ledger.get(&read.path)?;
    if !entry.dedup_armed || entry.window != read.window {
        return None;
    }
    // A whole-file stub is only honest when the file really was shown whole.
    if read.whole_file && !entry.full_view {
        return None;
    }
    let meta = std::fs::metadata(&read.path).ok()?;
    if meta.modified().ok()? != entry.mtime || meta.len() != entry.size {
        return None;
    }
    let stub = stub(&read.command, &read.path);
    ledger.record_read(
        &read.path,
        LedgerEntry {
            dedup_armed: false,
            ..entry
        },
    );
    Some(finish(stub))
}

/// Record what the run showed, re-arming the next dedup. `shown_bytes` is the
/// run's log size: over the inline budget the body was elided, so that run
/// taught the model nothing it could be told is unchanged. (The caller only
/// records a run that finished on its own — see the `exit ` guard there.)
pub fn record(ledger: &FileLedger, read: &PlainRead, shown_bytes: u64) {
    if shown_bytes > INLINE_BUDGET_BYTES as u64 {
        return;
    }
    let Ok(meta) = std::fs::metadata(&read.path) else {
        return;
    };
    let Ok(mtime) = meta.modified() else {
        return;
    };
    // Whole file and byte-for-byte what was shown: nothing was cut.
    let full_view = read.whole_file && shown_bytes == meta.len();
    let lines = full_view.then(|| count_lines(&read.path));
    let (first_line, last_line) = match read.window {
        (_, None) => (1, lines.unwrap_or(0)),
        (0, Some(n)) => {
            let total = lines.unwrap_or(0);
            (total.saturating_sub(n as usize) + 1, total)
        }
        (first, Some(n)) => (
            first.max(1) as usize,
            (first.max(1) as usize) + n as usize - 1,
        ),
    };
    ledger.record_read(
        &read.path,
        LedgerEntry {
            mtime,
            size: meta.len(),
            // Freshness is decided by mtime+size here; the read tool hashes
            // what it read and this never re-reads the bytes.
            content_hash: None,
            full_view,
            window: read.window,
            first_line,
            last_line,
            dedup_armed: true,
            read_at: Instant::now(),
        },
    );
}

/// Lines in a file that was just shown whole, so it is bounded by the inline
/// budget. Unreadable counts as 0: the entry then names no lines, which only
/// costs the read tool's stub a line range.
fn count_lines(path: &Path) -> usize {
    std::fs::read(path)
        .map(|b| b.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0)
}

/// Self-describing like the read tool's: the command, the file, and the way
/// back to the bytes.
fn stub(command: &str, path: &Path) -> String {
    format!(
        "[bash: `{command}` showed {} unchanged since your previous read above; \
         content omitted. If that result is no longer visible (compacted or \
         masked), run the command again and it returns in full.]",
        path.display()
    )
}

#[path = "read_dedup_tests.rs"]
#[cfg(test)]
mod tests;
