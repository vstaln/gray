// UNRUN (cargo test banned under X): run in TTY/CI.

use super::*;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use gray_core::agent::{PluginHooks, Provider, ToolBefore, ToolExecutor};
use gray_core::event::StreamEvent;
use gray_core::message::{ChatRequest, Message, ToolDef};

/// Guards the exact TURN_STATE idiom used above: a poisoned turn-state
/// mutex must recover, never panic the REPL.
#[test]
fn turn_state_lock_survives_poison() {
    let m = std::sync::Mutex::new(Some(tokio_util::sync::CancellationToken::new()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison the mutex");
    }));
    *m.lock().unwrap_or_else(|e| e.into_inner()) = None;
    assert!(m.lock().unwrap_or_else(|e| e.into_inner()).is_none());
}

/// Provider whose responses are scripted up front, one event list per
/// request. `hang` streams the events and then never completes, so a cancel
/// lands mid-stream instead of after the turn already finished.
struct ScriptProvider {
    scripts: std::sync::Mutex<std::collections::VecDeque<Vec<StreamEvent>>>,
    hang: bool,
}

impl ScriptProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: std::sync::Mutex::new(std::collections::VecDeque::from(scripts)),
            hang: false,
        }
    }

    fn hanging(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: std::sync::Mutex::new(std::collections::VecDeque::from(scripts)),
            hang: true,
        }
    }
}

impl Provider for ScriptProvider {
    fn stream(&self, _req: ChatRequest) -> gray_core::agent::ProviderStream {
        let script = self
            .scripts
            .lock()
            .expect("scripts lock")
            .pop_front()
            .unwrap_or_default();
        let events: gray_core::agent::ProviderStream =
            Box::pin(futures::stream::iter(script.into_iter().map(Ok)));
        if self.hang {
            let hang: gray_core::agent::ProviderStream = Box::pin(futures::stream::pending());
            Box::pin(events.chain(hang))
        } else {
            events
        }
    }
}

/// Executor that never answers and ignores cancellation: a stuck child
/// process group, or a sidecar prompt waiting on a user who walked away.
struct StuckExecutor;

impl ToolExecutor for StuckExecutor {
    fn execute(
        &self,
        _ctx: &gray_core::agent::ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, gray_core::agent::ToolOutput> {
        Box::pin(std::future::pending())
    }
}

/// `tool/before` hook that never returns. The loop awaits it with no cancel
/// arm, so a Ctrl-C during a plugin permission prompt parks the whole turn
/// here — exactly the stall the REPL's bounded grace has to survive.
struct StuckBeforeHook;

#[async_trait::async_trait]
impl PluginHooks for StuckBeforeHook {
    async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
        std::future::pending().await
    }
}

fn stub_tool() -> ToolDef {
    ToolDef::new(
        "lookup",
        "A fake lookup tool",
        serde_json::json!({"type":"object"}),
    )
}

fn tool_round(id: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("checking..."),
        StreamEvent::tool_call_delta(0, Some(id.to_string()), Some("lookup".into()), "{}"),
        StreamEvent::message_complete(Some(gray_core::event::StopReason::ToolUse), None),
    ]
}

fn answer_round() -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("done"),
        StreamEvent::message_complete(Some(gray_core::event::StopReason::EndTurn), None),
    ]
}

/// Every `tool_use` the assistant asked for must carry a `tool_result`:
/// strict providers 400 on an orphaned call and the saved session is then
/// permanently broken. This is the transcript `persist_turn_messages`
/// writes verbatim after an interrupt.
fn orphaned_tool_uses(messages: &[Message]) -> Vec<String> {
    let mut answered = std::collections::HashSet::new();
    let mut asked = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                gray_core::message::ContentBlock::ToolUse { id, .. } => asked.push(id.clone()),
                gray_core::message::ContentBlock::ToolResult { id, .. } => {
                    answered.insert(id.clone());
                }
                _ => {}
            }
        }
    }
    asked
        .into_iter()
        .filter(|id| !answered.contains(id))
        .collect()
}

/// Cancel from a watcher task, the way the key watcher does mid-turn.
fn spawn_canceller(cancel: &tokio_util::sync::CancellationToken) -> tokio::task::JoinHandle<()> {
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        token.cancel();
    })
}

fn ctx(cancel: tokio_util::sync::CancellationToken) -> gray_core::agent::ToolContext {
    gray_core::agent::ToolContext {
        cwd: std::env::temp_dir(),
        cancel,
        session_id: None,
    }
}

/// The repro: Ctrl-C lands while the loop is parked in a plugin hook that
/// nothing can interrupt. The REPL's grace expires, the run future is
/// dropped, and whatever reaches `persist_turn_messages` must still be an
/// answerable transcript — the interrupted turn must save completely.
#[tokio::test]
async fn interrupt_with_stuck_hook_saves_answerable_transcript() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut agent = gray_core::agent::Agent::new(
        Box::new(ScriptProvider::new(vec![tool_round("c1"), answer_round()])),
        std::sync::Arc::new(StuckExecutor),
    )
    .with_tools(vec![stub_tool()])
    .with_hooks(vec![std::sync::Arc::new(StuckBeforeHook)]);
    let watcher = spawn_canceller(&cancel);
    let mut events = 0usize;

    let err = run_streaming_cancellable(
        &mut agent,
        Message::user("go"),
        ctx(cancel.clone()),
        &cancel,
        &mut |_| events += 1,
    )
    .await
    .expect_err("cancelled turn must surface Cancelled");

    assert!(
        matches!(err, gray_core::error::CoreError::Cancelled),
        "{err:?}"
    );
    assert!(events > 0, "the wrapper must still forward live events");
    let _ = watcher.await;
    assert_eq!(
        orphaned_tool_uses(agent.messages()),
        Vec::<String>::new(),
        "an interrupted turn must never persist an unanswered tool_use: {:?}",
        agent.messages()
    );
}

/// Same contract for a tool that ignores cancellation outright: the loop's
/// own grace answers it, so the saved transcript keeps its result.
#[tokio::test]
async fn interrupt_with_stuck_tool_saves_answerable_transcript() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut agent = gray_core::agent::Agent::new(
        Box::new(ScriptProvider::new(vec![tool_round("c1"), answer_round()])),
        std::sync::Arc::new(StuckExecutor),
    )
    .with_tools(vec![stub_tool()]);
    let watcher = spawn_canceller(&cancel);
    let mut events = 0usize;

    let err = run_streaming_cancellable(
        &mut agent,
        Message::user("go"),
        ctx(cancel.clone()),
        &cancel,
        &mut |_| events += 1,
    )
    .await
    .expect_err("cancelled turn must surface Cancelled");

    assert!(
        matches!(err, gray_core::error::CoreError::Cancelled),
        "{err:?}"
    );
    assert!(events > 0, "the wrapper must still forward live events");
    let _ = watcher.await;
    assert!(
        orphaned_tool_uses(agent.messages()).is_empty(),
        "a cancelled tool must still land its result: {:?}",
        agent.messages()
    );
}

/// Mid-stream text the user already saw must survive the interrupt: the
/// transcript is what a resumed session replays.
#[tokio::test]
async fn interrupt_keeps_streamed_text_visible_on_resume() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut agent = gray_core::agent::Agent::new(
        Box::new(ScriptProvider::hanging(vec![vec![
            StreamEvent::text_delta("half an answer"),
        ]])),
        std::sync::Arc::new(StuckExecutor),
    );
    let watcher = spawn_canceller(&cancel);
    let mut events = 0usize;

    run_streaming_cancellable(
        &mut agent,
        Message::user("go"),
        ctx(cancel.clone()),
        &cancel,
        &mut |_| events += 1,
    )
    .await
    .expect_err("cancelled turn must surface Cancelled");

    assert!(events > 0, "the wrapper must still forward live events");
    let _ = watcher.await;
    let text: String = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            gray_core::message::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(
        text.contains("half an answer"),
        "streamed text must be salvaged into the transcript: {text:?}"
    );
}
