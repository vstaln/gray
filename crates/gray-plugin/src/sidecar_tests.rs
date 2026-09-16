use super::*;
use gray_core::agent::ToolContext;
use gray_core::event::Usage;

#[tokio::test]
async fn hanging_hook_times_out_and_skips() {
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
        .await
        .unwrap();
    let t = std::time::Instant::now();
    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn crashed_plugin_returns_error_not_panic() {
    let p = SidecarPlugin::spawn(vec!["testdata/crash_plugin.sh".into()])
        .await
        .unwrap();
    let out = p.tools()[0]
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("plugin crashed: crash"),
        "got: {}",
        out.content
    );
}

#[tokio::test]
async fn empty_manifest_name_bails() {
    let err = SidecarPlugin::spawn(vec!["testdata/empty_name_plugin.sh".into()])
        .await
        .err()
        .expect("spawn must bail on missing/empty name");
    assert!(err.to_string().contains("empty name"), "got: {err:#}");
}

#[tokio::test]
async fn notify_sends_no_id_and_needs_no_reply() {
    // hang fixture never replies to event/notify; if on_event waited for a
    // reply it would hit the 5s timeout. True notification returns fast.
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
        .await
        .unwrap();
    let t = std::time::Instant::now();
    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}
