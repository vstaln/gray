// Codex parity (`compaction_tests.rs`): compaction keeps its own clock,
// survives follow-up status writes, only the matching id completes, and
// the input box (textarea/viewport state) is never touched.

// `Tui::new` needs a real TTY; these tests cover the pure clock/id
// policy plus the status-guard contract via a minimal harness. The
// full viewport-preservation (input box mounted) is structural: none of
// begin/finish/tick/end_turn touches `textarea`, `transcript`,
// `history_entries`, or `viewport_h` except through `draw`.
