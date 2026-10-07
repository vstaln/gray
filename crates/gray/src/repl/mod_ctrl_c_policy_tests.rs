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
    let line = super::turn_footer(
        &usage,
        "test-persist-model",
        &totals,
        Some(6500),
        Some(6500),
    );
    assert!(line.contains("6.5s"), "footer should show time: {line}");
    assert!(line.contains("tokens"), "footer should keep tokens: {line}");
}

/// One test, both branches: `TURN_IN_FLIGHT` is a process-global, so two
/// concurrent tests would clear each other's flag.
#[tokio::test]
async fn drain_waits_for_a_turn_but_never_blocks_the_exit_forever() {
    // The turn clears its own flag on the way out, as run_prompt_turn does.
    {
        let _guard = super::mark_turn_in_flight();
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            drop(super::mark_turn_in_flight());
        });
        assert!(
            super::drain_in_flight_turn(std::time::Duration::from_secs(5)).await,
            "the exit path must wait for the turn to persist"
        );
    }
    assert!(
        !super::TURN_IN_FLIGHT.load(std::sync::atomic::Ordering::Relaxed),
        "the guard clears the flag on drop"
    );
    // A wedged turn must not make Ctrl-C feel dead.
    let _wedged = super::mark_turn_in_flight();
    assert!(
        !super::drain_in_flight_turn(std::time::Duration::from_millis(120)).await,
        "a wedged turn must not block the exit forever"
    );
}

#[test]
fn exit_hint_line_matches_quit_output() {
    assert_eq!(
        super::session::exit_hint_line("calm-river-fox", false),
        "To resume: gray -r calm-river-fox"
    );
    assert_eq!(
        super::session::exit_hint_line("calm-river-fox", true),
        "\x1b[2mTo resume: gray -r calm-river-fox\x1b[0m"
    );
}

#[test]
fn signal_exit_prints_the_resume_hint_not_a_goodbye() {
    assert_eq!(
        super::signal_exit_text(Some("calm-river-fox"), false),
        "To resume: gray -r calm-river-fox\r\n"
    );
    assert_eq!(super::signal_exit_text(None, false), "");
}

/// One test, both directions: `EXIT_SESSION` is a process-global.
#[test]
fn exit_session_tracks_the_latest_session() {
    super::session::remember_exit_session(Some("calm-river-fox"));
    assert_eq!(
        super::session::exit_session().as_deref(),
        Some("calm-river-fox")
    );
    super::session::remember_exit_session(None);
    assert_eq!(super::session::exit_session(), None);
}
