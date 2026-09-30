// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

struct StubRunner {
    text: String,
    fail: bool,
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait(?Send)]
impl AsyncRunner for StubRunner {
    async fn run(&self, prompt: String, _cwd: PathBuf) -> anyhow::Result<String> {
        self.seen.lock().unwrap().push(prompt);
        if self.fail {
            anyhow::bail!("boom")
        } else {
            Ok(self.text.clone())
        }
    }
}

/// A runner that produced the same text for the transcript and the answer.
fn out(text: &str) -> FireOutput {
    FireOutput {
        transcript: text.to_string(),
        final_text: text.to_string(),
    }
}

fn due_store(home: &tempfile::TempDir, records: serde_json::Value) -> crate::cron::CronStore {
    let store = crate::cron::CronStore::open(home.path().join("cron")).unwrap();
    std::fs::write(
        home.path().join("cron").join("jobs.json"),
        serde_json::to_string_pretty(&records).unwrap(),
    )
    .unwrap();
    store
}

fn one_due(id: &str, deliver: serde_json::Value) -> serde_json::Value {
    serde_json::json!([{
        "id": id, "name": id, "prompt": "say hi",
        "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
        "created_at": 1, "next_run_at": 1, "deliver": deliver,
    }])
}

fn test_config() -> crate::config::Config {
    crate::config::Config {
        temperature: None,
        top_p: None,
        model: Some("startup-model".to_string()),
        base_url: "https://startup.example/v1".to_string(),
        api_key: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    }
}

#[test]
fn fire_follows_saved_model_switches() {
    // A /model switch persists base_url+model; a later fire must use the
    // switch, not the runner's startup snapshot. Pure temp path, no env, so
    // parallel tests cannot interfere.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"base_url": "https://api.commandcode.ai/provider/v1", "model": "switched-model"}"#,
    )
    .unwrap();
    let mut cfg = test_config();
    refresh_model_from_saved_at(&mut cfg, &path);
    assert_eq!(cfg.model.as_deref(), Some("switched-model"));
    assert_eq!(cfg.base_url, "https://api.commandcode.ai/provider/v1");
}

#[test]
fn fire_keeps_snapshot_without_saved_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    refresh_model_from_saved_at(&mut cfg, &dir.path().join("missing.json"));
    assert_eq!(cfg.model.as_deref(), Some("startup-model"));
    assert_eq!(cfg.base_url, "https://startup.example/v1");
}

#[tokio::test]
async fn tick_fires_due_job_and_marks_ok() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(&home, one_due("j1", serde_json::json!("local")));
    let runner = StubRunner {
        text: "hello".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 1);
    assert_eq!(rep.errors, 0);
    let job = store.get("j1").unwrap().unwrap();
    assert_eq!(job.last_status, Some(crate::cron::RunStatus::Ok));
    assert!(job.fire_claim.is_none());
    assert_eq!(runner.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn tick_agent_failure_records_error_and_continues() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(
        &home,
        serde_json::json!([
            {"id": "a", "name": "a", "prompt": "x",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
            {"id": "b", "name": "b", "prompt": "y",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
        ]),
    );
    let runner = StubRunner {
        text: String::new(),
        fail: true,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 2);
    assert_eq!(rep.errors, 2);
    for id in ["a", "b"] {
        let job = store.get(id).unwrap().unwrap();
        assert_eq!(job.last_status, Some(crate::cron::RunStatus::Error));
        assert!(job.fire_claim.is_none());
    }
}

#[tokio::test]
async fn tick_fires_two_due_jobs_and_keeps_claim_order() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(
        &home,
        serde_json::json!([
            {"id": "a", "name": "a", "prompt": "x",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
            {"id": "b", "name": "b", "prompt": "y",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
        ]),
    );
    let runner = StubRunner {
        text: "hi".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 2);
    assert_eq!(rep.errors, 0);
    assert_eq!(rep.delivered.len(), 2);
    assert_eq!(rep.delivered[0].id, "a");
    assert_eq!(rep.delivered[1].id, "b");
    for id in ["a", "b"] {
        let job = store.get(id).unwrap().unwrap();
        assert_eq!(job.last_status, Some(crate::cron::RunStatus::Ok));
        assert!(job.fire_claim.is_none());
    }
}

#[tokio::test]
async fn tick_silent_response_skips_write_but_ok() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(&home, one_due("s1", serde_json::json!("local")));
    let runner = StubRunner {
        text: "nothing to report\n[SILENT]".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!((rep.fired, rep.errors), (1, 0));
    let job = store.get("s1").unwrap().unwrap();
    assert_eq!(job.last_status, Some(crate::cron::RunStatus::Ok));
    assert!(!home.path().join("cron").join("output").join("s1").exists());
}

#[test]
fn claim_outlives_max_fire_time() {
    let max_fire =
        (crate::cron_serve::FIRE_TIMEOUT_SECS + crate::cron_fire::SCRIPT_TIMEOUT_SECS) as i64;
    assert!(
        crate::cron::store::FIRE_CLAIM_TTL_SECS > max_fire,
        "a claim must outlive the longest supported fire (script + agent)"
    );
}

#[tokio::test]
async fn origin_delivery_appends_to_session_and_keeps_file_on_failure() {
    use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta};
    let home = tempfile::tempdir().unwrap();
    let sessions = JsonlSessionStore::new(home.path().join("sessions"));
    let chat = "origin-chat-1";
    sessions
        .create(SessionMeta::new(
            SessionId::new(chat),
            1_700_000_000_000,
            home.path(),
            "test",
        ))
        .await
        .unwrap();
    let mk_job = |id: &str, chat: &str| crate::cron::CronJob {
        id: id.to_string(),
        name: "nightly".to_string(),
        prompt: "p".to_string(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: Some(1),
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: crate::cron::Deliver::Origin,
        origin: Some(crate::cron::store::Origin {
            platform: "local".to_string(),
            chat: chat.to_string(),
            thread: None,
            route: None,
        }),
        workdir: None,
        fire_claim: None,
        reminder: false,
        skills: vec![],
        script: None,
    };
    let deliver = SaveLocalDeliver {
        home: home.path().to_path_buf(),
    };
    // Happy path: session append + file save. The mirror is the clean
    // excerpt (no wrapper, no file path) as a USER turn.
    let saved = deliver
        .deliver_full(
            &mk_job("j-origin", chat),
            1_700_000_001,
            &out("hello output"),
            0,
        )
        .await
        .unwrap();
    assert!(saved.to_chat);
    assert_eq!(saved.id, "j-origin");
    assert_eq!(saved.excerpt, "hello output");
    let file_text = std::fs::read_to_string(
        home.path()
            .join("cron")
            .join("output")
            .join("j-origin")
            .join("1700000001.md"),
    )
    .unwrap();
    assert!(file_text.contains("hello output"));
    let session_text =
        std::fs::read_to_string(home.path().join("sessions").join(format!("{chat}.jsonl")))
            .unwrap();
    assert!(
        session_text.contains("[Cron delivery: nightly]"),
        "origin session missing hermes mirror label"
    );
    assert!(session_text.contains("hello output"));
    assert!(
        !session_text.contains("Cronjob Response:"),
        "mirror must not carry the chat wrapper"
    );
    assert!(
        !session_text.contains("1700000001.md"),
        "mirror must not carry the file path"
    );
    assert!(
        session_text.contains("\"role\":\"user\""),
        "mirror must be a USER turn, never assistant"
    );
    // No session for the chat id: the mirror is skipped, NOT a delivery
    // failure. A host whose chat id has no session yet (a job added in a
    // conversation's first turn) must still receive the message; only the
    // in-context mirror is lost.
    let saved = deliver
        .deliver_full(
            &mk_job("j-bad", "no-such-session"),
            1_700_000_002,
            &out("kept output"),
            0,
        )
        .await
        .expect("a missing session must not swallow the delivery");
    assert!(saved.to_chat, "the route still gets the message");
    let kept = std::fs::read_to_string(
        home.path()
            .join("cron")
            .join("output")
            .join("j-bad")
            .join("1700000002.md"),
    )
    .unwrap();
    assert!(kept.contains("kept output"));
}

#[tokio::test]
async fn cron_runner_receives_job_workdir() {
    struct CwdRunner(PathBuf);
    #[async_trait::async_trait(?Send)]
    impl AsyncRunner for CwdRunner {
        async fn run(&self, _prompt: String, cwd: PathBuf) -> anyhow::Result<String> {
            assert_eq!(cwd, self.0);
            Ok("ok".into())
        }
    }
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let mut record = one_due("wd1", serde_json::json!("local"));
    record[0]["workdir"] = serde_json::json!(workdir.path());
    let store = due_store(&home, record);
    let report = tick_once(
        &store,
        &CwdRunner(workdir.path().to_path_buf()),
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(report.errors, 0);
}

#[tokio::test]
async fn local_and_target_jobs_are_save_only() {
    // Local, unknown target, and session-less origin all save only. One
    // temp home per job: each `due_store` writes a fresh `jobs.json`, so a
    // shared home would leave only the last record behind.
    for (id, d) in [
        ("l1", serde_json::json!("local")),
        ("t1", serde_json::json!({"target": "somewhere"})),
        ("o1", serde_json::json!("origin")),
    ] {
        let home = tempfile::tempdir().unwrap();
        let deliver = SaveLocalDeliver {
            home: home.path().to_path_buf(),
        };
        let mut v = one_due(id, d);
        v[0]["name"] = serde_json::json!("n");
        let store = due_store(&home, v);
        let job = store.list().unwrap().into_iter().next().unwrap();
        assert_eq!(job.id, id, "store lost {id}");
        let saved = deliver
            .deliver_full(&job, 1_700_000_001, &out("body"), 0)
            .await
            .unwrap();
        assert!(!saved.to_chat, "{id} must be save-only");
        assert!(
            home.path()
                .join("cron")
                .join("output")
                .join(id)
                .join("1700000001.md")
                .exists()
        );
    }
}

#[test]
fn fire_chat_is_plain_text_with_no_frame_id_or_path() {
    let saved = DeliveredFire {
        id: "abc123".to_string(),
        name: "nightly".to_string(),
        path: std::path::PathBuf::from("/home/u/.gray/cron/output/abc123/1.md"),
        excerpt: "hello output".to_string(),
        to_chat: true,
        reminder: false,
        failed: false,
        elapsed_ms: 0,
    };
    let out = format_fire_chat(&saved);
    assert_eq!(out, "nightly\n\nhello output");
    for bad in [
        "abc123",
        "job_id",
        "-----",
        "Cronjob Response",
        "Full output",
        "/home/u",
    ] {
        assert!(!out.contains(bad), "plain fallback leaked {bad}");
    }
}

#[tokio::test]
async fn tick_fires_nothing_while_the_switch_is_off_but_keeps_ticking() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(&home, one_due("j1", serde_json::json!("local")));
    let runner = StubRunner {
        text: "hello".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once_with(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
        false,
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 0);
    assert_eq!(rep.errors, 0);
    assert!(rep.delivered.is_empty());
    // The job was never claimed, so it stays due for the flip back on.
    let job = store.get("j1").unwrap().unwrap();
    assert!(job.fire_claim.is_none());
    assert_eq!(job.last_status, None);
    assert!(runner.seen.lock().unwrap().is_empty());
    // The heartbeat stamped anyway — liveness stays truthful, the ticker is
    // alive, it just fired nothing.
    let now = crate::cron::now_secs();
    let health = store.health(now).unwrap();
    assert!(health.ticker_live(now));
    // And with the switch on, the very same store fires normally.
    let rep = tick_once_with(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
        true,
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 1);
}

// ── chat delivery: the line a platform host routes ──

fn delivered() -> DeliveredFire {
    DeliveredFire {
        id: "job1".into(),
        name: "daily check".into(),
        path: std::path::PathBuf::from("/tmp/out.md"),
        excerpt: "all clear".into(),
        to_chat: true,
        reminder: false,
        failed: false,
        elapsed_ms: 0,
    }
}

#[test]
fn delivery_json_carries_the_route_and_the_hermes_frame() {
    let origin = crate::cron::store::Origin {
        platform: "discord".into(),
        chat: "chat:one".into(),
        thread: None,
        route: Some("1234567890".into()),
    };
    let v: serde_json::Value = serde_json::from_str(&crate::cron_serve::delivery_json(
        &delivered(),
        Some(&origin),
    ))
    .unwrap();
    assert_eq!(v["type"], "cron_delivery");
    assert_eq!(v["job_id"], "job1");
    assert_eq!(v["platform"], "discord");
    assert_eq!(v["chat"], "chat:one");
    assert_eq!(v["route"], "1234567890");
    // Structured fields for a native renderer; `text` is only the plain
    // fallback and carries no job id, dashes, footer or path.
    assert_eq!(v["kind"], "task");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["final_text"], "all clear");
    assert_eq!(v["path"], "/tmp/out.md");
    assert_eq!(v["text"].as_str().unwrap(), "daily check\n\nall clear");
}

#[test]
fn delivery_json_without_an_origin_still_renders() {
    let v: serde_json::Value =
        serde_json::from_str(&crate::cron_serve::delivery_json(&delivered(), None)).unwrap();
    assert_eq!(v["route"], serde_json::Value::Null);
    assert!(v["text"].as_str().unwrap().contains("all clear"));
}

#[tokio::test]
async fn a_platform_chat_id_gets_its_own_transcript() {
    // A host passes the real session id when it knows one (so a reply
    // continues in context); otherwise the chat id names the transcript.
    // Either way the delivery is a chat delivery, never local-only.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let store_dir = home.join("sessions");
    {
        use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta};
        JsonlSessionStore::new(store_dir.clone())
            .create(SessionMeta::new(
                SessionId::new("4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c"),
                1_700_000_000_000,
                tmp.path(),
                "test",
            ))
            .await
            .unwrap();
    }
    let job = crate::cron::CronJob {
        id: "job1".into(),
        name: "nightly".into(),
        prompt: "p".into(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: Some(1),
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: crate::cron::Deliver::Origin,
        origin: Some(crate::cron::store::Origin {
            platform: "discord".into(),
            // A host passes a session-safe id: the live session uuid when it
            // knows one, else its conversation key. Anything the session
            // store would reject cannot be mirrored.
            chat: "4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c".into(),
            thread: None,
            route: Some("42".into()),
        }),
        workdir: None,
        fire_claim: None,
        reminder: false,
        skills: vec![],
        script: None,
    };
    let deliver = SaveLocalDeliver { home: home.clone() };
    let saved = deliver
        .deliver_full(&job, 1, &out("hello"), 0)
        .await
        .unwrap();
    assert!(saved.to_chat, "a routed delivery is still a chat delivery");
    let transcript =
        std::fs::read_to_string(store_dir.join("4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c.jsonl")).unwrap();
    assert!(
        transcript.contains("[Cron delivery: nightly]"),
        "the mirrored turn is what makes a reply continue in context"
    );
}

#[tokio::test]
async fn a_missing_origin_session_still_delivers_to_the_route() {
    // A job added in a conversation's first turn: the host has a chat id
    // but no session for it yet. The mirror is best-effort context; losing
    // it must not swallow the delivery the platform is waiting for.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let job = crate::cron::CronJob {
        id: "job1".into(),
        name: "nightly".into(),
        prompt: "p".into(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: Some(1),
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: crate::cron::Deliver::Origin,
        origin: Some(crate::cron::store::Origin {
            platform: "discord".into(),
            chat: "4f3c2b1a9d8e7f6a".into(),
            thread: None,
            route: Some("1234567890".into()),
        }),
        workdir: None,
        fire_claim: None,
        reminder: false,
        skills: vec![],
        script: None,
    };
    let deliver = SaveLocalDeliver { home: home.clone() };
    let saved = deliver
        .deliver_full(&job, 1, &out("the deploy is green"), 0)
        .await
        .expect("a missing session is not a delivery failure");
    assert!(saved.to_chat);
    assert!(saved.excerpt.contains("the deploy is green"));
    // The local transcript of the run still exists either way.
    let out = home.join("cron").join("output").join("job1").join("1.md");
    assert!(
        std::fs::read_to_string(&out)
            .unwrap()
            .contains("the deploy is green")
    );
}

#[tokio::test]
async fn a_reminder_fires_without_a_model_turn_and_delivers_verbatim() {
    let home = tempfile::tempdir().unwrap();
    let mut record = one_due("r1", serde_json::json!("origin"));
    record[0]["reminder"] = serde_json::json!(true);
    record[0]["prompt"] = serde_json::json!("clean my roo");
    record[0]["origin"] = serde_json::json!({"platform": "discord", "chat": "4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c", "route": "42"});
    let store = due_store(&home, record);
    let runner = StubRunner {
        text: "MUST NOT RUN".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 1);
    assert_eq!(rep.errors, 0);
    assert!(
        runner.seen.lock().unwrap().is_empty(),
        "a reminder must not reach the agent"
    );
    let d = &rep.delivered[0];
    assert!(d.reminder && d.to_chat && !d.failed);
    assert_eq!(d.excerpt, "clean my roo");
    // No framing at all: the user's own words, byte for byte.
    assert_eq!(format_fire_chat(d), "clean my roo");
}

#[tokio::test]
async fn a_failed_origin_fire_delivers_a_short_failure_line() {
    let home = tempfile::tempdir().unwrap();
    let mut record = one_due("f1", serde_json::json!("origin"));
    record[0]["origin"] = serde_json::json!({"platform": "discord", "chat": "4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c", "route": "42"});
    let store = due_store(&home, record);
    let runner = StubRunner {
        text: String::new(),
        fail: true,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.errors, 1);
    let d = &rep.delivered[0];
    assert!(d.failed && d.to_chat && !d.reminder);
    assert_eq!(d.excerpt, "agent run failed: boom");
    let job = store.get("f1").unwrap().unwrap();
    assert_eq!(job.last_status, Some(crate::cron::RunStatus::Error));
}

#[tokio::test]
async fn chat_gets_only_the_final_text_while_the_transcript_stays_on_disk() {
    struct SplitRunner;
    #[async_trait::async_trait(?Send)]
    impl AsyncRunner for SplitRunner {
        async fn run(&self, _p: String, _c: PathBuf) -> anyhow::Result<String> {
            unreachable!("fire_one calls run_full")
        }
        async fn run_full(&self, _p: String, _c: PathBuf) -> anyhow::Result<FireOutput> {
            Ok(FireOutput {
                transcript: "[tool:bash]\n[result:exit 0]\n<untrusted-output>\nls\n</untrusted-output>\nclean my roo".into(),
                final_text: "clean my roo".into(),
            })
        }
    }
    let home = tempfile::tempdir().unwrap();
    let mut record = one_due("s2", serde_json::json!("origin"));
    record[0]["origin"] = serde_json::json!({"platform": "discord", "chat": "4f3c2b1a9d8e7f6a5b4c3d2e1f0a9b8c", "route": "42"});
    let store = due_store(&home, record);
    let rep = tick_once(
        &store,
        &SplitRunner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    let d = &rep.delivered[0];
    assert_eq!(
        d.excerpt, "clean my roo",
        "the tool transcript leaked into chat"
    );
    let saved = std::fs::read_to_string(&d.path).unwrap();
    assert!(
        saved.contains("[tool:bash]"),
        "full transcript stays on disk"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&d.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
