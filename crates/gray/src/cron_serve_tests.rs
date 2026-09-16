// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

struct StubRunner {
    text: String,
    fail: bool,
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait(?Send)]
impl AsyncRunner for StubRunner {
    async fn run(&self, prompt: String) -> anyhow::Result<String> {
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
        &LocalDeliver {
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
        &LocalDeliver {
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
        &LocalDeliver {
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
