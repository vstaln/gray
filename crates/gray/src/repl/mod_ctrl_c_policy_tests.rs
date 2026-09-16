use super::*;

#[test]
fn sigint_second_press_within_window_exits() {
    // First press (no prior) never exits — verified by last==0 guard at
    // the call site; pure helper: far apart → false, close → true.
    assert!(!sigint_should_exit(
        1_000,
        1_000 + CTRL_C_EXIT_WINDOW_MS + 1
    ));
    assert!(sigint_should_exit(1_000, 1_000 + 1_000));
    assert!(sigint_should_exit(1_000, 1_000 + CTRL_C_EXIT_WINDOW_MS));
    // Clock skew backwards → wrapping_sub is huge → false.
    assert!(!sigint_should_exit(2_000, 1_000));
}

#[test]
fn totals_sum_durations_and_skip_untimed() {
    let entry = |id: u64, duration_ms: Option<u64>| crate::session_store::SessionEntry {
        compaction_boundary: false,
        entry_id: id,
        parent_id: None,
        timestamp: 0,
        message: gray_core::message::Message::user("hi"),
        usage: Some(gray_core::event::Usage::new(10, 5)),
        duration_ms,
    };
    let entries = vec![entry(0, Some(6000)), entry(1, Some(4000)), entry(2, None)];
    let t = super::SessionTotals::from_entries(&entries, "test-persist-model");
    assert_eq!(t.turns, 3);
    assert_eq!(t.total_duration_ms, 10_000);
    assert_eq!(t.timed_turns, 2);
}

#[test]
fn turn_footer_includes_duration_when_known() {
    let usage = gray_core::event::Usage::new(1000, 500);
    let totals = super::SessionTotals::default();
    let line = super::turn_footer(&usage, "test-persist-model", &totals, Some(6500));
    assert!(line.contains("6.5s"), "footer should show time: {line}");
    assert!(line.contains("tok"), "footer should keep tokens: {line}");
}
