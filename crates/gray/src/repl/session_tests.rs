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
        compact_at: None,
    };

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
    assert!(!state.as_ref().unwrap().full_save_pending);
    let (_, entries) = state.as_ref().unwrap().store.load(&sid).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].message.text_content(),
        "compacted history",
        "the corrupt tail heals and the replacement lands"
    );

    std::fs::write(&path, &corrupt).unwrap();
    state.as_mut().unwrap().full_save_pending = true;
    assert!(
        save_full_history(state.as_mut().unwrap(), agent.messages())
            .await
            .is_ok()
    );
    assert!(!state.as_ref().unwrap().full_save_pending);

    let mut torn = original.clone();
    torn.pop();
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

    std::fs::write(&path, &original).unwrap();
    state.as_mut().unwrap().full_save_pending = true;
    agent.set_messages(vec![
        Message::user("compacted history"),
        Message::assistant("later answer"),
    ]);

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

#[test]
fn stream_clock_measures_only_the_time_tokens_flowed() {
    use super::TurnStreamClock;
    let mut clock = TurnStreamClock::default();
    assert_eq!(clock.streamed_ms(), None, "nothing streamed yet");
    let start = std::time::Instant::now();
    clock.open_span_at(start);
    clock.tick_at(start + std::time::Duration::from_millis(60));
    let first = clock
        .streamed_ms()
        .expect("the dispatch+generate leg is generation time");

    assert!(first >= 40, "{first}");
    clock.tick_at(start + std::time::Duration::from_millis(100));
    let burst = clock.streamed_ms().expect("inter-delta gap counted");
    assert!(burst > first, "{burst} > {first}");

    clock.open_span_at(start + std::time::Duration::from_millis(100));
    clock.open_span_at(start + std::time::Duration::from_millis(180));
    clock.tick_at(start + std::time::Duration::from_millis(230));
    let second = clock.streamed_ms().expect("second request window counted");
    assert!(
        second > burst && second < burst + 80,
        "{second} should add ~50ms of generation, not the 80ms tool wait"
    );

    clock.close_span();
    assert_eq!(clock.streamed_ms(), Some(second));
}

#[test]
fn stream_clock_bills_batch_delivery_its_generation_window() {
    use super::TurnStreamClock;
    let mut clock = TurnStreamClock::default();
    let start = std::time::Instant::now();
    clock.open_span_at(start);
    clock.tick_at(start + std::time::Duration::from_millis(90));
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

#[test]
fn repl_tool_header_renders_label_and_template_second_line() {
    let labels = std::collections::HashMap::from([(
        "subagent_spawn".to_string(),
        "Started agent".to_string(),
    )]);
    let previews = std::collections::HashMap::from([(
        "subagent_spawn".to_string(),
        "{name} · {model|model} · {effort}\n{prompt|firstline}".to_string(),
    )]);
    let args = serde_json::json!({
        "name": "e2e-theme",
        "model": "claude-haiku-5-5",
        "effort": "max",
        "prompt": "verify the theme fixes\nmore",
    });
    let display = crate::tool_fmt::ToolDisplay {
        labels: Some(&labels),
        previews: Some(&previews),
    };
    let rows = super::tool_header(display, "subagent_spawn", &args, None);
    let texts: Vec<String> = rows
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts[0].contains("Started agent e2e-theme · "), "{texts:?}");
    assert!(texts[0].ends_with(" · max"), "{texts:?}");
    assert!(texts[1].contains("verify the theme fixes"), "{texts:?}");
}
