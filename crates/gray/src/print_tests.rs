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
    let labels = HashMap::new();
    let mut out = Vec::new();
    let a = serde_json::json!({"x": 1});
    let b = serde_json::json!({"y": 2});
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_start("id1", "alpha"),
        None,
        &mut in_flight,
        &labels,
    )
    .unwrap();
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_start("id2", "beta"),
        None,
        &mut in_flight,
        &labels,
    )
    .unwrap();
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_call_end("id1", a.clone()),
        None,
        &mut in_flight,
        &labels,
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
        &labels,
    )
    .unwrap();
    assert_eq!(in_flight["id2"].name, "beta");
    render_event_with_context(
        &mut out,
        &AgentEvent::tool_result("id1", "ok-a", false),
        None,
        &mut in_flight,
        &labels,
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
        &labels,
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
    let labels = HashMap::new();
    let err = render_event_with_context(
        &mut Fail,
        &gray_core::event::AgentEvent::text_delta("hi"),
        None,
        &mut in_flight,
        &labels,
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

// ── progress narration rows (tool detail / reasoning on the --json wire) ──

fn json_out(show_reasoning: bool) -> JsonOutput {
    JsonOutput {
        turn_id: "t1".to_string(),
        session_id: None,
        text: String::new(),
        usage: Default::default(),
        meter: None,
        tools: HashMap::new(),
        thinking: String::new(),
        show_reasoning,
        stream_text: false,
        segment: TextSegment::default(),
        labels: HashMap::new(),
    }
}

#[test]
fn tool_rows_carry_a_display_name() {
    let mut out = json_out(false);
    out.labels
        .insert("discord_send_ui".into(), "Discord Send UI".into());
    let start = |out: &mut JsonOutput, name: &str| {
        out.rows(&AgentEvent::ToolCallStart {
            id: format!("id-{name}"),
            name: name.into(),
        })[0]["label"]
            .clone()
    };
    assert_eq!(start(&mut out, "discord_send"), "Discord Send", "humanized");
    assert_eq!(
        start(&mut out, "discord_send_ui"),
        "Discord Send UI",
        "plugin label wins"
    );
    assert_eq!(
        start(&mut out, "bash"),
        "bash",
        "a single token stays as it is"
    );
    let ran = out.rows(&AgentEvent::ToolCallEnd {
        id: "id-discord_send".into(),
        args: serde_json::json!({"content": "hi"}),
    });
    assert_eq!(ran[0]["label"], "Discord Send");
    let done = out.rows(&AgentEvent::ToolResult {
        id: "id-discord_send_ui".into(),
        output: "ok".into(),
        is_error: false,
    });
    assert_eq!(done[0]["label"], "Discord Send UI");
}

fn text_out() -> JsonOutput {
    JsonOutput {
        stream_text: true,
        ..json_out(false)
    }
}

/// What a consumer shows after replaying `rows`: concatenated deltas per
/// segment plus the latest tail.
fn replay(rows: &[serde_json::Value]) -> Vec<String> {
    let mut stable: Vec<String> = Vec::new();
    let mut tails: Vec<String> = Vec::new();
    for row in rows.iter().filter(|r| r["phase"] == "text") {
        let index = row["segment"].as_u64().unwrap() as usize;
        while stable.len() <= index {
            stable.push(String::new());
            tails.push(String::new());
        }
        stable[index].push_str(row["delta"].as_str().unwrap());
        tails[index] = row["tail"].as_str().unwrap().to_string();
    }
    stable.into_iter().zip(tails).map(|(s, t)| s + &t).collect()
}

fn delta(text: &str) -> AgentEvent {
    AgentEvent::TextDelta {
        delta: text.to_string(),
    }
}

#[test]
fn text_rows_are_off_unless_asked_for() {
    let mut out = json_out(false);
    assert!(out.rows(&delta("hello world")).is_empty());
    let rows = out.rows(&AgentEvent::TurnEnd {
        stop_reason: gray_core::event::StopReason::EndTurn,
        usage: Default::default(),
    });
    assert!(rows.iter().all(|r| r["phase"] != "text"), "{rows:?}");
}

#[test]
fn text_streams_at_once_and_holds_the_word_being_typed() {
    let mut out = text_out();
    let rows = out.rows(&delta("Hello wor"));
    assert_eq!(
        rows.len(),
        1,
        "first fragment of a segment goes out at once"
    );
    assert_eq!(rows[0]["phase"], "text");
    assert_eq!(rows[0]["segment"], 0);
    assert_eq!(rows[0]["done"], false);
    assert_eq!(rows[0]["tail"], "Hello", "the half-typed word is held");
}

#[test]
fn text_segments_close_at_tool_calls_and_carry_everything() {
    let mut out = text_out();
    let mut rows = Vec::new();
    for piece in ["Let me ", "check.\nOne ", "moment"] {
        rows.extend(out.rows(&delta(piece)));
    }
    rows.extend(out.rows(&AgentEvent::ToolCallStart {
        id: "c1".into(),
        name: "bash".into(),
    }));
    let closing = rows.iter().rposition(|r| r["phase"] == "text").unwrap();
    assert_eq!(rows[closing]["done"], true);
    assert_eq!(rows[closing + 1]["phase"], "tool_started");
    for piece in ["All ", "good."] {
        rows.extend(out.rows(&delta(piece)));
    }
    rows.extend(out.rows(&AgentEvent::TurnEnd {
        stop_reason: gray_core::event::StopReason::EndTurn,
        usage: Default::default(),
    }));
    assert_eq!(
        replay(&rows),
        vec![
            "Let me check.\nOne moment".to_string(),
            "All good.".to_string()
        ]
    );
    let last = rows.iter().rposition(|r| r["phase"] == "text").unwrap();
    assert_eq!(rows[last]["segment"], 1);
    assert_eq!(rows[last]["done"], true);
}

#[test]
fn text_rows_redact_secrets_and_never_show_them_half_typed() {
    let mut out = text_out();
    let mut rows = Vec::new();
    for piece in ["key: sk-abc1", "23SECRETxyz789 ok"] {
        rows.extend(out.rows(&delta(piece)));
        // Defeat the frame gap so every fragment is observable.
        out.segment.last_emit = None;
    }
    rows.extend(out.take_text(true));
    let wire = serde_json::to_string(&rows).unwrap();
    assert!(!wire.contains("sk-abc1"), "leaked: {wire}");
    assert!(replay(&rows)[0].starts_with("key: "), "{rows:?}");
}

#[test]
fn progress_narrates_a_bash_call_with_its_command() {
    let mut out = json_out(true);
    out.tools.insert("c1".into(), "bash".into());
    let rows = out.rows(&AgentEvent::ToolCallEnd {
        id: "c1".into(),
        args: serde_json::json!({"command": "cargo test -p gray"}),
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["phase"], "tool_ran");
    assert_eq!(rows[0]["tool"], "bash");
    assert_eq!(rows[0]["call_id"], "c1");
    assert_eq!(rows[0]["detail"], "cargo test -p gray");
}

#[test]
fn progress_narrates_start_and_result_with_the_tool_name() {
    let mut out = json_out(true);
    let started = out.rows(&AgentEvent::ToolCallStart {
        id: "c1".into(),
        name: "bash".into(),
    });
    assert_eq!(started[0]["phase"], "tool_started");
    assert_eq!(started[0]["tool"], "bash");
    assert_eq!(started[0]["call_id"], "c1");
    let done = out.rows(&AgentEvent::ToolResult {
        id: "c1".into(),
        output: "ok".into(),
        is_error: false,
    });
    assert_eq!(done[0]["phase"], "tool_finished");
    assert_eq!(done[0]["tool"], "bash");
    assert_eq!(done[0]["call_id"], "c1");
    assert_eq!(done[0]["output"], "ok");
    assert!(done[0].get("error").is_none());
}

#[test]
fn progress_output_preserves_lines_redacts_and_caps() {
    let mut out = json_out(false);
    out.tools.insert("c1".into(), "bash".into());
    let rows = out.rows(&AgentEvent::ToolResult {
        id: "c1".into(),
        output: format!(
            "line one\nAuthorization: Bearer sk-abc123SECRET\n{}",
            "x ".repeat(OUTPUT_CAP * 2)
        ),
        is_error: false,
    });
    let output = rows[0]["output"].as_str().unwrap();
    assert!(output.contains("line one\n"), "{output}");
    assert!(
        !output.contains("sk-abc123SECRET"),
        "leaked output: {output}"
    );
    assert!(output.contains("<redacted>"), "{output}");
    assert!(output.chars().count() <= OUTPUT_CAP + 1, "{}", output.len());
    assert!(output.ends_with('…'));
}

#[test]
fn progress_marks_a_failed_tool() {
    let mut out = json_out(true);
    out.tools.insert("c1".into(), "bash".into());
    let rows = out.rows(&AgentEvent::ToolResult {
        id: "c1".into(),
        output: "boom".into(),
        is_error: true,
    });
    assert_eq!(rows[0]["error"], true);
}

#[test]
fn progress_read_detail_is_a_one_based_line_range() {
    let detail = tool_detail(
        "read",
        &serde_json::json!({"path": "config.yaml", "offset": 110, "limit": 30}),
    );
    assert_eq!(detail.as_deref(), Some("config.yaml L110-139"));
}

#[test]
fn progress_read_detail_is_just_the_path_for_tail_and_zero() {
    assert_eq!(
        tool_detail("read", &serde_json::json!({"path": "f"})).as_deref(),
        Some("f")
    );
    assert_eq!(
        tool_detail(
            "read",
            &serde_json::json!({"path": "f", "offset": -20, "limit": 20})
        )
        .as_deref(),
        Some("f")
    );
    assert_eq!(
        tool_detail(
            "read",
            &serde_json::json!({"path": "f", "offset": 5, "limit": 0})
        )
        .as_deref(),
        Some("f")
    );
}

#[test]
fn progress_detail_redacts_secrets_from_commands() {
    let detail = tool_detail(
        "bash",
        &serde_json::json!({"command": "curl -H 'Authorization: Bearer sk-abc123SECRET' https://x"}),
    )
    .unwrap();
    assert!(!detail.contains("sk-abc123SECRET"), "leaked: {detail}");
    assert!(detail.contains("<redacted>"), "{detail}");
}

#[test]
fn progress_detail_preserves_secret_free_paths() {
    // The --json wire feeds owner-local surfaces (Discord narration); a path
    // with no secret in it is the whole point, not a leak.
    let detail = tool_detail("bash", &serde_json::json!({"command": "cat /tmp/shot.png"})).unwrap();
    assert!(detail.contains("/tmp/shot.png"), "{detail}");
    assert!(!detail.contains("<path>"), "{detail}");
    let read = tool_detail("read", &serde_json::json!({"path": "/home/u/notes.md"})).unwrap();
    assert!(read.contains("/home/u/notes.md"), "{read}");
}

#[test]
fn progress_output_preserves_secret_free_paths() {
    let mut out = json_out(true);
    out.tools.insert("c1".into(), "bash".into());
    let rows = out.rows(&AgentEvent::ToolResult {
        id: "c1".into(),
        output: "Image shown: /tmp/shot.png".into(),
        is_error: false,
    });
    let output = rows[0]["output"].as_str().unwrap();
    assert!(output.contains("/tmp/shot.png"), "{output}");
    assert!(!output.contains("<path>"), "{output}");
}

#[test]
fn progress_detail_for_an_unknown_tool_drops_the_args() {
    // A value we do not understand is exactly where a token hides.
    assert!(
        tool_detail(
            "some_plugin_tool",
            &serde_json::json!({"query": "sk-secret"})
        )
        .is_none()
    );
}

#[test]
fn progress_detail_is_capped() {
    let long = "x".repeat(DETAIL_CAP * 2);
    let detail = tool_detail("bash", &serde_json::json!({"command": long})).unwrap();
    assert!(
        detail.chars().count() <= DETAIL_CAP + 1,
        "{}",
        detail.chars().count()
    );
    assert!(detail.ends_with('…'));
}

#[test]
fn progress_batches_reasoning_instead_of_one_row_per_token() {
    let mut out = json_out(true);
    for _ in 0..10 {
        assert!(
            out.rows(&AgentEvent::ThinkingDelta { delta: "a".into() })
                .is_empty()
        );
    }
    let flushed = out.rows(&AgentEvent::ThinkingDelta {
        delta: "b".repeat(THINKING_FLUSH),
    });
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0]["phase"], "thinking");
    // Turn end flushes whatever reasoning is left.
    let end = out.rows(&AgentEvent::TurnEnd {
        stop_reason: gray_core::event::StopReason::EndTurn,
        usage: Default::default(),
    });
    assert_eq!(end.last().unwrap()["phase"], "persisting");
    assert_eq!(end.len(), 1, "empty buffer must not emit a thinking row");
}

#[test]
fn progress_flushes_reasoning_before_persisting() {
    let mut out = json_out(true);
    out.rows(&AgentEvent::ThinkingDelta {
        delta: "why".into(),
    });
    let end = out.rows(&AgentEvent::TurnEnd {
        stop_reason: gray_core::event::StopReason::EndTurn,
        usage: Default::default(),
    });
    assert_eq!(end[0]["phase"], "thinking");
    assert_eq!(end[0]["detail"], "why");
    assert_eq!(end[1]["phase"], "persisting");
}

#[test]
fn progress_reasoning_is_gated_by_show_reasoning() {
    let mut out = json_out(false);
    for _ in 0..100 {
        out.rows(&AgentEvent::ThinkingDelta {
            delta: "secret thoughts".into(),
        });
    }
    assert!(
        out.rows(&AgentEvent::TurnEnd {
            stop_reason: gray_core::event::StopReason::EndTurn,
            usage: Default::default()
        })
        .iter()
        .all(|r| r["phase"] != "thinking")
    );
}
