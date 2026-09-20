use super::*;

#[tokio::test]
async fn append_continues_session_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let sid = save_session(&store, "m", dir.path(), &[Message::user("first")])
        .await
        .unwrap();
    let before = store.load(&sid).await.unwrap().1.len();
    // Prior history + one new turn: only the new message lands in the file.
    let with_new = vec![Message::user("first"), Message::user("second")];
    append_new_messages(&store, &sid, before, &with_new, false)
        .await
        .unwrap();
    let (_, entries) = store.load(&sid).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].message.text_content(), "second");
    // Same file, not a new session.
    assert_eq!(store.list().await.len(), 1);
}

#[tokio::test]
async fn append_bogus_session_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let err = append_new_messages(
        &store,
        &SessionId::new("bogus"),
        0,
        &[Message::user("x")],
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to append message to session"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn saved_sessions_redact_secrets_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let sid = save_session(
        &store,
        "m",
        dir.path(),
        &[
            Message::user(
                "read /Users/hunter/src/app/main.rs with ZAI_API_KEY=supersecretvalue12345",
            ),
            // Secret-free: persists verbatim (resume fidelity).
            Message::user("read crates/foo/src/main.rs"),
        ],
    )
    .await
    .unwrap();
    let (_, entries) = store.load(&sid).await.unwrap();
    let text = entries[0].message.text_content();
    assert!(!text.contains("/Users/hunter"), "{text}");
    assert!(!text.contains("supersecretvalue12345"), "{text}");
    assert_eq!(
        entries[1].message.text_content(),
        "read crates/foo/src/main.rs"
    );
}

#[test]
fn error_surfaces_are_scrubbed_before_display() {
    let scrubbed = scrub_error_text("auth failed: ZAI_API_KEY=supersecretvalue12345");
    assert!(!scrubbed.contains("supersecretvalue12345"), "{scrubbed}");
    assert!(scrubbed.contains("<redacted>"), "{scrubbed}");
}

#[test]
fn concurrent_tool_calls_track_by_id() {
    use gray_core::event::AgentEvent;
    let mut in_flight = HashMap::new();
    let mut out = Vec::new();
    let a = serde_json::json!({"x": 1});
    let b = serde_json::json!({"y": 2});
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_start("id1", "alpha"),
        None,
        &mut in_flight,
    )
    .unwrap();
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_start("id2", "beta"),
        None,
        &mut in_flight,
    )
    .unwrap();
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_end("id1", a.clone()),
        None,
        &mut in_flight,
    )
    .unwrap();
    // Second call's end must not clobber the first call's name/args.
    assert_eq!(in_flight["id1"].name, "alpha");
    assert_eq!(in_flight["id1"].args.as_ref(), Some(&a));
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_end("id2", b.clone()),
        None,
        &mut in_flight,
    )
    .unwrap();
    assert_eq!(in_flight["id2"].name, "beta");
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_result("id1", "ok-a", false),
        None,
        &mut in_flight,
    )
    .unwrap();
    // id2 survives id1's result (no single-slot take() wiping both).
    assert!(in_flight.contains_key("id2"));
    assert!(!in_flight.contains_key("id1"));
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_result("id2", "ok-b", false),
        None,
        &mut in_flight,
    )
    .unwrap();
    assert!(in_flight.is_empty());
}

#[test]
fn render_error_propagates_for_retry_policy() {
    use std::io::{Error, ErrorKind};
    struct Fail;
    impl std::io::Write for Fail {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(Error::new(ErrorKind::BrokenPipe, "closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(Error::new(ErrorKind::BrokenPipe, "closed"))
        }
    }
    let mut in_flight = HashMap::new();
    let err = render_event_with_context(
        &mut Fail,
        &gray_core::event::AgentEvent::text_delta("hi"),
        None,
        &mut in_flight,
    )
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn rewritten_history_replaces_session_even_after_growing_past_cursor() {
    for final_count in [1, 2, 3] {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let sid = save_session(
            &store,
            "m",
            dir.path(),
            &[
                Message::user("old prompt"),
                Message::assistant("old answer"),
            ],
        )
        .await
        .unwrap();
        let replacement: Vec<_> = (0..final_count)
            .map(|i| Message::user(format!("retained {i}")))
            .collect();
        append_new_messages(&store, &sid, 2, &replacement, true)
            .await
            .unwrap();
        let loaded: Vec<_> = store
            .load(&sid)
            .await
            .unwrap()
            .1
            .into_iter()
            .map(|entry| entry.message)
            .collect();
        assert_eq!(loaded, replacement, "final_count={final_count}");
    }
}

#[test]
fn provider_failures_classify_as_retryable_infra() {
    // The bench-class failure: a dropped stream must be distinguishable from
    // an agent-side stop so a harness re-runs the turn instead of giving up.
    // CoreError::Provider is the generic fallback (e.g. the turn-event cap),
    // not an infra class - real provider deaths keep their variant.
    for error in [
        gray_core::error::CoreError::Connection("dns".into()),
        gray_core::error::CoreError::Timeout("read".into()),
        gray_core::error::CoreError::ServerError("502".into()),
        gray_core::error::CoreError::Stream("eof".into()),
        gray_core::error::CoreError::RateLimited("slow down".into()),
    ] {
        let failure = PrintFailure::of(&anyhow::Error::new(error));
        assert!(failure.retryable, "{failure:?}");
        assert_eq!(failure.exit, EXIT_INFRA);
        assert!(failure.hint.is_some(), "{failure:?}");
    }
}

#[test]
fn config_failures_are_not_retryable_even_though_they_arrive_from_the_provider() {
    // The campaign's model-404: the endpoint answers "model does not exist"
    // as a 400-class bad request. Retrying that turn can never help, and the
    // hint must point at the configuration, not the network.
    let model_missing = PrintFailure::of(&anyhow::Error::new(
        gray_core::error::CoreError::BadRequest(
            "status 404 Not Found: model deepseek-v4.1-flash does not exist".into(),
        ),
    ));
    assert_eq!(model_missing.code, "bad_request");
    assert!(!model_missing.retryable);
    assert_eq!(model_missing.exit, EXIT_TURN_FAILED);
    assert!(
        model_missing.hint.unwrap().contains("/model"),
        "{model_missing:?}"
    );

    let auth = PrintFailure::of(&anyhow::Error::new(gray_core::error::CoreError::Auth(
        "401".into(),
    )));
    assert_eq!(auth.code, "auth_failed");
    assert!(!auth.retryable);
    assert!(auth.hint.unwrap().contains("gray setup"), "{auth:?}");
}

#[test]
fn agent_side_stops_are_not_retryable() {
    let looped = PrintFailure::of(&anyhow::Error::new(
        gray_core::error::CoreError::LoopDetected("same call 3x".into()),
    ));
    assert_eq!(looped.code, "loop_detected");
    assert!(!looped.retryable);
    assert_eq!(looped.exit, EXIT_TURN_FAILED);

    let cancelled = PrintFailure::of(&anyhow::Error::new(gray_core::error::CoreError::Cancelled));
    assert_eq!(cancelled.code, "cancelled");
    assert!(!cancelled.retryable);

    let unknown = PrintFailure::of(&anyhow::anyhow!("session store on fire"));
    assert_eq!(unknown.code, "turn_failed");
    assert!(!unknown.retryable);
    assert_eq!(unknown.exit, EXIT_TURN_FAILED);
    assert!(unknown.hint.is_none());
}

#[test]
fn failure_message_never_carries_provider_detail() {
    // The JSON record must stay scrubbed even when the underlying error text
    // quotes the provider's response body.
    let failure = PrintFailure::of(&anyhow::Error::new(gray_core::error::CoreError::Auth(
        "401 sk-live-secret".into(),
    )));
    let message = failure.message();
    assert!(!message.contains("sk-live-secret"), "{message}");
    let rendered = failure.to_string();
    assert!(!rendered.contains("sk-live-secret"), "{rendered}");
    assert!(rendered.contains("hint:"), "{rendered}");
    assert!(rendered.contains("(exit code"), "{rendered}");

    // A generic provider error keeps the turn-failure message and no hint.
    let generic = PrintFailure::of(&anyhow::Error::new(gray_core::error::CoreError::Provider(
        "401 sk-live-secret".into(),
    )));
    assert_eq!(generic.code, "provider_error");
    assert!(generic.hint.is_none());
}
