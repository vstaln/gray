// UNRUN (cargo test banned under X): run in TTY/CI.
// The lazy-build gate: a fresh session with a model mints before the
// first build (real sid from turn one); no model (or existing session)
// never mints, so the unconfigured REPL still opens session-free.
use super::should_ensure_session_before_build;

#[test]
fn first_build_ensures_session_only_when_model_configured() {
    assert!(should_ensure_session_before_build(
        false,
        Some("openai/gpt-4o")
    ));
    assert!(!should_ensure_session_before_build(false, None));
    assert!(!should_ensure_session_before_build(false, Some("")));
    assert!(!should_ensure_session_before_build(
        true,
        Some("openai/gpt-4o")
    ));
    assert!(!should_ensure_session_before_build(true, None));
}

#[tokio::test]
async fn failed_compaction_save_retries_full_history_before_appending() {
    use super::*;
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let sid = SessionId::new("savefail");
    store
        .create(SessionMeta::new(sid.clone(), 1, dir.path(), "test"))
        .await
        .unwrap();
    store
        .append(&sid, &Message::user("old history"))
        .await
        .unwrap();
    let path = dir.path().join("savefail.jsonl");
    let original = std::fs::read(&path).unwrap();
    let mut state = Some(SessionState {
        full_save_pending: false,
        store,
        session_id: sid,
    });
    let config = Config {
        temperature: None,
        top_p: None,
        model: None,
        base_url: String::new(),
        api_key: None,
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    };
    // No provider request is made: these tests exercise persistence only.
    struct NoRequests;
    impl gray_core::agent::Provider for NoRequests {
        fn stream(&self, _: gray_core::message::ChatRequest) -> gray_core::agent::ProviderStream {
            panic!("persistence must not call the provider")
        }
    }
    let provider = NoRequests;
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(gray_tools::Registry::default()),
    )
    .with_messages(vec![Message::user("compacted history")]);
    let mut corrupt = original.clone();
    corrupt.extend_from_slice(b"{torn\n");
    std::fs::write(&path, &corrupt).unwrap();
    persist_compaction_tail(&mut agent, &config, &mut state, dir.path(), None).await;
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert!(state.as_ref().unwrap().full_save_pending);
    let error = save_full_history(state.as_mut().unwrap(), agent.messages())
        .await
        .unwrap_err();
    assert!(error.contains("compaction save failed"));
    assert!(error.contains("History remains in memory"));
    assert!(state.as_ref().unwrap().full_save_pending);
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    // Repair the storage failure, then advance to a later turn. The next
    // save must replace the stale history, not append only the new answer.
    std::fs::write(&path, &original).unwrap();
    agent.set_messages(vec![
        Message::user("compacted history"),
        Message::assistant("later answer"),
    ]);
    persist_turn_messages(&mut state, &agent, &config, dir.path(), 1, None, None).await;
    assert!(!state.as_ref().unwrap().full_save_pending);
    let mut next = agent.messages().to_vec();
    next.push(Message::user("ordinary next turn"));
    agent.set_messages(next);
    persist_turn_messages(&mut state, &agent, &config, dir.path(), 2, None, None).await;
    let state = state.unwrap();
    let (_, entries) = state.store.load(&state.session_id).await.unwrap();
    let messages: Vec<_> = entries.into_iter().map(|entry| entry.message).collect();
    assert_eq!(messages, agent.messages());
}

/// The tps denominator: only the time tokens were actually flowing counts,
/// so a turn that sat in tools for a minute is not billed as slow generation.
#[test]
fn stream_clock_measures_only_the_time_tokens_flowed() {
    use super::TurnStreamClock;
    let mut clock = TurnStreamClock::default();
    assert_eq!(clock.streamed_ms(), None, "nothing streamed yet");
    clock.tick();
    assert_eq!(
        clock.streamed_ms(),
        None,
        "a lone delta opens the span but has no gap yet"
    );
    std::thread::sleep(std::time::Duration::from_millis(60));
    clock.tick();
    let first = clock.streamed_ms().expect("the gap is streaming time");
    // Lower bound only: a loaded machine can overshoot the sleep, and the
    // assertions that matter below are exact (spans, not wall clock).
    assert!(first >= 40, "{first}");

    // Tool wait between rounds: the span closes, so the gap until the next
    // round's first delta never lands in the rate.
    clock.close_span();
    std::thread::sleep(std::time::Duration::from_millis(80));
    clock.tick();
    assert_eq!(
        clock.streamed_ms(),
        Some(first),
        "the tool wait must not count as generation"
    );
    std::thread::sleep(std::time::Duration::from_millis(60));
    clock.tick();
    let second = clock.streamed_ms().expect("second burst counted");
    assert!(second > first, "{second} > {first}");

    // Turn end: the finalize gap after the last delta is dropped too.
    clock.close_span();
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(clock.streamed_ms(), Some(second));
}
