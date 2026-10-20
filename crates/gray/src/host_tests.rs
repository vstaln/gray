use super::*;

#[tokio::test]
async fn background_rejects_headless_and_malformed_requests() {
    let handler = default_handler(PathBuf::from("/tmp"));
    for params in [
        serde_json::json!({}),
        serde_json::json!({"path": 42}),
        serde_json::json!({"path": null, "raw_escape": "bad"}),
    ] {
        let reply = handler("host/background".into(), params).await;
        assert!(reply.get("error").is_some(), "{reply}");
    }
    let reply = handler("host/background".into(), serde_json::json!({"path": null})).await;
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("interactive Gray TUI")
    );
}

fn drain_queued(entries: Vec<SayEntry>) -> Vec<Vec<Line<'static>>> {
    let mut q = SAY_QUEUE.lock().expect("say queue");
    q.clear();
    for e in entries {
        q.push(e);
    }
    drain_say_entries(std::mem::take(&mut *q))
}

fn line_text(lines: &[Line<'static>]) -> String {
    lines[0].spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn plugin_crash_notices_drain_as_one_coalesced_line() {
    let drained = drain_queued(vec![
        SayEntry::Crash(CrashNotice {
            plugin: "gray-gateway".into(),
            status: "exit 1".into(),
            restarted: true,
            error: None,
            repeats: 0,
        }),
        SayEntry::Crash(CrashNotice {
            plugin: "gray-mcp".into(),
            status: "exit 1".into(),
            restarted: true,
            error: None,
            repeats: 0,
        }),
    ]);
    assert_eq!(drained.len(), 1, "{drained:?}");
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway, mcp crashed (exit 1), restarted · sidecar-stderr.log"
    );
}

#[test]
fn plugin_crash_line_lists_differing_statuses_and_restart_failure() {
    let drained = drain_queued(vec![
        SayEntry::Crash(CrashNotice {
            plugin: "gray-gateway".into(),
            status: "exit 1".into(),
            restarted: true,
            error: None,
            repeats: 0,
        }),
        SayEntry::Crash(CrashNotice {
            plugin: "gray-mcp".into(),
            status: "signal 9".into(),
            restarted: true,
            error: None,
            repeats: 0,
        }),
    ]);
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway (exit 1), mcp (killed) crashed, restarted · sidecar-stderr.log"
    );

    let drained = drain_queued(vec![SayEntry::Crash(CrashNotice {
        plugin: "gray-gateway".into(),
        status: "exit 1".into(),
        restarted: false,
        error: Some("spawn failed".into()),
        repeats: 0,
    })]);
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway crashed (exit 1), restart failed: spawn failed · sidecar-stderr.log"
    );
}

fn crash(plugin: &str, status: &str) -> CrashNotice {
    CrashNotice {
        plugin: plugin.into(),
        status: status.into(),
        restarted: true,
        error: None,
        repeats: 0,
    }
}

fn queue_crashes(notices: Vec<CrashNotice>) -> Vec<Vec<Line<'static>>> {
    let mut tail = CRASH_TAIL.lock().expect("crash tail");
    let mut q = SAY_QUEUE.lock().expect("say queue");
    tail.take();
    q.clear();
    for n in notices {
        queue_crash_inner(&mut tail, &mut q, n);
    }
    drain_say_entries(std::mem::take(&mut *q))
}

#[test]
fn identical_crash_reports_fold_into_a_more_count() {
    let drained = queue_crashes(vec![
        crash("gray-gateway", "signal 15"),
        crash("gray-gateway", "signal 15"),
        crash("gray-gateway", "signal 15"),
        crash("gray-mcp", "signal 15"),
    ]);
    assert_eq!(drained.len(), 1, "{drained:?}");
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway, mcp crashed (killed), restarted (2 more) · sidecar-stderr.log"
    );
}

#[test]
fn suppressed_crashes_emit_at_the_cap_and_on_the_next_say() {
    let notices: Vec<CrashNotice> = (0..8).map(|_| crash("gray-gateway", "exit 1")).collect();
    let drained = queue_crashes(notices);
    assert_eq!(drained.len(), 1, "{drained:?}");
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway crashed (exit 1), restarted (5 more) · sidecar-stderr.log"
    );

    queue_say("back".to_string());
    let drained = take_host_say();
    assert_eq!(drained.len(), 2, "{drained:?}");
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway crashed (exit 1), restarted (2 more) · sidecar-stderr.log"
    );
    assert_eq!(line_text(&drained[1]), "  back");
}

#[test]
fn a_different_crash_breaks_the_fold() {
    let drained = queue_crashes(vec![
        crash("gray-gateway", "signal 9"),
        crash("gray-gateway", "signal 9"),
        crash("gray-mcp", "signal 9"),
        crash("gray-gateway", "exit 2"),
    ]);
    assert_eq!(drained.len(), 1, "{drained:?}");
    assert_eq!(
        line_text(&drained[0]),
        "  ! gateway (killed, 1 more), mcp (killed), gateway (exit 2) crashed, restarted · sidecar-stderr.log"
    );
}

#[test]
fn suppression_keeps_the_say_queue_below_drop_oldest_eviction() {
    let len = {
        let mut tail = CRASH_TAIL.lock().expect("crash tail");
        let mut q = SAY_QUEUE.lock().expect("say queue");
        tail.take();
        q.clear();
        for _ in 0..40 {
            queue_crash_inner(&mut tail, &mut q, crash("gray-gateway", "exit 1"));
        }
        q.len()
    };
    assert_eq!(len, 8);
}
