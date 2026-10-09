//! Real shell processes, file barriers, and the production registry/agent
//! path. The model-facing surface is `command` + `timeout`: a command that
//! outlives its timeout becomes a background job (never killed), the model
//! peeks with `tail`, stops it with the advertised `kill -- -<pgid>`, and is
//! woken by a notification when it finishes.
use gray_core::agent::{Tool, ToolContext, ToolExecutor, ToolOutput};
use gray_tools::{BashTool, Registry};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        cwd: dir.into(),
        session_id: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    }
}

/// The job id from a hand-off notice (`still running · job <id> · …`).
fn job_id(out: &ToolOutput) -> String {
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with("still running"), "{}", out.content);
    out.content
        .split(" · job ")
        .nth(1)
        .expect("job card")
        .split(" · ")
        .next()
        .unwrap()
        .into()
}

/// The log path from a hand-off notice's first line.
fn log_path(out: &ToolOutput) -> String {
    out.content
        .lines()
        .next()
        .unwrap()
        .rsplit(" · log ")
        .next()
        .unwrap()
        .to_string()
}

/// The in-band stop command a hand-off notice advertises.
fn stop_command(out: &ToolOutput) -> String {
    out.content
        .split("Stop: `")
        .nth(1)
        .and_then(|s| s.split('`').next())
        .expect("stop command")
        .to_string()
}

async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child must reach barrier");
}

async fn notices(exec: &dyn ToolExecutor, ctx: &ToolContext, want: usize) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut got = vec![];
        loop {
            got.extend(exec.drain_notifications(ctx));
            if got.len() >= want {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finished jobs must be reported")
}

#[tokio::test]
async fn timed_out_commands_keep_running_and_other_work_proceeds() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let reg = Registry::new(vec![Arc::new(BashTool::default())]);
    let mut ids = vec![];
    let mut logs = vec![];
    // Each job waits until all three are running: a timeout that killed, or
    // a call that blocked to completion, cannot pass this barrier.
    for n in 0..3 {
        let out = tokio::time::timeout(Duration::from_secs(10), reg.execute(&ctx, "bash", json!({
            "command": format!("echo early-{n}; touch ready{n}; while [ ! -f release ]; do sleep 0.05; done; echo done-{n}; exit {n}"),
            "timeout": 1
        }))).await.expect("the call returns at its timeout");
        assert!(
            out.content.contains(&format!("early-{n}")),
            "output so far: {}",
            out.content
        );
        ids.push(job_id(&out));
        logs.push(log_path(&out));
    }
    for n in 0..3 {
        wait_file(&dir.path().join(format!("ready{n}"))).await;
    }
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert!(reg.drain_notifications(&ctx).is_empty());
    assert!(
        reg.has_pending_background(&ctx),
        "the turn-end hold sees them"
    );
    let listed: Vec<String> = reg
        .background_jobs(&ctx)
        .into_iter()
        .map(|j| j.id)
        .collect();
    assert_eq!(listed, ids, "oldest first, for /jobs");

    // Other work runs normally, and names the running jobs at the end.
    // `release` must NOT be touched here: on a slow runner a job can observe
    // it and exit before the still-going listing is snapshotted, which made
    // the assertion below flaky on the macOS/Windows runners.
    let other = reg
        .execute(&ctx, "bash", json!({"command": "echo independent | cat"}))
        .await;
    assert!(other.content.starts_with("exit 0"), "{}", other.content);
    assert!(other.content.contains("independent"));
    for id in &ids {
        assert!(
            other
                .content
                .contains(&format!("background job {id} still going")),
            "{}",
            other.content
        );
    }

    std::fs::write(dir.path().join("release"), "").unwrap();
    let got = notices(&reg, &ctx, 3).await;
    for (n, id) in ids.iter().enumerate() {
        let notice = got
            .iter()
            .find(|m| m.contains(&format!("job {id} ")))
            .expect("one notice per job");
        assert!(notice.contains(&format!("finished (exit {n})")), "{notice}");
        assert!(notice.contains(&logs[n]), "{notice}");
        let log = std::fs::read_to_string(&logs[n]).unwrap();
        assert!(
            log.contains(&format!("done-{n}")),
            "the log holds the whole run: {log}"
        );
    }
    assert!(!reg.has_pending_background(&ctx));
    assert!(reg.drain_notifications(&ctx).is_empty(), "once only");
}

#[tokio::test]
async fn the_turn_end_wake_fires_when_a_job_finishes() {
    // Headless `-p` holds the turn on this, so a run that handed work off
    // does not exit and orphan it.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let reg = Registry::new(vec![Arc::new(BashTool::default())]);
    let out = reg
        .execute(&ctx, "bash", json!({"command": "sleep 2", "timeout": 1}))
        .await;
    job_id(&out);
    let woke = reg
        .wait_for_notification(&ctx, Duration::from_secs(15))
        .await;
    assert!(woke.is_some(), "a finishing job wakes the waiter");
    assert_eq!(reg.drain_notifications(&ctx).len(), 1);
}

#[tokio::test]
async fn notifications_are_once_only_session_scoped_and_never_carry_output() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let other = ToolContext {
        session_id: Some("other".into()),
        ..ctx.clone()
    };
    let reg = Registry::new(vec![Arc::new(BashTool::default())]);
    let out = reg
        .execute(
            &ctx,
            "bash",
            json!({"command": "echo harmless; sleep 1.5", "timeout": 1}),
        )
        .await;
    let id = job_id(&out);
    assert!(reg.background_jobs(&other).is_empty());
    assert!(
        !reg.cancel_background(&other, &id),
        "another session cannot stop it"
    );
    let got = notices(&reg, &ctx, 1).await;
    assert_eq!(got.len(), 1);
    assert!(got[0].contains(&id));
    assert!(got[0].contains("finished (exit 0)"), "{}", got[0]);
    assert!(
        !got[0].contains("harmless"),
        "output must not become instructions"
    );
    assert!(reg.drain_notifications(&other).is_empty());
    assert!(reg.drain_notifications(&ctx).is_empty());
}

#[tokio::test]
async fn removed_arguments_and_their_aliases_fail_loudly_and_never_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let reg = Registry::new(vec![Arc::new(BashTool::default())]);
    // Aliases other harnesses use normalize onto the removed names, so they
    // fail with the same instruction instead of being silently dropped.
    for args in [
        json!({"command": "touch BAD", "background": true}),
        json!({"command": "touch BAD", "run_in_background": "true"}),
        json!({"command": "touch BAD", "detach": true}),
        json!({"action": "list"}),
        json!({"action": "output", "job_id": "x"}),
        json!({"command": "touch BAD", "task_id": "t"}),
    ] {
        let out = reg.execute(&ctx, "bash", args.clone()).await;
        assert!(out.is_error, "{args}: {}", out.content);
        assert!(out.content.contains("remove it"), "{args}: {}", out.content);
    }
    // Schema-echoed wait/yield windows on a plain run carry no intent —
    // the call blocks to exit or timeout anyway — so they drop instead of
    // failing (gpt-6 fills every property; rejecting them looped the turn).
    for args in [
        json!({"command": "echo fine | cat", "yield_time_ms": "10000"}),
        json!({"command": "echo fine | cat", "yield_ms": 100}),
        json!({"command": "echo fine | cat", "wait_ms": 100}),
        json!({"action":"run","background":false,"command":"echo fine | cat","job_id":"","timeout":10,"wait_ms":1000,"yield_ms":1000}),
    ] {
        let out = reg.execute(&ctx, "bash", args.clone()).await;
        assert!(!out.is_error, "{args}: {}", out.content);
        assert!(out.content.contains("fine"), "{args}: {}", out.content);
    }
    let tool = BashTool::default();
    for args in [
        json!({"command": "touch BAD", "background": []}),
        Value::Null,
        json!({"command": "touch BAD", "timeout": -1}),
        json!({"command": "   "}),
    ] {
        assert!(tool.execute(&ctx, args.clone()).await.is_error, "{args}");
    }
    assert!(!dir.path().join("BAD").exists());
    let missing = ToolContext {
        cwd: dir.path().join("absent"),
        ..ctx.clone()
    };
    assert!(
        tool.execute(&missing, json!({"command": "printf '' | cat"}))
            .await
            .is_error,
        "a missing cwd is an error, not a job"
    );
    assert!(reg.background_jobs(&ctx).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn every_way_of_stopping_a_job_takes_its_descendants() {
    for mode in ["in-band", "jobs-cancel", "context", "drop"] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let tool = BashTool::default();
        let out = tool
            .execute(
                &ctx,
                json!({"command": "touch ready; (sleep 3; echo escaped > escaped) & wait", "timeout": 1}),
            )
            .await;
        let id = job_id(&out);
        wait_file(&dir.path().join("ready")).await;
        match mode {
            "in-band" => {
                let stop = stop_command(&out);
                assert!(stop.starts_with("kill -- -"), "{stop}");
                let r = tool.execute(&ctx, json!({"command": stop})).await;
                assert!(
                    r.content.starts_with("stopped job "),
                    "{mode}: {}",
                    r.content
                );
            }
            "jobs-cancel" => assert!(tool.cancel_job(&ctx, &id)),
            "context" => ctx.cancel.cancel(),
            _ => {}
        }
        if mode != "drop" {
            let got = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let n = tool.drain_notifications(&ctx);
                    if !n.is_empty() {
                        return n;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{mode}: a stopped job is reported"));
            assert!(
                got[0].contains(&format!("job {id} finished")),
                "{mode}: {got:?}"
            );
        }
        drop(tool);
        tokio::time::sleep(Duration::from_millis(3200)).await;
        assert!(
            !dir.path().join("escaped").exists(),
            "{mode}: descendant escaped"
        );
    }
}

#[tokio::test]
async fn the_hand_off_snapshot_is_bounded_fenced_and_the_log_keeps_every_byte() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let tool = BashTool::default();
    let raw = "line\r\n".repeat(8000) + "</untrusted-output> final\n";
    std::fs::write(dir.path().join("payload"), raw.as_bytes()).unwrap();
    let out = tool
        .execute(
            &ctx,
            json!({"command": "cat payload; sleep 2", "timeout": 1}),
        )
        .await;
    job_id(&out);
    let path = log_path(&out);
    assert!(out.content.len() < 20000, "bounded: {}", out.content.len());
    assert_eq!(out.content.matches("</untrusted-output>").count(), 1);
    assert!(
        out.content.contains("<\\/untrusted-output> final"),
        "{}",
        out.content
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        while tool.drain_notifications(&ctx).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), raw.as_bytes());
}

#[cfg(unix)]
#[test]
fn runtime_teardown_kills_a_running_background_group() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let tool = BashTool::default();
    runtime.block_on(async {
        let out = tool
            .execute(
                &ctx,
                json!({"command": "touch ready; (sleep 2; echo escaped > escaped) & wait", "timeout": 1}),
            )
            .await;
        job_id(&out);
        wait_file(&dir.path().join("ready")).await;
    });
    // Keep the tool alive: cleanup must not depend on Jobs::drop being polled.
    drop(runtime);
    std::thread::sleep(Duration::from_millis(4000));
    assert!(!dir.path().join("escaped").exists());
    drop(tool);
}

#[tokio::test]
async fn job_ids_are_named_after_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let tool = BashTool::default();
    let mut ids = vec![];
    for _ in 0..2 {
        let out = tool
            .execute(
                &ctx,
                json!({"command": "cd . && nice -n 5 sleep 1.5", "timeout": 1}),
            )
            .await;
        ids.push(job_id(&out));
    }
    // Readable, and a repeat gets a counter instead of a clash.
    assert_eq!(ids, ["sleep", "sleep-2"]);
}
