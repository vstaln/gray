use super::*;

const NOW: i64 = 1_700_000_000;

fn cron_job(id: &str, next: Option<i64>, sid: Option<&str>) -> serde_json::Value {
    let deliver = if sid.is_some() { "origin" } else { "local" };
    let mut j = serde_json::json!({
        "id": id,
        "name": format!("{id}-name"),
        "enabled": true,
        "state": "active",
        "deliver": deliver,
        "next_run_at": next,
    });
    if let Some(s) = sid {
        j["origin"] = serde_json::json!({
            "platform": crate::session_mailbox::SESSION_PLATFORM,
            "chat": s,
        });
    }
    j
}

fn list_body(jobs: &[serde_json::Value]) -> Vec<u8> {
    serde_json::json!({ "jobs": jobs, "health": null })
        .to_string()
        .into_bytes()
}

fn bg(id: &str, secs: u64) -> BackgroundJob {
    BackgroundJob {
        id: id.to_string(),
        elapsed: Duration::from_secs(secs),
        stopping: false,
    }
}

#[test]
fn only_this_sessions_pending_wakes_count_soonest_first() {
    let mut paused = cron_job("paused", Some(NOW + 60), Some("s1"));
    paused["state"] = "paused".into();
    let mut disabled = cron_job("disabled", Some(NOW + 60), Some("s1"));
    disabled["enabled"] = false.into();
    let mut chat = cron_job("chat", Some(NOW + 60), Some("s1"));
    chat["origin"]["platform"] = "discord".into();
    let body = list_body(&[
        cron_job("later", Some(NOW + 3600), Some("s1")),
        cron_job("soon", Some(NOW + 120), Some("s1")),
        cron_job("other-session", Some(NOW + 10), Some("s2")),
        cron_job("local", Some(NOW + 10), None),
        cron_job("unscheduled", None, Some("s1")),
        paused,
        disabled,
        chat,
    ]);
    let home = tempfile::tempdir().unwrap();
    let ids: Vec<String> = load_wakes_with(home.path(), "s1", |_| Some(body.clone()))
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, ["soon", "later"]);
}

#[test]
fn a_missing_or_unreadable_gateway_lists_no_wakes() {
    let home = tempfile::tempdir().unwrap();
    assert!(load_wakes_with(home.path(), "s1", |_| None).is_empty());
    assert!(
        load_wakes_with(home.path(), "s1", |_| Some(b"not json".to_vec())).is_empty()
    );
}

#[test]
fn footer_label_shapes() {
    let wake = |id: &str, at: i64| Wake {
        id: id.into(),
        name: id.into(),
        at,
    };
    assert_eq!(footer_label(0, &[], NOW), None);
    assert_eq!(footer_label(1, &[], NOW).as_deref(), Some("1 job"));
    assert_eq!(
        footer_label(2, &[wake("a", NOW + 28 * 60)], NOW).as_deref(),
        Some("2 jobs \u{b7} wake in 28m")
    );
    assert_eq!(
        footer_label(0, &[wake("a", NOW + 45), wake("b", NOW + 7200)], NOW).as_deref(),
        Some("2 wakes \u{b7} next in 45s")
    );
    assert_eq!(
        footer_label(0, &[wake("a", NOW - 3)], NOW).as_deref(),
        Some("wake now")
    );
    assert_eq!(
        footer_label(0, &[wake("a", NOW + 2 * 86_400 + 3 * 3600)], NOW).as_deref(),
        Some("wake in 2d 3h")
    );
}

#[test]
fn picker_rows_tag_their_kind_and_lock_stopping_jobs() {
    let mut stopping = bg("bench", 5);
    stopping.stopping = true;
    let wakes = [Wake {
        id: "abcdef123456".into(),
        name: "check-bench".into(),
        at: NOW + 90,
    }];
    let rows = items(&[bg("cargo-test", 192), stopping], &wakes, NOW);
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["job:cargo-test", "job:bench", "wake:abcdef123456"]);
    assert!(
        rows[0].row.contains("cargo-test \u{2014} running 3m 12s"),
        "{}",
        rows[0].row
    );
    assert!(!rows[0].read_only);
    assert!(rows[1].read_only, "a stopping job has nothing left to stop");
    assert!(
        rows[2]
            .row
            .contains("check-bench (abcdef12) \u{2014} in 2m"),
        "{}",
        rows[2].row
    );
    assert!(dashboard(&[], &[], NOW).contains("nothing pending"));
}

struct FakeExec(std::sync::Mutex<Vec<String>>);

impl ToolExecutor for FakeExec {
    fn cancel_background(&self, _ctx: &ToolContext, id: &str) -> bool {
        let mut running = self.0.lock().unwrap();
        let before = running.len();
        running.retain(|j| j != id);
        running.len() != before
    }

    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
        unreachable!("the picker never runs tools")
    }
}

#[test]
fn remove_stops_jobs_and_asks_the_plugin_to_delete_wakes() {
    let home = tempfile::tempdir().unwrap();
    let exec = FakeExec(std::sync::Mutex::new(vec!["cargo-test".into()]));
    let ctx = ToolContext::default();
    remove_with(&exec, &ctx, home.path(), "job:cargo-test", |_, _| {
        unreachable!()
    })
    .unwrap();
    let again = remove_with(&exec, &ctx, home.path(), "job:cargo-test", |_, _| {
        unreachable!()
    })
    .unwrap_err();
    assert!(again.to_string().contains("not running"), "{again}");

    let asked = std::sync::Mutex::new(Vec::new());
    let rm = |_: &Path, id: &str| -> anyhow::Result<bool> {
        asked.lock().unwrap().push(id.to_string());
        Ok(true)
    };
    remove_with(&exec, &ctx, home.path(), "wake:abc123", rm).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), ["abc123"]);
    let gone = remove_with(&exec, &ctx, home.path(), "wake:abc123", |_, _| {
        Ok(false)
    })
    .unwrap_err();
    assert!(gone.to_string().contains("already fired"), "{gone}");
    assert!(
        remove_with(&exec, &ctx, home.path(), "plugin:x", |_, _| unreachable!()).is_err()
    );
}

#[test]
fn quit_warns_once_while_jobs_run() {
    assert_eq!(quit_warning(&[], false), None, "nothing running: just quit");
    let one = quit_warning(&[bg("cargo-test", 3)], false).unwrap();
    assert!(
        one.starts_with("1 background job is still running (cargo-test)"),
        "{one}"
    );
    assert!(one.contains("stop it and exit"), "{one}");
    let two = quit_warning(&[bg("a", 1), bg("b", 2)], false).unwrap();
    assert!(
        two.contains("2 background jobs are still running (a, b)"),
        "{two}"
    );
    assert_eq!(
        quit_warning(&[bg("a", 1)], true),
        None,
        "a standing warning means this quit is the confirmation"
    );
}
