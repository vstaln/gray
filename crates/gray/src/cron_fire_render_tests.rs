use super::*;
use gray_core::redaction::REDACTED;

#[test]
fn final_text_keeps_only_text_after_the_last_tool_boundary() {
    let pieces = vec![
        Piece::Text("Let me check. "),
        Piece::Boundary,
        Piece::Text("Now "),
        Piece::Boundary,
        Piece::Text("  Time to clean.  "),
    ];
    assert_eq!(final_text_of(pieces.into_iter()), "Time to clean.");
}

#[test]
fn final_text_is_empty_when_the_turn_ends_on_a_tool() {
    let pieces = vec![Piece::Text("working"), Piece::Boundary];
    assert_eq!(final_text_of(pieces.into_iter()), "");
}

#[test]
fn final_assistant_text_reads_the_event_stream() {
    use gray_core::event::AgentEvent;
    let events = vec![
        AgentEvent::TextDelta {
            delta: "let me look. ".into(),
        },
        AgentEvent::ToolCallStart {
            id: "1".into(),
            name: "bash".into(),
        },
        AgentEvent::ToolResult {
            id: "1".into(),
            output: "<untrusted-output>\nls\n</untrusted-output>".into(),
            is_error: false,
        },
        AgentEvent::ThinkingDelta {
            delta: "private".into(),
        },
        AgentEvent::TextDelta {
            delta: "Time to clean.".into(),
        },
    ];
    assert_eq!(final_assistant_text(&events), "Time to clean.");
}

#[test]
fn final_assistant_text_starts_over_at_a_message_boundary() {
    use gray_core::event::AgentEvent;
    // A mid-turn note ends the streamed message; what follows is a new
    // message, and a delivery carries only that final one.
    let events = vec![
        AgentEvent::TextDelta {
            delta: "Time to eat.".into(),
        },
        AgentEvent::MessageBoundary,
        AgentEvent::TextDelta {
            delta: "Task complete.".into(),
        },
    ];
    assert_eq!(final_assistant_text(&events), "Task complete.");
}

#[test]
fn transcript_separates_messages_at_a_boundary() {
    use gray_core::event::AgentEvent;
    let events = vec![
        AgentEvent::TextDelta {
            delta: "Time to eat.".into(),
        },
        AgentEvent::MessageBoundary,
        AgentEvent::TextDelta {
            delta: "Task complete.".into(),
        },
    ];
    assert_eq!(transcript_text(&events), "Time to eat.\n\nTask complete.");
}

#[test]
fn untrusted_blocks_are_balanced_after_a_length_cap() {
    let cut = close_untrusted("<untrusted-output>\nabc".to_string());
    assert!(cut.ends_with("</untrusted-output>"));
    let ok = close_untrusted("<untrusted-output>a</untrusted-output>".to_string());
    assert_eq!(ok.matches("untrusted-output>").count(), 2);
    let us = close_untrusted("<untrusted_output>\nabc".to_string());
    assert!(us.ends_with("</untrusted_output>"));
}

#[test]
fn plain_format_has_no_frame_id_dashes_or_path() {
    let s = format_delivery_plain("nightly", "all clear", false, false);
    assert_eq!(s, "nightly\n\nall clear");
    let f = format_delivery_plain("nightly", "boom", false, true);
    assert_eq!(f, "nightly failed\n\nboom");
    for bad in [
        "job_id",
        "-----",
        "Cronjob Response",
        "Full output",
        "To stop or manage",
    ] {
        assert!(!s.contains(bad) && !f.contains(bad), "leaked {bad}");
    }
}

#[test]
fn reminder_plain_text_is_byte_for_byte() {
    let text = "  clean my roo \n\n\n:done: https://x.y ";
    assert_eq!(format_delivery_plain("n", text, true, false), text);
}

#[test]
fn reminder_name_is_a_slug_of_the_exact_text() {
    assert_eq!(reminder_name("clean my roo"), "clean-my-roo"); // typo kept
    assert_eq!(reminder_name("  Call MOM!! "), "call-mom");
    assert_eq!(reminder_name("!!!"), "reminder");
    let long = reminder_name(&"word ".repeat(30));
    assert!(long.chars().count() <= 40 && !long.ends_with('-'));
}

#[test]
fn redacts_auth_json_values_and_token_shapes() {
    let home = tempfile::tempdir().unwrap();
    let bot = format!("{}.{}.{}", "B".repeat(24), "I".repeat(10), "c".repeat(27));
    let key = format!("sk-{}", "a1".repeat(12));
    std::fs::write(
        home.path().join("auth.json"),
        serde_json::json!({ "discord": { "token": bot } }).to_string(),
    )
    .unwrap();
    let out = redact_secrets(&format!("bot {bot} and {key} ok"), home.path());
    assert!(!out.contains(&bot), "an auth.json value survived");
    assert!(!out.contains(&key), "a key-shaped token survived");
    assert!(out.contains(REDACTED) && out.contains("ok"), "{out}");
}

#[test]
fn redacts_secret_named_assignments_but_not_ordinary_lines() {
    let home = tempfile::tempdir().unwrap();
    let out = redact_secrets("\"api_key\": \"0123456789abcdef\"", home.path());
    assert!(!out.contains("0123456789abcdef") && out.contains(REDACTED));
    let plain = "total 16\ndrwxr-xr-x 2 u u 4096 supervise\ntoken budget: 5";
    assert_eq!(redact_secrets(plain, home.path()), plain);
}

#[test]
fn a_secret_free_transcript_keeps_its_paths_verbatim() {
    // The transcript is a tool log: paths are the point of it, so they only
    // go when a secret fired alongside them.
    let home = tempfile::tempdir().unwrap();
    let t = "[tool:bash]\n[result:exit 0]\n<untrusted-output>\ndrwxr-xr-x 2 u u 4096 /home/u/roo\n</untrusted-output>\nTime to clean.";
    assert_eq!(redact_secrets(t, home.path()), t);
}
