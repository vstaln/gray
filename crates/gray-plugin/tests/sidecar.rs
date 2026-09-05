use gray_core::agent::ToolContext;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{Plugin, ToolBefore};

#[tokio::test]
async fn sidecar_manifest_and_tool_call() {
    let p = SidecarPlugin::spawn(vec!["testdata/echo_plugin.sh".into()]).await.unwrap();
    assert_eq!(p.manifest().name, "echo");
    let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
    assert_eq!(out.content, "hi");
    assert!(!out.is_error);
}

#[tokio::test]
async fn concurrent_tool_calls_resolve_by_id_not_order() {
    let p = SidecarPlugin::spawn(vec!["testdata/reorder_plugin.sh".into()]).await.unwrap();
    let tool = p.tools()[0].clone();
    let ctx = ToolContext::default();
    let (a, b) = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        async {
            tokio::join!(
                tool.execute(&ctx, serde_json::json!({"n": "1"})),
                tool.execute(&ctx, serde_json::json!({"n": "2"})),
            )
        },
    )
    .await
    .expect("concurrent tool/calls hung — reader task must route replies by id");
    assert!(!a.is_error && !b.is_error, "a={a:?} b={b:?}");
    assert_eq!(a.content, "1", "{a:?}");
    assert_eq!(b.content, "2", "{b:?}");
}

#[tokio::test]
async fn prompt_context_returns_claimed_text() {
    let p = SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()]).await.unwrap();
    assert_eq!(p.prompt_context("/tmp").await.as_deref(), Some("CTX"));
}

#[tokio::test]
async fn tool_before_allow_and_command_joins_argv() {
    let p = SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()]).await.unwrap();
    assert_eq!(p.tool_before("shout", &serde_json::json!({})).await, ToolBefore::Allow);
    assert_eq!(
        p.run_command("/echo", vec!["hi".into(), "there".into()]).await.as_deref(),
        Some("hi there")
    );
    // Unclaimed command on a claiming plugin: no RPC, None.
    assert_eq!(p.run_command("/nope", vec![]).await, None);
}

#[tokio::test]
async fn unclaimed_hooks_fail_open_without_rpc() {
    // hang fixture never replies: any request would hit the 30s timeout, so
    // fast defaults prove the hooks/commands gate sends nothing.
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()]).await.unwrap();
    let t = std::time::Instant::now();
    assert_eq!(p.prompt_context("/tmp").await, None);
    assert_eq!(p.tool_before("x", &serde_json::json!({})).await, ToolBefore::Allow);
    assert_eq!(p.run_command("/echo", vec!["hi".into()]).await, None);
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn tool_before_parses_deny_modify_and_unknown() {
    let args = serde_json::json!({"path": "/x"});
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "deny", "reason": "no"}), &args),
        ToolBefore::Deny { reason: "no".into() }
    );
    assert_eq!(
        ToolBefore::from_result(
            &serde_json::json!({"decision": "modify", "args": {"path": "/y"}}),
            &args
        ),
        ToolBefore::Modify { args: serde_json::json!({"path": "/y"}) }
    );
    // Unknown shapes fail open (pre-v1 behavior).
    assert_eq!(ToolBefore::from_result(&serde_json::json!({}), &args), ToolBefore::Allow);
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "bogus"}), &args),
        ToolBefore::Allow
    );
}

#[tokio::test]
async fn echo_reference_plugin_manifest_and_command_round_trip() {
    // Reference plugin ships with the repo; boot the real script, not a fixture.
    let p = SidecarPlugin::spawn(vec!["../../plugins/echo/echo.sh".into()]).await.unwrap();
    let m = p.manifest();
    assert_eq!(m.name, "echo");
    assert_eq!(m.commands, vec!["/echo".to_string()]);
    assert_eq!(m.tools.len(), 1);
    assert_eq!(m.tools[0].name, "echo");
    assert!(m.tools[0].parameters.get("properties").is_some(), "{:?}", m.tools[0]);
    let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({"text": "hi"})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.content, "hi", "{out:?}");
    assert_eq!(
        p.run_command("/echo", vec!["hello".into(), "world".into()]).await.as_deref(),
        Some("hello world")
    );
}
