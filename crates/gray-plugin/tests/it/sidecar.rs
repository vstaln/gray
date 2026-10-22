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

    assert_eq!(p.run_command("/nope", vec![]).await, None);
}

#[tokio::test]
async fn unclaimed_hooks_fail_open_without_rpc() {
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
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "deny", "reason": "no"})),
        ToolBefore::Deny("no".into())
    );
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "modify", "args": {"path": "/y"}})),
        ToolBefore::Modify(serde_json::json!({"path": "/y"}))
    );

    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({})),
        ToolBefore::Deny("plugin returned an unrecognized policy verdict".into())
    );
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "bogus"})),
        ToolBefore::Deny("plugin returned an unrecognized policy verdict".into())
    );
    assert_eq!(
        ToolBefore::from_result(&serde_json::json!({"decision": "modify"})),
        ToolBefore::Deny("plugin modify verdict missing object args".into())
    );
}

#[tokio::test]
async fn echo_reference_plugin_manifest_and_command_round_trip() {
    let p = SidecarPlugin::spawn(vec!["testdata/echo.sh".into()])
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

struct ScriptedProvider {
    scripted: std::sync::Mutex<std::collections::VecDeque<Vec<StreamEvent>>>,
    seen_requests: Arc<std::sync::Mutex<Vec<ChatRequest>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripted: std::sync::Mutex::new(scripts.into()),
            seen_requests: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen_requests.lock().expect("seen lock").push(req);
        let script = self
            .scripted
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

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
async fn e2e_sidecar_prompt_context_lands_as_context_note() {
    let hooks = e2e_hooks("/tmp").await;
    let provider = ScriptedProvider::new(vec![end_script("done")]);
    let seen = provider.seen_requests.clone();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(RecordingExecutor {
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
    let req = &seen[0];
    assert_eq!(
        req.system.as_deref(),
        Some("BASE-SYSTEM"),
        "hook text must not perturb the system prefix"
    );
    assert!(
        req.messages
            .iter()
            .any(|m| m.text_content() == "[Context update]\nCTX"),
        "sidecar prompt/context text rides a context note in history: {:?}",
        req.messages
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
        Arc::new(RecordingExecutor {
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

    assert_eq!(hooks[0].run_command("/echo", vec![]).await, None);
}

#[tokio::test]
async fn command_run_prompt_variant() {
    let p = SidecarPlugin::spawn(vec!["testdata/prompt_command_plugin.sh".into()])
        .await
        .unwrap();
    assert_eq!(
        p.run_command("/ask", vec![]).await,
        Some(CommandOutcome::Prompt("hello from plugin".into()))
    );

    assert_eq!(p.run_command("/nope", vec![]).await, None);
}

#[tokio::test]
async fn session_param_present_when_claimed() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gray-session-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("session.log");
    let script = dir.join("logger.sh");

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

    let staging = dir.join("caller.sh.new");
    std::fs::write(&staging, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::rename(&staging, &script).unwrap();
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();

    let prompt_cwd = "/tmp/session-prompt-cwd";
    assert_eq!(p.prompt_context(prompt_cwd).await.as_deref(), Some("CTX"));
    assert_eq!(
        p.tool_before("sess", &serde_json::json!({})).await,
        ToolBefore::Allow
    );
    assert_eq!(
        p.run_command("/sess", vec![]).await,
        Some(CommandOutcome::Say("ok".into()))
    );
    let ctx = ToolContext {
        cwd: "/tmp/session-tool-cwd".into(),
        session_id: Some("sess-123".into()),
        ..Default::default()
    };
    let out = p.tools()[0].execute(&ctx, serde_json::json!({})).await;
    assert!(!out.is_error, "{out:?}");
    use gray_core::event::Usage;
    use gray_plugin::CoreEvent;
    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;

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

    for l in &lines {
        assert!(l.contains("\"session\""), "session present: {l}");
        assert!(l.contains("\"cwd\""), "cwd present: {l}");
    }

    let tool_line = lines
        .iter()
        .find(|l| l.contains("tool/call"))
        .expect("tool/call logged");
    assert!(
        tool_line.contains("sess-123"),
        "ctx session_id: {tool_line}"
    );
    assert!(
        tool_line.contains("/tmp/session-tool-cwd"),
        "ctx cwd: {tool_line}"
    );

    let prompt_line = lines
        .iter()
        .find(|l| l.contains("prompt/context"))
        .expect("prompt/context logged");
    assert!(
        prompt_line.contains(prompt_cwd),
        "prompt cwd: {prompt_line}"
    );
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

    let argv = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("tee \"{}\" | testdata/hooks_plugin.sh", log.display()),
    ];
    let p = SidecarPlugin::spawn(argv).await.unwrap();
    assert!(p.manifest().protocol.is_none(), "pre-v1 has no protocol");
    let t = std::time::Instant::now();
    p.shutdown(std::time::Duration::from_secs(1)).await;
    assert!(
        t.elapsed() < std::time::Duration::from_secs(6),
        "no hang on pre-v1 shutdown"
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        logged.contains("plugin/manifest"),
        "tee saw traffic, got: {logged}"
    );
    assert!(
        !logged.contains("plugin/shutdown"),
        "pre-v1 must never receive shutdown line, got: {logged}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_ungranted_host_call_is_refused_with_the_grant_command() {
    use gray_plugin::capabilities::capability_for_host_method;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("gray-nogrant-{}-{n}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("host.log");
    let script = dir.join("caller.sh");
    let body = format!(
        r#"#!/bin/sh
LOG="{}"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{{"id":%s,"result":{{"name":"caller","version":"0.1.0","protocol":"1.1","tools":[],"capabilities":["host.ask"]}}}}\n' "$id"
      sleep 0.2
      printf '{{"id":"s1","method":"host/ask","params":{{"questions":[],"blocking":true}}}}\n'
      ;;
    *'"result"'*)
      printf '%s\n' "$line" >> "$LOG"
      ;;
  esac
done
"#,
        log.display()
    );

    let staging = dir.join("caller.sh.new");
    std::fs::write(&staging, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::rename(&staging, &script).unwrap();

    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();
    let cap = capability_for_host_method("host/ask").expect("host/ask maps to a capability");
    assert!(
        p.manifest().capabilities.contains(&"host.ask".to_string()),
        "the fixture declares what it wants"
    );
    assert!(
        p.capabilities().is_empty(),
        "nothing is granted until the host grants it"
    );
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_secs(5) {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("capability_not_granted") {
            assert!(
                text.contains(cap),
                "the error names the capability, got: {text}"
            );
            assert!(
                text.contains("gray plugin capabilities"),
                "the error names how to grant it, got: {text}"
            );

            assert!(!text.contains("no host handler"));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "ungranted host/ask was not refused; log: {}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
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

    let body = format!(
        r#"#!/bin/sh
LOG="{}"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{{"id":%s,"result":{{"name":"caller","version":"0.1.0","protocol":"1.1","tools":[],"capabilities":["host.say","host.turn"]}}}}\n' "$id"
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

    let staging = dir.join("caller.sh.new");
    std::fs::write(&staging, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::rename(&staging, &script).unwrap();
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();
    let seen: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let handler: HostHandler = Arc::new(move |method: String, params: serde_json::Value| {
        let seen2 = seen2.clone();
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>> =
            Box::pin(async move {
                seen2
                    .lock()
                    .expect("seen lock")
                    .push((method.clone(), params));
                match method.as_str() {
                    "host/say" => serde_json::json!({"ok": true}),
                    _ => serde_json::json!({"text": "ran it"}),
                }
            });
        fut
    });
    p.set_host_handler(handler).await;

    p.set_capabilities(vec!["host.say".into(), "host.turn".into()]);

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
    assert!(
        text.contains("\"s1\"") && text.contains("\"ok\""),
        "say reply: {text}"
    );
    assert!(
        text.contains("\"s2\"") && text.contains("ran it"),
        "run reply: {text}"
    );
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

fn write_exit_after_manifest_script(
    dir: &std::path::Path,
    code: i32,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let runs = dir.join("runs.log");
    let first = dir.join("first.marker");
    let script = dir.join(format!("exit{code}.sh"));
    let body = format!(
        r#"#!/bin/sh
RUNS="{runs}"
FIRST="{first}"
echo run >> "$RUNS"
if [ -f "$FIRST" ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"id"'*)
        id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
        printf '{{"id":%s,"result":{{}}}}\n' "$id"
        ;;
    esac
  done
else
  touch "$FIRST"
  while IFS= read -r line; do
    case "$line" in
      *plugin/manifest*)
        id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
        printf '{{"id":%s,"result":{{"name":"exit{code}","version":"0.1.0","protocol":"1.1","tools":[]}}}}\n' "$id"
        exit {code}
        ;;
    esac
  done
fi
"#,
        runs = runs.display(),
        first = first.display(),
        code = code
    );
    let staging = dir.join(format!("exit{code}.sh.new"));
    std::fs::write(&staging, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::rename(&staging, &script).unwrap();
    (script, runs)
}

async fn drive_until_respawned(p: &SidecarPlugin, runs: &std::path::Path) {
    use gray_core::event::Usage;
    use gray_plugin::CoreEvent;
    let t = std::time::Instant::now();
    loop {
        p.on_event(CoreEvent::TurnEnd {
            usage: Usage::default(),
        })
        .await;
        let n = std::fs::read_to_string(runs)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        if n >= 2 {
            return;
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(10),
            "sidecar was not respawned; runs: {n}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn clean_exit_respawns_without_host_say() {
    use gray_plugin::HostHandler;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gray-exit0-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, runs) = write_exit_after_manifest_script(&dir, 0);
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();

    let seen: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let handler: HostHandler = Arc::new(move |method: String, params: serde_json::Value| {
        let seen2 = seen2.clone();
        Box::pin(async move {
            seen2.lock().expect("seen lock").push((method, params));
            serde_json::json!({"ok": true})
        })
    });
    p.set_host_handler(handler).await;

    drive_until_respawned(&p, &runs).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let seen = seen.lock().expect("seen lock").clone();
    assert!(
        seen.iter().all(|(m, _)| m != "host/say"),
        "a clean exit must not produce a host/say notice, got {seen:?}"
    );
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn nonzero_exit_respawns_with_one_plugin_crash_say() {
    use gray_plugin::HostHandler;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gray-exit1-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, runs) = write_exit_after_manifest_script(&dir, 1);
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();

    let seen: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let handler: HostHandler = Arc::new(move |method: String, params: serde_json::Value| {
        let seen2 = seen2.clone();
        Box::pin(async move {
            seen2.lock().expect("seen lock").push((method, params));
            serde_json::json!({"ok": true})
        })
    });
    p.set_host_handler(handler).await;

    drive_until_respawned(&p, &runs).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let seen = seen.lock().expect("seen lock").clone();
    let says: Vec<_> = seen.iter().filter(|(m, _)| m == "host/say").collect();
    assert_eq!(says.len(), 1, "expected exactly one host/say, got {seen:?}");
    let params = &says[0].1;
    assert_eq!(params["kind"], "plugin_crash");
    assert_eq!(params["plugin"], "exit1");
    assert_eq!(params["status"], "exit 1");
    assert_eq!(params["restarted"], true);
    assert!(
        params["text"].as_str().is_some_and(|t| t.contains("exit1")),
        "fallback text should name the plugin: {params}"
    );
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn missing_executable_backs_off_instead_of_announcing_every_call() {
    use gray_core::event::Usage;
    use gray_plugin::{CoreEvent, HostHandler};
    let dir = std::env::temp_dir().join(format!("gray-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, runs) = write_exit_after_manifest_script(&dir, 1);
    let p = SidecarPlugin::spawn(vec![script.to_string_lossy().to_string()])
        .await
        .unwrap();
    let says = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let says2 = says.clone();
    let handler: HostHandler = Arc::new(move |method: String, _params: serde_json::Value| {
        if method == "host/say" {
            says2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Box::pin(async move { serde_json::json!({"ok": true}) })
    });
    p.set_host_handler(handler).await;
    std::fs::remove_file(&script).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    for _ in 0..30 {
        p.on_event(CoreEvent::TurnEnd {
            usage: Usage::default(),
        })
        .await;
    }
    assert_eq!(
        says.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a missing executable must be announced once, not once per hook call"
    );
    assert_eq!(std::fs::read_to_string(&runs).unwrap().lines().count(), 1);
    p.shutdown(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}
