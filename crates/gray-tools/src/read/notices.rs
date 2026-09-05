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
//!    sites already comply — route them through [`read_failed`] when touched.
//! 2. Integrator: move T1.4 `hygiene.rs` (`mime_note`, `nul_note`) and T1.5
//!    `tail.rs` ([`tail_note`], [`limit_ignored_note`]) contract strings here
//!    verbatim; those files stage them locally on purpose until this file
//!    landed. Do NOT duplicate them here first (one owner per string).

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

/// Per-line clamp note. `line(s)` stays literal for every count (no plural logic).
pub fn clamped(count: usize) -> String {
    format!(
        "[read: {count} line(s) longer than 2000 chars were clamped; \
         use grep -n or bash cut -c to inspect a specific one.]"
    )
}

/// Genuine I/O failures stay `is_error=true`; the prefix is `read failed:`,
/// never `Error:`.
pub fn read_failed(display: &str, detail: &str) -> String {
    format!("read failed for {display}: {detail}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_note_is_contract_exact() {
        assert_eq!(
            directory("docs"),
            "[read: docs is a directory. Use ls or find.]"
        );
    }

    #[test]
    fn empty_note_is_contract_exact() {
        assert_eq!(empty("docs/TODO.md"), "[read: docs/TODO.md is empty (0 bytes)]");
    }

    #[test]
    fn eof_note_suggests_tail() {
        // long.txt shape: 3000 lines, offset=9999 → suggest 2951.
        assert_eq!(
            offset_past_eof("long.txt", 9999, 3000),
            "[read: offset 9999 is beyond the end of long.txt (3000 lines). \
             Retry with offset=2951 to see the tail, or offset=1.]"
        );
        assert_eq!(tail_suggestion(3000), 2951);
        assert_eq!(tail_suggestion(50), 1);
        assert_eq!(tail_suggestion(4), 1);
        assert_eq!(tail_suggestion(0), 1);
    }

    #[test]
    fn cap_notes_carry_resume_offsets() {
        assert_eq!(
            line_cap(1, 2000, 3000),
            "[read: showing lines 1-2000 of 3000. Continue with offset=2001.]"
        );
        assert_eq!(
            byte_cap(1, 1846, 1847),
            "[read: showing lines 1-1846 (50 KiB budget). \
             Continue with offset=1847 — that line was not shown.]"
        );
    }

    #[test]
    fn clamp_note_names_recovery() {
        assert_eq!(
            clamped(1),
            "[read: 1 line(s) longer than 2000 chars were clamped; \
             use grep -n or bash cut -c to inspect a specific one.]"
        );
    }

    #[test]
    fn join_separates_with_blank_line_or_returns_note_alone() {
        assert_eq!(join("", "[read: x]"), "[read: x]");
        assert_eq!(join("a", "[read: x]"), "a\n\n[read: x]");
    }

    #[test]
    fn no_notice_contains_error_prefix() {
        for s in [
            directory("p"),
            empty("p"),
            offset_past_eof("p", 9999, 3000),
            line_cap(1, 2, 3),
            byte_cap(1, 2, 3),
            clamped(1),
            read_failed("p", "No such file or directory (os error 2)"),
            write_unread("p"),
            write_changed("p"),
            edit_changed("p"),
            write_partial("p", 1, 2000, 80412),
            dedup_stub("p", 1, 2000),
        ] {
            assert!(!s.contains("Error:"), "{s}");
        }
    }

    #[test]
    fn write_guard_notes_are_contract_exact() {
        assert_eq!(
            write_unread("src/new.rs"),
            "write refused: src/new.rs exists and has not been read this session. \
             Read it first (read src/new.rs), or pass force=true to overwrite blind."
        );
        assert_eq!(
            write_changed("src/new.rs"),
            "write refused: src/new.rs changed on disk since you read it. Re-read it."
        );
        assert_eq!(
            edit_changed("src/new.rs"),
            "edit refused: src/new.rs changed on disk since you read it. Re-read it."
        );
        assert_eq!(
            write_partial("Cargo.lock", 1, 2000, 80412),
            "write refused: only part of Cargo.lock has been read (lines 1-2000 of 80412). \
             Read the rest (offset=2001) or use edit for a targeted change."
        );
    }

    #[test]
    fn dedup_stub_is_contract_exact_and_small() {
        let s = dedup_stub("src/agent.rs", 1, 2000);
        assert_eq!(
            s,
            "[read: src/agent.rs lines 1-2000 unchanged since your previous read above; \
             content omitted. If that result is no longer visible (compacted), \
             call read again and it will be returned in full.]"
        );
        // T3.3 accept: stub < 60 tokens (bytes/4).
        assert!((s.len() as u64) / 4 < 60, "{s}");
    }
}
