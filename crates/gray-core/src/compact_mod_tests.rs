use super::*;
use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
use crate::event::{StopReason, StreamEvent};
use crate::message::{ChatRequest, Role, ToolDef};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};

fn with_home<T>(body: impl FnOnce() -> T) -> T {
    let _guard = crate::paths::HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().expect("tempdir");

    unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
    let out = body();
    unsafe { std::env::remove_var("GRAY_HOME") };
    out
}

fn assistant_tool_use(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        injected: false,
        content: vec![ContentBlock::tool_use(id, "sh", serde_json::json!({}))],
    }
}

fn user_tool_result(id: &str) -> Message {
    Message {
        role: Role::User,
        injected: false,
        content: vec![ContentBlock::tool_result(id, "ok", false)],
    }
}

fn assistant_tool_uses(ids: &[&str]) -> Message {
    Message {
        role: Role::Assistant,
        injected: false,
        content: ids
            .iter()
            .map(|id| ContentBlock::tool_use(*id, "sh", serde_json::json!({})))
            .collect(),
    }
}

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
fn atomic_batch_2_calls_drop_without_orphans() {
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
fn atomic_batch_3_calls_retain_without_orphans() {
    let mut batch = vec![Message {
        role: Role::Assistant,
        injected: false,
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
fn oversized_single_assistant_truncated_to_cap() {
    let big = Message::assistant("y".repeat(12_000 * 4));
    let out = build_retained(std::slice::from_ref(&big), RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(out.len(), 1);
    assert!(out[0].text_content().contains(TRUNCATION_MARKER));
    assert!(
        crate::agent_compact::est_tokens(&out) <= MAX_RETAINED_AGENT_MESSAGE_TOKENS as usize,
        "truncated to the per-group cap"
    );
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
    let old = Message {
        role: Role::Assistant,
        injected: false,
        content: vec![
            ContentBlock::Thinking {
                text: "t".repeat(2000),
                encrypted_content: Some("e".repeat(3200)),
                item_id: None,
                model: None,
            },
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

    let batch_old = vec![
        Message {
            role: Role::Assistant,
            injected: false,
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

struct CapturingProvider {
    script: Mutex<Vec<StreamEvent>>,
    seen: Arc<Mutex<Vec<ChatRequest>>>,
    iso: Option<Arc<Mutex<Vec<Option<bool>>>>>,
}

impl CapturingProvider {
    fn new(script: Vec<StreamEvent>, seen: Arc<Mutex<Vec<ChatRequest>>>) -> Self {
        Self {
            script: Mutex::new(script),
            seen,
            iso: None,
        }
    }

    fn with_iso_recorder(mut self, iso: Arc<Mutex<Vec<Option<bool>>>>) -> Self {
        self.iso = Some(iso);
        self
    }
}

#[async_trait]
impl Provider for CapturingProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        if let Some(iso) = &self.iso {
            iso.lock()
                .expect("iso lock poisoned")
                .push(crate::agent::current_isolated_stream().map(|i| i.cache_off));
        }
        self.seen.lock().expect("seen lock poisoned").push(req);
        let script = std::mem::take(&mut *self.script.lock().expect("script lock poisoned"));
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

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
    expected.push(Message::user_injected(COMPACTION_TRIGGER));
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
    let mk = |tag: &str| Message::user(format!("{tag}:{}", "z".repeat(10_000 * 4 - tag.len() - 1)));
    let msgs = vec![mk("m1"), mk("m2"), mk("m3")];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(out.len(), 2);
    assert!(out[0].text_content().starts_with("m2:"));
    assert!(out[1].text_content().starts_with("m3:"));
}

#[test]
fn images_priced_and_dropped_oldest_first() {
    assert_eq!(image_block_tokens("image/png", 5_000), 1_000);
    assert_eq!(image_block_tokens("image/png", 16_000), 3_000);
    assert_eq!(image_block_tokens("image/png", 64), 1_000);

    let img = Message {
        role: Role::User,
        injected: false,
        content: vec![ContentBlock::image("image/png", "a".repeat(5_000))],
    };
    let new = Message::user("b".repeat(400));

    assert_eq!(
        build_retained(&[img.clone(), new.clone()], 500),
        vec![new.clone()]
    );

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
    assert!(
        !COMPACTION_TRIGGER.contains("file states"),
        "summarizer must not assume a coding task"
    );
}

#[test]
fn image_never_split_by_boundary_truncation() {
    let old = Message::user("o".repeat(40));
    let img = Message {
        role: Role::User,
        injected: false,
        content: vec![ContentBlock::image("image/png", "a".repeat(5_000))],
    };
    let new = Message::user("n".repeat(4_000));

    let out = build_retained(&[old.clone(), img, new.clone()], 1_010);
    assert_eq!(out, vec![old, new]);
    assert!(
        out.iter()
            .flat_map(|m| &m.content)
            .all(|b| !matches!(b, ContentBlock::Image { .. })),
        "no split base64 survives anywhere"
    );
}

struct ChangingContext(std::sync::atomic::AtomicUsize);
#[async_trait]
impl crate::agent::PluginHooks for ChangingContext {
    async fn prompt_context(&self) -> Option<String> {
        Some(format!(
            "CTX-{}",
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    }
}

#[tokio::test]
async fn compaction_preserves_last_live_hook_context() {
    let seen = Arc::default();
    let hook = Arc::new(ChangingContext(std::sync::atomic::AtomicUsize::new(0)));
    let script = || {
        vec![
            StreamEvent::text_delta("ok"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]
    };
    let mut agent = trigger_test_agent(script(), Arc::clone(&seen), Arc::default(), Vec::new())
        .with_hooks(vec![hook.clone()]);
    for i in 0..2 {
        agent.provider = Arc::new(CapturingProvider::new(script(), Arc::clone(&seen)));
        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();
        agent.provider = Arc::new(CapturingProvider::new(script(), Arc::clone(&seen)));
        assert_eq!(
            run_compaction_call(&agent, agent.messages(), None)
                .await
                .unwrap(),
            "ok"
        );
        let requests = seen.lock().unwrap();
        let live = &requests[i * 2];
        let compact = &requests[i * 2 + 1];
        assert_eq!(live.system, Some("S".to_string()));
        assert_eq!(compact.system, live.system);
        assert_eq!(compact.tools, live.tools);

        assert!(
            live.messages
                .iter()
                .any(|m| m.text_content() == format!("[Context update]\nCTX-{i}")),
            "turn {i}: fresh hook text lands as a context note"
        );
    }
    assert_eq!(hook.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}

fn big_tool_group(id: &str, bytes: usize) -> Vec<Message> {
    vec![
        assistant_tool_use(id),
        Message {
            role: Role::User,
            injected: false,
            content: vec![ContentBlock::tool_result(id, "x".repeat(bytes), false)],
        },
    ]
}

fn only_tool_result_content(msgs: &[Message]) -> Option<&str> {
    msgs.iter()
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
}

#[test]
fn tool_only_round_with_large_output_survives_as_citation() {
    let mut msgs = big_tool_group("call_abc", ARC_STUB_MIN_BYTES + 100);
    msgs.push(Message::user("done?"));
    let out = build_retained_with_session(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET, Some("sess1"));

    assert_eq!(
        call_result_ids(&out),
        (vec!["call_abc".to_string()], vec!["call_abc".to_string()]),
        "the stubbed round must not orphan the call or the result"
    );
    let stub = only_tool_result_content(&out).expect("stubbed result survives");
    assert!(stub.contains("elided from context"), "{stub}");
    assert!(
        stub.contains("call_abc"),
        "citation names the address: {stub}"
    );
    assert!(
        stub.contains("~/.gray/sessions/sess1.jsonl"),
        "citation names the file: {stub}"
    );
    assert!(stub.contains("grep"), "citation is self-describing: {stub}");
    assert!(
        stub.contains("8292"),
        "citation names the elided size: {stub}"
    );
    assert_eq!(out.last().unwrap(), &Message::user("done?"));
}

#[test]
fn anchor_stubs_large_media_but_keeps_the_words() {
    with_home(|| {
        let mut m = Message::user("what does this diagram show?");
        m.content.push(ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "QUJD".repeat(IMAGE_STUB_MIN_BYTES),
        });
        let anchor = anchor_message(&[m]).expect("user turn anchors");
        assert_eq!(anchor.content.len(), 2);
        match &anchor.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "what does this diagram show?"),
            b => panic!("text block must survive, got {b:?}"),
        }
        match &anchor.content[1] {
            ContentBlock::Text { text } => {
                assert!(text.contains("elided from context"), "{text}");
                assert!(text.contains("image/png"), "{text}");
                assert!(text.contains("Saved to"), "{text}");
                let saved = std::fs::read_dir(crate::paths::attachments_dir().unwrap())
                    .unwrap()
                    .count();
                assert_eq!(saved, 1, "elided bytes must land on disk");
            }
            b => panic!("media must become a citation stub, got {b:?}"),
        }
    });
}

#[test]
fn anchor_leaves_small_media_alone() {
    let mut m = Message::user("look");
    m.content.push(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: "tiny".to_string(),
    });
    let anchor = anchor_message(&[m]).expect("user turn anchors");
    match &anchor.content[1] {
        ContentBlock::Image { data, .. } => assert_eq!(data, "tiny"),
        b => panic!("small media must pass through, got {b:?}"),
    }
}

#[test]
fn media_older_than_two_kept_groups_becomes_a_stub() {
    with_home(|| {
        let big = || "QUJD".repeat(IMAGE_STUB_MIN_BYTES);
        let mut old_pic = Message::user("old picture");
        old_pic.content.push(ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: big(),
        });
        let mut recent_pic = Message::user("recent picture");
        recent_pic.content.push(ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: big(),
        });

        let mut msgs = vec![old_pic];
        msgs.extend(big_tool_group("c1", ARC_STUB_MIN_BYTES + 100));
        msgs.extend(big_tool_group("c2", ARC_STUB_MIN_BYTES + 100));
        msgs.push(recent_pic);
        let out = build_retained_with_session(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET, Some("sess1"));
        let has_raw_image = |m: &Message| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        };
        let stubbed = |m: &Message| {
            m.content.iter().any(|b| match b {
                ContentBlock::Text { text } => text.contains("elided from context"),
                _ => false,
            })
        };

        assert!(stubbed(&out[0]), "old media must be stubbed: {:?}", out[0]);
        assert!(!has_raw_image(&out[0]));
        let last = out.last().unwrap();
        assert!(
            has_raw_image(last),
            "newest media keeps its pixels: {last:?}"
        );
    });
}

#[test]
fn tool_only_round_with_small_output_still_drops_without_citation() {
    let msgs = vec![
        assistant_tool_use("c1"),
        user_tool_result("c1"),
        Message::user("done?"),
    ];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    assert_eq!(call_result_ids(&out), (vec![], vec![]));
    assert_eq!(out, vec![Message::user("done?")]);
}

#[test]
fn citation_stub_falls_back_to_generic_path_without_session() {
    let msgs = big_tool_group("call_xyz", ARC_STUB_MIN_BYTES + 1);
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    let stub = only_tool_result_content(&out).expect("stubbed result survives");
    assert!(stub.contains("grep 'call_xyz'"), "{stub}");
    assert!(stub.contains("~/.gray/sessions/"), "{stub}");
}

#[test]
fn stubbing_rescues_over_budget_text_group() {
    let msgs = vec![
        Message {
            role: Role::Assistant,
            injected: false,
            content: vec![
                ContentBlock::Text {
                    text: "the plan is to frobnicate the config".to_string(),
                },
                ContentBlock::tool_use("c9", "sh", serde_json::json!({})),
            ],
        },
        Message {
            role: Role::User,
            injected: false,
            content: vec![ContentBlock::tool_result(
                "c9",
                "y".repeat(ARC_STUB_MIN_BYTES * 4),
                false,
            )],
        },
        Message::user("done?"),
    ];
    let out = build_retained(&msgs, 2_000);
    let text: String = out
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("frobnicate"),
        "the stub fallback keeps the text that fixed costs would have dropped: {text}"
    );
    let stub = only_tool_result_content(&out).expect("result elided to a citation");
    assert!(stub.contains("grep 'c9'"), "{stub}");
    assert_eq!(
        call_result_ids(&out),
        (vec!["c9".to_string()], vec!["c9".to_string()]),
        "pairing survives the fallback"
    );
}

#[test]
fn fitting_groups_keep_large_output_verbatim() {
    let big = "z".repeat(ARC_STUB_MIN_BYTES + 10);
    let msgs = vec![
        Message {
            role: Role::Assistant,
            injected: false,
            content: vec![
                ContentBlock::Text {
                    text: "looking at the output".to_string(),
                },
                ContentBlock::tool_use("c1", "sh", serde_json::json!({})),
            ],
        },
        Message {
            role: Role::User,
            injected: false,
            content: vec![ContentBlock::tool_result("c1", big.clone(), false)],
        },
        Message::user("done?"),
    ];
    let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
    let content = only_tool_result_content(&out).expect("fitting group keeps its result");
    assert_eq!(content, big, "no stub when the group fits");
}

fn big_tool_result(id: &str) -> Message {
    Message {
        role: Role::User,
        injected: false,
        content: vec![ContentBlock::tool_result(
            id,
            "x".repeat(ARC_STUB_MIN_BYTES),
            false,
        )],
    }
}

fn result_body(m: &Message) -> &str {
    match &m.content[0] {
        ContentBlock::ToolResult { content, .. } => content,
        _ => panic!("not a tool result"),
    }
}

#[test]
fn soft_elision_stubs_old_large_results_and_keeps_the_recent_window_verbatim() {
    let mut msgs = vec![Message::user("fix it")];
    for id in ["a", "b", "c", "d"] {
        msgs.push(assistant_tool_use(id));
        msgs.push(big_tool_result(id));
    }
    let (calls_before, results_before) = call_result_ids(&msgs);
    let len_before = msgs.len();

    let elided = elide_stale_tool_results(&mut msgs, 2_100, Some("sess"));
    assert_eq!(elided, 2, "rounds a and b are elided, c and d kept");
    assert_eq!(msgs.len(), len_before, "nothing dropped or reordered");
    assert_eq!(call_result_ids(&msgs), (calls_before, results_before));
    assert!(result_body(&msgs[2]).starts_with("[tool output elided from context"));
    assert!(
        result_body(&msgs[2]).contains("sess"),
        "stub cites the session file"
    );
    assert!(result_body(&msgs[4]).starts_with("[tool output elided"));
    assert_eq!(
        result_body(&msgs[6]).len(),
        ARC_STUB_MIN_BYTES,
        "recent round c verbatim"
    );
    assert_eq!(
        result_body(&msgs[8]).len(),
        ARC_STUB_MIN_BYTES,
        "recent round d verbatim"
    );
    assert_eq!(msgs[0], Message::user("fix it"), "intent untouched");

    assert_eq!(elide_stale_tool_results(&mut msgs, 2_100, Some("sess")), 0);
}

#[test]
fn soft_elision_leaves_small_results_and_a_generous_window_alone() {
    let mut msgs = vec![Message::user("go")];
    for id in ["a", "b", "c"] {
        msgs.push(assistant_tool_use(id));
        msgs.push(user_tool_result(id));
    }
    msgs.insert(1, assistant_tool_use("big"));
    msgs.insert(2, big_tool_result("big"));
    let snapshot = msgs.clone();

    assert_eq!(elide_stale_tool_results(&mut msgs, 1_000_000, None), 0);
    assert_eq!(msgs, snapshot);

    assert_eq!(elide_stale_tool_results(&mut msgs, 1, None), 1);
    assert_eq!(result_body(&msgs[4]), "ok");
}

fn pic_msg(tag: &str, bytes: usize) -> Message {
    let mut m = Message::user(tag);
    m.content
        .push(ContentBlock::image("image/png", "QUJD".repeat(bytes / 4)));
    m
}

fn raw_media(msgs: &[Message]) -> (usize, usize) {
    msgs.iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::Image { data, .. } | ContentBlock::Media { data, .. } => Some(data.len()),
            _ => None,
        })
        .fold((0, 0), |(count, bytes), n| (count + 1, bytes + n))
}

#[test]
fn media_under_payload_budget_is_untouched() {
    with_home(|| {
        let mut msgs: Vec<Message> = (0..9)
            .map(|i| pic_msg(&format!("shot{i}"), 2_000_000))
            .collect();
        let before = msgs.clone();
        assert_eq!(elide_media_over_payload_budget(&mut msgs), 0);
        assert_eq!(msgs, before);
    });
}

#[test]
fn media_over_payload_budget_keeps_the_newest_and_stubs_the_rest() {
    with_home(|| {
        let mut msgs: Vec<Message> = (0..12)
            .map(|i| pic_msg(&format!("shot{i}"), 2_000_000))
            .collect();
        let elided = elide_media_over_payload_budget(&mut msgs);
        assert_eq!(elided, 7);
        let (count, bytes) = raw_media(&msgs);
        assert_eq!(count, 5);
        assert!(bytes <= MEDIA_PAYLOAD_KEEP_BYTES);
        assert_eq!(raw_media(&msgs[7..]).0, 5, "the kept images are the newest");
        for m in &msgs[..7] {
            assert!(m.text_content().contains("shot"), "words survive: {m:?}");
            assert!(
                m.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text.contains("elided from context"))),
                "older image becomes a stub: {m:?}"
            );
        }
        let attachments = crate::paths::attachments_dir().unwrap();
        assert!(
            !attachments.exists() || std::fs::read_dir(&attachments).unwrap().next().is_none(),
            "the budget writes no extra copies to disk"
        );

        let settled = msgs.clone();
        msgs.push(pic_msg("next", 2_000_000));
        assert_eq!(
            elide_media_over_payload_budget(&mut msgs),
            0,
            "back under the max: the next image must not move the prefix"
        );
        assert_eq!(&msgs[..settled.len()], &settled[..]);
    });
}

#[test]
fn media_count_budget_caps_many_small_images() {
    with_home(|| {
        let mut msgs: Vec<Message> = (0..=MEDIA_PAYLOAD_MAX_COUNT)
            .map(|i| pic_msg(&format!("icon{i}"), 64))
            .collect();
        let elided = elide_media_over_payload_budget(&mut msgs);
        assert_eq!(
            elided,
            MEDIA_PAYLOAD_MAX_COUNT + 1 - MEDIA_PAYLOAD_KEEP_COUNT
        );
        assert_eq!(raw_media(&msgs).0, MEDIA_PAYLOAD_KEEP_COUNT);
    });
}

#[test]
fn media_pressure_stubs_cold_but_compacts_when_warm() {
    let mut agent = trigger_test_agent(
        Vec::new(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Vec::new(),
    );
    let mut msgs = vec![Message::user("hi")];
    for i in 0..=crate::compact::MEDIA_PAYLOAD_MAX_COUNT {
        let mut m = Message::user(format!("pic{i}"));
        m.content.push(ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "QUJD".to_string(),
        });
        msgs.push(m);
    }
    assert!(crate::compact::media_over_budget(&msgs));
    let baseline: Vec<Message> = msgs.iter().map(|m| m.clone()).collect();
    agent.set_messages(msgs);

    // Cold cache: in-place stubbing is nearly free — the prefix has not
    // been billed yet.
    assert_eq!(
        agent.media_pressure(),
        crate::agent_compact::MediaPressure::Stub
    );

    // Warm cache: an in-place stub would re-bill the whole suffix, so the
    // same pressure must route through compaction, and deciding must not
    // have touched history.
    agent.warm_cache_reported = true;
    assert_eq!(
        agent.media_pressure(),
        crate::agent_compact::MediaPressure::Compact
    );
    assert_eq!(agent.messages(), baseline.as_slice());

    // Under budget: nothing to do in either phase.
    let mut agent = trigger_test_agent(
        Vec::new(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Vec::new(),
    );
    agent.set_messages(vec![Message::user("no media")]);
    assert_eq!(
        agent.media_pressure(),
        crate::agent_compact::MediaPressure::Clear
    );
}

#[tokio::test]
async fn compaction_shares_the_cached_prefix_when_provable() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let iso: Arc<Mutex<Vec<Option<bool>>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let tools = vec![ToolDef::new(
        "read",
        "r",
        serde_json::json!({"type": "object"}),
    )];
    let mut agent = Agent::new(
        Box::new(
            CapturingProvider::new(
                vec![
                    StreamEvent::text_delta("reply"),
                    StreamEvent::message_complete(Some(StopReason::EndTurn), None),
                ],
                seen.clone(),
            )
            .with_iso_recorder(iso.clone()),
        ),
        Arc::new(RecordingExecutor { calls }),
    )
    .with_system("S")
    .with_tools(tools)
    .with_messages(vec![Message::user("m1")]);

    agent
        .run(Message::user("m2"), ToolContext::default())
        .await
        .expect("turn");
    let _ = agent.compact_v2(None, Some(0)).await.expect("compact ran");

    let reqs = seen.lock().expect("seen lock poisoned");
    assert_eq!(reqs.len(), 2, "turn + summary call, got {reqs:?}");
    let (turn_req, sum_req) = (&reqs[0], &reqs[1]);
    // The sent prefix is repeated verbatim, then the turn's reply and the
    // appended trigger extend it — a strict cache-extending suffix.
    assert_eq!(sum_req.messages.len(), turn_req.messages.len() + 2);
    assert_eq!(
        sum_req.messages[..turn_req.messages.len()],
        turn_req.messages[..],
        "summary request must repeat the sent prefix verbatim"
    );
    assert!(
        sum_req
            .messages
            .last()
            .expect("trigger")
            .text_content()
            .contains("Summarize this conversation")
    );
    assert_eq!(sum_req.system, turn_req.system);
    assert_eq!(sum_req.tools, turn_req.tools);
    assert_eq!(
        *iso.lock().expect("iso lock poisoned"),
        vec![None, Some(false)],
        "normal turn unscoped, summary call with cache markers on"
    );
}

#[tokio::test]
async fn compaction_falls_back_to_cold_call_when_prefix_diverged() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let iso: Arc<Mutex<Vec<Option<bool>>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut agent = Agent::new(
        Box::new(
            CapturingProvider::new(
                vec![
                    StreamEvent::text_delta("reply"),
                    StreamEvent::message_complete(Some(StopReason::EndTurn), None),
                ],
                seen.clone(),
            )
            .with_iso_recorder(iso.clone()),
        ),
        Arc::new(RecordingExecutor { calls }),
    )
    .with_system("S")
    .with_messages(vec![Message::user("m1")]);

    agent
        .run(Message::user("m2"), ToolContext::default())
        .await
        .expect("turn");
    // The wire view now differs from what was sent — no shared prefix.
    agent.contaminated.insert(0);
    let _ = agent.compact_v2(None, Some(0)).await.expect("compact ran");

    let reqs = seen.lock().expect("seen lock poisoned");
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1].messages[0]
            .text_content()
            .contains("cut off by a stream error"),
        "fallback must send the scrubbed (diverged) view: {:?}",
        reqs[1].messages[0]
    );
    assert_eq!(
        *iso.lock().expect("iso lock poisoned"),
        vec![None, Some(true)],
        "unprovable prefix must keep cache markers off"
    );
}

fn agent_with_wire_fp(agent: &mut Agent, wire: Vec<Message>) {
    let fp = crate::prefix_guard::fingerprint(
        agent.provider.model_id(),
        &ChatRequest {
            system: Some(agent.system_text().to_string()),
            messages: wire,
            tools: agent.request_tools(),
            max_tokens: None,
        },
    );
    agent.seed_prefix_fingerprint(fp);
}

#[test]
fn shared_prefix_appends_continue_tail_when_reply_is_pending() {
    let history = vec![Message::user("m1"), Message::assistant("a1")];
    let mut wire = history.clone();
    wire.push(Message::user_injected(CONTINUE_TAIL));
    let mut agent = trigger_test_agent(Vec::new(), Arc::default(), Arc::default(), Vec::new());
    agent_with_wire_fp(&mut agent, wire);

    let out = shared_prefix_compaction_messages(&agent, &history, None)
        .expect("tail must still be appended when history stops before it");
    assert_eq!(out.len(), 4, "history + tail + trigger: {out:?}");
    assert_eq!(out[2].text_content(), CONTINUE_TAIL);
}

#[test]
fn shared_prefix_splices_continue_tail_over_the_stored_reply() {
    let base = vec![Message::user("m1"), Message::assistant("a1")];
    let mut wire = base.clone();
    wire.push(Message::user_injected(CONTINUE_TAIL));
    let mut agent = trigger_test_agent(Vec::new(), Arc::default(), Arc::default(), Vec::new());
    agent_with_wire_fp(&mut agent, wire);

    // The reply is already in history where the wire carried the tail.
    let mut history = base;
    history.push(Message::assistant("reply"));
    let out = shared_prefix_compaction_messages(&agent, &history, None)
        .expect("spliced tail must re-prove the shared prefix");
    assert_eq!(out.len(), 5, "history + spliced tail + trigger: {out:?}");
    assert_eq!(out[2].text_content(), CONTINUE_TAIL);
    assert_eq!(out[3], Message::assistant("reply"));
    assert!(
        out[4].text_content().contains("Summarize this conversation"),
        "trigger still lands last: {:?}",
        out[4]
    );
}

#[test]
fn shared_prefix_stays_cold_when_divergence_is_not_the_tail() {
    let mut agent = trigger_test_agent(Vec::new(), Arc::default(), Arc::default(), Vec::new());
    agent_with_wire_fp(
        &mut agent,
        vec![Message::user("m1"), Message::assistant("a1")],
    );

    let history = vec![Message::user("CHANGED"), Message::assistant("a1")];
    assert!(shared_prefix_compaction_messages(&agent, &history, None).is_none());
}

#[tokio::test]
async fn over_budget_media_takes_the_cold_compaction_path() {
    let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
    let iso: Arc<Mutex<Vec<Option<bool>>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut history = vec![Message::user("m1")];
    for i in 0..=MEDIA_PAYLOAD_MAX_COUNT {
        let mut m = Message::user(format!("pic{i}"));
        m.content.push(ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "QUJD".to_string(),
        });
        history.push(m);
    }
    let mut agent = Agent::new(
        Box::new(
            CapturingProvider::new(
                vec![
                    StreamEvent::text_delta("SUMMARY"),
                    StreamEvent::message_complete(Some(StopReason::EndTurn), None),
                ],
                seen.clone(),
            )
            .with_iso_recorder(iso.clone()),
        ),
        Arc::new(RecordingExecutor { calls }),
    )
    .with_system("S")
    .with_messages(history.clone());
    agent_with_wire_fp(&mut agent, history);

    let out = agent.compact_v2(None, Some(0)).await.expect("compact ran");
    assert!(out.is_some(), "cold path must still produce a summary");
    assert_eq!(
        *iso.lock().expect("iso lock poisoned"),
        vec![Some(true)],
        "over-budget media must take the stubbed cold call, not the shared request"
    );
    let reqs = seen.lock().expect("seen lock poisoned");
    assert_eq!(reqs.len(), 1);
    let images = reqs[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::Image { .. }))
        .count();
    assert!(
        images <= MEDIA_PAYLOAD_KEEP_COUNT,
        "the cold request must go out elided: {images} images"
    );
}
