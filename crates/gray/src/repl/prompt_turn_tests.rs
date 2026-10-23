use super::*;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use gray_core::agent::{PluginHooks, Provider, ToolBefore, ToolExecutor};
use gray_core::event::StreamEvent;
use gray_core::message::{ChatRequest, Message, ToolDef};

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

#[test]
fn steer_takes_the_oldest_text_entry_and_leaves_the_rest() {
    let mut queued = std::collections::VecDeque::from([
        ("first".to_string(), vec![]),
        (
            "with image".to_string(),
            vec![std::path::PathBuf::from("/tmp/x.png")],
        ),
        ("third".to_string(), vec![]),
    ]);
    assert_eq!(super::take_steerable(&mut queued).as_deref(), Some("first"));
    let left: Vec<&str> = queued.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(left, vec!["with image", "third"]);
}

#[test]
fn steer_drops_empty_entries_and_takes_the_next() {
    let mut queued = std::collections::VecDeque::from([
        (String::new(), vec![]),
        ("   ".to_string(), vec![]),
        ("\u{1b}[3~".to_string(), vec![]),
        ("real".to_string(), vec![]),
    ]);
    assert_eq!(super::take_steerable(&mut queued).as_deref(), Some("real"));
    assert!(queued.is_empty());
}

#[test]
fn pop_actionable_skips_empty_entries_and_keeps_images() {
    let mut queued = std::collections::VecDeque::from([
        (String::new(), vec![]),
        ("\u{1b}".to_string(), vec![]),
        (
            "kept".to_string(),
            vec![std::path::PathBuf::from("/tmp/i.png")],
        ),
        ("next".to_string(), vec![]),
    ]);
    let (text, images) = super::pop_actionable(&mut queued).expect("a real entry");
    assert_eq!(text, "kept");
    assert_eq!(images.len(), 1);
    assert_eq!(queued.len(), 1);
}

#[test]
fn steer_on_an_empty_queue_takes_nothing() {
    let mut queued = std::collections::VecDeque::new();
    assert_eq!(super::take_steerable(&mut queued), None);

    let mut images = std::collections::VecDeque::from([(
        "pic".to_string(),
        vec![std::path::PathBuf::from("/tmp/x.png")],
    )]);
    assert_eq!(super::take_steerable(&mut images), None);
    assert_eq!(images.len(), 1, "the image entry keeps its place");
}
