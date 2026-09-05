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
        ] {
            assert!(!s.contains("Error:"), "{s}");
        }
    }
}
