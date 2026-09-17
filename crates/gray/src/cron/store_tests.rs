// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

impl CronStore {
    fn add(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
        deliver: Deliver,
    ) -> anyhow::Result<String> {
        self.add_full(name, schedule, prompt, deliver, None, None, vec![], None)
    }
}

fn test_store() -> (tempfile::TempDir, CronStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = CronStore::open(dir.path()).unwrap();
    (dir, store)
}

#[test]
fn claim_is_at_most_once_across_handles() {
    let (_dir, store) = test_store();
    let id = store
        .add("ping", "every 1h", "say hi", Deliver::Local)
        .unwrap();
    // force due:
    store.set_next_run_for_test(&id, 1).unwrap();
    let cron_dir = store.cron_dir.clone();
    let a = CronStore::open(&cron_dir).unwrap();
    let b = CronStore::open(&cron_dir).unwrap();
    let got_a = a.claim_due(now_secs(), "owner-a").unwrap();
    let got_b = b.claim_due(now_secs(), "owner-b").unwrap();
    assert_eq!(got_a.len() + got_b.len(), 1, "exactly one claimant wins");
}

#[test]
fn live_claim_never_reclaimed_within_ttl() {
    let (_dir, store) = test_store();
    let id = store
        .add("hourly", "every 1h", "say hi", Deliver::Local)
        .unwrap();
    let t0 = 1_700_000_000;
    store.set_next_run_for_test(&id, 1).unwrap();
    assert_eq!(store.claim_due(t0, "owner-a").unwrap().len(), 1);
    // Force the job due again while the claim is still live: the live
    // claim must still block re-fire (at-most-once).
    store.set_next_run_for_test(&id, 1).unwrap();
    let second = store.claim_due(t0 + 10, "owner-b").unwrap();
    assert!(second.is_empty(), "live claim must not re-fire");
}

#[test]
fn stale_claim_is_reclaimed_with_warning_and_refires_once() {
    let (_dir, store) = test_store();
    let id = store
        .add("hourly", "every 1h", "say hi", Deliver::Local)
        .unwrap();
    let t0 = 1_700_000_000;
    store.set_next_run_for_test(&id, 1).unwrap();
    assert_eq!(store.claim_due(t0, "owner-a").unwrap().len(), 1);
    // Crashed worker: the job is due, the claim is past the TTL.
    store.set_next_run_for_test(&id, 1).unwrap();
    let due = store
        .claim_due(t0 + FIRE_CLAIM_TTL_SECS + 1, "owner-b")
        .unwrap();
    assert_eq!(due.len(), 1, "stale claim must be reclaimed past the TTL");
    assert_eq!(due[0].fire_claim.as_ref().unwrap().by, "owner-b");
    // The reclaim is a single winner: the new claim is live for others.
    let third = store
        .claim_due(t0 + FIRE_CLAIM_TTL_SECS + 2, "owner-c")
        .unwrap();
    assert!(third.is_empty(), "reclaimed job is at-most-once again");
}

#[test]
fn corrupt_store_fails_loads_and_mutations() {
    let (dir, store) = test_store();
    std::fs::write(dir.path().join("jobs.json"), "{torn").unwrap();
    assert!(store.list().is_err());
    assert!(store.claim_due(1_700_000_000, "o").is_err());
    assert!(store.add("x", "every 1h", "hi", Deliver::Local).is_err());
    // Missing store still reads empty and accepts the first add.
    std::fs::remove_file(dir.path().join("jobs.json")).unwrap();
    assert!(store.list().unwrap().is_empty());
    store.add("x", "every 1h", "hi", Deliver::Local).unwrap();
}

#[test]
fn add_rejects_bad_specs() {
    let (_dir, store) = test_store();
    assert!(store.add("", "every 1h", "hi", Deliver::Local).is_err());
    assert!(
        store
            .add(
                &"n".repeat(MAX_NAME_LEN + 1),
                "every 1h",
                "hi",
                Deliver::Local
            )
            .is_err()
    );
    assert!(store.add("x", "every 1h", "   ", Deliver::Local).is_err());
    assert!(store.add("x", "every 30s", "hi", Deliver::Local).is_err());
    assert!(
        store
            .add("x", "not a schedule", "hi", Deliver::Local)
            .is_err()
    );
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                Some(PathBuf::from("relative/path")),
                vec![],
                None,
            )
            .is_err()
    );
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                Some(PathBuf::from("/no/such/dir/gray-cron-test")),
                vec![],
                None,
            )
            .is_err()
    );
    // benign prompt mentioning no lifecycle shape passes
    let id = store
        .add("ok", "every 1h", "check CI and report", Deliver::Local)
        .unwrap();
    assert_eq!(store.get(&id).unwrap().unwrap().name, "ok");
}

#[test]
fn missed_oneshot_retires_without_firing() {
    let (_dir, store) = test_store();
    let at = now_secs() - 1000;
    let raw_job = serde_json::json!({
        "id": "once1",
        "name": "once",
        "prompt": "hi",
        "schedule": {"Once": {"at": at}},
        "enabled": true,
        "created_at": at,
        "next_run_at": at,
    });
    std::fs::write(
        store.jobs_path(),
        serde_json::to_string_pretty(&vec![raw_job]).unwrap(),
    )
    .unwrap();
    let due = store.claim_due(now_secs(), "owner").unwrap();
    assert!(due.is_empty(), "missed one-shot must never fire");
    let job = store.get("once1").unwrap().unwrap();
    assert!(!job.enabled);
    assert_eq!(job.state, JobState::Done);
    assert_eq!(job.last_error.as_deref(), Some("missed one-shot window"));
}

#[test]
fn stale_recurring_fast_forwards_but_fires_once() {
    let (_dir, store) = test_store();
    let id = store
        .add("hourly", "every 1h", "hi", Deliver::Local)
        .unwrap();
    store
        .set_next_run_for_test(&id, now_secs() - 5 * 3600)
        .unwrap();
    let due = store.claim_due(now_secs(), "owner").unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, id);
    let job = store.get(&id).unwrap().unwrap();
    assert!(job.next_run_at.unwrap() > now_secs(), "advanced past now");
    assert!(job.fire_claim.unwrap().by == "owner");
    // second tick finds nothing due (already advanced + claimed)
    assert!(store.claim_due(now_secs(), "owner2").unwrap().is_empty());
}

#[test]
fn mark_done_clears_claim_and_records_status() {
    let (_dir, store) = test_store();
    let id = store
        .add("hourly", "every 1h", "hi", Deliver::Local)
        .unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    let due = store.claim_due(now_secs(), "owner").unwrap();
    assert_eq!(due.len(), 1);
    store
        .mark_done(
            &id,
            store.get(&id).unwrap().unwrap().fire_claim.as_ref(),
            RunStatus::Ok,
            None,
        )
        .unwrap();
    let job = store.get(&id).unwrap().unwrap();
    assert!(job.fire_claim.is_none());
    assert_eq!(job.last_status, Some(RunStatus::Ok));
    assert!(job.last_run_at.is_some());
    // one-shot completion goes terminal
    let dir2 = tempfile::tempdir().unwrap();
    let s2 = CronStore::open(dir2.path()).unwrap();
    let oid = s2.add("once", "in 10m", "hi", Deliver::Local).unwrap();
    s2.mark_done(
        &oid,
        s2.get(&oid).unwrap().unwrap().fire_claim.as_ref(),
        RunStatus::Error,
        Some("boom"),
    )
    .unwrap();
    let o = s2.get(&oid).unwrap().unwrap();
    assert_eq!(o.state, JobState::Done);
    assert_eq!(o.last_status, Some(RunStatus::Error));
    assert_eq!(o.last_error.as_deref(), Some("boom"));
}

#[test]
fn add_accepts_skills_and_script() {
    let dir = tempfile::tempdir().unwrap();
    let store = CronStore::open(dir.path()).unwrap();
    let script = dir.path().join("pre.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    let id = store
        .add_full(
            "job",
            "every 1h",
            "do work",
            Deliver::Local,
            None,
            None,
            vec!["briefing".to_string()],
            Some(script.clone()),
        )
        .unwrap();
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.skills, vec!["briefing".to_string()]);
    assert_eq!(job.script, Some(script));
}

#[test]
fn add_rejects_bad_skills_and_script() {
    let (_dir, store) = test_store();
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                None,
                vec!["  ".to_string()],
                None,
            )
            .is_err()
    );
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                None,
                vec!["n".repeat(MAX_NAME_LEN + 1)],
                None,
            )
            .is_err()
    );
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                None,
                vec![],
                Some(PathBuf::from("relative/pre.sh")),
            )
            .is_err()
    );
    assert!(
        store
            .add_full(
                "x",
                "every 1h",
                "hi",
                Deliver::Local,
                None,
                None,
                vec![],
                Some(PathBuf::from("/no/such/file.sh")),
            )
            .is_err()
    );
}

#[test]
fn pause_resume_and_claim_one() {
    let (_dir, store) = test_store();
    let id = store
        .add("hourly", "every 1h", "hi", Deliver::Local)
        .unwrap();
    assert!(store.set_paused(&id, true).unwrap());
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Paused);
    assert!(job.enabled);
    store.set_next_run_for_test(&id, 1).unwrap();
    assert!(store.claim_due(1_700_000_000, "o").unwrap().is_empty());
    assert!(store.set_paused(&id, false).unwrap());
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Active);
    assert!(job.next_run_at.unwrap() > 1_700_000_000);
    let claimed = store
        .claim_one(1_700_000_001, "owner", &id)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, id);
    assert!(
        !store
            .claim_one(1_700_000_001, "other", &id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn malformed_record_does_not_abort_scan() {
    let (_dir, store) = test_store();
    let id = store.add("good", "every 1h", "hi", Deliver::Local).unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    let mut raw: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(store.jobs_path()).unwrap()).unwrap();
    raw.push(serde_json::json!({"id": "bad", "name": 42}));
    std::fs::write(
        store.jobs_path(),
        serde_json::to_string_pretty(&raw).unwrap(),
    )
    .unwrap();
    let due = store.claim_due(now_secs(), "owner").unwrap();
    assert_eq!(due.len(), 1, "good record still fires past a bad one");
    assert_eq!(due[0].id, id);
    // bad record survives the save pass
    let raw2: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(store.jobs_path()).unwrap()).unwrap();
    assert!(
        raw2.iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some("bad"))
    );
}

#[test]
fn tick_stamp_round_trips_and_missing_reads_as_none() {
    let (_dir, store) = test_store();
    assert!(
        store.last_tick().unwrap().is_none(),
        "fresh store ticked never"
    );
    store.record_tick("serve").unwrap();
    let stamp = store.last_tick().unwrap().expect("stamp written");
    assert_eq!(stamp.kind, "serve");
    assert_eq!(stamp.pid, std::process::id());
    assert!(stamp.at > 0);
}

#[test]
fn corrupt_tick_stamp_reads_as_never_ticked() {
    let (dir, store) = test_store();
    std::fs::write(dir.path().join(".last_tick"), "{not json").unwrap();
    assert!(store.last_tick().unwrap().is_none());
}

#[test]
fn health_flags_never_ticked_store_with_overdue_job() {
    let (_dir, store) = test_store();
    let id = store
        .add("reminder", "every 1h", "say hi", Deliver::Local)
        .unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    let now = now_secs();
    let health = store.health(now).unwrap();
    assert!(health.last_tick.is_none());
    assert!(!health.ticker_live(now), "no tick yet is not live");
    assert_eq!(health.overdue.len(), 1);
    assert_eq!(health.overdue[0].id, id);
    assert_eq!(health.overdue[0].next_run_at, 1);
}

#[test]
fn health_live_ticker_still_reports_due_job() {
    let (_dir, store) = test_store();
    let id = store
        .add("reminder", "every 1h", "say hi", Deliver::Local)
        .unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    store.record_tick("repl").unwrap();
    let now = now_secs();
    let health = store.health(now).unwrap();
    assert!(health.ticker_live(now));
    assert_eq!(health.overdue.len(), 1, "due and unclaimed stays overdue");
}

#[test]
fn health_ignores_paused_and_not_yet_due_jobs() {
    let (_dir, store) = test_store();
    let due = store.add("due", "every 1h", "p", Deliver::Local).unwrap();
    let future = store
        .add("future", "every 1h", "p", Deliver::Local)
        .unwrap();
    let paused = store
        .add("paused", "every 1h", "p", Deliver::Local)
        .unwrap();
    store.set_next_run_for_test(&due, 1).unwrap();
    store
        .set_next_run_for_test(&future, now_secs() + 3600)
        .unwrap();
    store.set_next_run_for_test(&paused, 1).unwrap();
    store.set_paused(&paused, true).unwrap();
    let health = store.health(now_secs()).unwrap();
    assert_eq!(health.overdue.len(), 1, "only the due, active job counts");
    assert_eq!(health.overdue[0].id, due);
}

#[test]
fn health_skips_job_with_live_fire_claim() {
    let (_dir, store) = test_store();
    let id = store.add("due", "every 1h", "p", Deliver::Local).unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    assert_eq!(store.claim_due(now_secs(), "owner-a").unwrap().len(), 1);
    assert!(store.health(now_secs()).unwrap().overdue.is_empty());
}

#[test]
fn old_worker_cannot_clear_reclaimed_job() {
    let (_dir, store) = test_store();
    let id = store.add("job", "every 1m", "hi", Deliver::Local).unwrap();
    store.set_next_run_for_test(&id, 1).unwrap();
    let old = store.claim_due(1000, "a").unwrap().remove(0);
    let new = store
        .claim_due(1000 + FIRE_CLAIM_TTL_SECS + 1, "b")
        .unwrap()
        .remove(0);
    assert!(
        store
            .mark_done(&id, old.fire_claim.as_ref(), RunStatus::Ok, None)
            .is_err()
    );
    assert_eq!(store.get(&id).unwrap().unwrap().fire_claim, new.fire_claim);
}
