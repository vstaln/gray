use std::sync::Arc;

use gray_core::agent::{
    Agent, CommandOutcome, PluginHooks, Provider, ProviderStream, ToolContext, ToolExecutor,
    ToolOutput,
};
use gray_core::event::{StopReason, StreamEvent};
use gray_core::message::{ChatRequest, ContentBlock, Message, ToolDef};
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{Plugin, PluginHookAdapter, ToolBefore};

#[tokio::test]
async fn sidecar_manifest_and_tool_call() {
    let p = SidecarPlugin::spawn(vec!["testdata/echo_plugin.sh".into()])
        .await
        .unwrap();
    assert_eq!(p.manifest().name, "echo");
    let out = p.tools()[0]
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert_eq!(out.content, "hi");
    assert!(!out.is_error);
}

#[tokio::test]
async fn concurrent_tool_calls_resolve_by_id_not_order() {
    let p = SidecarPlugin::spawn(vec!["testdata/reorder_plugin.sh".into()])
        .await
        .unwrap();
    let tool = p.tools()[0].clone();
    let ctx = ToolContext::default();
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(
            tool.execute(&ctx, serde_json::json!({"n": "1"})),
            tool.execute(&ctx, serde_json::json!({"n": "2"})),
        )
    })
    .await
    .expect("concurrent tool/calls hung — reader task must route replies by id");
    assert!(!a.is_error && !b.is_error, "a={a:?} b={b:?}");
    assert_eq!(a.content, "1", "{a:?}");
    assert_eq!(b.content, "2", "{b:?}");
}

#[tokio::test]
async fn prompt_context_returns_claimed_text() {
    let p = SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()])
        .await
        .unwrap();
    assert_eq!(p.prompt_context("/tmp").await.as_deref(), Some("CTX"));
}

#[tokio::test]
async fn tool_before_allow_and_command_joins_argv() {
    let p = SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()])
        .await
        .unwrap();
    assert_eq!(
        p.tool_before("shout", &serde_json::json!({})).await,
        ToolBefore::Allow
    );
    assert_eq!(
        p.run_command("/echo", vec!["hi".into(), "there".into()])
            .await,
        Some(CommandOutcome::Say("hi there".into()))
    );
    // Unclaimed command on a claiming plugin: no RPC, None.
    assert_eq!(p.run_command("/nope", vec![]).await, None);
}

#[tokio::test]
async fn unclaimed_hooks_fail_open_without_rpc() {
    // hang fixture never replies: any request would hit the 30s timeout, so
    // fast defaults prove the hooks/commands gate sends nothing.
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
        .await
        .unwrap();
    let t = std::time::Instant::now();
    assert_eq!(p.prompt_context("/tmp").await, None);
    assert_eq!(
        p.tool_before("x", &serde_json::json!({})).await,
        ToolBefore::Allow
    );
    assert_eq!(p.run_command("/echo", vec!["hi".into()]).await, None);
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn tool_before_parses_deny_modify_and_unknown() {
    let args = serde_json::json!({"path": "/x"});
    assert_eq!(
        ToolBefore::from_result(
            &serde_json::json!({"decision": "deny", "reason": "no"}),
            &args
        ),
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
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({}), &args),
        ToolBefore::Allow
    );
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "bogus"}), &args),
        ToolBefore::Allow
    );
}

#[tokio::test]
async fn echo_reference_plugin_manifest_and_command_round_trip() {
    // Reference plugin ships with the repo; boot the real script, not a fixture.
    let p = SidecarPlugin::spawn(vec!["../../plugins/echo/echo.sh".into()])
        .await
        .unwrap();
    let m = p.manifest();
    assert_eq!(m.name, "echo");
    assert_eq!(m.commands, vec!["/echo".to_string()]);
    assert_eq!(m.tools.len(), 1);
    assert_eq!(m.tools[0].name, "echo");
    assert!(
        m.tools[0].parameters.get("properties").is_some(),
        "{:?}",
        m.tools[0]
    );
    let out = p.tools()[0]
        .execute(&ToolContext::default(), serde_json::json!({"text": "hi"}))
        .await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.content, "hi", "{out:?}");
    assert_eq!(
        p.run_command("/echo", vec!["hello".into(), "world".into()])
            .await,
        Some(CommandOutcome::Say("hello world".into()))
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
        self.seen_systems
            .lock()
            .expect("seen lock")
            .push(req.system.clone());
        let script = self
            .scripted
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_default();
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
        self.calls
            .lock()
            .expect("calls lock")
            .push(name.to_string());
        let out = self.output.clone();
        Box::pin(async move { out })
    }
}

async fn e2e_hooks(cwd: &str) -> Vec<Arc<dyn PluginHooks>> {
    let p: Arc<dyn Plugin> = Arc::new(
        SidecarPlugin::spawn(vec!["testdata/hooks_plugin.sh".into()])
            .await
            .unwrap(),
    );
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
    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    let seen = seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 1, "one turn → one request, got {seen:?}");
    let system = seen[0].as_deref().unwrap_or("");
    assert!(
        system.contains("BASE-SYSTEM"),
        "base prompt preserved, got: {system}"
    );
    assert!(
        system.contains("CTX"),
        "sidecar prompt/context text present, got: {system}"
    );
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
        Box::new(RecordingExecutor {
            calls: calls.clone(),
            output: ToolOutput::ok("must-not-run"),
        }),
    )
    .with_tools(vec![ToolDef::new(
        "blocked",
        "deny me",
        serde_json::json!({}),
    )])
    .with_hooks(hooks);
    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert!(
        calls.lock().expect("calls lock").is_empty(),
        "denied tool must never execute"
    );
    let denied = agent.messages().iter().flat_map(|m| m.content.iter()).any(|b| {
        matches!(b, ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("BLOCKED-BY-E2E"))
    });
    assert!(
        denied,
        "deny reason must surface as an is_error tool result"
    );
}

#[tokio::test]
async fn e2e_sidecar_command_run_routes_echo() {
    let hooks = e2e_hooks("/tmp").await;
    // Same source the REPL reads via `agent.hooks()` for `/help` + routing.
    let names: Vec<String> = hooks
        .iter()
        .flat_map(|h| h.commands())
        .map(|c| c.name)
        .collect();
    assert!(
        names.contains(&"/echo".to_string()),
        "sidecar claims /echo, got {names:?}"
    );
    assert_eq!(
        hooks[0]
            .run_command("/echo", vec!["hello".into(), "world".into()])
            .await,
        Some(CommandOutcome::Say("hello world".into()))
    );
    // Empty-text replies filter to None so the REPL never prints a blank line.
    assert_eq!(hooks[0].run_command("/echo", vec![]).await, None);
}

#[tokio::test]
async fn command_run_prompt_variant() {
    let p = SidecarPlugin::spawn(vec!["testdata/prompt_command_plugin.sh".into()]).await.unwrap();
    assert_eq!(
        p.run_command("/ask", vec![]).await,
        Some(CommandOutcome::Prompt("hello from plugin".into()))
    );
    // Unclaimed command: no RPC, None.
    assert_eq!(p.run_command("/nope", vec![]).await, None);
}

#[tokio::test]
async fn session_param_present_when_claimed() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "gray-session-{}-{}",
        std::process::id(),
        n
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("session.log");
    let script = dir.join("logger.sh");
    // Logger sidecar: records every wire line with a session, replies canned.
    // Claims prompt/context + tool/before + /sess + tool `sess` so all 4
    // requests + notify are exercised.
    let body = format!(
        r#"#!/bin/sh
LOG="{}"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{{"id":%s,"result":{{"name":"sess","version":"0.1.0","protocol":"1.1","tools":["sess"],"commands":["/sess"],"hooks":["prompt/context","tool/before"]}}}}\n' "$id"
      ;;
    *plugin/shutdown*)
      exit 0
      ;;
    *prompt/context*|*tool/before*|*command/run*|*tool/call*|*event/notify*)
      printf '%s\n' "$line" >> "$LOG"
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      case "$line" in
        *prompt/context*) printf '{{"id":%s,"result":{{"text":"CTX"}}}}\n' "$id" ;;
        *tool/before*) printf '{{"id":%s,"result":{{"decision":"allow"}}}}\n' "$id" ;;
        *command/run*) printf '{{"id":%s,"result":{{"text":"ok"}}}}\n' "$id" ;;
        *tool/call*) printf '{{"id":%s,"result":{{"content":"ok"}}}}\n' "$id" ;;
      esac
      ;;
  esac
done
"#,
        log.display()
    );
    std::fs::write(&script, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()]).await.unwrap();

    // Drive all 4 requests + one notify with distinctive cwd/session.
    let prompt_cwd = "/tmp/session-prompt-cwd";
    assert_eq!(p.prompt_context(prompt_cwd).await.as_deref(), Some("CTX"));
    assert_eq!(p.tool_before("sess", &serde_json::json!({})).await, ToolBefore::Allow);
    assert_eq!(
        p.run_command("/sess", vec![]).await,
        Some(CommandOutcome::Say("ok".into()))
    );
    let mut ctx = ToolContext::default();
    ctx.cwd = "/tmp/session-tool-cwd".into();
    ctx.session_id = Some("sess-123".into());
    let out = p.tools()[0].execute(&ctx, serde_json::json!({})).await;
    assert!(!out.is_error, "{out:?}");
    use gray_plugin::CoreEvent;
    use gray_core::event::Usage;
    p.on_event(CoreEvent::TurnEnd { usage: Usage::default() }).await;
    // Notify is fire-and-forget: poll for the log line.
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_secs(2) {
        let Ok(text) = std::fs::read_to_string(&log) else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        };
        if text.lines().count() >= 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() >= 5, "4 requests + notify logged, got: {text}");
    // Every wire line carries a session object with id + cwd.
    for l in &lines {
        assert!(l.contains("\"session\""), "session present: {l}");
        assert!(l.contains("\"cwd\""), "cwd present: {l}");
    }
    // tool/call carries ctx session_id + cwd.
    let tool_line = lines.iter().find(|l| l.contains("tool/call")).expect("tool/call logged");
    assert!(tool_line.contains("sess-123"), "ctx session_id: {tool_line}");
    assert!(tool_line.contains("/tmp/session-tool-cwd"), "ctx cwd: {tool_line}");
    // prompt/context carries its cwd arg.
    let prompt_line =
        lines.iter().find(|l| l.contains("prompt/context")).expect("prompt/context logged");
    assert!(prompt_line.contains(prompt_cwd), "prompt cwd: {prompt_line}");
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn pre_v1_sidecar_never_receives_shutdown_line() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static M: AtomicU64 = AtomicU64::new(0);
    let n = M.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gray-prev1-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("pre_v1.log");
    // Tee wrapper: logs every stdin line, pipes to pre-v1 hooks fixture
    // (which has no shutdown handling and no `protocol`).
    let argv = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("tee \"{}\" | testdata/hooks_plugin.sh", log.display()),
    ];
    let p = SidecarPlugin::spawn(argv).await.unwrap();
    assert!(p.manifest().protocol.is_none(), "pre-v1 has no protocol");
    let t = std::time::Instant::now();
    p.shutdown(std::time::Duration::from_secs(1)).await;
    assert!(t.elapsed() < std::time::Duration::from_secs(6), "no hang on pre-v1 shutdown");
    // Give tee a moment to flush, then prove the shutdown line never arrived.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(logged.contains("plugin/manifest"), "tee saw traffic, got: {logged}");
    assert!(
        !logged.contains("plugin/shutdown"),
        "pre-v1 must never receive shutdown line, got: {logged}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn host_requests_round_trip_with_string_ids() {
    use gray_plugin::HostHandler;
    use std::sync::atomic::{AtomicU64, Ordering};
    static H: AtomicU64 = AtomicU64::new(0);
    let n = H.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gray-host-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("host.log");
    let script = dir.join("caller.sh");
    // Caller sidecar: answers manifest, then issues host/say + host/run
    // with STRING ids and logs the replies it gets back.
    let body = format!(
        r#"#!/bin/sh
LOG="{}"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{{"id":%s,"result":{{"name":"caller","version":"0.1.0","protocol":"1.1","tools":[]}}}}\n' "$id"
      sleep 0.2
      printf '{{"id":"s1","method":"host/say","params":{{"text":"hello host"}}}}\n'
      printf '{{"id":"s2","method":"host/run","params":{{"session":{{"id":"","cwd":"/tmp"}},"prompt":"do x"}}}}\n'
      ;;
    *'"result"'*)
      printf '%s\n' "$line" >> "$LOG"
      ;;
  esac
done
"#,
        log.display()
    );
    std::fs::write(&script, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()]).await.unwrap();
    let seen: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let handler: HostHandler = Arc::new(move |method: String, params: serde_json::Value| {
        let seen2 = seen2.clone();
        let fut: std::pin::Pin<
            Box<dyn std::future::Future<Output = serde_json::Value> + Send>,
        > = Box::pin(async move {
            seen2.lock().expect("seen lock").push((method.clone(), params));
            match method.as_str() {
                "host/say" => serde_json::json!({"ok": true}),
                _ => serde_json::json!({"text": "ran it"}),
            }
        });
        fut
    });
    p.set_host_handler(handler).await;
    // Poll for both replies (string ids echoed back, separate namespace).
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_secs(5) {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(seen.len(), 2, "host saw say + run, got {seen:?}");
    assert_eq!(seen[0].0, "host/say");
    assert_eq!(seen[1].0, "host/run");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(text.contains("\"s1\"") && text.contains("\"ok\""), "say reply: {text}");
    assert!(text.contains("\"s2\"") && text.contains("ran it"), "run reply: {text}");
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}
