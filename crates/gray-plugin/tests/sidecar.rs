use gray_core::agent::ToolContext;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::Plugin;

#[tokio::test]
async fn sidecar_manifest_and_tool_call() {
    let p = SidecarPlugin::spawn(vec!["testdata/echo_plugin.sh".into()]).await.unwrap();
    assert_eq!(p.manifest().name, "echo");
    let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
    assert_eq!(out.content, "hi");
    assert!(!out.is_error);
}
