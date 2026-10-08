use super::*;

fn due() -> Inputs {
    Inputs {
        enabled: true,
        every_mins: 30,
        now: 10_000,
        minute_of_day: 12 * 60,
        active: Window::parse("08:00-22:00"),
        ..Default::default()
    }
}

#[test]
fn gates_refuse_in_order() {
    assert_eq!(verdict(&due()), Verdict::Go);
    let with = |f: fn(&mut Inputs)| {
        let mut i = due();
        f(&mut i);
        verdict(&i)
    };
    assert_eq!(with(|i| i.enabled = false), Verdict::Skip("disabled"));
    // Paused wins even over force.
    assert_eq!(
        with(|i| (i.paused, i.force) = (true, true)),
        Verdict::Skip("paused")
    );
    assert_eq!(with(|i| i.last_at = 10_000 - 60), Verdict::NotDue);
    assert_eq!(
        with(|i| i.minute_of_day = 23 * 60),
        Verdict::Skip("outside active hours")
    );
    // Force skips the schedule, not the lane or the checklist.
    assert_eq!(
        with(|i| (i.force, i.last_at, i.minute_of_day) = (true, 9_999, 3 * 60)),
        Verdict::Go
    );
    assert_eq!(
        with(|i| (i.force, i.main_busy) = (true, true)),
        Verdict::Skip("main session busy")
    );
    assert_eq!(
        with(|i| i.checklist_empty = true),
        Verdict::Skip("HEARTBEAT.md is empty")
    );
}

#[test]
fn only_real_lines_count_as_work() {
    assert!(
        effectively_empty(TEMPLATE),
        "a fresh install must not wake the model"
    );
    assert!(effectively_empty("# t\n\n<!-- a\nb -->\n  \n"));
    assert!(effectively_empty("# t\n<!-- never closed\n- item"));
    assert!(!effectively_empty("# t\n- check CI"));
    assert!(!effectively_empty("<!-- x --> check mail"));
}

#[test]
fn the_template_never_overwrites() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("HEARTBEAT.md"), "- mine").unwrap();
    ensure_template(home.path());
    assert_eq!(
        std::fs::read_to_string(home.path().join("HEARTBEAT.md")).unwrap(),
        "- mine"
    );
}

#[test]
fn the_prompt_carries_what_earlier_heartbeats_said() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let now = crate::cron::now_secs();
    let queued = |kind: &str, text: &str| {
        crate::gateway::activity::log(
            dir,
            "queued",
            serde_json::json!({"kind": kind, "text": text}),
        )
    };
    queued("heartbeat", "CI is red on main");
    queued("user", "a chat reply");
    let p = prompt(dir, dir, now);
    assert!(p.contains("- CI is red on main"), "{p}");
    assert!(!p.contains("a chat reply"), "{p}");
    assert!(!prompt(dir, dir, now + 2 * 86_400).contains("Already told"));
}
