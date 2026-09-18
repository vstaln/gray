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
    // Happy path: session append + file save.
    deliver
        .deliver(&mk_job("j-origin", chat), 1_700_000_001, "hello output")
        .await
        .unwrap();
    let saved = std::fs::read_to_string(
        home.path()
            .join("cron")
            .join("output")
            .join("j-origin")
            .join("1700000001.md"),
    )
    .unwrap();
    assert!(saved.contains("hello output"));
    let session_text =
        std::fs::read_to_string(home.path().join("sessions").join(format!("{chat}.jsonl")))
            .unwrap();
    assert!(
        session_text.contains("nightly"),
        "origin session missing delivery note"
    );
    assert!(session_text.contains("1700000001.md"));
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
