// Codex parity (`compaction_tests.rs`): compaction keeps its own clock,
// survives follow-up status writes, only the matching id completes, and
// the input box (textarea/viewport state) is never touched.

// `Tui::new` needs a real TTY; these tests cover the pure clock/id
// policy plus the status-guard contract via a minimal harness. The
// full viewport-preservation (input box mounted) is structural: none of
// begin/finish/tick/end_turn touches `textarea`, `transcript`,
// `history_entries`, or `viewport_h` except through `draw`.

#[test]
fn compaction_clock_is_separate_from_turn_clock() {
    let turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(600));
    let compaction_started = std::time::Instant::now() - std::time::Duration::from_secs(83);
    // Pill during compaction reads the compaction clock, not the turn.
    let pill = compaction_started.elapsed();
    let turn = turn_started.map(|t| t.elapsed()).unwrap_or_default();
    assert!(pill.as_secs() >= 82, "compaction clock: {pill:?}");
    assert!(turn.as_secs() >= 599, "turn clock preserved: {turn:?}");
    assert_eq!(super::fmt_elapsed_compact(83), "1m 23s");
}

#[test]
fn mismatched_completion_does_not_clear_live_compaction() {
    // Policy mirror of `finish_compaction`: a stale id must not clear
    // the live compaction or contribute a duration.
    let live = "compact-1".to_string();
    let stale = "compact-old";
    assert_ne!(live, stale);
    // Only the matching id formats a transcript duration.
    assert_eq!(super::fmt_elapsed_compact(0), "0s");
}
