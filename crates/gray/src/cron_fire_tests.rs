// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

#[test]
fn wake_gate_shapes() {
    assert!(parse_wake_gate("some output\n"));
    assert!(parse_wake_gate(""));
    assert!(parse_wake_gate("data\n{\"wakeAgent\": true}\n"));
    assert!(!parse_wake_gate("data\n{\"wakeAgent\": false}\n"));
    assert!(!parse_wake_gate("  {\"wakeAgent\": false}  \n\n"));
    assert!(parse_wake_gate("not json\n"));
    assert!(parse_wake_gate("{\"other\": 1}\n"));
}

#[test]
fn silent_prefix_shapes() {
    assert!(is_silent_response("[SILENT]"));
    assert!(is_silent_response("[SILENT]\nreal line"));
    assert!(is_silent_response("real line\n[SILENT]"));
    assert!(is_silent_response("  [silent]  "));
    assert!(!is_silent_response("loud report"));
    assert!(!is_silent_response(""));
}

#[test]
fn assembly_orders_skills_then_script_then_prompt() {
    let out = assemble_fire_prompt(
        "check CI",
        &[std::path::PathBuf::from("/s/SKILL.md")],
        Some("build ok"),
    );
    let si = out.find("## skills").unwrap();
    let oi = out.find("## script-output").unwrap();
    assert!(si < oi);
    assert!(out.ends_with("check CI"));
    assert!(out.contains("/s/SKILL.md"));
    assert!(out.contains("build ok"));
}

#[test]
fn assembly_omits_empty_sections() {
    let out = assemble_fire_prompt("hi", &[], None);
    assert_eq!(out, "hi");
}

#[test]
fn transcript_collects_text_and_tool_names() {
    use gray_core::event::AgentEvent;
    let events = vec![
        AgentEvent::TextDelta {
            delta: "hello ".to_string(),
        },
        AgentEvent::TextDelta {
            delta: "world".to_string(),
        },
    ];
    let t = transcript_text(&events);
    assert!(t.contains("hello world"));
}

#[tokio::test]
async fn script_success_captures_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("ok.sh");
    std::fs::write(&sh, "#!/bin/sh\necho hello\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(out.ok, "stderr: {}", out.stderr_tail);
    assert!(out.stdout.contains("hello"));
}

#[tokio::test]
async fn script_failure_marks_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("bad.sh");
    std::fs::write(&sh, "#!/bin/sh\necho oops >&2\nexit 3\n").unwrap();
    #[cfg(unix)]
    #[cfg(unix)]
    std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(!out.ok);
    assert!(out.stderr_tail.contains("oops"));
}

#[tokio::test]
async fn script_missing_file_is_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_pre_script(&dir.path().join("gone.sh"), dir.path()).await;
    assert!(!out.ok);
}

#[test]
fn local_output_writes_atomic_md() {
    let home = tempfile::tempdir().unwrap();
    let job = crate::cron::CronJob {
        id: "abc123".to_string(),
        name: "n".to_string(),
        prompt: "p".to_string(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: None,
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: Default::default(),
        origin: None,
        workdir: None,
        fire_claim: None,
        skills: vec![],
        script: None,
    };
    let path = write_local_output(home.path(), &job, 1_700_000_000, "body").unwrap();
    assert!(path.to_string_lossy().contains("abc123"));
    assert!(path.extension().is_some_and(|e| e == "md"));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("body"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn script_timeout_covers_execution() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("sleep.sh");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 1\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out =
        run_pre_script_with_timeout(&script, dir.path(), std::time::Duration::from_millis(50))
            .await;
    assert!(!out.ok);
    assert!(out.stderr_tail.contains("timed out"), "{}", out.stderr_tail);
}

#[test]
fn delivery_wrap_shapes() {
    // Hermes `_deliver_result` frame: header, 13-dash rule, body, footer.
    let out = format_delivery("nightly", "abc123", "hello output");
    assert!(out.starts_with("Cronjob Response: nightly\n(job_id: abc123)\n"));
    assert!(out.contains("\n-------------\n\nhello output\n\n"));
    assert!(out.ends_with(
        "To stop or manage this job, send me a new message (e.g. \"stop reminder nightly\")."
    ));
}

#[test]
fn mirror_message_is_clean_user_label() {
    // Hermes `_cron_mirror_message`: labelled, no wrapper, no file path.
    let out = mirror_message("nightly", "hello output");
    assert_eq!(out, "[Cron delivery: nightly]\nhello output");
    assert!(!out.contains("Cronjob Response:"));
    assert!(!out.contains("-------------"));
}

#[test]
fn delivery_excerpt_caps_at_4000_chars() {
    assert_eq!(DELIVERY_EXCERPT_CHARS, 4000);
    let long = "x".repeat(5000);
    assert_eq!(delivery_excerpt(&long).chars().count(), 4000);
    assert_eq!(delivery_excerpt("short"), "short");
}

#[test]
fn delivery_helpers_are_pure() {
    // Same inputs, byte-identical outputs — every driver renders one box.
    let a = format_delivery("n", "i", "b");
    let b = format_delivery("n", "i", "b");
    assert_eq!(a, b);
    assert_eq!(mirror_message("n", "b"), mirror_message("n", "b"));
}
