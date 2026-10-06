use super::*;

const NOW: i64 = 1_700_000_000;

fn origin(chat: &str) -> crate::cron::store::Origin {
    crate::cron::store::Origin {
        platform: crate::cron_serve::SESSION_PLATFORM.to_string(),
        chat: chat.to_string(),
        thread: None,
        route: None,
    }
}

fn cron(id: &str, next: Option<i64>, sid: Option<&str>) -> crate::cron::CronJob {
    crate::cron::CronJob {
        id: id.to_string(),
        name: format!("{id}-name"),
        prompt: "check the bench".to_string(),
        schedule: crate::cron::Schedule::Once {
            at: next.unwrap_or(NOW),
        },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: next,
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: if sid.is_some() {
            crate::cron::Deliver::Origin
        } else {
            crate::cron::Deliver::Local
        },
        origin: sid.map(origin),
        workdir: None,
        fire_claim: None,
        reminder: true,
        skills: vec![],
        script: None,
    }
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
    let mut paused = cron("paused", Some(NOW + 60), Some("s1"));
    paused.state = crate::cron::store::JobState::Paused;
    let mut disabled = cron("disabled", Some(NOW + 60), Some("s1"));
    disabled.enabled = false;
    let mut chat = cron("chat", Some(NOW + 60), Some("s1"));
    chat.origin.as_mut().unwrap().platform = "discord".into();
    let jobs = [
        cron("later", Some(NOW + 3600), Some("s1")),
        cron("soon", Some(NOW + 120), Some("s1")),
        cron("other-session", Some(NOW + 10), Some("s2")),
        cron("local", Some(NOW + 10), None),
        cron("unscheduled", None, Some("s1")),
        paused,
        disabled,
        chat,
    ];
    let ids: Vec<String> = session_wakes(&jobs, "s1")
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, ["soon", "later"]);
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

/// Answers the two calls `/jobs` makes; `execute` is never reached.
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
fn remove_stops_jobs_and_deletes_wakes() {
    let home = tempfile::tempdir().unwrap();
    let store = crate::cron::CronStore::open(home.path().join("cron")).unwrap();
    let id = store
        .add_full(
            "check",
            "in 30m",
            "check the bench",
            crate::cron::Deliver::Origin,
            Some(origin("s1")),
            None,
            vec![],
            None,
            true,
        )
        .unwrap();
    assert_eq!(load_wakes(home.path(), Some("s1")).len(), 1);
    assert!(load_wakes(home.path(), None).is_empty());

    let exec = FakeExec(std::sync::Mutex::new(vec!["cargo-test".into()]));
    let ctx = ToolContext::default();
    remove(&exec, &ctx, home.path(), "job:cargo-test").unwrap();
    let again = remove(&exec, &ctx, home.path(), "job:cargo-test").unwrap_err();
    assert!(again.to_string().contains("not running"), "{again}");

    remove(&exec, &ctx, home.path(), &format!("wake:{id}")).unwrap();
    assert!(load_wakes(home.path(), Some("s1")).is_empty());
    let gone = remove(&exec, &ctx, home.path(), &format!("wake:{id}")).unwrap_err();
    assert!(gone.to_string().contains("already fired"), "{gone}");
    assert!(remove(&exec, &ctx, home.path(), "plugin:x").is_err());
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
