use super::*;
use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
use crate::event::{StopReason, StreamEvent};
use crate::message::{ChatRequest, Role, ToolDef};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};

// --- Task 2 (RED): retention grouping + budget walk --------------------

fn assistant_tool_use(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(id, "sh", serde_json::json!({}))],
    }
}

fn user_tool_result(id: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::tool_result(id, "ok", false)],
    }
}

/// Assistant message carrying a whole parallel-call batch: gray's loop
/// pushes one `ToolResult` user message per call, so an N-call batch is
/// 1 assistant + N user messages.
fn assistant_tool_uses(ids: &[&str]) -> Message {
    Message {
        role: Role::Assistant,
        content: ids
            .iter()
            .map(|id| ContentBlock::tool_use(*id, "sh", serde_json::json!({})))
            .collect(),
    }
}

/// Sorted (calls, results) id sets in `msgs`: equal iff no orphaned call
/// or result survives the retain/drop decision.
fn call_result_ids(msgs: &[Message]) -> (Vec<String>, Vec<String>) {
    let mut uses: Vec<String> = msgs
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let mut results: Vec<String> = msgs
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    uses.sort();
    results.sort();
    (uses, results)
}

fn is_subsequence(hay: &[Message], needle: &[Message]) -> bool {
    let mut j = 0;
    for m in hay {
        if j < needle.len() && *m == needle[j] {
            j += 1;
        }
    }
    j == needle.len()
}

#[test]
fn retention_drops_tool_chatter_atomically() {
    let msgs = vec![
        Message::assistant("thinking out loud"),
        assistant_tool_use("c1"),
        user_tool_result("c1"),
        Message::user("done?"),
    ];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    let mut uses: Vec<&str> = out
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let mut results: Vec<&str> = out
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    uses.sort();
    results.sort();
    assert_eq!(uses, results, "no orphaned calls either direction");
    assert!(
        is_subsequence(&msgs, &out),
        "output preserves chronological order"
    );
    assert_eq!(out.last().unwrap(), &Message::user("done?"));
}

#[test]
fn atomic_batch_1_call_drops_without_orphans() {
    let msgs = vec![assistant_tool_uses(&["c1"]), user_tool_result("c1")];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(call_result_ids(&out), (vec![], vec![]));
}

#[test]
fn atomic_batch_2_calls_drop_without_orphans() {
    // Results span 2 user messages; the tool-use-only batch must drop
    // whole — never a retained `c2` result orphaned from its call.
    let msgs = vec![
        assistant_tool_uses(&["c1", "c2"]),
        user_tool_result("c1"),
        user_tool_result("c2"),
    ];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(call_result_ids(&out), (vec![], vec![]));
    assert!(
        is_subsequence(&msgs, &out),
        "output preserves chronological order"
    );
}

#[test]
fn atomic_batch_3_calls_drop_without_orphans() {
    let msgs = vec![
        assistant_tool_uses(&["c1", "c2", "c3"]),
        user_tool_result("c1"),
        user_tool_result("c2"),
        user_tool_result("c3"),
    ];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(call_result_ids(&out), (vec![], vec![]));
    assert!(
        is_subsequence(&msgs, &out),
        "output preserves chronological order"
    );
}

#[test]
fn atomic_batch_3_calls_retain_without_orphans() {
    // Text-carrying assistant: the batch is retained, still atomically.
    let mut batch = vec![Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::text("running three lookups"),
            ContentBlock::tool_use("c1", "sh", serde_json::json!({})),
            ContentBlock::tool_use("c2", "sh", serde_json::json!({})),
            ContentBlock::tool_use("c3", "sh", serde_json::json!({})),
        ],
    }];
    batch.extend(["c1", "c2", "c3"].into_iter().map(user_tool_result));
    let out = build_retained(&batch, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(out, batch, "retainable batch kept whole");
    let (uses, results) = call_result_ids(&out);
    assert_eq!(uses, vec!["c1", "c2", "c3"]);
    assert_eq!(uses, results, "no orphaned calls either direction");
}

#[test]
fn oversized_single_assistant_dropped() {
    let big = Message::assistant("y".repeat(12_000 * 4));
    assert!(build_retained(std::slice::from_ref(&big), RETAINED_MESSAGE_TOKEN_BUDGET).is_empty());
}

#[test]
fn boundary_group_middle_truncated_not_dropped() {
    let old = Message::user("a".repeat(4000));
    let new = Message::user("b".repeat(4000));
    let out = build_retained(&[old, new.clone()], 1500);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1], new, "newest group kept verbatim");
    let text = out[0].text_content();
    assert!(text.contains(TRUNCATION_MARKER), "boundary group truncated");
    assert!(
        crate::agent_compact::est_tokens(&out) <= 1500,
        "truncated output fits budget"
    );
}

#[test]
fn boundary_truncation_charges_thinking_and_tool_blocks() {
    // Thinking (500 tokens) + text (1000) in the boundary group, newest
    // kept (1000) at a 2000 budget: fixed costs bill first, text takes
    // the remaining 500 with a marker, total stays within budget. The
    // old text/image-only accounting kept the text whole (1000) with the
    // 500 thinking tokens free → 2500 over a 2000 budget.
    let old = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::thinking("t".repeat(2000)),
            ContentBlock::text("a".repeat(4000)),
        ],
    };
    let new = Message::user("b".repeat(4000));
    let out = build_retained(&[old, new.clone()], 2000);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1], new, "newest group kept verbatim");
    assert!(
        out[0].text_content().contains(TRUNCATION_MARKER),
        "boundary text truncated to cover the thinking overhead"
    );
    assert!(
        crate::agent_compact::est_tokens(&out) <= 2000,
        "truncated output fits budget"
    );

    // Tool-arg-heavy mixed batch: ToolUse args (~500) + small result are
    // fixed costs; the batch's text truncates to the remainder.
    let batch_old = vec![
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("a".repeat(4000)),
                ContentBlock::tool_use("c9", "sh", serde_json::json!({"data": "x".repeat(2000)})),
            ],
        },
        user_tool_result("c9"),
    ];
    let out = build_retained(&[batch_old.clone(), vec![new.clone()]].concat(), 2000);
    assert_eq!(out.len(), 3, "batch truncated, not dropped: {out:?}");
    assert_eq!(out[2], new, "newest group kept verbatim");
    let (uses, results) = call_result_ids(&out);
    assert_eq!(uses, results, "no orphaned calls either direction");
    assert!(
        out[0].text_content().contains(TRUNCATION_MARKER),
        "batch text truncated to cover the tool-arg overhead"
    );
    assert!(
        crate::agent_compact::est_tokens(&out) <= 2000,
        "truncated output fits budget"
    );
}

// --- Task 3 (RED): in-band trigger call -------------------------------

/// Test fake mirroring `agent::agent_tests::FakeProvider`'s `Provider`
/// impl shape: records the `ChatRequest` it receives, replays one
/// scripted event list.
struct CapturingProvider {
    script: Mutex<Vec<StreamEvent>>,
    seen: Arc<Mutex<Vec<ChatRequest>>>,
}

impl CapturingProvider {
    fn new(script: Vec<StreamEvent>, seen: Arc<Mutex<Vec<ChatRequest>>>) -> Self {
        Self {
            script: Mutex::new(script),
            seen,
        }
    }
}

#[async_trait]
impl Provider for CapturingProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen.lock().expect("seen lock poisoned").push(req);
        let script = std::mem::take(&mut *self.script.lock().expect("script lock poisoned"));
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

/// Executor that records calls instead of running them (the trigger-call
/// drain must never reach it).
struct RecordingExecutor {
    calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(name.to_string());
        Box::pin(async move { ToolOutput::ok("must-not-run") })
    }
}

fn trigger_test_agent(
    script: Vec<StreamEvent>,
    seen: Arc<Mutex<Vec<ChatRequest>>>,
    calls: Arc<Mutex<Vec<String>>>,
    tools: Vec<ToolDef>,
) -> Agent {
    Agent::new(
        Box::new(CapturingProvider::new(script, seen)),
        Arc::new(RecordingExecutor { calls }),
    )
    .with_system("S")
    .with_tools(tools)
}

#[tokio::test]
async fn compaction_call_reuses_system_tools_and_appends_trigger() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let tools = vec![ToolDef::new(
        "read",
        "r",
        serde_json::json!({"type": "object"}),
    )];
    let agent = trigger_test_agent(
        vec![
            StreamEvent::text_delta("SUMMARY"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        seen.clone(),
        calls,
        tools.clone(),
    );
    let m1 = Message::user("m1");
    let m2 = Message::assistant("m2");
    let history = vec![m1.clone(), m2.clone()];

    let out = run_compaction_call(&agent, &history, None).await.unwrap();

    assert_eq!(out, "SUMMARY");
    let reqs = seen.lock().expect("seen lock poisoned");
    assert_eq!(reqs.len(), 1, "one trigger call, got {reqs:?}");
    let req = &reqs[0];
    assert_eq!(req.system, Some("S".to_string()));
    assert_eq!(req.tools, tools);
    let mut expected = history.clone();
    expected.push(Message::user(format!(
        "{COMPACTION_TRIGGER}\n\n{COMPACTION_MARKER_GUIDANCE}"
    )));
    assert_eq!(req.messages, expected);
    assert_eq!(
        history,
        vec![m1, m2],
        "trigger must not leak into caller history"
    );
}

#[tokio::test]
async fn compaction_call_appends_custom_instructions() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let agent = trigger_test_agent(
        vec![
            StreamEvent::text_delta("S"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        seen.clone(),
        calls,
        Vec::new(),
    );

    let out = run_compaction_call(&agent, &[Message::user("hi")], Some("focus on auth"))
        .await
        .unwrap();

    assert_eq!(out, "S");
    let reqs = seen.lock().expect("seen lock poisoned");
    let trigger = reqs[0].messages.last().expect("trigger message");
    assert!(
        trigger.text_content().contains("focus on auth"),
        "custom instructions ride the trigger: {}",
        trigger.text_content()
    );
}

#[tokio::test]
async fn compaction_call_drops_tool_use_blocks_from_reply() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let agent = trigger_test_agent(
        vec![
            StreamEvent::text_delta("S"),
            StreamEvent::tool_call_delta(0, Some("c1".into()), Some("read".into()), "{}"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        seen,
        calls.clone(),
        Vec::new(),
    );

    let out = run_compaction_call(&agent, &[Message::user("hi")], None)
        .await
        .unwrap();

    assert_eq!(out, "S");
    assert!(
        calls.lock().expect("calls lock poisoned").is_empty(),
        "no tool execution attempted"
    );
}

#[test]
fn budget_walk_newest_first() {
    // Exact-fit sizing: 3 × 32k tokens at a 64k budget leaves
    // `remaining == 0` after the two newest, so the oldest is dropped (a
    // 30k sizing would boundary-truncate it instead — covered above).
    let mk = |tag: &str| Message::user(format!("{tag}:{}", "z".repeat(32_000 * 4 - tag.len() - 1)));
    let msgs = vec![mk("m1"), mk("m2"), mk("m3")];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(out.len(), 2);
    assert!(out[0].text_content().starts_with("m2:"));
    assert!(out[1].text_content().starts_with("m3:"));
}

// --- Task 4 (RED): atomic image budget ------------------------------

#[test]
fn images_priced_and_dropped_oldest_first() {
    // Adaptation #3 pricing: decoded bytes/4, floored at 1_000.
    assert_eq!(image_block_tokens("image/png", 5_000), 1_000);
    assert_eq!(image_block_tokens("image/png", 16_000), 3_000);
    assert_eq!(image_block_tokens("image/png", 64), 1_000);

    let img = Message {
        role: Role::User,
        content: vec![ContentBlock::image("image/png", "a".repeat(5_000))],
    };
    let new = Message::user("b".repeat(400)); // 100 tokens
    // Tight budget: newest text kept, oldest image group dropped whole.
    assert_eq!(
        build_retained(&[img.clone(), new.clone()], 500),
        vec![new.clone()]
    );
    // Roomy budget (100 text + 1_000 priced image): both kept, image whole.
    let out = build_retained(&[img, new.clone()], 1_100);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1], new);
    match &out[0].content[..] {
        [ContentBlock::Image { data, .. }] => {
            assert_eq!(data.len(), 5_000, "atomic: no half-image")
        }
        other => panic!("expected single whole image block, got {other:?}"),
    }
}

#[test]
fn trigger_is_task_neutral() {
    assert!(COMPACTION_TRIGGER.contains("outcomes"));
    assert!(COMPACTION_TRIGGER.contains("decisions"));
    assert!(COMPACTION_TRIGGER.contains("open questions"));
    assert!(COMPACTION_TRIGGER.contains("omit superseded"));
    assert!(COMPACTION_TRIGGER.contains("adds no domain instructions"));
    assert!(
        !COMPACTION_TRIGGER.contains("file states"),
        "summarizer must not assume a coding task"
    );
}

#[test]
fn image_never_split_by_boundary_truncation() {
    let old = Message::user("o".repeat(40)); // 10 tokens
    let img = Message {
        role: Role::User,
        content: vec![ContentBlock::image("image/png", "a".repeat(5_000))], // 1_000 tokens
    };
    let new = Message::user("n".repeat(4_000)); // 1_000 tokens
    // Newest kept (1_000); the image group doesn't fit in the remaining 10
    // → whole-group drop with budget preserved, oldest text still kept.
    let out = build_retained(&[old.clone(), img, new.clone()], 1_010);
    assert_eq!(out, vec![old, new]);
    assert!(
        out.iter()
            .flat_map(|m| &m.content)
            .all(|b| !matches!(b, ContentBlock::Image { .. })),
        "no split base64 survives anywhere"
    );
}

#[test]
fn prune_old_tool_observations_keeps_recent_and_elides_older() {
    let mut msgs: Vec<Message> = (0..8)
        .map(|i| Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: format!("call_{i}"),
                content: format!(
                    "Output line 1 for tool {i}\nOutput line 2 for tool {i}\n{}",
                    "x".repeat(200)
                ),
                is_error: false,
            }],
        })
        .collect();

    prune_old_tool_observations(&mut msgs, 3);

    // First 5 should be elided
    for (i, msg) in msgs.iter().enumerate().take(5) {
        let ContentBlock::ToolResult { content, id, .. } = &msg.content[0] else {
            panic!()
        };
        assert_eq!(id, &format!("call_{i}"));
        assert!(content.starts_with("Old command output:"));
        assert!(content.contains("lines omitted"));
    }

    // Last 3 should be untouched
    for (i, msg) in msgs.iter().enumerate().skip(5) {
        let ContentBlock::ToolResult { content, id, .. } = &msg.content[0] else {
            panic!()
        };
        assert_eq!(id, &format!("call_{i}"));
        assert!(content.starts_with(&format!("Output line 1 for tool {i}")));
    }
}

#[test]
fn prune_keeps_log_path_from_header() {
    let header = "exit 1 · 0.3s · 40 lines · log ~/.gray/shell/s1/bash-abc.log";
    let mut msgs: Vec<Message> = (0..2)
        .map(|i| Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: format!("call_{i}"),
                content: format!(
                    "{header}\n<untrusted-output>\nfail {i}\n{}\n</untrusted-output>",
                    "x".repeat(120)
                ),
                is_error: false,
            }],
        })
        .collect();

    prune_old_tool_observations(&mut msgs, 1);

    let ContentBlock::ToolResult { content, .. } = &msgs[0].content[0] else {
        panic!()
    };
    assert!(
        content.contains("~/.gray/shell/s1/bash-abc.log"),
        "log path lost: {content}"
    );
    // The still-recent second observation is untouched.
    let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] else {
        panic!()
    };
    assert!(content.starts_with(header));
}

fn big_result(id: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            id: id.to_string(),
            content: format!("header · log /tmp/{id}.log\n{}", "x".repeat(500)),
            is_error: false,
        }],
    }
}

#[test]
fn elision_gate_fires_only_on_full_batches() {
    // keep(5) + batch(10) = 15: at 15 full observations nothing fires
    // (prefix stays append-only for the cache); the 16th arms the gate.
    let mut msgs: Vec<Message> = (0..15).map(|i| big_result(&format!("c{i}"))).collect();
    assert_eq!(full_tool_observations(&msgs), 15);
    assert!(!should_elide_observations(&msgs));
    msgs.push(big_result("c15"));
    assert!(should_elide_observations(&msgs));
    prune_old_tool_observations(&mut msgs, DEFAULT_KEEP_RECENT_TOOL_OBSERVATIONS);
    // 5 newest stay full, the rest are stubs — and stubs never re-arm it.
    assert_eq!(full_tool_observations(&msgs), 5);
    assert!(!should_elide_observations(&msgs));
    let ContentBlock::ToolResult { content, .. } = &msgs[0].content[0] else {
        panic!()
    };
    assert!(content.starts_with(ELIDED_OUTPUT_PREFIX));
    assert!(
        content.contains("/tmp/c0.log"),
        "log path survives: {content}"
    );
}
