// UNRUN (cargo test banned under X): run in TTY/CI.
// `Agent::repair_dropped_cancel` — the transcript repair a REPL runs when its
// bounded cancel grace expires and the run future has to be dropped.

use super::*;
use crate::message::{ContentBlock, Message, Role};

/// Neither the provider nor the executor is ever reached: these tests only
/// inspect and repair an existing transcript.
struct Noop;

impl Provider for Noop {
    fn stream(&self, _: ChatRequest) -> ProviderStream {
        Box::pin(futures::stream::empty())
    }
}

#[async_trait]
impl ToolExecutor for Noop {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, ToolOutput> {
        Box::pin(async { ToolOutput::ok("") })
    }
}

fn agent_with(messages: Vec<Message>) -> Agent {
    let mut agent = Agent::new(Box::new(Noop), Arc::new(Noop));
    agent.set_messages(messages);
    agent
}

fn orphan_ids(agent: &Agent) -> Vec<String> {
    let mut answered = std::collections::HashSet::new();
    let mut asked = Vec::new();
    for message in agent.messages() {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { id, .. } => asked.push(id.clone()),
                ContentBlock::ToolResult { id, .. } => {
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

/// Prose the assistant produced, in order (user turns excluded).
fn texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn unanswered_tool_calls_get_a_synthetic_result() {
    let mut agent = agent_with(vec![
        Message::user("go"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "checking...".into(),
                },
                ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "lookup".into(),
                    args: serde_json::json!({}),
                },
            ],
        },
    ]);

    agent.repair_dropped_cancel("", "");

    assert!(orphan_ids(&agent).is_empty(), "{:?}", agent.messages());
    let results: Vec<&ContentBlock> = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .collect();
    assert_eq!(results.len(), 1, "{:?}", agent.messages());
    match results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(content.contains("cancelled by user"), "{content}");
            assert!(is_error, "a cancelled call is an error result");
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
}

#[test]
fn a_turn_that_already_answered_is_left_untouched() {
    let mut agent = agent_with(vec![
        Message::user("go"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "lookup".into(),
                args: serde_json::json!({}),
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: "c1".into(),
                content: "ok".into(),
                is_error: false,
            }],
        },
        Message::assistant("all done"),
    ]);
    let before = agent.messages().to_vec();

    agent.repair_dropped_cancel("", "");

    assert_eq!(
        agent.messages(),
        before.as_slice(),
        "a run that finished its own cleanup must not be appended to"
    );
}

#[test]
fn streamed_text_is_salvaged_once() {
    let mut agent = agent_with(vec![Message::user("go")]);

    agent.repair_dropped_cancel("thinking...", "half an answer");

    assert_eq!(texts(&agent), vec!["half an answer".to_string()]);
    assert!(
        agent.messages()[1]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. })),
        "the thinking the user saw is salvaged with the text"
    );

    // The loop's own finalize already wrote this text: no duplicate.
    agent.repair_dropped_cancel("thinking...", "half an answer");
    assert_eq!(texts(&agent), vec!["half an answer".to_string()]);
}

#[test]
fn synthetic_results_do_not_hide_earlier_turns() {
    // A later round's orphan must not make round one's text look unsaved:
    // the turn boundary is the last genuine user message, not the last
    // user-role message (synthetic results are user-role too).
    let mut agent = agent_with(vec![
        Message::user("first"),
        Message::assistant("first answer"),
        Message::user("second"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "second partial".into(),
                },
                ContentBlock::ToolUse {
                    id: "c2".into(),
                    name: "lookup".into(),
                    args: serde_json::json!({}),
                },
            ],
        },
    ]);

    agent.repair_dropped_cancel("", "second partial");

    assert!(orphan_ids(&agent).is_empty());
    assert_eq!(
        texts(&agent),
        vec!["first answer".to_string(), "second partial".to_string()],
        "the already-finalized text must not be duplicated"
    );
}
