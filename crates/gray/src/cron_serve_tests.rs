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
        model: Some("startup-model".to_string()),
        base_url: "https://startup.example/v1".to_string(),
        api_key: None,
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
        }),
        workdir: None,
        fire_claim: None,
        skills: vec![],
        script: None,
    };
    let deliver = SaveLocalDeliver {
        home: home.path().to_path_buf(),
    };
    // Happy path: session append + file save. The mirror is the clean
    // excerpt (no wrapper, no file path) as a USER turn.
    let saved = deliver
        .deliver(&mk_job("j-origin", chat), 1_700_000_001, "hello output")
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
    // Failure path: unknown session -> Err (caller records DeliveryFailed),
    // file still saved.
    let err = deliver
        .deliver(
            &mk_job("j-bad", "no-such-session"),
            1_700_000_002,
            "kept output",
        )
        .await
        .unwrap_err();
    assert!(err.contains("no-such-session"), "unexpected: {err}");
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
        let saved = deliver.deliver(&job, 1_700_000_001, "body").await.unwrap();
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
fn fire_chat_frame_shapes() {
    // The live-chat box: hermes wrapper + output-file path.
    let saved = DeliveredFire {
        id: "abc123".to_string(),
        name: "nightly".to_string(),
        path: std::path::PathBuf::from("/home/u/.gray/cron/output/abc123/1.md"),
        excerpt: "hello output".to_string(),
        to_chat: true,
    };
    let out = format_fire_chat(&saved);
    assert!(out.starts_with("Cronjob Response: nightly\n(job_id: abc123)\n"));
    assert!(out.contains("hello output"));
    assert!(out.contains("Full output: /home/u/.gray/cron/output/abc123/1.md"));
}
