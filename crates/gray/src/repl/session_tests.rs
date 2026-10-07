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
        session_id: sid.clone(),
        _open_guard: None,
    });
    let config = Config {
        fast_mode: None,
        model_parts: Default::default(),
        temperature: None,
        top_p: None,
        model: None,
        base_url: String::new(),
        api_key: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        exec_prefix: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
        bare: false,
        lean: false,
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
    // Truncated-tail corruption: the last record parses as JSON but fails
    // entry validation, so `scan_entries` refuses the append (`Corrupt`).
    // But it IS the final line, so `load` heals it as a torn tail (drops the
    // fragment, backs the original up) and the retry lands the save: no
    // warning, no pending flag.
    let mut corrupt = original.clone();
    corrupt.extend_from_slice(b"{torn\n");
    std::fs::write(&path, &corrupt).unwrap();
    persist_compaction_tail(&mut agent, &config, &mut state, dir.path(), None).await;
    assert!(!state.as_ref().unwrap().full_save_pending);
    let (_, entries) = state.as_ref().unwrap().store.load(&sid).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].message.text_content(),
        "compacted history",
        "the corrupt tail heals and the replacement lands"
    );
    // A directly-saved corrupt tail heals the same way (not only through
    // the `persist_compaction_tail` wrapper).
    std::fs::write(&path, &corrupt).unwrap();
    state.as_mut().unwrap().full_save_pending = true;
    assert!(
        save_full_history(state.as_mut().unwrap(), agent.messages())
            .await
            .is_ok()
    );
    assert!(!state.as_ref().unwrap().full_save_pending);
    // A torn tail (crashed writer: last line unterminated) heals through
    // the repair-then-retry path instead of warning: the save succeeds and
    // the pending flag clears.
    let mut torn = original.clone();
    torn.pop(); // drop the final newline — the classic torn tail
    std::fs::write(&path, &torn).unwrap();
    let mut torn_state = Some(SessionState {
        full_save_pending: false,
        store: JsonlSessionStore::new(dir.path()),
        session_id: sid.clone(),
        _open_guard: None,
    });
    persist_compaction_tail(&mut agent, &config, &mut torn_state, dir.path(), None).await;
    assert!(!torn_state.as_ref().unwrap().full_save_pending);
    let (_, entries) = torn_state.as_ref().unwrap().store.load(&sid).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].message.text_content(),
        "compacted history",
        "the torn fragment heals and the replacement lands"
    );
    // Repair the storage failure, then advance to a later turn. The next
    // save must replace the stale history, not append only the new answer.
    // (Reset to the pristine original: the earlier heals landed compaction
    // replacements on disk, so the file no longer holds `original`.)
    std::fs::write(&path, &original).unwrap();
    state.as_mut().unwrap().full_save_pending = true;
    agent.set_messages(vec![
        Message::user("compacted history"),
        Message::assistant("later answer"),
    ]);
    // `initial_count` is the in-memory cursor: 2 messages already reached
    // memory, so nothing new appends — this turn only retries the pending
    // full save (replacement lands, flag clears).
    persist_turn_messages(&mut state, &agent, &config, dir.path(), 2, None, None).await;
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

/// The tps denominator: response events bill their gaps; boundaries
/// (tool results, round reports, turn end) re-anchor without billing,
/// so a turn that sat in tools for a minute is not billed as slow
/// generation — but a provider that delivers its whole reply in one
/// batch still reports the window it took to generate it.
#[test]
fn stream_clock_measures_only_the_time_tokens_flowed() {
    use super::TurnStreamClock;
    let mut clock = TurnStreamClock::default();
    assert_eq!(clock.streamed_ms(), None, "nothing streamed yet");
    let start = std::time::Instant::now();
    clock.open_span_at(start); // AgentEvent::Start: window opens at the request.
    clock.tick_at(start + std::time::Duration::from_millis(60));
    let first = clock
        .streamed_ms()
        .expect("the dispatch+generate leg is generation time");
    // Lower bound only: a loaded machine can overshoot the sleep, and the
    // assertions that matter below are exact (spans, not wall clock).
    assert!(first >= 40, "{first}");
    clock.tick_at(start + std::time::Duration::from_millis(100));
    let burst = clock.streamed_ms().expect("inter-delta gap counted");
    assert!(burst > first, "{burst} > {first}");

    // A round boundary then the tool wait: neither gap lands in the rate
    // (the boundary opens a fresh window; the tool result re-anchors it).
    clock.open_span_at(start + std::time::Duration::from_millis(100)); // StepUsage: round report.
    clock.open_span_at(start + std::time::Duration::from_millis(180)); // ToolResult: host-side tool wait just closed.
    clock.tick_at(start + std::time::Duration::from_millis(230));
    let second = clock.streamed_ms().expect("second request window counted");
    assert!(
        second > burst && second < burst + 80,
        "{second} should add ~50ms of generation, not the 80ms tool wait"
    );

    // Turn end: post-turn strays open a fresh window instead of billing.
    clock.close_span();
    assert_eq!(clock.streamed_ms(), Some(second));
}

/// Batch delivery: one response event a full generation after the window
/// opened still yields a real denominator — never a µs-scale one that
/// prints a million tps.
#[test]
fn stream_clock_bills_batch_delivery_its_generation_window() {
    use super::TurnStreamClock;
    let mut clock = TurnStreamClock::default();
    let start = std::time::Instant::now();
    clock.open_span_at(start); // request dispatched
    clock.tick_at(start + std::time::Duration::from_millis(90)); // the whole reply lands at once
    let ms = clock
        .streamed_ms()
        .expect("batch delivery bills its window");
    assert!(ms >= 60, "{ms}");
    clock.tick_at(start + std::time::Duration::from_micros(90_001));
    assert!(
        clock.streamed_ms().unwrap() < ms + 60,
        "back-to-back batch events add ~nothing on top"
    );
}
