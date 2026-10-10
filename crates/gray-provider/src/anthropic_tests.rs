use super::*;
use gray_core::message::{Message, ToolDef};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "claude-sonnet-4-5";

fn turn() -> ChatRequest {
    ChatRequest {
        system: Some("sys".into()),
        messages: vec![
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                injected: false,
                content: vec![
                    ContentBlock::Thinking {
                        text: "display only".into(),
                        encrypted_content: Some(
                            json!([{"type": "thinking", "thinking": "t", "signature": "sig"}])
                                .to_string(),
                        ),
                        item_id: Some(THINKING_ITEM_ID.into()),
                        model: Some(MODEL.into()),
                    },
                    ContentBlock::tool_use("c1", "bash", json!({"command": "ls"})),
                    ContentBlock::tool_use("c2", "bash", json!({"command": "pwd"})),
                ],
            },
            Message {
                role: Role::User,
                injected: false,
                content: vec![ContentBlock::ToolResult {
                    id: "c1".into(),
                    content: "out".into(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![
            ToolDef::new("read", "r", json!({"type": "object"})),
            ToolDef::new("bash", "run", json!({"type": "object"})),
        ],
        max_tokens: None,
    }
}

#[test]
fn breakpoints_sit_on_system_last_tool_and_last_block() {
    let v = map_request(turn(), MODEL, None, None, None).unwrap();
    assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(v["tools"][0].get("cache_control").is_none());
    assert_eq!(v["tools"][1]["cache_control"]["type"], "ephemeral");
    assert_eq!(v["tools"][1]["input_schema"]["type"], "object");
    let last = v["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["cache_control"]["type"], "ephemeral", "{v}");
    assert_eq!(v.to_string().matches("cache_control").count(), 3, "{v}");
}

#[test]
fn signed_thinking_replays_verbatim_for_the_same_model_only() {
    let v = map_request(turn(), MODEL, None, None, None).unwrap();
    let first = &v["messages"][1]["content"][0];
    assert_eq!(first["type"], "thinking");
    assert_eq!(
        first["thinking"], "t",
        "the signed text, not the display text"
    );
    assert_eq!(first["signature"], "sig");
    let other = map_request(turn(), "claude-opus-4-1", None, None, None).unwrap();
    assert_eq!(other["messages"][1]["content"][0]["type"], "tool_use");
}

#[test]
fn an_unanswered_tool_call_gets_a_stub_result_first() {
    let v = map_request(turn(), MODEL, None, None, None).unwrap();
    let results = v["messages"][2]["content"].as_array().unwrap();
    assert_eq!(results[0]["tool_use_id"], "c2");
    assert_eq!(results[0]["is_error"], true);
    assert_eq!(results[1]["tool_use_id"], "c1");
}

#[test]
fn a_stray_tool_result_becomes_text() {
    let req = ChatRequest {
        messages: vec![Message {
            role: Role::User,
            injected: false,
            content: vec![ContentBlock::ToolResult {
                id: "ghost".into(),
                content: "x".into(),
                is_error: false,
            }],
        }],
        ..Default::default()
    };
    let v = map_request(req, MODEL, None, None, None).unwrap();
    assert_eq!(v["messages"][0]["content"][0]["type"], "text");
}

#[test]
fn thinking_budget_fits_under_max_tokens_and_drops_sampling() {
    let v = map_request(turn(), MODEL, Some("high"), Some(0.5), None).unwrap();
    assert_eq!(v["thinking"]["budget_tokens"], 16384);
    assert_eq!(v["max_tokens"], 32_000);
    assert!(v.get("temperature").is_none());
    // The warmer's one-token replay cannot carry a budget: none is sent.
    let mut warm = turn();
    warm.max_tokens = Some(1);
    let v = map_request(warm, MODEL, Some("high"), None, None).unwrap();
    assert_eq!(v["max_tokens"], 1);
    assert!(v.get("thinking").is_none());
    let v = map_request(turn(), MODEL, Some("off"), Some(0.5), None).unwrap();
    assert!(v.get("thinking").is_none());
    assert_eq!(v["temperature"], 0.5);
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn script() -> Vec<Value> {
    vec![
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": 900, "cache_creation_input_tokens": 90, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "SIG"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "ok"}}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t1", "name": "bash"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"ls\"}"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 42}}),
        json!({"type": "message_stop"}),
    ]
}

#[test]
fn decoder_maps_the_event_stream() {
    let mut d = Decoder::default();
    let mut out = VecDeque::new();
    for e in script() {
        d.feed(&e.to_string(), &mut out);
    }
    let evs: Vec<StreamEvent> = out.into_iter().map(Result::unwrap).collect();
    assert!(matches!(&evs[0], StreamEvent::ThinkingDelta { delta } if delta == "hmm"));
    assert!(matches!(&evs[1], StreamEvent::TextDelta { delta } if delta == "ok"));
    assert!(
        matches!(&evs[2], StreamEvent::ToolCallDelta { index: 2, id: Some(id), name: Some(n), .. } if id == "t1" && n == "bash")
    );
    let args: String = evs
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta {
                arguments_delta, ..
            } => Some(arguments_delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(args, "{\"command\":\"ls\"}");
    let StreamEvent::ReasoningItem {
        item_id,
        encrypted_content,
    } = &evs[evs.len() - 2]
    else {
        panic!("{evs:?}")
    };
    assert_eq!(item_id, THINKING_ITEM_ID);
    let saved: Value = serde_json::from_str(encrypted_content).unwrap();
    assert_eq!(
        saved,
        json!([{"type": "thinking", "thinking": "hmm", "signature": "SIG"}])
    );
    let StreamEvent::MessageComplete {
        stop_reason,
        usage: Some(u),
    } = evs.last().unwrap()
    else {
        panic!("{evs:?}")
    };
    assert_eq!(*stop_reason, Some(StopReason::ToolUse));
    assert_eq!(u.input_tokens, 1000, "inclusive of cache reads and writes");
    assert_eq!(u.cache_read_input_tokens, 900);
    assert_eq!(u.cache_write_input_tokens, 90);
    assert_eq!(u.output_tokens, 42);
}

#[test]
fn separate_text_blocks_in_one_message_get_a_blank_line() {
    // One assistant message, [text][thinking][text]: the second text block
    // must not glue onto the first (`…now.Sorry…`).
    let events = vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "one"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " two"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "three"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": " four"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_stop"}),
    ];
    let mut d = Decoder::default();
    let mut out = VecDeque::new();
    for e in events {
        d.feed(&e.to_string(), &mut out);
    }
    let text: String = out
        .into_iter()
        .filter_map(|e| match e.unwrap() {
            StreamEvent::TextDelta { delta } => Some(delta),
            _ => None,
        })
        .collect();
    assert_eq!(text, "one two\n\nthree four");
}

#[tokio::test]
async fn streams_after_retrying_an_overloaded_529() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_string("overloaded"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "k"))
        .and(header("anthropic-version", API_VERSION))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&script())),
        )
        .mount(&server)
        .await;
    let p = AnthropicProvider::new("k", MODEL, format!("{}/v1", server.uri()), None).unwrap();
    let evs: Vec<_> = p.stream(turn()).collect().await;
    assert!(evs.iter().all(Result::is_ok), "{evs:?}");
    assert!(matches!(
        evs.last(),
        Some(Ok(StreamEvent::MessageComplete { .. }))
    ));
}

#[tokio::test]
async fn a_cut_stream_is_a_retryable_error() {
    let server = MockServer::start().await;
    let cut: Vec<Value> = script().into_iter().take(3).collect();
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&cut)),
        )
        .mount(&server)
        .await;
    let p = AnthropicProvider::new("k", MODEL, format!("{}/v1", server.uri()), None).unwrap();
    let evs: Vec<_> = p.stream(turn()).collect().await;
    let err = evs.last().unwrap().as_ref().unwrap_err();
    assert!(err.retryable(), "{err}");
}

#[tokio::test]
async fn a_bad_key_is_an_auth_error_without_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid x-api-key"))
        .expect(1)
        .mount(&server)
        .await;
    let p = AnthropicProvider::new("k", MODEL, format!("{}/v1", server.uri()), None).unwrap();
    let evs: Vec<_> = p.stream(turn()).collect().await;
    assert!(matches!(evs.as_slice(), [Err(ProviderError::Auth(_))]));
}

#[test]
fn only_anthropics_own_host_takes_the_native_wire() {
    assert!(is_anthropic_base_url("https://api.anthropic.com/v1"));
    assert!(!is_anthropic_base_url("https://openrouter.ai/api/v1"));
    assert!(!is_anthropic_base_url("not a url"));
}

#[test]
fn pdf_goes_as_a_document_and_video_as_its_fallback() {
    let req = ChatRequest {
        system: None,
        messages: vec![Message::new(
            Role::User,
            vec![
                ContentBlock::media("application/pdf", "UERG", vec![]),
                ContentBlock::media("video/mp4", "VklE", vec![ContentBlock::text("(sheet)")]),
            ],
        )],
        tools: Vec::new(),
        max_tokens: None,
    };
    let v = map_request(req, MODEL, None, None, None).unwrap();
    let blocks = &v["messages"][0]["content"];
    assert_eq!(blocks[0]["type"], "document");
    assert_eq!(blocks[0]["source"]["media_type"], "application/pdf");
    assert_eq!(blocks[1]["text"], "(sheet)");
}
