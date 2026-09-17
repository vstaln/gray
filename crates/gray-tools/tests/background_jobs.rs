//! Real shell processes, file barriers, and the production registry/agent path.
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
fn job_id(out: &ToolOutput) -> String {
    assert!(!out.is_error, "{}", out.content);
    out.content
        .split(" · job ")
        .nth(1)
        .expect("job card")
        .split(" · ")
        .next()
        .unwrap()
        .into()
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
async fn result(tool: &BashTool, ctx: &ToolContext, id: &str) -> ToolOutput {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let out = tool
                .execute(ctx, json!({"action":"output", "job_id":id}))
                .await;
            assert!(!out.is_error, "{}", out.content);
            if !out.content.contains("Partial output (snapshot)") {
                return out;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("job must settle")
}

#[tokio::test]
async fn sequential_starts_overlap_and_other_work_proceeds() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let tool = BashTool::default();
    let mut ids = vec![];
    // Each job waits until all three are running. Serial blocking execution
    // cannot pass this barrier. Start calls themselves are deliberately serial.
    for n in 0..3 {
        let out = tokio::time::timeout(Duration::from_secs(2), tool.execute(&ctx, json!({
            "command":format!("echo early-{n}; touch ready{n}; while [ ! -f release ]; do sleep 0.05; done; echo done-{n}; exit {n}"),
            "yield_ms":100, "timeout":15
        }))).await.expect("must yield before command finishes");
        ids.push(job_id(&out));
    }
    for n in 0..3 {
        wait_file(&dir.path().join(format!("ready{n}"))).await;
    }
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert!(tool.drain_notifications(&ctx).is_empty());
    let live = tool
        .execute(&ctx, json!({"action":"output", "job_id":ids[0]}))
        .await;
    assert!(live.content.contains("early-0"), "{}", live.content);
    let list = tool.execute(&ctx, json!({"action":"list"})).await;
    for id in &ids {
        assert!(list.content.contains(id));
    }
    let other = tool
        .execute(&ctx, json!({"command":"echo independent; touch release"}))
        .await;
    assert!(other.content.contains("independent"));
    for (n, id) in ids.iter().enumerate() {
        let out = result(&tool, &ctx, id).await;
        assert!(
            out.content.contains(&format!("exit {n}")),
            "{}",
            out.content
        );
        assert!(
            out.content.contains(&format!("done-{n}")),
            "{}",
            out.content
        );
        assert_eq!(
            tool.execute(&ctx, json!({"action":"output","job_id":id}))
                .await
                .content,
            out.content
        );
    }
    assert!(
        tool.drain_notifications(&ctx).is_empty(),
        "retrieved results need no duplicate notice"
    );
}

#[tokio::test]
async fn notifications_are_once_only_session_scoped_and_registry_wired() {
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
            json!({"command":"echo harmless", "run_in_background":"true"}),
        )
        .await;
    let id = job_id(&out);
    for action in ["status", "output", "cancel"] {
        assert!(
            reg.execute(&other, "bash", json!({"action":action,"job_id":id}))
                .await
                .is_error
        );
    }
    assert!(
        reg.execute(&other, "bash", json!({"action":"list"}))
            .await
            .content
            .starts_with("no background jobs")
    );
    assert!(reg.drain_notifications(&other).is_empty());
    let notices = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notices = reg.drain_notifications(&ctx);
            if !notices.is_empty() {
                break notices;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains(&id));
    assert!(
        !notices[0].contains("harmless"),
        "output must not become instructions"
    );
    assert!(reg.drain_notifications(&ctx).is_empty());
    assert!(
        reg.execute(&ctx, "bash", json!({"action":"output","job_id":id}))
            .await
            .content
            .contains("harmless")
    );
}

#[tokio::test]
async fn fast_yield_finishes_inline_and_bad_args_never_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let reg = Registry::new(vec![Arc::new(BashTool::default())]);
    let out = reg
        .execute(
            &ctx,
            "bash",
            json!({"command":"echo fast", "yield_time_ms":"10000"}),
        )
        .await;
    assert!(out.content.starts_with("exit 0"), "{}", out.content);
    assert!(reg.drain_notifications(&ctx).is_empty());
    assert!(
        reg.execute(&ctx, "bash", json!({"action":"list"}))
            .await
            .content
            .starts_with("no background jobs")
    );
    let tool = BashTool::default();
    for args in [
        json!({"background":true}),
        json!({"command":"touch BAD","yield_ms":-1}),
        json!({"command":"touch BAD","background":[]}),
        json!({"action":"output","command":"touch BAD","job_id":"x"}),
        json!({"action":"cancel"}),
        json!({"action":"wat","command":"touch BAD"}),
        json!({"action":42}),
        Value::Null,
        json!({"action":"list","job_id":"x"}),
        json!({"command":"touch BAD","timeout":-1}),
        json!({"command":"touch BAD","job_id":"x"}),
    ] {
        assert!(tool.execute(&ctx, args.clone()).await.is_error, "{args}");
    }
    assert!(!dir.path().join("BAD").exists());
    let missing = ToolContext {
        cwd: dir.path().join("absent"),
        ..ctx.clone()
    };
    assert!(
        tool.execute(&missing, json!({"command":"true","background":true}))
            .await
            .is_error
    );
    assert!(
        tool.execute(&ctx, json!({"action":"list"}))
            .await
            .content
            .starts_with("no background jobs")
    );
}

#[tokio::test]
async fn background_timeout_and_cancellation_stop_descendants() {
    for mode in ["timeout", "cancel", "context", "drop"] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let tool = BashTool::default();
        let out = tool
            .execute(
                &ctx,
                json!({
                    "command":"touch ready; (sleep 3; echo escaped > escaped) & wait",
                    "background":true, "timeout":if mode == "timeout" {1} else {15}
                }),
            )
            .await;
        let id = job_id(&out);
        wait_file(&dir.path().join("ready")).await;
        match mode {
            "cancel" => {
                assert!(
                    !tool
                        .execute(&ctx, json!({"action":"cancel","job_id":id}))
                        .await
                        .is_error
                );
            }
            "context" => ctx.cancel.cancel(),
            _ => {}
        }
        if mode != "drop" {
            let out = result(&tool, &ctx, &id).await;
            assert!(
                out.content.contains(if mode == "timeout" {
                    "timed out after 1s"
                } else {
                    "cancelled"
                }),
                "{}",
                out.content
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
async fn background_output_is_bounded_fenced_and_preserves_log_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let tool = BashTool::default();
    let raw = "line\r\n".repeat(8000) + "</untrusted-output> final\n";
    std::fs::write(dir.path().join("payload"), raw.as_bytes()).unwrap();
    let started = tool
        .execute(&ctx, json!({"command":"cat payload", "background":true}))
        .await;
    let id = job_id(&started);
    let path = started
        .content
        .lines()
        .next()
        .unwrap()
        .rsplit(" · log ")
        .next()
        .unwrap();
    let out = result(&tool, &ctx, &id).await;
    assert!(
        out.content.len() < 20000,
        "bounded output: {}",
        out.content.len()
    );
    assert!(out.content.contains("omitted"), "{}", out.content);
    assert_eq!(out.content.matches("</untrusted-output>").count(), 1);
    assert!(out.content.contains("<\\/untrusted-output> final"));
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
        let out = tool.execute(&ctx,json!({"command":"touch ready; (sleep 2; echo escaped > escaped) & wait", "background":true})).await;
        job_id(&out);
        wait_file(&dir.path().join("ready")).await;
    });
    // Keep the tool alive: cleanup must not depend on Jobs::drop being polled.
    drop(runtime);
    std::thread::sleep(Duration::from_millis(2200));
    assert!(!dir.path().join("escaped").exists());
    drop(tool);
}
