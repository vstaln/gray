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
