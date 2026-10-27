use super::*;
use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
use crate::agent_loop::CONTAMINATED_SCRUB_MARKER;
use crate::event::StreamEvent;
use crate::message::{ChatRequest, ContentBlock, Role};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::sync::Arc;

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

struct RecordingProvider {
    text: String,
    seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
}

#[async_trait]
impl Provider for RecordingProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.messages.clone());
        let text = self.text.clone();
        Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::text_delta(text)),
            Ok(StreamEvent::message_complete(None, None)),
        ]))
    }
}

fn recording_agent(
    summary_text: &str,
) -> (Agent, std::sync::Arc<std::sync::Mutex<Vec<Vec<Message>>>>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let agent = Agent::new(
        Box::new(RecordingProvider {
            text: summary_text.to_string(),
            seen: std::sync::Arc::clone(&seen),
        }),
        Arc::new(NoopExecutor),
    );
    (agent, seen)
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

fn sized_msg(tag: &str) -> Message {
    Message::user(format!("{tag}:{}", "x".repeat(400 - tag.len() - 1)))
}

#[tokio::test]
async fn budgeted_compact_keeps_recent_tail() {
    let mut agent = test_agent("FIXED-SUMMARY-123").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());

    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(ok);
    let msgs = agent.messages();

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
    let mut agent = test_agent("SUM").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=8).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    assert!(agent.try_compact_budgeted().await.unwrap());
    let first_anchor = agent.messages()[0].text_content();
    assert!(
        first_anchor.contains("msg1"),
        "anchor is the intent, not the summary"
    );

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
    let mut agent = test_agent("S").with_context_window(Some(16_200));
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let ok = agent.try_compact_budgeted().await.unwrap();
    assert!(ok);
    let msgs = agent.messages();

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
async fn compact_v2_caps_retained_under_ceiling() {
    let mut agent = test_agent("FIXED-SUMMARY-123")
        .with_context_window(Some(200_000))
        .with_compact_ceiling(Some(400), None);
    let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    agent.compact_v2(None, None).await.unwrap();
    assert!(
        agent.messages().len() <= 2,
        "expected summary + <=1 retained, got {}",
        agent.messages().len()
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

struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    fn stream(&self, _req: ChatRequest) -> ProviderStream {
        Box::pin(futures::stream::iter(vec![Err(
            crate::agent::ProviderError::Connection("boom".to_string()),
        )]))
    }
}

fn trimmable_history() -> Vec<Message> {
    ["c1", "c2", "c3"]
        .into_iter()
        .map(|id| Message {
            role: Role::User,
            injected: false,
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
    let mut agent = test_agent("LIVE-SUM").with_context_window(Some(24_000));
    let big = "y".repeat(40_000);
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

#[tokio::test]
async fn a_repeated_prompt_survives_in_the_tail() {
    let mut agent = test_agent("LIVE-SUM").with_context_window(Some(16_200));
    let intent = "ANCHOR-TOKEN-7742: cat these files";

    agent.set_messages(vec![
        Message::user(intent),
        Message::assistant("m".repeat(3_000)),
        Message::user(intent),
    ]);
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

    assert_eq!(
        out.iter()
            .filter(|m| m.text_content().starts_with("ANCHOR-TOKEN-7742"))
            .count(),
        2,
        "the repeat is a real turn and stays in the tail: {:?}",
        out.iter()
            .map(|m| m.text_content().chars().take(20).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        out.last()
            .unwrap()
            .text_content()
            .starts_with("ANCHOR-TOKEN-7742"),
        "the request still ends on the tail's user turn"
    );
}

#[tokio::test]
async fn compaction_never_summarizes_a_contaminated_partial() {
    let (mut agent, seen) = recording_agent("LIVE-SUM");
    agent = agent.with_context_window(Some(16_200));
    let partial = "BROKEN-TRAJECTORY-TOKEN: half a tool call that died mid-stream";

    let mut msgs = vec![Message::user("ANCHOR: fix the flaky test")];
    msgs.push(Message::assistant(partial.to_string()));
    msgs.push(Message::assistant("m".repeat(6_000)));
    msgs.push(Message::user("and also check the lockfile"));
    agent.set_messages(msgs);

    agent.contaminated.insert(1);

    assert!(agent.try_compact_budgeted().await.unwrap());

    let calls = seen.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(calls.len(), 1, "one compaction trigger call");
    let sent = calls[0]
        .iter()
        .map(|m| m.text_content())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !sent.contains("BROKEN-TRAJECTORY-TOKEN"),
        "the failed partial must not reach the summarizer:\n{sent}"
    );
    assert!(
        sent.contains(CONTAMINATED_SCRUB_MARKER),
        "the marker stands where the failure was:\n{sent}"
    );

    let after = agent
        .messages()
        .iter()
        .map(|m| m.text_content())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !after.contains("BROKEN-TRAJECTORY-TOKEN"),
        "the summary must not carry the partial forward:\n{after}"
    );
}

#[test]
fn inline_media_attachment_is_priced_per_block_not_per_base64_byte() {
    let payload = "A".repeat(10_700_000);
    let msg = Message {
        role: Role::User,
        injected: false,
        content: vec![
            ContentBlock::text("summarize this"),
            ContentBlock::media("application/pdf", payload, vec![]),
        ],
    };
    let core = crate::agent_compact::estimate_message_tokens(&msg);
    let cap = crate::compact::image_block_tokens("application/pdf", 10_700_000);
    assert!(
        core <= cap + 16,
        "core estimate {core} must be the per-block price, got base64/4"
    );
    assert!(
        core < 10_000,
        "a single attachment must not look like a full window: {core}"
    );

    assert!(!needs_pre_turn_compact(core, Some(200_000), None, None));

    let both = Message {
        role: Role::User,
        injected: false,
        content: vec![
            ContentBlock::image("image/png", "B".repeat(6_000_000)),
            ContentBlock::media("audio/mpeg", "C".repeat(6_000_000), vec![]),
        ],
    };
    assert!(crate::agent_compact::estimate_message_tokens(&both) < 10_000);
}

#[test]
fn soft_elide_fires_at_six_tenths_of_a_known_window_only() {
    use crate::agent_compact::needs_soft_elide;
    assert!(
        !needs_soft_elide(usize::MAX, None),
        "unknown window never fires"
    );
    assert!(!needs_soft_elide(59_999, Some(100_000)));
    assert!(needs_soft_elide(60_000, Some(100_000)));

    assert!(crate::compact::soft_elide_threshold(128_000) < 128_000 - 16_000);
}

fn starred_pic(tag: &str, bytes: usize) -> Message {
    let mut m = Message::user(tag);
    m.content
        .push(ContentBlock::image("image/png", "*".repeat(bytes)));
    m
}

fn media_bytes_in(msgs: &[Message]) -> usize {
    msgs.iter()
        .flat_map(|m| &m.content)
        .map(|b| match b {
            ContentBlock::Image { data, .. } | ContentBlock::Media { data, .. } => data.len(),
            _ => 0,
        })
        .sum()
}

#[tokio::test]
async fn turn_request_keeps_media_under_the_payload_budget() {
    let (mut agent, seen) = recording_agent("done");
    agent.set_messages(
        (0..16)
            .map(|i| starred_pic(&format!("shot{i}"), 2_000_000))
            .collect(),
    );
    agent.masked_prefix = 3;
    let revision = agent.history_revision();
    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    let sent = media_bytes_in(&seen[0]);
    assert!(
        sent > 0 && sent <= crate::compact::MEDIA_PAYLOAD_KEEP_BYTES,
        "request carries only the newest images: {sent} bytes"
    );
    assert_eq!(
        media_bytes_in(agent.messages()),
        sent,
        "elision lands in history so later requests keep the same prefix"
    );
    assert_eq!(
        agent.history_revision(),
        revision,
        "no full-history rewrite: the session file keeps one copy of each image"
    );
    assert_eq!(agent.masked_prefix, 3, "the stale-output mask survives");
}

struct MediaCapProvider {
    seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
}

#[async_trait]
impl Provider for MediaCapProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        let sent = media_bytes_in(&req.messages);
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.messages);
        if sent > 0 {
            // Wrapped the way the HTTP providers report a real 413.
            return Box::pin(futures::stream::iter(vec![Err(
                crate::agent::ProviderError::Detailed {
                    class: Box::new(crate::agent::ProviderError::ContextOverflow(
                        "Downloaded image content cannot exceed 30MB".to_string(),
                    )),
                    detail: Box::new(crate::error::ProviderDetail {
                        status: Some(413),
                        ..Default::default()
                    }),
                },
            )]));
        }
        Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::text_delta("TEXT-ONLY-SUMMARY")),
            Ok(StreamEvent::message_complete(None, None)),
        ]))
    }
}

#[tokio::test]
async fn compaction_drops_media_when_the_summary_call_overflows() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = Agent::new(
        Box::new(MediaCapProvider {
            seen: std::sync::Arc::clone(&seen),
        }),
        Arc::new(NoopExecutor),
    )
    .with_context_window(Some(5100));
    let mut history = trimmable_history();
    history.push(starred_pic("latest screenshot", 50_000));
    agent.set_messages(history);

    assert!(agent.try_compact_budgeted().await.unwrap());
    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(seen.len(), 2, "one overflow, one text-only retry");
    assert!(media_bytes_in(&seen[0]) > 0);
    assert_eq!(media_bytes_in(&seen[1]), 0);
    assert!(
        seen[1]
            .iter()
            .any(|m| m.text_content().contains("elided from context")),
        "the retry says where the pixels went"
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("TEXT-ONLY-SUMMARY"))
    );
}

#[tokio::test]
async fn compaction_provider_writes_the_summary() {
    let mut agent = test_agent("CHAT-SUMMARY");
    agent.set_compaction_provider(Some(Arc::new(SummaryProvider {
        text: "CHEAP-SUMMARY".to_string(),
    })));
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let out = agent.compact_v2(None, Some(0)).await.unwrap();
    assert_eq!(out.as_deref(), Some("CHEAP-SUMMARY"));
}

#[tokio::test]
async fn failed_compaction_provider_falls_back_to_chat_model() {
    let mut agent = test_agent("CHAT-SUMMARY");
    agent.set_compaction_provider(Some(Arc::new(FailingProvider)));
    let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
    agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
    let out = agent.compact_v2(None, Some(0)).await.unwrap();
    assert_eq!(out.as_deref(), Some("CHAT-SUMMARY"));
}
