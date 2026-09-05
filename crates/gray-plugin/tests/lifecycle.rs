use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::event::Usage;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{CoreEvent, Plugin};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn make_test_dir(prefix: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "gray-{}-{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        n
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn lifecycle_argv(dir: &PathBuf) -> Vec<String> {
    vec![
        "env".to_string(),
        format!("GRAY_TEST_DIR={}", dir.display()),
        "testdata/lifecycle_plugin.sh".to_string(),
    ]
}

fn bubble_argv(dir: &PathBuf) -> Vec<String> {
    vec![
        "env".to_string(),
        format!("GRAY_TEST_DIR={}", dir.display()),
        "testdata/bubble_plugin.sh".to_string(),
    ]
}

fn endpoint_files(dir: &PathBuf) -> Vec<(PathBuf, String, String)> {
    // (path, child-pid from filename, endpoint text)
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let Some(pid) = name.strip_prefix("endpoint-") else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        out.push((p, pid.to_string(), text));
    }
    out
}

fn child_alive(pid: &str) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

async fn poll_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t = std::time::Instant::now();
    while t.elapsed() < timeout {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

#[tokio::test]
async fn two_sidecars_produce_distinct_endpoints() {
    let dir = make_test_dir("lifecycle-a");
    let p1 = SidecarPlugin::spawn(lifecycle_argv(&dir)).await.unwrap();
    let p2 = SidecarPlugin::spawn(lifecycle_argv(&dir)).await.unwrap();
    let e1 = p1.prompt_context("/tmp").await.expect("ctx1");
    let e2 = p2.prompt_context("/tmp").await.expect("ctx2");
    assert!(e1.starts_with("ws://"), "endpoint1: {e1}");
    assert!(e2.starts_with("ws://"), "endpoint2: {e2}");
    assert_ne!(e1, e2, "two sidecars must produce distinct endpoints");
    p1.shutdown(Duration::from_secs(2)).await;
    p2.shutdown(Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn shutdown_one_leaves_other_alive() {
    let dir = make_test_dir("lifecycle-b");
    let p1 = SidecarPlugin::spawn(lifecycle_argv(&dir)).await.unwrap();
    let p2 = SidecarPlugin::spawn(lifecycle_argv(&dir)).await.unwrap();
    let e1 = p1.prompt_context("/tmp").await.expect("ctx1");
    let e2 = p2.prompt_context("/tmp").await.expect("ctx2");
    assert_ne!(e1, e2);

    let files = endpoint_files(&dir);
    assert_eq!(files.len(), 2, "two endpoint files, got {files:?}");
    let (path1, pid1, _) = files
        .iter()
        .find(|(_, _, t)| t == &e1)
        .expect("file for p1")
        .clone();
    let (path2, pid2, _) = files
        .iter()
        .find(|(_, _, t)| t == &e2)
        .expect("file for p2")
        .clone();
    assert!(child_alive(&pid1), "child {pid1} alive before shutdown");
    assert!(child_alive(&pid2), "child {pid2} alive before shutdown");

    p1.shutdown(Duration::from_secs(2)).await;

    // Killed one's file removed + child gone within 2s; other's file + child stay.
    assert!(
        poll_until(Duration::from_secs(2), || !path1.exists()).await,
        "endpoint file for shutdown sidecar removed within 2s"
    );
    assert!(
        poll_until(Duration::from_secs(2), || !child_alive(&pid1)).await,
        "killed one's child pid {pid1} gone within 2s"
    );
    assert!(path2.exists(), "other sidecar's endpoint file remains");
    assert!(
        child_alive(&pid2),
        "other sidecar's child {pid2} still alive"
    );
    // Other sidecar still serves prompt/context.
    let e2b = p2.prompt_context("/tmp").await.expect("other still alive");
    assert_eq!(e2b, e2);

    p2.shutdown(Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
    // Avoid unused ToolContext import warning when sibling changes land.
    let _ = ToolContext::default();
}

#[tokio::test]
async fn pre_v1_fixture_survives_shutdown_without_hanging() {
    // hooks_plugin.sh has no `protocol` (pre-v1) and no shutdown handling:
    // shutdown() must not hang waiting for a reply — killed after grace.
    let p = SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()])
        .await
        .unwrap();
    assert!(p.manifest().protocol.is_none(), "pre-v1 has no protocol");
    let grace = Duration::from_secs(1);
    let t = std::time::Instant::now();
    p.shutdown(grace).await;
    let el = t.elapsed();
    assert!(
        el >= grace,
        "pre-v1 killed after grace ({grace:?}), got {el:?}"
    );
    assert!(
        el < grace + Duration::from_secs(5),
        "shutdown must not hang, got {el:?}"
    );
}

#[tokio::test]
async fn bubble_lines_then_cleared() {
    let dir = make_test_dir("bubble");
    let p = SidecarPlugin::spawn(bubble_argv(&dir)).await.unwrap();
    let file = dir.join("bubble.txt");

    p.on_event(CoreEvent::PreTool {
        name: "read".into(),
        args: serde_json::json!({"path": "/x"}),
    })
    .await;
    p.on_event(CoreEvent::PostTool {
        name: "read".into(),
        output: ToolOutput::ok("body"),
    })
    .await;

    assert!(
        poll_until(Duration::from_secs(2), || {
            std::fs::read_to_string(&file)
                .map(|t| t.lines().count() >= 2)
                .unwrap_or(false)
        })
        .await,
        "bubble.txt grows with pre/post lines"
    );
    let body = std::fs::read_to_string(&file).unwrap();
    assert!(body.contains("pre_tool"), "has pre_tool, got: {body}");
    assert!(body.contains("post_tool"), "has post_tool, got: {body}");

    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;
    assert!(
        poll_until(Duration::from_secs(2), || {
            std::fs::read_to_string(&file)
                .map(|t| t.is_empty())
                .unwrap_or(false)
        })
        .await,
        "bubble.txt empty after turn_end"
    );

    p.shutdown(Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}
