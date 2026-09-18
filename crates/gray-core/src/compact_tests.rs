use super::*;
use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
use crate::event::StreamEvent;
use crate::message::{ChatRequest, ContentBlock, Role};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Scripted provider for `complete_with_history`: returns one fixed
/// summary text as `TextDelta` + `MessageComplete`, mirroring the
/// production consumption of `StreamEvent::TextDelta`/`MessageComplete`.
struct SummaryProvider {
    text: String,
}

#[async_trait]
impl Provider for SummaryProvider {
    fn stream(&self, _req: ChatRequest) -> ProviderStream {
        let text = self.text.clone();
        Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::text_delta(text)),
            Ok(StreamEvent::message_complete(None, None)),
        ]))
    }
}

struct NoopExecutor;

#[async_trait]
impl ToolExecutor for NoopExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        Box::pin(async move { ToolOutput::ok("unused") })
    }
}

fn test_agent(summary_text: &str) -> Agent {
    Agent::new(
        Box::new(SummaryProvider {
            text: summary_text.to_string(),
        }),
        Arc::new(NoopExecutor),
    )
}

/// One ~100-token message by the estimator (bytes/4): 400 chars.
fn sized_msg(tag: &str) -> Message {
    Message::user(format!("{tag}:{}", "x".repeat(400 - tag.len() - 1)))
}

#[tokio::test]
async fn budgeted_compact_keeps_recent_tail() {
    // pi order: [summary, retained...] (summary first, no ack).
    let mut agent = test_agent("FIXED-SUMMARY-123").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    // Retained budget = min(64k, 16_200−16_000) = 200 ≈ last 2 messages.
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(ok);
    let msgs = agent.messages();
    assert_eq!(msgs.len(), 3, "summary + 2 retained, got {}", msgs.len());
    assert!(
        msgs[0].text_content().contains("FIXED-SUMMARY-123"),
        "summary carries the provider summary, FIRST"
    );
    assert!(
        msgs[1].text_content().contains("msg5"),
        "retained order: {}",
        msgs[1].text_content().chars().take(20).collect::<String>()
    );
    assert!(
        msgs[2].text_content().contains("msg6"),
        "newest closes history"
    );
}

#[tokio::test]
async fn pipeline_summary_first_then_retained() {
    // 4 messages over a small window, trigger call returns "S":
    // assembly is [summary, retained...] (pi), trigger nowhere in history.
    let mut agent = test_agent("S").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(ok);
    let msgs = agent.messages();
    assert_eq!(msgs.len(), 3, "summary + 2 retained, got {}", msgs.len());
    assert!(
        msgs[0]
            .text_content()
            .contains("compacted into the following summary"),
        "summary leads"
    );
    assert!(msgs[1].text_content().contains("msg3"), "retained oldest");
    assert!(
        msgs[2].text_content().contains("msg4"),
        "retained newest last"
    );
    assert!(
        msgs.iter().all(|m| !m
            .text_content()
            .contains(crate::compact::COMPACTION_TRIGGER)),
        "trigger must never leak into history"
    );
}

#[tokio::test]
async fn compact_v2_returns_summary_and_honors_zero_budget() {
    let mut agent = test_agent("FIXED-SUMMARY-123");
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let out = agent.compact_v2(None, Some(0)).await.unwrap();
    assert_eq!(out.as_deref(), Some("FIXED-SUMMARY-123"));
    assert_eq!(
        agent.messages().len(),
        1,
        "keep=0 → summary only, got {}",
        agent.messages().len()
    );
    assert!(
        agent.messages()[0]
            .text_content()
            .contains("FIXED-SUMMARY-123"),
        "returned summary is the installed summary"
    );
}

#[tokio::test]
async fn budgeted_compact_false_when_nothing_to_gain() {
    let mut agent = test_agent("UNUSED");
    agent.set_messages(vec![Message::user("hi")]);
    let before = agent.messages().to_vec();
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(!ok);
    assert_eq!(agent.messages(), &before);
}

#[tokio::test]
async fn budgeted_compact_false_when_summary_would_not_shrink() {
    // Already-compact history (a prior summary) and a provider summary
    // LONGER than what it would replace. The enforced shrink invariant must
    // return Ok(false) with history byte-identical.
    let long = "z".repeat(2000);
    let mut agent = test_agent(&long);
    agent.set_messages(vec![summary_message("prior summary")]);
    let before = agent.messages().to_vec();
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(!ok);
    assert_eq!(
        agent.messages(),
        &before,
        "non-shrinking compact must leave history untouched"
    );
}

#[tokio::test]
async fn budget_math() {
    assert!(needs_pre_turn_compact(120_000, Some(128_000))); // 120k+16k >= 128k
    assert!(!needs_pre_turn_compact(100_000, Some(128_000)));
    assert!(!needs_pre_turn_compact(999_999_999, None)); // unknown window: never
}

/// Provider whose stream immediately fails: exercises the provider-error
/// path of `try_compact_budgeted` (same pre-swap early return as cancel).
struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    fn stream(&self, _req: ChatRequest) -> ProviderStream {
        Box::pin(futures::stream::iter(vec![Err(
            crate::agent::ProviderError::Connection("boom".to_string()),
        )]))
    }
}

/// Trimmable history: 3 × 2500-token tool results over a 5100 window, so
/// an in-place pre-trim would rewrite the newest result before the
/// summary call runs.
fn trimmable_history() -> Vec<Message> {
    ["c1", "c2", "c3"]
        .into_iter()
        .map(|id| Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: id.to_string(),
                content: "x".repeat(10_000),
                is_error: false,
            }],
        })
        .collect()
}

fn transcript_bytes(msgs: &[Message]) -> String {
    serde_json::to_string(msgs).expect("transcript serializes")
}

#[tokio::test]
async fn compact_error_leaves_history_byte_identical() {
    let mut agent = Agent::new(Box::new(FailingProvider), Arc::new(NoopExecutor))
        .with_context_window(Some(5100));
    agent.set_messages(trimmable_history());
    let before = transcript_bytes(agent.messages());
    let err = agent.try_compact_budgeted().await;
    assert!(err.is_err(), "provider failure must propagate");
    assert_eq!(
        transcript_bytes(agent.messages()),
        before,
        "failed compact must leave history byte-identical"
    );
}

#[tokio::test]
async fn compact_blank_summary_leaves_history_byte_identical() {
    let mut agent = test_agent("   ").with_context_window(Some(5100));
    agent.set_messages(trimmable_history());
    let before = transcript_bytes(agent.messages());
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(!ok);
    assert_eq!(
        transcript_bytes(agent.messages()),
        before,
        "blank-summary compact must leave history byte-identical"
    );
}
