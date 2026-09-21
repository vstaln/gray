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
    // arXiv:2512.22087 stable anchor: [anchor, summary, retained...].
    // The anchor is charged against the retained budget (200 ≈ 2 messages
    // here), so it displaces the oldest retained message rather than
    // inflating the request: anchor + summary + newest message.
    assert_eq!(
        msgs.len(),
        3,
        "anchor + summary + 1 retained, got {}",
        msgs.len()
    );
    assert!(
        msgs[0].text_content().contains("msg1"),
        "original user intent is pinned FIRST: {}",
        msgs[0].text_content().chars().take(20).collect::<String>()
    );
    assert!(
        msgs[1].text_content().contains("FIXED-SUMMARY-123"),
        "summary carries the provider summary, second"
    );
    assert!(
        msgs[2].text_content().contains("msg6"),
        "newest closes history: {}",
        msgs[2].text_content().chars().take(20).collect::<String>()
    );
}

#[tokio::test]
async fn recompaction_repins_the_same_anchor() {
    // The anchor is recovered from the transcript (summary envelope at
    // index 1), so a second compaction re-pins the SAME original intent
    // instead of pinning the previous summary.
    let mut agent = test_agent("SUM").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=8).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    assert!(agent.try_compact_budgeted().await.unwrap());
    let first_anchor = agent.messages()[0].text_content();
    assert!(
        first_anchor.contains("msg1"),
        "anchor is the intent, not the summary"
    );
    // Grow past the budget again and compact a second time.
    let mut grown = agent.messages().to_vec();
    for i in 0..8 {
        grown.push(sized_msg(&format!("later{i}")));
    }
    agent.set_messages(grown);
    assert!(agent.try_compact_budgeted().await.unwrap());
    let msgs = agent.messages();
    assert_eq!(
        msgs[0].text_content(),
        first_anchor,
        "re-compaction must re-pin the same original intent"
    );
    assert!(
        msgs[1].text_content().contains("SUM"),
        "summary still follows the anchor"
    );
}

#[tokio::test]
async fn tiny_history_keeps_the_anchor_unduplicated() {
    // When the retained tail already holds the first message (small
    // histories the budget fully covers never reach here, but the guard is
    // real), the anchor is not duplicated.
    let mut agent = test_agent("SUM").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    assert!(agent.try_compact_budgeted().await.unwrap());
    let anchors = agent
        .messages()
        .iter()
        .filter(|m| m.text_content().starts_with("msg1:"))
        .count();
    assert_eq!(anchors, 1, "the intent appears exactly once");
}

#[tokio::test]
async fn pipeline_summary_first_then_retained() {
    // 4 messages over a small window, trigger call returns "S":
    // assembly is [anchor, summary, retained...] (pi + the stable anchor),
    // trigger nowhere in history.
    let mut agent = test_agent("S").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(ok);
    let msgs = agent.messages();
    // Anchor is charged against the 200-token budget, so the retained tail
    // shrinks to the newest message.
    assert_eq!(
        msgs.len(),
        3,
        "anchor + summary + 1 retained, got {}",
        msgs.len()
    );
    assert!(
        msgs[0].text_content().contains("msg1"),
        "the pinned intent leads"
    );
    assert!(
        msgs[1]
            .text_content()
            .contains("compacted into the following summary"),
        "summary follows the anchor"
    );
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

#[tokio::test]
async fn anchor_leads_even_when_the_walk_circles_back() {
    // Regression: the newest-first retained walk can circle all the way
    // back to the oldest messages when the middle groups do not fit the
    // budget, keeping the intent inside the tail. It must still be pinned
    // exactly once, at the front (arXiv:2512.22087 fixed segment).
    let mut agent = test_agent("LIVE-SUM").with_context_window(Some(24_000));
    let big = "y".repeat(40_000); // ~10k tokens of tool output per round
    let mut msgs = vec![Message::user("ANCHOR-TOKEN-7742: cat these files")];
    for _ in 0..3 {
        msgs.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "bash",
                serde_json::json!({"command": "cat x"}),
            )],
        ));
        msgs.push(Message::new(
            Role::User,
            vec![ContentBlock::tool_result("t1", big.clone(), false)],
        ));
    }
    msgs.push(Message::assistant("Summaries — one line each"));
    agent.set_messages(msgs);
    assert!(agent.try_compact_budgeted().await.unwrap());
    let out = agent.messages();
    assert!(
        out[0].text_content().starts_with("ANCHOR-TOKEN-7742"),
        "the pinned intent leads: {:?}",
        out.iter()
            .map(|m| m.text_content().chars().take(20).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        out[1]
            .text_content()
            .contains("compacted into the following summary"),
        "summary follows the anchor"
    );
    assert!(
        out.last().unwrap().text_content().contains("Summaries"),
        "retained tail closes history"
    );
    assert_eq!(
        out.iter()
            .filter(|m| m.text_content().starts_with("ANCHOR-TOKEN-7742"))
            .count(),
        1,
        "the intent is pinned exactly once"
    );
}
