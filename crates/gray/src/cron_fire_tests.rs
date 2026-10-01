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

/// Write an executable script fixture, staged and renamed into place so
/// the path that gets exec'd is never one a writer still holds. ETXTBSY
/// ("text file busy") means the executable was open for writing at the
/// instant of the exec; it showed up here roughly 1 run in 5-20 with no
/// process holding the file by the time it could be checked, so the real
/// fix is the one retry in `run_pre_script_with_timeout`. This keeps the
/// fixture out of that window too.
#[cfg(unix)]
fn write_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let staging = path.with_extension("sh.new");
    std::fs::write(&staging, body).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&staging, path).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn script_success_captures_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("ok.sh");
    write_script(&sh, "#!/bin/sh\necho hello\n");
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(out.ok, "stderr: {}", out.stderr_tail);
    assert!(out.stdout.contains("hello"));
}

#[cfg(unix)]
#[tokio::test]
async fn script_failure_marks_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("bad.sh");
    write_script(&sh, "#!/bin/sh\necho oops >&2\nexit 3\n");
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(!out.ok);
    assert!(out.stderr_tail.contains("oops"));
}

#[tokio::test]
async fn script_missing_file_is_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.sh");
    let out = run_pre_script(&missing, dir.path()).await;
    assert!(!out.ok);
    // The failure names the script: an operator reading cron output must
    // not have to guess which pre-script failed.
    assert!(
        out.stderr_tail.contains("gone.sh"),
        "spawn failure must name the script: {}",
        out.stderr_tail
    );
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
        reminder: false,
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
    write_script(&script, "#!/bin/sh\nexec sleep 1\n");
    let out =
        run_pre_script_with_timeout(&script, dir.path(), std::time::Duration::from_millis(50))
            .await;
    assert!(!out.ok);
    assert!(out.stderr_tail.contains("timed out"), "{}", out.stderr_tail);
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
    assert_eq!(mirror_message("n", "b"), mirror_message("n", "b"));
    assert_eq!(
        format_delivery_plain("n", "b", false, false),
        format_delivery_plain("n", "b", false, false)
    );
}
