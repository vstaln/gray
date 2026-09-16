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
    assert_eq!(
        empty("docs/TODO.md"),
        "[read: docs/TODO.md is empty (0 bytes)]"
    );
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
    assert_eq!(
        line_cap_count_skipped(1, 2000, 2001, 200 * 1024 * 1024, 2001),
        "[read: showing lines 1-2000 of ≥2001 lines (file is 200.0MB, count skipped). \
             Continue with offset=2001.]"
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
        line_cap_count_skipped(1, 2, 3, 200 * 1024 * 1024, 3),
        clamped(1),
        write_unread("p"),
        write_changed("p"),
        edit_changed("p"),
        write_partial("p", 1, 2000, 80412),
        dedup_stub("p", 1, 2000),
        tail_note(3, 3000),
        limit_ignored_note(2),
        mime_note("p", "image/png", 1032),
        nul_note("p"),
        cancelled_note(2),
        count_skipped_total(2001, 200 * 1024 * 1024),
        repaired_note("a/café.txt", "a/cafe.txt"),
        aggregate_note(100, 300, &["a.rs".to_string()]),
        no_files_matched(&["*.rs".to_string()]),
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
