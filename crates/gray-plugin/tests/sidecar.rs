use std::sync::Arc;

use gray_core::agent::{
    Agent, PluginHooks, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput,
};
use gray_core::event::{StopReason, StreamEvent};
use gray_core::message::{ChatRequest, ContentBlock, Message, ToolDef};
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{Plugin, PluginHookAdapter, ToolBefore};

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
        ToolBefore::Deny("no".into())
    );
    assert_eq!(
        ToolBefore::from_result(
            &serde_json::json!({"decision": "modify", "args": {"path": "/y"}}),
            &args
        ),
        ToolBefore::Modify(serde_json::json!({"path": "/y"}))
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

/// Scripted provider: one event list per expected request, recording every
/// `ChatRequest.system` so e2e tests can assert on prompt wiring.
struct ScriptedProvider {
    scripted: std::sync::Mutex<std::collections::VecDeque<Vec<StreamEvent>>>,
    seen_systems: Arc<std::sync::Mutex<Vec<Option<String>>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripted: std::sync::Mutex::new(scripts.into()),
            seen_systems: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen_systems.lock().expect("seen lock").push(req.system.clone());
        let script = self.scripted.lock().expect("script lock").pop_front().unwrap_or_default();
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

/// Executor that records every call and answers a canned output.
struct RecordingExecutor {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    output: ToolOutput,
}

#[async_trait::async_trait]
impl ToolExecutor for RecordingExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        name: &str,
        _args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, ToolOutput> {
        self.calls.lock().expect("calls lock").push(name.to_string());
        let out = self.output.clone();
        Box::pin(async move { out })
    }
}

async fn e2e_hooks(cwd: &str) -> Vec<Arc<dyn PluginHooks>> {
    let p: Arc<dyn Plugin> =
        Arc::new(SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()]).await.unwrap());
    PluginHookAdapter::for_plugins(std::slice::from_ref(&p), cwd)
}

fn end_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta(text.to_string()),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]
}

#[tokio::test]
async fn e2e_sidecar_prompt_context_lands_in_system() {
    let hooks = e2e_hooks("/tmp").await;
    let provider = ScriptedProvider::new(vec![end_script("done")]);
    let seen = provider.seen_systems.clone();
    let mut agent = Agent::new(
        Box::new(provider),
        Box::new(RecordingExecutor {
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            output: ToolOutput::ok("unused"),
        }),
    )
    .with_system("BASE-SYSTEM")
    .with_hooks(hooks);
    agent.run(Message::user("go"), ToolContext::default()).await.unwrap();
    let seen = seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 1, "one turn → one request, got {seen:?}");
    let system = seen[0].as_deref().unwrap_or("");
    assert!(system.contains("BASE-SYSTEM"), "base prompt preserved, got: {system}");
    assert!(system.contains("CTX"), "sidecar prompt/context text present, got: {system}");
}

#[tokio::test]
async fn e2e_sidecar_tool_before_deny_blocks_executor() {
    let hooks = e2e_hooks("/tmp").await;
    let provider = ScriptedProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("c1".to_string()),
                Some("blocked".to_string()),
                r#"{}"#,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script("recovered"),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = Agent::new(
        Box::new(provider),
        Box::new(RecordingExecutor { calls: calls.clone(), output: ToolOutput::ok("must-not-run") }),
    )
    .with_tools(vec![ToolDef::new("blocked", "deny me", serde_json::json!({}))])
    .with_hooks(hooks);
    agent.run(Message::user("go"), ToolContext::default()).await.unwrap();
    assert!(calls.lock().expect("calls lock").is_empty(), "denied tool must never execute");
    let denied = agent.messages().iter().flat_map(|m| m.content.iter()).any(|b| {
        matches!(b, ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("BLOCKED-BY-E2E"))
    });
    assert!(denied, "deny reason must surface as an is_error tool result");
}

#[tokio::test]
async fn e2e_sidecar_command_run_routes_echo() {
    let hooks = e2e_hooks("/tmp").await;
    // Same source the REPL reads via `agent.hooks()` for `/help` + routing.
    let names: Vec<String> = hooks.iter().flat_map(|h| h.commands()).map(|c| c.name).collect();
    assert!(names.contains(&"/echo".to_string()), "sidecar claims /echo, got {names:?}");
    assert_eq!(
        hooks[0].run_command("/echo", vec!["hello".into(), "world".into()]).await.as_deref(),
        Some("hello world")
    );
    // Empty-text replies filter to None so the REPL never prints a blank line.
    assert_eq!(hooks[0].run_command("/echo", vec![]).await, None);
}
