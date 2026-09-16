//! T1.3 — read notice wording: the single owner for `[read: …]` notes.
//!
//! Pure `String` builders only (std only, no new deps). Wired by T1.3 as
//! `pub mod notices;` in `read/mod.rs`; only the Directory arm is wired so
//! far (Wave-C siblings own the other `mod.rs` regions — see FOLLOW-UPS).
//!
//! Spec: plan.ts T1.3 ("Every dead end names its recovery"). Exact contract
//! strings live here so the reviewer diffs one file. Facts are notes
//! (`is_error=false`); only genuine I/O failures stay `is_error=true`,
//! prefixed `read failed:` — never `Error:`.
//!
//! FOLLOW-UPS (not done here — regions owned by concurrent Wave-C tasks):
//! 1. `read/mod.rs` empty-file arm → [`empty`]; EOF arm → [`offset_past_eof`]
//!    (requested offset, not the 0-indexed `start`; suggestion is
//!    [`tail_suggestion`]); line-cap arm → [`line_cap`]; byte-cap arm →
//!    [`byte_cap`] with `next` = first unshown line; clamp arm → [`clamped`].
//!    Join content + note with [`join`]. Existing `read failed for …` call
//!    sites already comply (the driver formats them inline).
//!    (Wave-C INT-C wired the empty/EOF/cap/clamp arms; this list stays as the
//!    wording index.)
//! 2. Done (wave gate): T1.4 `hygiene.rs` (`mime_note`, `nul_note`), T1.5
//!    `tail.rs` ([`tail_note`], [`limit_ignored_note`]), T2.1 `stream.rs`
//!    ([`cancelled_note`], [`count_skipped_total`]), T4.1 `resolve.rs`
//!    ([`repaired_note`]), T4.2/T6.1 `bulk.rs` ([`aggregate_note`],
//!    [`MISSING_INPUT_MESSAGE`], [`no_files_matched`]) moved here verbatim.
//!    One owner per string: no `[read:` literal lives outside this file.
//! 3. Remaining: `write.rs`/`edit.rs` resolve-retry + device-guard reuse,
//!    T4.2 did-you-mean, pixel/vision ops. (`find.rs` fallback already walks
//!    via the `ignore` crate.)

use crate::truncate::format_size;

/// `[read: <path> is a directory. Use ls or find.]` — a fact (`is_error=false`).
pub fn directory(display: &str) -> String {
    format!("[read: {display} is a directory. Use ls or find.]")
}

/// `[read: <path> is empty (0 bytes)]` — a fact (`is_error=false`).
pub fn empty(display: &str) -> String {
    format!("[read: {display} is empty (0 bytes)]")
}

/// Tail suggestion for the past-EOF note: `max(1, T-49)`.
pub fn tail_suggestion(total_lines: usize) -> usize {
    total_lines.saturating_sub(49).max(1)
}

/// Past-EOF note — a fact (`is_error=false`). `requested` is the offset the
/// caller passed (1-indexed), not the internal 0-indexed `start`.
pub fn offset_past_eof(display: &str, requested: u64, total_lines: usize) -> String {
    format!(
        "[read: offset {requested} is beyond the end of {display} ({total_lines} lines). \
         Retry with offset={} to see the tail, or offset=1.]",
        tail_suggestion(total_lines)
    )
}

/// Line-cap note. `next` resume offset is always `last + 1`.
pub fn line_cap(first: usize, last: usize, total: usize) -> String {
    format!(
        "[read: showing lines {first}-{last} of {total}. Continue with offset={}.]",
        last + 1
    )
}

/// Byte-cap note. `next` is the first UNSHOWN line (resume ON it — that line
/// was not shown). Budget label is the spec-fixed `50 KiB`.
pub fn byte_cap(first: usize, last: usize, next: usize) -> String {
    format!(
        "[read: showing lines {first}-{last} (50 KiB budget). \
         Continue with offset={next} — that line was not shown.]"
    )
}

/// Line-cap note when the file exceeds the stream's exact-count limit (T2.2):
/// the total is a lower bound, but `next` still names an observed line, so
/// resuming there returns content.
pub fn line_cap_count_skipped(
    first: usize,
    last: usize,
    min_total: usize,
    file_size: u64,
    next: usize,
) -> String {
    format!(
        "[read: showing lines {first}-{last} of {}. Continue with offset={next}.]",
        count_skipped_total(min_total, file_size)
    )
}

/// Per-line clamp note. `line(s)` stays literal for every count (no plural logic).
pub fn clamped(count: usize) -> String {
    format!(
        "[read: {count} line(s) longer than 2000 chars were clamped; \
         use grep -n or bash cut -c to inspect a specific one.]"
    )
}

/// `[read: last <shown> lines of <T> (lines <a>-<T>)]` (T1.5, verbatim).
/// `<shown>` is the lines actually shown (`min(|offset|, T)`).
pub fn tail_note(shown: u64, total: usize) -> String {
    let first = total as u64 - shown + 1;
    format!("[read: last {shown} lines of {total} (lines {first}-{total})]")
}

/// One-line note when `limit` accompanies a negative offset (T1.5, verbatim).
pub fn limit_ignored_note(limit: u64) -> String {
    format!(
        "[read: limit={limit} ignored with negative offset; showing the tail instead. \
         Omit limit when offset is negative.]"
    )
}

/// Magic-byte sniff hit (T1.4, verbatim): `[read: <path> is <mime> (<size>), not shown]`.
pub fn mime_note(display: &str, mime: &str, size: usize) -> String {
    format!(
        "[read: {display} is {mime} ({}), not shown]",
        format_size(size)
    )
}

/// NUL-byte sniff hit (T1.4, verbatim).
pub fn nul_note(display: &str) -> String {
    format!("[read: {display} looks binary (NUL bytes), not shown]")
}

/// `[read: cancelled after <n> lines]` (T2.1, verbatim).
pub fn cancelled_note(lines_read: usize) -> String {
    format!("[read: cancelled after {lines_read} lines]")
}

/// `≥<min> lines (file is <size>, count skipped)` fragment for totals over
/// the stream's exact-count limit (T2.1, verbatim).
pub fn count_skipped_total(min_total: usize, file_size: u64) -> String {
    format!(
        "≥{min_total} lines (file is {}, count skipped)",
        format_size(file_size as usize)
    )
}

/// `[read: opened <actual> (path repaired from <given>)]` (T4.1, verbatim).
pub fn repaired_note(actual: &str, given: &str) -> String {
    format!("[read: opened {actual} (path repaired from {given})]")
}

/// Enforced when neither `path` nor `paths` is given (T6.1, verbatim).
pub const MISSING_INPUT_MESSAGE: &str =
    "read: provide path (one file) or paths (list of files/globs)";

/// Skipped names printed in [`aggregate_note`] before the `…`.
const MAX_NOTE_NAMES: usize = 10;

/// Trailing summary once the bulk budget stops the list (T6.1, verbatim).
pub fn aggregate_note(shown: usize, total: usize, skipped: &[String]) -> String {
    let left = total.saturating_sub(shown);
    let names: Vec<&str> = skipped
        .iter()
        .take(MAX_NOTE_NAMES)
        .map(String::as_str)
        .collect();
    let list = if skipped.len() > MAX_NOTE_NAMES {
        format!("{}, …", names.join(", "))
    } else {
        names.join(", ")
    };
    format!(
        "[read: showed {shown} of {total} files; {left} skipped (over 100 KiB total): {list}. \
         Read them individually or narrow the glob.]"
    )
}

/// Bulk no-match failure (staged in `read/mod.rs`, verbatim wording).
pub fn no_files_matched(paths: &[String]) -> String {
    let mut shown = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > 3 {
        shown.push_str(", …");
    }
    format!(
        "read failed: no files matched {} pattern(s): {shown}. \
         Check the globs and exclude[].",
        paths.len()
    )
}

/// Write refused: the file exists but was never read this session (T3.2 rule
/// 2). Names the recovery (`read <path>`) and the `force=true` escape.
pub fn write_unread(display: &str) -> String {
    format!(
        "write refused: {display} exists and has not been read this session. \
         Read it first (read {display}), or pass force=true to overwrite blind."
    )
}

/// Write refused: the file changed on disk since it was read (T3.2 rule 3).
pub fn write_changed(display: &str) -> String {
    format!("write refused: {display} changed on disk since you read it. Re-read it.")
}

/// Edit refused: the same staleness rule as [`write_changed`], with the tool name.
pub fn edit_changed(display: &str) -> String {
    format!("edit refused: {display} changed on disk since you read it. Re-read it.")
}

/// Write refused: only part of the file was read (T3.2 rule 6). This wording
/// — never the rule-2 wording — is used whenever an entry exists: partial is
/// not unread. `next` resume offset is always `last + 1`.
pub fn write_partial(display: &str, first: usize, last: usize, total: usize) -> String {
    format!(
        "write refused: only part of {display} has been read \
         (lines {first}-{last} of {total}). Read the rest (offset={}) \
         or use edit for a targeted change.",
        last + 1
    )
}

/// Repeat-read stub (T3.3): this window is unchanged since the previous read.
/// A fact (`is_error=false`); consumed on hit, so a compacted-away result
/// comes back in full on the next identical read.
pub fn dedup_stub(display: &str, first: usize, last: usize) -> String {
    format!(
        "[read: {display} lines {first}-{last} unchanged since your previous read above; \
         content omitted. If that result is no longer visible (compacted), \
         call read again and it will be returned in full.]"
    )
}

/// Joins windowed content with its trailing note: blank-line separated; the
/// note alone when there is no content (so no path returns `ok("")`).
pub fn join(content: &str, note: &str) -> String {
    if content.is_empty() {
        note.to_string()
    } else {
        format!("{content}\n\n{note}")
    }
}

#[path = "notices_tests.rs"]
#[cfg(test)]
mod tests;
