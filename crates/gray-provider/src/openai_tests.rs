use super::*;
use crate::{
    OpenAiAuthorization, OpenAiHeader, OpenAiHeaderSource, OpenAiProviderProfile,
    OpenAiRequestPolicy, OpenAiWire,
};
use gray_core::message::Message;

#[test]
fn structured_input_is_mapped_as_marked_user_text_in_both_request_shapes() {
    let request = ChatRequest {
        system: None,
        messages: vec![Message::new(
            gray_core::Role::User,
            vec![ContentBlock::StructuredInput {
                protocol: "gray.discord.input".into(),
                version: 1,
                kind: "component_event".into(),
                payload: serde_json::json!({"action": "refresh", "values": {"id": "7"}}),
            }],
        )],
        tools: Vec::new(),
    };

    let chat = serde_json::to_value(map_chat_request(request.clone(), "test-model", None).unwrap())
        .unwrap();
    let chat_text = chat["messages"][0]["content"].as_str().unwrap();
    assert!(chat_text.contains("gray_structured_input"));
    assert!(chat_text.contains("component_event"));

    let responses =
        serde_json::to_value(map_chat_to_responses(request, "test-model", None, None)).unwrap();
    let responses_text = responses["input"][0]["content"].as_str().unwrap();
    assert!(responses_text.contains("gray_structured_input"));
    assert!(responses_text.contains("component_event"));
}

#[test]
fn debug_never_contains_api_key() {
    let p = OpenAiProvider::new("sk-sentinel-secret", "m", "", None, None).unwrap();
    let dbg = format!("{p:?}");
    assert!(!dbg.contains("sk-sentinel-secret"), "{dbg}");
}

#[test]
fn unsupported_model_500_maps_to_bad_request_and_preserves_cf_ray() {
    let err = classify_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "Model not supported: xyz",
        Some("abc123-ray"),
        Some("req-1"),
    );
    assert!(matches!(err, ProviderError::BadRequest(_)));
    assert!(!is_retryable_error(&err));
    let msg = err.to_string();
    assert!(msg.contains("cf-ray: abc123-ray"), "{msg}");
    assert!(msg.contains("request-id: req-1"), "{msg}");
}

#[test]
fn rate_limited_429_is_retryable() {
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "rate limit",
        None,
        None,
    );
    assert!(matches!(err, ProviderError::RateLimited(_)));
    assert!(is_retryable_error(&err));
}

#[test]
fn auth_401_insufficient_balance_is_not_retryable() {
    let err = classify_http_error(
        reqwest::StatusCode::UNAUTHORIZED,
        "insufficient balance or invalid api key",
        None,
        None,
    );
    assert!(matches!(err, ProviderError::Auth(_)));
    assert!(!is_retryable_error(&err));
}

#[test]
fn plain_500_without_model_hint_is_server_error_retryable() {
    let err = classify_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "internal error",
        None,
        None,
    );
    assert!(matches!(err, ProviderError::ServerError(_)));
    assert!(is_retryable_error(&err));
}

#[test]
fn retry_notice_uses_codex_reconnecting_format() {
    // Codex steal: `Reconnecting... n/m` header + short underlying error.
    let err = ProviderError::ServerError("status 503: backend overloaded".to_string());
    let ev = retry_notice_event(1, 3, &err);
    match ev {
        StreamEvent::StreamError { message, details } => {
            assert_eq!(message, "Reconnecting... 1/3");
            assert!(details.contains("503"), "details keeps cause: {details}");
        }
        other => panic!("expected StreamError, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_burst_emits_a_single_reconnect_notice() {
    // Screenshot bug: attempts 1/3, 2/3 each pushed a `⚠ Reconnecting`
    // cell so the transcript showed the same failure twice. One burst =
    // one notice; the burst still ends with the terminal error.
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    let notices = events
        .iter()
        .filter(|r| matches!(r, Ok(StreamEvent::StreamError { .. })))
        .count();
    assert_eq!(notices, 1, "one reconnect notice per burst: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(_))),
        "burst ends with terminal error: {events:?}"
    );
}

#[tokio::test]
async fn session_header_sent_on_chat_post() {
    // Console Go 400s without `x-opencode-session`: every POST in the
    // default retry burst must carry the configured session id.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new(
        "key",
        "test-model",
        server.uri(),
        None,
        Some("sess-123".to_string()),
    )
    .expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let _events: Vec<_> = provider.stream(req).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        MAX_ATTEMPTS,
        "default retry burst POSTs once per attempt"
    );
    for r in &received {
        let got = r
            .headers
            .get("x-opencode-session")
            .expect("session header sent");
        assert_eq!(got.to_str().expect("header ascii"), "sess-123");
    }
}

fn responses_req_with_thinking(_model: &str) -> ChatRequest {
    use gray_core::message::Message;
    ChatRequest {
        system: Some("sys".to_string()),
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "hmm".to_string(),
                    encrypted_content: Some("blob".to_string()),
                    item_id: Some("rs_1".to_string()),
                    model: Some("m1".to_string()),
                },
                ContentBlock::text("answer"),
            ],
        }],
        tools: Vec::new(),
    }
}

#[test]
fn responses_include_and_replay_round_trip() {
    let body = map_chat_to_responses(
        responses_req_with_thinking("m1"),
        "m1",
        Some("sess"),
        Some("high"),
    );
    let v = serde_json::to_value(&body).expect("serializes");
    // encrypted-content include rides with reasoning
    let include = v
        .get("include")
        .and_then(|i| i.as_array())
        .expect("include");
    assert!(
        include
            .iter()
            .any(|s| s.as_str() == Some("reasoning.encrypted_content"))
    );
    assert!(
        v.get("reasoning")
            .and_then(|r| r.get("summary"))
            .and_then(|s| s.as_str())
            == Some("auto")
    );
    // same-model reasoning item replayed verbatim ahead of text
    let input = v.get("input").and_then(|i| i.as_array()).expect("input");
    let reason = input
        .iter()
        .find(|i| i.get("type").and_then(|t| t.as_str()) == Some("reasoning"))
        .expect("reasoning item");
    assert_eq!(reason.get("id").and_then(|v| v.as_str()), Some("rs_1"));
    assert_eq!(
        reason.get("encrypted_content").and_then(|v| v.as_str()),
        Some("blob")
    );
}

#[test]
fn responses_replay_drops_foreign_model_thinking() {
    let body = map_chat_to_responses(
        responses_req_with_thinking("m1"),
        "m2",
        Some("sess"),
        Some("high"),
    );
    let v = serde_json::to_value(&body).expect("serializes");
    let input = v.get("input").and_then(|i| i.as_array()).expect("input");
    assert!(
        input
            .iter()
            .all(|i| i.get("type").and_then(|t| t.as_str()) != Some("reasoning"))
    );
    // prose still sent
    assert!(
        input
            .iter()
            .any(|i| i.get("role").and_then(|r| r.as_str()) == Some("assistant"))
    );
}

#[test]
fn responses_include_omitted_when_reasoning_off() {
    let body = map_chat_to_responses(
        responses_req_with_thinking("m1"),
        "m1",
        Some("sess"),
        Some("off"),
    );
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(v.get("include").is_none());
    assert!(v.get("reasoning").is_none());
}
#[test]
fn previous_response_id_serializes_only_when_set() {
    let body = map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high"));
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        v.get("previous_response_id").is_none(),
        "absent by default: {v}"
    );
    let mut resumed = map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high"));
    resumed.previous_response_id = Some("resp_123".to_string());
    let v2 = serde_json::to_value(&resumed).expect("serializes");
    assert_eq!(
        v2.get("previous_response_id").and_then(|s| s.as_str()),
        Some("resp_123"),
        "present when set: {v2}"
    );
}

#[test]
fn resume_carries_tool_prefix_on_id_drops_on_none() {
    // Simulated mid-tool interruption: `{"q":"x"` arrived, suffix pending.
    let prefix = || {
        let mut tools: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
        tools.insert(
            "call_1".to_string(),
            (0, "lookup".to_string(), "{\"q\":\"x\"".to_string()),
        );
        let mut index: BTreeMap<usize, String> = BTreeMap::new();
        index.insert(0, "call_1".to_string());
        (tools, index)
    };
    // Some(id): continuation — prefix carried for suffix appends, id stamped.
    // NOTE: `map_chat_to_responses(..., "high")` sets reasoning, and the
    // provider rejects `previous_response_id` with encrypted reasoning —
    // so `resume_body_and_tool_prefix` drops the id here. Pass `None`
    // effort to exercise the id-carrying path.
    let (tools, index) = prefix();
    let (body, kept_tools, kept_index) = resume_body_and_tool_prefix(
        map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), None),
        tools,
        index,
        Some("resp_9".to_string()),
    );
    assert_eq!(body.previous_response_id.as_deref(), Some("resp_9"));
    assert_eq!(
        kept_tools.get("call_1").map(|e| e.2.as_str()),
        Some("{\"q\":\"x\""),
        "prefix args survive for suffix append"
    );
    assert_eq!(kept_index.get(&0).map(String::as_str), Some("call_1"));
    let v = serde_json::to_value(&body).expect("serializes");
    assert_eq!(
        v.get("previous_response_id").and_then(|s| s.as_str()),
        Some("resp_9")
    );
    // None: true full replay — stale prefix dropped, no id sent.
    let (tools, index) = prefix();
    let (body, dropped_tools, dropped_index) = resume_body_and_tool_prefix(
        map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high")),
        tools,
        index,
        None,
    );
    assert!(body.previous_response_id.is_none());
    assert!(dropped_tools.is_empty() && dropped_index.is_empty());
}

#[test]
fn responses_usage_maps_input_tokens_details_cached() {
    // Responses API shape: input_tokens_details.cached_tokens (not prompt_tokens_details)
    let v = serde_json::json!({
        "input_tokens": 1000,
        "output_tokens": 200,
        "input_tokens_details": {"cached_tokens": 800},
        "output_tokens_details": {"reasoning_tokens": 50},
        "total_tokens": 1200
    });
    let u: OpenAiUsageChunk = serde_json::from_value(v).expect("parses");
    let usage = map_usage(&u);
    assert_eq!(usage.cache_read_input_tokens, 800, "cache read: {usage:?}");
    assert_eq!(usage.cached_tokens, 800, "legacy alias: {usage:?}");
    assert_eq!(usage.reasoning_tokens, 50, "reasoning: {usage:?}");
    assert_eq!(usage.input_tokens, 1000, "input inclusive: {usage:?}");
}

#[test]
fn chat_usage_prefers_explicit_cache_miss_tokens() {
    // DeepSeek-style: explicit miss count beats prompt-minus-cached subtraction.
    let v = serde_json::json!({
        "prompt_tokens": 1000,
        "completion_tokens": 50,
        "prompt_tokens_details": {"cached_tokens": 800},
        "prompt_cache_miss_tokens": 150
    });
    let u: OpenAiUsageChunk = serde_json::from_value(v).expect("parses");
    let usage = map_usage(&u);
    assert_eq!(
        usage.non_cached_input_tokens, 150,
        "miss preferred: {usage:?}"
    );
    assert_eq!(usage.cache_read_input_tokens, 800, "read kept: {usage:?}");
}

#[test]
fn context_overflow_bodies_map_to_non_retryable_context_overflow() {
    for body in [
        "maximum context length is 128000 tokens, requested 200000",
        "this model's maximum context window is exceeded",
        "too many tokens in this request",
        "prompt is too long for this model",
        "max_tokens exceeded: reduce input size",
    ] {
        let err = classify_http_error(reqwest::StatusCode::BAD_REQUEST, body, None, None);
        assert!(
            matches!(err, ProviderError::ContextOverflow(_)),
            "body: {body}"
        );
        assert!(!is_retryable_error(&err), "must not retry: {body}");
        assert!(err.should_compress(), "must flag compression: {body}");
        assert!(
            err.to_string().contains("context exhausted"),
            "actionable: {err}"
        );
    }
}

#[test]
fn content_filter_bodies_map_to_non_retryable_bad_request() {
    for body in [
        "content_filter: response was flagged by safety classifier",
        "request violates usage policies",
    ] {
        let err = classify_http_error(reqwest::StatusCode::BAD_REQUEST, body, None, None);
        assert!(matches!(err, ProviderError::BadRequest(_)), "body: {body}");
        assert!(!is_retryable_error(&err), "must not retry: {body}");
        assert!(!err.should_compress());
    }
    // Narrowed: hyphen form still counts (500 proves filter path, not generic 400).
    for body in ["content-filter triggered", "content_filter triggered"] {
        let err = classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
        assert!(
            matches!(err, ProviderError::BadRequest(_)),
            "filter must win over 500: {body} -> {err}"
        );
    }
    // Bare violates/flagged/safety without a filter qualifier are NOT filters:
    // 500 must stay retryable ServerError, not BadRequest.
    for body in [
        "request violates policy",
        "response was flagged",
        "safety check failed",
    ] {
        let err = classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
        assert!(
            matches!(err, ProviderError::ServerError(_)),
            "bare hint must not filter: {body} -> {err}"
        );
    }
    // Bare max_tokens without a context qualifier is NOT overflow.
    {
        let err = classify_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            "max_tokens must be positive",
            None,
            None,
        );
        assert!(
            !matches!(err, ProviderError::ContextOverflow(_)),
            "bare max_tokens: {err}"
        );
    }
    // max_tokens needs a context qualifier to count as overflow.
    for body in [
        "max_tokens exceeded: context too long",
        "max_tokens context exceeded",
    ] {
        let err = classify_http_error(reqwest::StatusCode::BAD_REQUEST, body, None, None);
        assert!(
            matches!(err, ProviderError::ContextOverflow(_)),
            "body: {body}"
        );
    }
}

#[test]
fn retry_after_header_is_backoff_floor() {
    let base = Duration::from_millis(50);
    assert!(backoff_delay(base, 1, None) < Duration::from_secs(1));
    assert!(
        backoff_delay(base, 1, Some(Duration::from_secs(5))) >= Duration::from_secs(5),
        "numeric Retry-After must floor the backoff"
    );
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
    assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(3)));
    headers.insert(
        reqwest::header::RETRY_AFTER,
        "not-a-number".parse().unwrap(),
    );
    assert_eq!(parse_retry_after(&headers), None);
    assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
}

fn empty_chat_req() -> gray_core::message::ChatRequest {
    gray_core::message::ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    }
}

#[tokio::test]
async fn sampling_params_are_sent_when_configured() {
    // GRAY_TEMPERATURE / GRAY_TOP_P passthrough: set -> present in the body.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds")
        .with_sampling(Some(1.0), Some(0.95));
    let _events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("request body is json");
    assert_eq!(
        body.get("temperature").and_then(|v| v.as_f64()),
        Some(1.0),
        "temperature sent: {body}"
    );
    // f32 -> JSON f64 widens (0.95f32 != 0.95f64), so compare in f32 space.
    assert_eq!(
        body.get("top_p").and_then(|v| v.as_f64()),
        Some(f64::from(0.95f32)),
        "top_p sent: {body}"
    );
}

#[tokio::test]
async fn sampling_params_are_omitted_by_default() {
    // Unset -> the keys stay out of the body entirely, so the server default
    // applies exactly as before this option existed.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let _events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("request body is json");
    assert!(
        body.get("temperature").is_none(),
        "no temperature by default: {body}"
    );
    assert!(body.get("top_p").is_none(), "no top_p by default: {body}");
}

#[test]
fn chat_mapping_off_sends_thinking_disabled_only() {
    let body = map_chat_request(empty_chat_req(), "zai/glm-5.2", Some("off")).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        v.get("reasoning_effort").is_none(),
        "off sends no reasoning_effort: {v}"
    );
    assert!(v.get("reasoning").is_none(), "off sends no reasoning: {v}");
    assert_eq!(
        v.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
        "off disables thinking: {v}"
    );
}

#[test]
fn chat_mapping_low_sends_all_three_reasoning_params() {
    let body = map_chat_request(empty_chat_req(), "zai/glm-5.2", Some("low")).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert_eq!(
        v.get("reasoning_effort").and_then(|s| s.as_str()),
        Some("low"),
        "reasoning_effort: {v}"
    );
    assert_eq!(
        v.get("reasoning")
            .and_then(|r| r.get("effort"))
            .and_then(|s| s.as_str()),
        Some("low"),
        "reasoning.effort: {v}"
    );
    assert_eq!(
        v.get("thinking")
            .and_then(|t| t.get("type"))
            .and_then(|s| s.as_str()),
        Some("enabled"),
        "thinking enabled: {v}"
    );
    assert_eq!(
        v.get("thinking")
            .and_then(|t| t.get("budget_tokens"))
            .and_then(|n| n.as_u64()),
        Some(1024),
        "low budget: {v}"
    );
}

#[test]
fn responses_reasoning_forwards_max_and_off_to_none() {
    assert!(map_responses_reasoning(None).is_none());
    assert!(map_responses_reasoning(Some("off")).is_none());
    let max = map_responses_reasoning(Some("max")).expect("max maps");
    assert_eq!(max.get("effort").and_then(|s| s.as_str()), Some("max"));
    assert_eq!(max.get("summary").and_then(|s| s.as_str()), Some("auto"));
    let high = map_responses_reasoning(Some("high")).expect("high maps");
    assert_eq!(high.get("effort").and_then(|s| s.as_str()), Some("high"));
}

#[test]
fn reasoning_400_with_extra_inputs_retries_without_reasoning() {
    assert!(
        should_retry_without_reasoning(
            400,
            "Extra inputs are not permitted, field: 'reasoning'",
            true
        ),
        "glm max/low 400 must retry stripped"
    );
    assert!(
        should_retry_without_reasoning(
            400,
            "status 400: Extra inputs are not permitted, field: 'reasoning'",
            true
        ),
        "classified BadRequest message must also match"
    );
}

#[test]
fn thinking_only_400_retries_without_reasoning_when_params_were_sent() {
    // glm at off sends `thinking: disabled`; a thinking-only model 400s.
    // Stripping the disable lets the provider fall back to default thinking.
    assert!(
        should_retry_without_reasoning(400, "thinking-only model", true),
        "off-path 400 must retry stripped"
    );
}

#[test]
fn reasoning_retry_predicate_rejects_non_cases() {
    assert!(
        !should_retry_without_reasoning(
            400,
            "Extra inputs are not permitted, field: 'reasoning'",
            false
        ),
        "nothing to strip -> no retry"
    );
    assert!(
        !should_retry_without_reasoning(400, "model not found: xyz", true),
        "unrelated 400 -> no retry"
    );
    assert!(
        !should_retry_without_reasoning(
            429,
            "Extra inputs are not permitted, field: 'reasoning'",
            true
        ),
        "only 400 retries"
    );
    assert!(
        !should_retry_without_reasoning(500, "internal error", true),
        "only 400 retries"
    );
}

#[test]
fn strip_chat_reasoning_omits_all_three_wire_fields() {
    let mut body = map_chat_request(empty_chat_req(), "zai/glm-5.2", Some("low")).expect("maps");
    assert!(
        chat_has_reasoning_params(&body),
        "precondition: low sends params"
    );
    strip_chat_reasoning_params(&mut body);
    assert!(!chat_has_reasoning_params(&body), "stripped");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(v.get("reasoning").is_none());
    assert!(v.get("reasoning_effort").is_none());
    assert!(v.get("thinking").is_none());
}

#[test]
fn reasoning_conflict_error_names_model_and_conflict() {
    let msg = reasoning_conflict_hint(
        "zai/glm-5.2",
        "status 400: Extra inputs are not permitted, field: 'reasoning'",
    );
    assert!(msg.contains("zai/glm-5.2"), "names model: {msg}");
    assert!(msg.contains("reasoning"), "names conflict: {msg}");
    assert!(msg.contains("/thinking"), "actionable hint: {msg}");
}

#[test]
fn chat_mapping_user_and_system_roles_map_without_panic() {
    // Wave 5: the old `unreachable!()` on a novel role is now a
    // `BadRequest` error; user/system roles keep mapping.
    let req = gray_core::message::ChatRequest {
        system: None,
        messages: vec![
            gray_core::message::Message::user("hi"),
            gray_core::message::Message::system("be nice"),
        ],
        tools: Vec::new(),
    };
    let body = map_chat_request(req, "test-model", None).expect("user/system roles map");
    let roles: Vec<&str> = body.messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "system"]);
}

#[test]
fn serialize_body_maps_failure_to_bad_request() {
    // Wave 5: the old `to_value().expect()` on the send hot path is now
    // an `Err`; prove the mapping with a value JSON cannot represent.
    let bad: std::collections::BTreeMap<(), u8> = [((), 1)].into_iter().collect();
    let err = serialize_body(&bad, "test body").expect_err("unit keys must fail");
    assert!(matches!(err, ProviderError::BadRequest(_)), "got {err:?}");
}

#[test]
fn serialize_body_round_trips_chat_request() {
    let body = map_chat_request(empty_chat_req(), "test-model", None).expect("maps");
    let v = serialize_body(&body, "chat request").expect("serializes");
    assert_eq!(v.get("model").and_then(|m| m.as_str()), Some("test-model"));
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn quota_429_surfaces_immediately_as_non_retryable() {
    for body in [
        "insufficient_quota: you exceeded your current quota",
        "Insufficient quota: check your plan",
        "insufficient credits: top up to continue",
        "insufficient balance on this key",
        "You exceeded your current quota, please check your plan and billing details",
        "usage failed: GoUsageLimitError: limit reached",
        "check your available balance and billing details",
        "quota exceeded for this project",
        "QUOTA_EXCEEDED: monthly budget spent",
    ] {
        let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, None, None);
        assert!(
            matches!(err, ProviderError::Auth(_)),
            "quota 429 must surface as terminal Auth: {body} -> {err}"
        );
        assert!(!is_retryable_error(&err), "must not fast-retry: {body}");
        assert!(
            !err.should_compress(),
            "compaction cannot fix billing: {body}"
        );
    }
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn plain_429_without_quota_hints_stays_retryable() {
    for body in [
        "Rate limit reached for model",
        "rate_limit_exceeded: slow down",
        // Upsell copy mentions billing but is still a rate limit: the
        // quota phrases stay narrow so this keeps retrying.
        "Rate limit exceeded. Upgrade your billing plan for higher limits.",
    ] {
        let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, None, None);
        assert!(
            matches!(err, ProviderError::RateLimited(_)),
            "plain 429 stays retryable: {body} -> {err}"
        );
        assert!(is_retryable_error(&err), "must retry: {body}");
    }
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn http_413_maps_to_context_overflow_never_retried() {
    let err = classify_http_error(
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "Request Entity Too Large",
        None,
        None,
    );
    assert!(
        matches!(err, ProviderError::ContextOverflow(_)),
        "413 must compact, not retry: {err}"
    );
    assert!(!is_retryable_error(&err), "never generic-retry: {err}");
    assert!(err.should_compress(), "must flag compression: {err}");
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn retry_after_accepts_ms_and_http_date_and_caps_at_60s() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after-ms", "1500".parse().unwrap());
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_millis(1500)),
        "millis header honored"
    );
    headers.insert("retry-after-ms", "90000".parse().unwrap());
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(60)),
        "millis capped at 60s"
    );
    headers.remove("retry-after-ms");
    headers.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(60)),
        "seconds capped at 60s"
    );
    // Fixed far-future date: delay is huge, cap pins it at 60s.
    headers.insert(
        reqwest::header::RETRY_AFTER,
        "Sun, 06 Nov 2033 08:49:37 GMT".parse().unwrap(),
    );
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(60)),
        "HTTP-date capped at 60s"
    );
    // Fixed past date: already due, retry now.
    headers.insert(
        reqwest::header::RETRY_AFTER,
        "Sun, 06 Nov 1994 08:49:37 GMT".parse().unwrap(),
    );
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::ZERO),
        "past HTTP-date means now"
    );
    headers.insert(reqwest::header::RETRY_AFTER, "not-a-date".parse().unwrap());
    assert_eq!(parse_retry_after(&headers), None, "garbage stays None");
    // Millis wins when both headers are present (more precise).
    headers.insert(reqwest::header::RETRY_AFTER, "45".parse().unwrap());
    headers.insert("retry-after-ms", "2000".parse().unwrap());
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(2)),
        "ms takes precedence"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn long_body_quota_signal_survives_snippet_cap() {
    // Mirrors the `take(4000)` snippet cap in `send_json_once`: the quota
    // phrase sits past char 500 (the old cap cut it off and the 429 fell
    // through to retryable RateLimited).
    let snippet = |body: &str| -> String { body.chars().take(4000).collect() };
    let pad = "x".repeat(600);
    let body = format!("{pad} insufficient_quota: billing exhausted");
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &snippet(&body),
        None,
        None,
    );
    assert!(
        matches!(err, ProviderError::Auth(_)),
        "late quota signal must surface: {err}"
    );
    let plain = format!("{pad} Rate limit reached, slow down");
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &snippet(&plain),
        None,
        None,
    );
    assert!(
        matches!(err, ProviderError::RateLimited(_)),
        "long != quota: {err}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[tokio::test]
async fn quota_429_body_past_500_chars_terminal_end_to_end() {
    // Server puts `insufficient_quota` past the old 500-char cut: the
    // turn must end with one terminal Auth Err (no `Reconnecting...`
    // notice, no retry POSTs).
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let server = wiremock::MockServer::start().await;
    let body = format!(
        "{} insufficient_quota: you exceeded your current quota",
        "x".repeat(600)
    );
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(429).set_body_string(body))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    assert_eq!(events.len(), 1, "immediate terminal, no notice: {events:?}");
    assert!(
        matches!(events[0], Err(ProviderError::Auth(_))),
        "terminal Auth: {events:?}"
    );
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "no retry POST burns quota: {received:?}");
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[tokio::test]
async fn dropped_stream_fires_no_retry_post_after_backoff() {
    // Cancel-abort contract: `Retry-After: 2` parks the retry behind a 2s
    // sleep; dropping the stream mid-backoff must abort it — no second
    // POST may escape after the drop. (A broken abort would fire retry
    // POST #2 at ~2s, so the ~3s wait below fails iff abort regresses.)
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(429)
                .insert_header("retry-after", "2")
                .set_body_string("Rate limit reached, slow down"),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let mut stream = provider.stream(req);
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("notice arrives fast")
        .expect("stream yields");
    assert!(
        matches!(first, Ok(StreamEvent::StreamError { .. })),
        "one reconnect notice: {first:?}"
    );
    // The retry now sleeps 2s; abort it via drop (this is what the
    // consumer's cancel `select!` does on Esc), then wait past the
    // 2s mark: a surviving sleep would have fired POST #2 by then.
    let pending = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
    assert!(pending.is_err(), "backoff still sleeping at 300ms");
    drop(stream);
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        1,
        "aborted backoff fires no retry POST: {received:?}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[tokio::test]
async fn chat_post_carries_prompt_cache_key_body() {
    // Chat turns must pin the cache shard in the BODY (the
    // `x-opencode-session` header alone left chat rotating shards).
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new(
        "key",
        "test-model",
        server.uri(),
        None,
        Some("sess-123".to_string()),
    )
    .expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let _events: Vec<_> = provider.stream(req).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        MAX_ATTEMPTS,
        "default retry burst POSTs once per attempt"
    );
    let body: serde_json::Value = received[0].body_json().expect("json body");
    assert_eq!(
        body.get("prompt_cache_key").and_then(|k| k.as_str()),
        Some("sess-123"),
        "chat pins shard like Responses: {body}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn chat_affinity_omitted_when_unset() {
    // No-op elsewhere: unset session sends no `prompt_cache_key`, so
    // transports that don't understand it never see the field.
    let body = map_chat_request(empty_chat_req(), "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        v.get("prompt_cache_key").is_none(),
        "absent by default: {v}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn unknown_resume_id_retries_stripped_once() {
    // (2) a rejected `previous_response_id` replays once, not terminal.
    assert!(
        should_retry_without_previous_response(
            400,
            "status 400: previous_response_not_found",
            true
        ),
        "canonical 400 strips"
    );
    assert!(
        should_retry_without_previous_response(404, "previous response not found: resp_9", true),
        "404 prose strips"
    );
    assert!(
        should_retry_without_previous_response(400, "previous_response_id expired", true),
        "expired strips"
    );
    assert!(
        !should_retry_without_previous_response(400, "previous_response_not_found", false),
        "nothing sent -> no retry"
    );
    assert!(
        !should_retry_without_previous_response(400, "model not found: xyz", true),
        "unrelated 400 -> no retry"
    );
    assert!(
        !should_retry_without_previous_response(500, "previous_response_not_found", true),
        "only 400/404 retry"
    );
    // The retry arm clears the id, so the replay omits the field.
    let mut resumed = map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high"));
    resumed.previous_response_id = Some("resp_stale".to_string());
    resumed.previous_response_id = None;
    let v = serde_json::to_value(&resumed).expect("serializes");
    assert!(v.get("previous_response_id").is_none(), "stripped: {v}");
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn deepseek_assistant_messages_always_carry_reasoning_content() {
    // (3) DeepSeek expects the reasoning field on every assistant
    // message; other models keep today's omit-when-empty.
    use gray_core::message::Message;
    let plain_assistant = || ChatRequest {
        system: None,
        messages: vec![Message::assistant("hi")],
        tools: Vec::new(),
    };
    let last_msg = |v: &serde_json::Value| {
        v.get("messages")
            .and_then(|m| m.as_array())
            .and_then(|a| a.last().cloned())
            .expect("assistant msg")
    };
    let body = map_chat_request(plain_assistant(), "deepseek-reasoner", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert_eq!(
        last_msg(&v)
            .get("reasoning_content")
            .and_then(|r| r.as_str()),
        Some(""),
        "deepseek gets the (empty) field: {v}"
    );
    let body = map_chat_request(plain_assistant(), "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        last_msg(&v).get("reasoning_content").is_none(),
        "non-deepseek still omits: {v}"
    );
    // With thinking: deepseek keeps the chain like everyone else.
    let thinking_req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "hmm".to_string(),
                    encrypted_content: None,
                    item_id: None,
                    model: None,
                },
                ContentBlock::text("answer"),
            ],
        }],
        tools: Vec::new(),
    };
    let body = map_chat_request(thinking_req, "deepseek-chat", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert_eq!(
        last_msg(&v)
            .get("reasoning_content")
            .and_then(|r| r.as_str()),
        Some("hmm"),
        "chain preserved: {v}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn tool_error_flag_survives_chat_wire_encoding() {
    // (4) `is_error` has no wire slot on either transport: errors ride
    // as an `Error:`-prefixed body, successes pass through untouched.
    use gray_core::message::Message;
    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "sh".to_string(),
                    args: serde_json::json!({}),
                },
                ContentBlock::ToolResult {
                    id: "c1".to_string(),
                    content: "boom".to_string(),
                    is_error: true,
                },
                ContentBlock::ToolResult {
                    id: "c2".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                },
            ],
        }],
        tools: Vec::new(),
    };
    let body = map_chat_request(req, "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    let tools: Vec<&serde_json::Value> = v
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("messages")
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .collect();
    let by_id = |id: &str| {
        tools
            .iter()
            .find(|m| m.get("tool_call_id").and_then(|v| v.as_str()) == Some(id))
            .expect("tool msg")
    };
    assert_eq!(
        by_id("c1").get("content").and_then(|c| c.as_str()),
        Some("Error: boom"),
        "error flagged: {v}"
    );
    assert_eq!(
        by_id("c2").get("content").and_then(|c| c.as_str()),
        Some("ok"),
        "success untouched: {v}"
    );
    // User-arm tool results keep the flag too.
    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: "u1".to_string(),
                content: "denied".to_string(),
                is_error: true,
            }],
        }],
        tools: Vec::new(),
    };
    let body = map_chat_request(req, "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    let tool = v
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("messages")
        .iter()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .expect("tool msg");
    assert_eq!(
        tool.get("content").and_then(|c| c.as_str()),
        Some("Error: denied"),
        "user-arm error flagged: {v}"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn tool_error_flag_survives_responses_wire_encoding() {
    use gray_core::message::Message;
    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "sh".to_string(),
                    args: serde_json::json!({}),
                },
                ContentBlock::ToolResult {
                    id: "c1".to_string(),
                    content: "boom".to_string(),
                    is_error: true,
                },
            ],
        }],
        tools: Vec::new(),
    };
    let body = map_chat_to_responses(req, "m1", Some("sess"), None);
    let v = serde_json::to_value(&body).expect("serializes");
    let out = v
        .get("input")
        .and_then(|i| i.as_array())
        .expect("input")
        .iter()
        .find(|i| i.get("type").and_then(|t| t.as_str()) == Some("function_call_output"))
        .expect("call output");
    assert_eq!(
        out.get("output").and_then(|o| o.as_str()),
        Some("Error: boom"),
        "error flagged: {v}"
    );
}

#[tokio::test]
async fn oversize_tool_index_is_forwarded_not_silently_dropped() {
    // The agent owns the index guard (hard error); the provider must not
    // silently drop the call first — that loses model intent with a
    // clean EndTurn. Index 5000 must arrive downstream.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let chunk = serde_json::json!({
        "choices": [{
            "delta": { "tool_calls": [{ "index": 5000, "id": "c-huge",
                "function": {"name": "bash", "arguments": "{}"} }] },
            "finish_reason": "tool_calls"
        }]
    });
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let req = gray_core::message::ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    assert!(
        events
            .iter()
            .any(|r| matches!(r, Ok(StreamEvent::ToolCallDelta { index: 5000, .. }))),
        "index 5000 must reach the agent (agent guard decides): {events:?}"
    );
}

#[tokio::test]
async fn responses_twin_ids_require_confirmed_completion() {
    // Server sends output_item.added with BOTH call_id and item_id (OpenAI's
    // real shape); args deltas land on the item_id twin. The stream then
    // drops before response.completed: no call may execute (#68/#72).
    // Confirmed completion must merge both IDs and keep the full arguments.
    use futures::StreamExt;
    for confirmed in [false, true] {
        let server = wiremock::MockServer::start().await;
        let added = serde_json::json!({
            "type": "response.output_item.added",
            "response_id": "resp_1",
            "item": {"type": "function_call", "call_id": "call_1", "id": "fc_1", "name": "bash"}
        });
        let delta = serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "response_id": "resp_1",
            "item_id": "fc_1",
            "delta": r#"{"command":"ls"}"#
        });
        // Body ends without [DONE] and without response.completed → EOF path.
        let mut body = format!("data: {added}\n\ndata: {delta}\n\n");
        if confirmed {
            body.push_str("data: {\"type\":\"response.completed\"}\n\n");
        }
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let provider = OpenAiProvider::new(
            "key",
            "muse-test",
            format!("{}/opencode.ai/zen", server.uri()),
            None,
            None,
        )
        .expect("provider builds");
        let req = gray_core::message::ChatRequest {
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
        };
        let events: Vec<_> = provider.stream(req).collect().await;
        if confirmed {
            let calls: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Ok(StreamEvent::ToolCallDelta {
                        id,
                        arguments_delta,
                        ..
                    }) => Some((id.as_deref(), arguments_delta.as_str())),
                    _ => None,
                })
                .collect();
            assert_eq!(calls, vec![(Some("call_1"), r#"{"command":"ls"}"#)]);
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, Ok(StreamEvent::MessageComplete { .. })))
                    .count(),
                1
            );
        } else {
            assert!(events.iter().any(|event| matches!(event, Err(e) if e.to_string().contains("before response.completed"))));
            assert!(!events.iter().any(|event| matches!(
                event,
                Ok(StreamEvent::ToolCallDelta { .. } | StreamEvent::MessageComplete { .. })
            )));
        }
    }
}

#[tokio::test]
async fn responses_text_only_eof_without_completed_completes() {
    // A gateway that closes SSE after output text but omits
    // `response.completed` is still a usable text turn. The old EOF path
    // failed the whole turn even though the user-visible text was complete.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let delta = serde_json::json!({
        "type": "response.output_text.delta",
        "response_id": "resp_1",
        "delta": "hello"
    });
    let body = format!(
        "data: {delta}

"
    );
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new(
        "key",
        "muse-test",
        format!("{}/opencode.ai/zen", server.uri()),
        None,
        None,
    )
    .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Ok(StreamEvent::TextDelta { delta }) => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello", "text must be preserved: {events:?}");
    assert!(
        events.iter().all(|event| event.is_ok()),
        "text-only EOF must not error: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                Ok(StreamEvent::MessageComplete {
                    stop_reason: Some(StopReason::EndTurn),
                    ..
                })
            ))
            .count(),
        1,
        "text-only EOF must complete once: {events:?}"
    );
}

#[tokio::test]
async fn responses_failure_event_is_an_error_not_endturn() {
    // response.failed must not fall through the catch-all into a successful
    // EndTurn: the turn must surface the failure (#71).
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let failed = serde_json::json!({
        "type": "response.failed",
        "response": {"id": "resp_1", "error": {"code": "server_error", "message": "kaboom"}}
    });
    let body = format!("data: {failed}\n\n");
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new(
        "key",
        "muse-test",
        format!("{}/opencode.ai/zen", server.uri()),
        None,
        None,
    )
    .expect("provider builds");
    let req = gray_core::message::ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    assert!(
        events
            .iter()
            .any(|r| matches!(r, Err(e) if e.to_string().contains("kaboom"))),
        "response.failed must end in an error, got: {events:?}"
    );
}

#[test]
fn chat_tool_images_follow_all_tool_results() {
    use gray_core::message::Message;
    let request = ChatRequest {
        system: None,
        tools: vec![],
        messages: vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "a".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "b".into(),
                        name: "bash".into(),
                        args: serde_json::json!({}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        id: "a".into(),
                        content: "image".into(),
                        is_error: false,
                    },
                    ContentBlock::image("image/png", "AA=="),
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    id: "b".into(),
                    content: "done".into(),
                    is_error: false,
                }],
            },
        ],
    };
    let mapped = map_chat_request(request, "test", None).unwrap();
    let value = serde_json::to_value(mapped).unwrap();
    let roles: Vec<_> = value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["assistant", "tool", "tool", "user"]);
    assert_eq!(value["messages"][2]["content"], "done");
}

#[tokio::test]
async fn responses_incomplete_and_top_level_error_are_not_success() {
    use futures::StreamExt;
    for (event, max_tokens) in [
        (
            serde_json::json!({"type":"error","message":"top-level boom"}),
            false,
        ),
        (
            serde_json::json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":100,"output_tokens":50,"input_tokens_details":{"cached_tokens":20},"output_tokens_details":{"reasoning_tokens":40}}}}),
            true,
        ),
    ] {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {event}\n\n")),
            )
            .mount(&server)
            .await;
        let provider = OpenAiProvider::new(
            "key",
            "muse",
            format!("{}/opencode.ai/zen", server.uri()),
            None,
            None,
        )
        .unwrap();
        let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
        if max_tokens {
            assert!(events.iter().any(|e| matches!(
                e,
                Ok(StreamEvent::MessageComplete {
                    stop_reason: Some(StopReason::MaxTokens),
                    usage: Some(usage),
                }) if usage.input_tokens == 100 && usage.output_tokens == 50
                    && usage.cached_tokens == 20 && usage.reasoning_tokens == 40
            )));
        } else {
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, Err(e) if e.to_string().contains("top-level boom")))
            );
        }
    }
}

#[tokio::test]
async fn chat_eof_keeps_complete_tool_calls() {
    // Zen closes the stream right after the last delta (no finish chunk): a
    // COMPLETE tool call is still usable, so it survives with a warning.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let chunk = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"command\":\"ls -la\"}"}}]}}]});
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {chunk}\n\n")),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test", server.uri(), None, None).unwrap();
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    assert!(!events.iter().any(Result::is_err), "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::ToolCallDelta { .. })))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Ok(StreamEvent::MessageComplete {
            stop_reason: Some(StopReason::ToolUse),
            ..
        })
    )));
}

#[tokio::test]
async fn chat_eof_never_executes_unconfirmed_tools() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let chunk = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"command\":\"ls\""}}]}}]});
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {chunk}\n\n")),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test", server.uri(), None, None).unwrap();
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    assert!(events.iter().any(Result::is_err));
    assert!(!events.iter().any(|e| matches!(
        e,
        Ok(StreamEvent::ToolCallDelta { .. } | StreamEvent::MessageComplete { .. })
    )));
}

#[tokio::test]
async fn zen_500_retry_hardening_recovers_on_fifth_attempt() {
    // Zen provider-side 500 blip: 500x4 then 200 must recover within the
    // hardened budget (5 attempts) with one reconnect notice + MessageComplete.
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
        .up_to_n_times(4)
        .mount(&server)
        .await;
    let chunk = serde_json::json!({"choices":[{"delta":{"content":"hi"}}]});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        5,
        "500x4 then 200 must take 5 attempts: {events:?}"
    );
    let notices = events
        .iter()
        .filter(|r| matches!(r, Ok(StreamEvent::StreamError { .. })))
        .count();
    assert_eq!(notices, 1, "one reconnect notice per burst: {events:?}");
    assert!(
        events
            .iter()
            .any(|r| matches!(r, Ok(StreamEvent::MessageComplete { .. }))),
        "recovered stream must complete: {events:?}"
    );
}

#[test]
fn zen_500_retry_budget_constants() {
    // Opencode parity: 1s base x5 retries vs old ~150ms over 3 attempts.
    assert_eq!(MAX_ATTEMPTS, 5, "retry budget is 5 attempts");
    assert_eq!(
        INITIAL_BACKOFF,
        Duration::from_secs(1),
        "initial backoff is 1s"
    );
    assert_eq!(MAX_BACKOFF, Duration::from_secs(30), "backoff caps at 30s");
    // Exponential growth must cap at 30s (attempt 20 would be hours uncapped).
    assert!(
        backoff_delay(INITIAL_BACKOFF, 20, None) <= Duration::from_secs(30),
        "backoff caps at 30s"
    );
    // Server-asked delay still wins over the cap.
    assert!(
        backoff_delay(INITIAL_BACKOFF, 1, Some(Duration::from_secs(60))) >= Duration::from_secs(60),
        "Retry-After wins over the cap"
    );
    // 5xx is provider-side: request was valid, keep routing clues.
    let err = classify_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "boom",
        Some("ray-1"),
        Some("req-1"),
    );
    let msg = err.to_string();
    assert!(matches!(err, ProviderError::ServerError(_)), "{msg}");
    assert!(msg.contains("provider-side"), "{msg}");
    assert!(msg.contains("request was valid"), "{msg}");
    assert!(msg.contains("cf-ray: ray-1"), "{msg}");
    assert!(msg.contains("request-id: req-1"), "{msg}");
}

fn cached_turn_req() -> gray_core::message::ChatRequest {
    use gray_core::message::{Message, ToolDef};
    gray_core::message::ChatRequest {
        system: Some("sys".to_string()),
        messages: vec![
            Message::user("first"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::tool_use(
                    "c1",
                    "bash",
                    serde_json::json!({"cmd": "ls"}),
                )],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result("c1", "out", false)],
            },
        ],
        tools: vec![ToolDef::new(
            "bash",
            "run",
            serde_json::json!({"type": "object"}),
        )],
    }
}

#[test]
fn no_model_carries_cache_control() {
    // The Anthropic `cache_control` breakpoint path was removed: prompt-cache
    // affinity rides the Responses `prompt_cache_key` alone, so no model — and
    // specifically not Claude/Anthropic — may emit a `cache_control` marker,
    // and plain string content must stay a string (never promoted to a
    // one-part array).
    for model in ["anthropic/claude-sonnet-4.5", "openai/gpt-5"] {
        let body = map_chat_request(cached_turn_req(), model, None).expect("maps");
        let v = serde_json::to_value(&body).expect("serializes");
        assert!(!v.to_string().contains("cache_control"), "{model}: {v}");
        let msgs = v["messages"].as_array().expect("messages");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(
            msgs[0]["content"], "sys",
            "{model}: plain string content kept"
        );
        assert_eq!(
            v["tools"][0]["function"]["name"], "bash",
            "{model}: tool def intact"
        );
    }
}

#[test]
fn session_affinity_header_is_host_independent() {
    // The per-host `x-session-id` sticky-routing header was removed: the only
    // session header is `x-opencode-session` (required by Console Go), sent for
    // every non-empty session regardless of upstream, and dropped entirely when
    // there is no session.
    let openrouter = Url::parse("https://openrouter.ai/api/v1").expect("url");
    let commandcode = Url::parse("https://api.commandcode.ai/provider/v1").expect("url");
    let other = Url::parse("https://api.deepseek.com/v1").expect("url");
    for url in [&openrouter, &commandcode, &other] {
        assert_eq!(
            session_affinity_headers(Some("s1")),
            vec![("x-opencode-session", "s1")],
            "{}: only x-opencode-session, host-independent",
            url.host_str().unwrap_or("")
        );
    }
    assert!(session_affinity_headers(None).is_empty());
    assert!(session_affinity_headers(Some("")).is_empty());
}

/// Truncated body: the server opens a chunked SSE response and drops the
/// socket before terminating it. reqwest reports that as
/// "error decoding response body", which today is terminal even when
/// nothing has been shown to the user yet.
async fn serve_truncated_then_good() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut served = 0usize;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            // Drain the request head so the client sees a clean reset.
            let mut buf = [0u8; 4096];
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut sock, &mut buf),
            )
            .await;
            served += 1;
            if served == 1 {
                // Head + one chunk, then the socket dies: the chunked body
                // never gets its terminating `0\r\n\r\n`.
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                drop(sock);
                continue;
            }
            // A complete stream: one delta, then [DONE].
            let chunk =
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"recovered\"}}]}\n\n";
            let done = "data: [DONE]\n\n";
            let mut body = String::new();
            for part in [chunk, done] {
                body.push_str(&format!("{:x}\r\n{}\r\n", part.len(), part));
            }
            body.push_str("0\r\n\r\n");
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            if sock
                .write_all(format!("{head}{body}").as_bytes())
                .await
                .is_err()
            {
                return;
            }
            let _ = sock.flush().await;
            drop(sock);
        }
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn a_truncated_body_retries_when_nothing_was_emitted() {
    // The bug: a body that dies before its first delta ended the turn with
    // "agent error: stream broken: Transport error: error decoding response
    // body". Nothing reached the user yet, so the retry is invisible.
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let (base, server) = serve_truncated_then_good().await;
    let provider =
        OpenAiProvider::new("key", "test-model", base, None, None).expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    let text: String = events
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text, "recovered",
        "the retried stream delivered the text: {events:?}"
    );
    assert!(
        events.iter().all(|r| r.is_ok()),
        "a truncated first body must not end the turn: {events:?}"
    );
    server.abort();
}

async fn serve_truncated_after_delta() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = [0u8; 4096];
        let _ =
            tokio::time::timeout(std::time::Duration::from_millis(500), sock.read(&mut buf)).await;
        let event = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let body = format!("{:x}\r\n{event}\r\n", event.len());
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
        let _ = sock.write_all(format!("{head}{body}").as_bytes()).await;
        let _ = sock.flush().await;
        drop(sock);
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn a_truncated_body_after_delta_completes_with_a_notice() {
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let (base, server) = serve_truncated_after_delta().await;
    let provider =
        OpenAiProvider::new("key", "test-model", base, None, None).expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    let text: String = events
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "partial", "{events:?}");
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(
        events.iter().any(|event| matches!(
            event.as_ref().ok(),
            Some(StreamEvent::StreamError { message, .. })
                if message == "Connection interrupted; partial answer kept"
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.as_ref().ok(),
            Some(StreamEvent::MessageComplete {
                stop_reason: Some(StopReason::EndTurn),
                ..
            })
        )),
        "{events:?}"
    );
    server.abort();
}

struct StaticProviderSource {
    secrets: gray_core::credential::SecretMap,
    metadata: std::collections::BTreeMap<String, String>,
}

impl Default for StaticProviderSource {
    fn default() -> Self {
        Self {
            secrets: gray_core::credential::SecretMap::from_iter([("access_token", "test-access")]),
            metadata: std::collections::BTreeMap::from([(
                "account_id".to_string(),
                "acct_test".to_string(),
            )]),
        }
    }
}

#[async_trait]
impl gray_core::credential::CredentialSource for StaticProviderSource {
    async fn acquire(
        &self,
    ) -> Result<gray_core::credential::CredentialLease, gray_core::credential::CredentialError>
    {
        Ok(gray_core::credential::CredentialLease {
            secrets: self.secrets.clone(),
            metadata: self.metadata.clone(),
        })
    }
}

#[tokio::test]
async fn dynamic_codex_profile_sends_declared_transport() {
    use futures::StreamExt;
    use gray_core::message::{ChatRequest, ContentBlock, Message, Role};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap();
        let received = String::from_utf8_lossy(&buffer[..read]).to_string();
        let head_end = received.find("\r\n\r\n").expect("request head");
        let head = &received[..head_end];
        let body = &received[head_end + 4..];
        assert!(head.contains("authorization: Bearer test-access"), "{head}");
        assert!(head.contains("chatgpt-account-id: acct_test"), "{head}");
        assert!(head.contains("session-id: session-test"), "{head}");
        assert!(head.contains("originator: gray"), "{head}");
        assert!(head.contains("application/json"), "{head}");
        let body: serde_json::Value = serde_json::from_str(body).expect("request body");
        assert!(body.get("prompt_cache_key").is_none(), "{body}");
        assert_eq!(body["store"], false, "{body}");
        assert_eq!(body["tool_choice"], "auto", "{body}");
        assert_eq!(body["parallel_tool_calls"], true, "{body}");
        assert_eq!(
            body["include"],
            serde_json::json!(["reasoning.encrypted_content"]),
            "{body}"
        );
        assert_eq!(body["text"]["verbosity"], "low", "{body}");
        assert!(body.get("previous_response_id").is_none(), "{body}");

        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
            "data: [DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            sse.len(),
            sse
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });

    let profile = OpenAiProviderProfile {
        base_url: format!("http://{addr}/v1").parse().unwrap(),
        wire: OpenAiWire::Responses,
        authorization: OpenAiAuthorization::Bearer {
            secret_name: "access_token".into(),
        },
        headers: vec![
            OpenAiHeader {
                name: "chatgpt-account-id".into(),
                source: OpenAiHeaderSource::Metadata("account_id".into()),
                required: true,
            },
            OpenAiHeader {
                name: "session-id".into(),
                source: OpenAiHeaderSource::SessionId,
                required: true,
            },
            OpenAiHeader {
                name: "originator".into(),
                source: OpenAiHeaderSource::Static("gray".into()),
                required: false,
            },
        ],
        request: OpenAiRequestPolicy {
            prompt_cache_key: false,
            store: false,
            include_reasoning_encrypted: true,
            previous_response_id: false,
            tool_choice: Some("auto".into()),
            parallel_tool_calls: Some(true),
            text_verbosity: Some("low".into()),
        },
        follow_redirects: false,
    };
    let provider = OpenAiProvider::new_with_profile(
        "gpt-test",
        Some("low".into()),
        Some("session-test".into()),
        profile,
        std::sync::Arc::new(StaticProviderSource {
            secrets: gray_core::credential::SecretMap::from_iter([("access_token", "test-access")]),
            metadata: [("account_id".into(), "acct_test".into())]
                .into_iter()
                .collect(),
        }),
    )
    .unwrap();

    let request = ChatRequest {
        system: None,
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )],
        tools: Vec::new(),
    };
    let mut stream = provider.stream(request);
    match stream.next().await.unwrap() {
        Ok(StreamEvent::TextDelta { delta }) => assert_eq!(delta, "hello"),
        other => panic!("expected text delta, got {other:?}"),
    }
    match stream.next().await.unwrap() {
        Ok(StreamEvent::MessageComplete { stop_reason, usage }) => {
            assert_eq!(stop_reason, Some(StopReason::EndTurn));
            assert_eq!(usage.unwrap().output_tokens, 2);
        }
        other => panic!("expected complete event, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
    server.await.unwrap();
}

#[test]
fn dynamic_profile_debug_is_redacted() {
    let profile = OpenAiProviderProfile {
        base_url: "https://example.test/v1".parse().unwrap(),
        wire: OpenAiWire::Responses,
        authorization: OpenAiAuthorization::Bearer {
            secret_name: "access_token".into(),
        },
        headers: Vec::new(),
        request: OpenAiRequestPolicy::default(),
        follow_redirects: false,
    };
    let provider = OpenAiProvider::new_with_profile(
        "gpt-test",
        None,
        None,
        profile,
        std::sync::Arc::new(StaticProviderSource::default()),
    )
    .unwrap();
    assert!(!format!("{provider:?}").contains("test-access"));
    assert!(!format!("{provider:?}").contains("acct_test"));
}

#[test]
fn model_accepts_video_is_gemini_only() {
    use crate::openai::model_accepts_video;
    assert!(model_accepts_video("gemini-3-pro"));
    assert!(model_accepts_video("google/gemini-2.5-flash"));
    assert!(model_accepts_video("gemma-3-27b"));
    // Everything else gets the contact sheet instead of a 400.
    for m in [
        "gpt-5.1",
        "claude-opus-5",
        "deepseek-v4",
        "step-5-preview",
        "gpt-oss-120b",
    ] {
        assert!(!model_accepts_video(m), "{m} must not claim video");
    }
}

#[test]
fn a_video_block_reaches_the_wire_only_for_a_video_model() {
    use crate::openai::model_accepts_video;
    let req = || ChatRequest {
        system: None,
        messages: vec![Message::new(
            gray_core::Role::User,
            vec![ContentBlock::video("video/mp4", "QUJD")],
        )],
        tools: Vec::new(),
    };

    // Gemini: a real video_url part carrying the data URL.
    let chat = map_chat_request(req(), "gemini-3-pro", None).expect("maps");
    let body = serde_json::to_value(&chat).unwrap();
    let parts = body["messages"][0]["content"].as_array().unwrap();
    let video = parts
        .iter()
        .find(|p| p["type"] == "video_url")
        .expect("video part must be present");
    let url = video["video_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:video/mp4;base64,"), "{url}");

    // A model without native video refuses loudly, and names the fallback.
    let err = map_chat_request(req(), "step-5-preview", None).expect_err("must refuse");
    let text = err.to_string();
    assert!(text.contains("gray view"), "{text}");

    // Same rule on the Responses path: a note, not a stray part.
    let responses = map_chat_to_responses(req(), "gpt-5.1", None, None);
    let body = serde_json::to_value(&responses).unwrap();
    let text = body["input"][0]["content"].as_str().unwrap_or_default();
    assert!(text.contains("no native video input"), "{text}");
    let _ = model_accepts_video("gemini-3-pro");
}
