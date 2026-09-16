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
    append_new_messages(&store, &sid, before, &with_new)
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
    let err = append_new_messages(&store, &SessionId::new("bogus"), 0, &[Message::user("x")])
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
