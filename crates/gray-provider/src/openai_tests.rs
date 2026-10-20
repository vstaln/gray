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
        max_tokens: None,
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
    assert!(matches!(err.class(), ProviderError::BadRequest(_)));
    assert!(!is_retryable_error(&err));
    let msg = err.to_string();
    assert!(msg.contains("cf-ray: abc123-ray"), "{msg}");
    assert!(msg.contains("request-id: req-1"), "{msg}");
}

#[test]
fn unsupported_model_401_body_outranks_auth_status() {
    let err = classify_http_error(
        reqwest::StatusCode::UNAUTHORIZED,
        r#"{"type":"error","error":{"type":"ModelError","message":"Model deepseek/deepseek-chat is not supported"}}"#,
        None,
        None,
    );
    assert!(
        matches!(err.class(), ProviderError::BadRequest(_)),
        "must classify as bad request, got {err:?}"
    );
    assert!(!is_retryable_error(&err));
}

#[test]
fn auth_401_insufficient_balance_is_not_retryable() {
    let err = classify_http_error(
        reqwest::StatusCode::UNAUTHORIZED,
        "insufficient balance or invalid api key",
        None,
        None,
    );
    assert!(matches!(err.class(), ProviderError::Auth(_)));
    assert!(!is_retryable_error(&err));
}

#[test]
fn cloudflare_edge_403_is_retryable_not_auth() {
    let err = classify_http_error(
        reqwest::StatusCode::FORBIDDEN,
        "error code: 1010",
        Some("9abc-SIN"),
        None,
    );
    assert!(matches!(err.class(), ProviderError::ServerError(_)));
    assert!(is_retryable_error(&err));
}

#[test]
fn real_api_403_stays_auth() {
    let err = classify_http_error(
        reqwest::StatusCode::FORBIDDEN,
        r#"{"error":{"message":"invalid api key"}}"#,
        None,
        None,
    );
    assert!(matches!(err.class(), ProviderError::Auth(_)));
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
    assert!(matches!(err.class(), ProviderError::ServerError(_)));
    assert!(is_retryable_error(&err));
}

#[test]
fn retry_notice_uses_codex_reconnecting_format() {
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
        max_tokens: None,
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
        max_tokens: None,
    };
    let _events: Vec<_> = provider.stream(req).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        MAX_ATTEMPTS,
        "default retry burst POSTs once per attempt"
    );
    for r in &received {
        assert!(
            r.headers.get("x-opencode-session").is_none(),
            "session header must not leak to non-zen endpoints"
        );
    }
}

fn responses_req_with_thinking(_model: &str) -> ChatRequest {
    use gray_core::message::Message;
    ChatRequest {
        system: Some("sys".to_string()),
        messages: vec![Message {
            role: Role::Assistant,
            injected: false,
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
        max_tokens: None,
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
            matches!(err.class(), ProviderError::ContextOverflow(_)),
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
        assert!(
            matches!(err.class(), ProviderError::BadRequest(_)),
            "body: {body}"
        );
        assert!(!is_retryable_error(&err), "must not retry: {body}");
        assert!(!err.should_compress());
    }

    for body in ["content-filter triggered", "content_filter triggered"] {
        let err = classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
        assert!(
            matches!(err.class(), ProviderError::BadRequest(_)),
            "filter must win over 500: {body} -> {err}"
        );
    }

    for body in [
        "request violates policy",
        "response was flagged",
        "safety check failed",
    ] {
        let err = classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
        assert!(
            matches!(err.class(), ProviderError::ServerError(_)),
            "bare hint must not filter: {body} -> {err}"
        );
    }

    {
        let err = classify_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            "max_tokens must be positive",
            None,
            None,
        );
        assert!(
            !matches!(err.class(), ProviderError::ContextOverflow(_)),
            "bare max_tokens: {err}"
        );
    }

    for body in [
        "max_tokens exceeded: context too long",
        "max_tokens context exceeded",
    ] {
        let err = classify_http_error(reqwest::StatusCode::BAD_REQUEST, body, None, None);
        assert!(
            matches!(err.class(), ProviderError::ContextOverflow(_)),
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
        max_tokens: None,
    }
}

#[tokio::test]
async fn sampling_params_are_sent_when_configured() {
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

    assert_eq!(
        body.get("top_p").and_then(|v| v.as_f64()),
        Some(f64::from(0.95f32)),
        "top_p sent: {body}"
    );
}

#[tokio::test]
async fn sampling_params_are_omitted_by_default() {
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
fn a_capped_chat_request_leaves_out_a_budget_it_cannot_fit() {
    let req = gray_core::message::ChatRequest {
        max_tokens: Some(1),
        ..empty_chat_req()
    };
    let v =
        serde_json::to_value(map_chat_request(req, "zai/glm-5.2", Some("low")).unwrap()).unwrap();
    assert_eq!(v["reasoning_effort"], "low", "{v}");
    assert_eq!(v["max_tokens"], 1, "{v}");
    assert!(v.get("thinking").is_none(), "{v}");
}

#[test]
fn only_openai_names_the_cap_max_completion_tokens() {
    let url = |s: &str| Url::parse(s).unwrap();
    assert!(names_cap_max_completion_tokens(&url(
        "https://api.openai.com/v1"
    )));
    assert!(!names_cap_max_completion_tokens(&url(
        "https://openrouter.ai/api/v1"
    )));
    assert!(!names_cap_max_completion_tokens(&url(
        "http://localhost:11434/v1"
    )));
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
    assert!(msg.contains("/model"), "actionable hint: {msg}");
}

#[test]
fn chat_mapping_user_and_system_roles_map_without_panic() {
    let req = gray_core::message::ChatRequest {
        system: None,
        messages: vec![
            gray_core::message::Message::user("hi"),
            gray_core::message::Message::system("be nice"),
        ],
        tools: Vec::new(),
        max_tokens: None,
    };
    let body = map_chat_request(req, "test-model", None).expect("user/system roles map");
    let roles: Vec<&str> = body.messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "system"]);
}

#[test]
fn serialize_body_maps_failure_to_bad_request() {
    let bad: std::collections::BTreeMap<(), u8> = [((), 1)].into_iter().collect();
    let err = serialize_body(&bad, "test body").expect_err("unit keys must fail");
    assert!(
        matches!(err.class(), ProviderError::BadRequest(_)),
        "got {err:?}"
    );
}

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
            matches!(err.class(), ProviderError::Auth(_)),
            "quota 429 must surface as terminal Auth: {body} -> {err}"
        );
        assert!(!is_retryable_error(&err), "must not fast-retry: {body}");
        assert!(
            !err.should_compress(),
            "compaction cannot fix billing: {body}"
        );
    }
}

#[test]
fn plain_429_without_quota_hints_stays_retryable() {
    for body in [
        "Rate limit reached for model",
        "rate_limit_exceeded: slow down",
        "Rate limit exceeded. Upgrade your billing plan for higher limits.",
    ] {
        let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, None, None);
        assert!(
            matches!(err.class(), ProviderError::RateLimited(_)),
            "plain 429 stays retryable: {body} -> {err}"
        );
        assert!(is_retryable_error(&err), "must retry: {body}");
    }
}

#[test]
fn http_413_maps_to_context_overflow_never_retried() {
    let err = classify_http_error(
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "Request Entity Too Large",
        None,
        None,
    );
    assert!(
        matches!(err.class(), ProviderError::ContextOverflow(_)),
        "413 must compact, not retry: {err}"
    );
    assert!(!is_retryable_error(&err), "never generic-retry: {err}");
    assert!(err.should_compress(), "must flag compression: {err}");
}

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

    headers.insert(
        reqwest::header::RETRY_AFTER,
        "Sun, 06 Nov 2033 08:49:37 GMT".parse().unwrap(),
    );
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(60)),
        "HTTP-date capped at 60s"
    );

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

    headers.insert(reqwest::header::RETRY_AFTER, "45".parse().unwrap());
    headers.insert("retry-after-ms", "2000".parse().unwrap());
    assert_eq!(
        parse_retry_after(&headers),
        Some(Duration::from_secs(2)),
        "ms takes precedence"
    );
}

#[test]
fn long_body_quota_signal_survives_snippet_cap() {
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
        matches!(err.class(), ProviderError::Auth(_)),
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
        matches!(err.class(), ProviderError::RateLimited(_)),
        "long != quota: {err}"
    );
}

#[tokio::test]
async fn quota_429_body_past_500_chars_terminal_end_to_end() {
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
        max_tokens: None,
    };
    let events: Vec<_> = provider.stream(req).collect().await;
    assert_eq!(events.len(), 1, "immediate terminal, no notice: {events:?}");
    assert!(
        matches!(events[0], Err(ref e) if matches!(e.class(), ProviderError::Auth(_))),
        "terminal Auth: {events:?}"
    );
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "no retry POST burns quota: {received:?}");
}

#[tokio::test]
async fn dropped_stream_fires_no_retry_post_after_backoff() {
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
        max_tokens: None,
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

#[tokio::test]
async fn chat_post_carries_prompt_cache_key_body() {
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
        max_tokens: None,
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

#[test]
fn chat_affinity_omitted_when_unset() {
    let body = map_chat_request(empty_chat_req(), "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        v.get("prompt_cache_key").is_none(),
        "absent by default: {v}"
    );
}

#[test]
fn unknown_resume_id_retries_stripped_once() {
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

    let mut resumed = map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high"));
    resumed.previous_response_id = Some("resp_stale".to_string());
    resumed.previous_response_id = None;
    let v = serde_json::to_value(&resumed).expect("serializes");
    assert!(v.get("previous_response_id").is_none(), "stripped: {v}");
}

#[test]
fn assistant_messages_carry_reasoning_content_only_when_thinking() {
    use gray_core::message::Message;
    let plain_assistant = || ChatRequest {
        system: None,
        messages: vec![Message::assistant("hi")],
        tools: Vec::new(),
        max_tokens: None,
    };
    let last_msg = |v: &serde_json::Value| {
        v.get("messages")
            .and_then(|m| m.as_array())
            .and_then(|a| a.last().cloned())
            .expect("assistant msg")
    };
    let body = map_chat_request(plain_assistant(), "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        last_msg(&v).get("reasoning_content").is_none(),
        "non-deepseek still omits: {v}"
    );

    let thinking_req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            injected: false,
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
        max_tokens: None,
    };
    let body = map_chat_request(thinking_req.clone(), "deepseek-v4", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert_eq!(
        last_msg(&v)
            .get("reasoning_content")
            .and_then(|r| r.as_str()),
        Some("hmm"),
        "deepseek chain preserved: {v}"
    );

    let body = map_chat_request(thinking_req, "test-model", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(
        last_msg(&v).get("reasoning_content").is_none(),
        "non-deepseek drops thinking: {v}"
    );
}

#[test]
fn tool_error_flag_survives_chat_wire_encoding() {
    use gray_core::message::Message;
    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            injected: false,
            content: vec![
                ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "sh".to_string(),
                    args: serde_json::json!({}),
                    thought_signature: None,
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
        max_tokens: None,
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

    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::User,
            injected: false,
            content: vec![ContentBlock::ToolResult {
                id: "u1".to_string(),
                content: "denied".to_string(),
                is_error: true,
            }],
        }],
        tools: Vec::new(),
        max_tokens: None,
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

#[test]
fn tool_error_flag_survives_responses_wire_encoding() {
    use gray_core::message::Message;
    let req = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            injected: false,
            content: vec![
                ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "sh".to_string(),
                    args: serde_json::json!({}),
                    thought_signature: None,
                },
                ContentBlock::ToolResult {
                    id: "c1".to_string(),
                    content: "boom".to_string(),
                    is_error: true,
                },
            ],
        }],
        tools: Vec::new(),
        max_tokens: None,
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
        max_tokens: None,
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
            max_tokens: None,
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
async fn responses_text_parts_get_a_blank_line_between_them() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let ev = |output_index, content_index, delta| {
        serde_json::json!({
            "type": "response.output_text.delta",
            "response_id": "resp_1",
            "output_index": output_index,
            "content_index": content_index,
            "delta": delta,
        })
    };
    let mut body = String::new();
    for e in [
        ev(0, 0, "one"),
        ev(0, 0, " two"),
        ev(0, 1, "three"),
        ev(1, 0, " four"),
    ] {
        body.push_str(&format!("data: {e}\n\n"));
    }
    body.push_str("data: {\"type\":\"response.completed\"}\n\n");
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
            Ok(StreamEvent::TextDelta { delta }) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "one two\n\nthree\n\n four", "{events:?}");
}

#[tokio::test]
async fn responses_failure_event_is_an_error_not_endturn() {
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
        max_tokens: None,
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
                injected: false,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "a".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                        thought_signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "b".into(),
                        name: "bash".into(),
                        args: serde_json::json!({}),
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                injected: false,
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
                injected: false,
                content: vec![ContentBlock::ToolResult {
                    id: "b".into(),
                    content: "done".into(),
                    is_error: false,
                }],
            },
        ],
        max_tokens: None,
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
async fn empty_tool_call_padding_deltas_are_dropped() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let real = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}}]});
    let ghost = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":1},{"index":2,"function":{"name":"","arguments":""}}]}}]});
    let finish = serde_json::json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]});
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!(
                    "data: {real}\n\ndata: {ghost}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
                )),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test", server.uri(), None, None).unwrap();
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    assert!(!events.iter().any(Result::is_err), "{events:?}");
    let tool_deltas: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Ok(StreamEvent::ToolCallDelta { index, name, .. }) => Some((*index, name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_deltas,
        vec![(0, Some("bash".to_string()))],
        "exactly the real call survives: {tool_deltas:?}"
    );
}

#[tokio::test]
async fn chat_eof_keeps_complete_tool_calls() {
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
    assert_eq!(MAX_ATTEMPTS, 5, "retry budget is 5 attempts");
    assert_eq!(
        INITIAL_BACKOFF,
        Duration::from_secs(1),
        "initial backoff is 1s"
    );
    assert_eq!(MAX_BACKOFF, Duration::from_secs(30), "backoff caps at 30s");

    assert!(
        backoff_delay(INITIAL_BACKOFF, 20, None) <= Duration::from_secs(30),
        "backoff caps at 30s"
    );

    assert!(
        backoff_delay(INITIAL_BACKOFF, 1, Some(Duration::from_secs(60))) >= Duration::from_secs(60),
        "Retry-After wins over the cap"
    );

    let err = classify_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "boom",
        Some("ray-1"),
        Some("req-1"),
    );
    let msg = err.to_string();
    assert!(
        matches!(err.class(), ProviderError::ServerError(_)),
        "{msg}"
    );
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
                injected: false,
                content: vec![ContentBlock::tool_use(
                    "c1",
                    "bash",
                    serde_json::json!({"cmd": "ls"}),
                )],
            },
            Message {
                role: Role::User,
                injected: false,
                content: vec![ContentBlock::tool_result("c1", "out", false)],
            },
        ],
        tools: vec![ToolDef::new(
            "bash",
            "run",
            serde_json::json!({"type": "object"}),
        )],
        max_tokens: None,
    }
}

#[test]
fn chat_wire_carries_no_cache_control() {
    let body = map_chat_request(cached_turn_req(), "openai/gpt-5", None).expect("maps");
    let v = serde_json::to_value(&body).expect("serializes");
    assert!(!v.to_string().contains("cache_control"), "{v}");
    assert_eq!(
        v["messages"][0]["content"], "sys",
        "plain string content kept"
    );
    assert_eq!(v["tools"][0]["function"]["name"], "bash", "tool def intact");
}

#[test]
fn session_affinity_header_is_host_independent() {
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

            let mut buf = [0u8; 4096];
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut sock, &mut buf),
            )
            .await;
            served += 1;
            if served == 1 {
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                drop(sock);
                continue;
            }

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
    use futures::StreamExt;
    use gray_core::message::ChatRequest;
    let (base, server) = serve_truncated_then_good().await;
    let provider =
        OpenAiProvider::new("key", "test-model", base, None, None).expect("provider builds");
    let req = ChatRequest {
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: None,
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
        max_tokens: None,
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

async fn serve_empty_then_good() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut served = 0usize;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut sock, &mut buf),
            )
            .await;
            served += 1;
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";

            let mut body = String::new();
            if served > 1 {
                for part in [
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"recovered\"}}]}\n\n",
                    "data: [DONE]\n\n",
                ] {
                    body.push_str(&format!("{:x}\r\n{part}\r\n", part.len()));
                }
            }
            body.push_str("0\r\n\r\n");
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
async fn eof_without_finish_reason_retries_when_nothing_was_emitted() {
    use futures::StreamExt;
    let (base, server) = serve_empty_then_good().await;
    let provider =
        OpenAiProvider::new("key", "test-model", base, None, None).expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
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
        events.iter().all(Result::is_ok),
        "a finish-less EOF must not end the turn on the first attempt: {events:?}"
    );
    let notices = events
        .iter()
        .filter(|r| matches!(r, Ok(StreamEvent::StreamError { .. })))
        .count();
    assert_eq!(notices, 1, "one reconnect notice per burst: {events:?}");
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
async fn dynamic_profile_sends_declared_transport() {
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
        assert!(head.contains("x-example-account: acct_test"), "{head}");
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
                name: "x-example-account".into(),
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
        max_tokens: None,
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

#[tokio::test]
async fn relay_url_override_posts_to_lease_url() {
    use futures::StreamExt;
    use gray_core::message::{ChatRequest, ContentBlock, Message, Role};

    let declared = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let declared_addr = declared.local_addr().unwrap();
    let declared_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let declared_hits2 = declared_hits.clone();
    let declared_server = tokio::spawn(async move {
        if tokio::time::timeout(std::time::Duration::from_secs(5), declared.accept())
            .await
            .is_ok()
        {
            declared_hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });

    let relay = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay.local_addr().unwrap();
    let relay_server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = relay.accept().await.unwrap();
        let mut buffer = [0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap();
        let received = String::from_utf8_lossy(&buffer[..read]).to_string();
        let head_end = received.find("\r\n\r\n").expect("request head");
        let head = &received[..head_end];
        assert!(head.contains("POST /relay/tok/responses"), "{head}");
        assert!(head.contains("Bearer per-turn-bearer"), "{head}");
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi-relay\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
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
        base_url: format!("http://{declared_addr}/v1").parse().unwrap(),
        wire: OpenAiWire::Responses,
        authorization: OpenAiAuthorization::Bearer {
            secret_name: "relay_token".into(),
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
        std::sync::Arc::new(StaticProviderSource {
            secrets: gray_core::credential::SecretMap::from_iter([(
                "relay_token",
                "per-turn-bearer",
            )]),
            metadata: [(
                "relay_url".into(),
                format!("http://{relay_addr}/relay/tok/responses"),
            )]
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
        max_tokens: None,
    };
    let mut stream = provider.stream(request);
    match stream.next().await.unwrap() {
        Ok(StreamEvent::TextDelta { delta }) => assert_eq!(delta, "hi-relay"),
        other => panic!("expected relay text delta, got {other:?}"),
    }
    declared_server.abort();
    relay_server.abort();
    assert_eq!(
        declared_hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "declared base must see no POST when the lease carries a relay URL"
    );
}

#[test]
fn admission_consumed_400_is_terminal_bad_request() {
    let err = classify_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"ANTIGRAVITY_MODEL_ADMISSION_CONSUMED"}}"#,
        None,
        None,
    );
    assert!(matches!(err.class(), ProviderError::BadRequest(_)));
    assert!(!is_retryable_error(&err));
}

#[test]
fn media_goes_native_where_the_wire_takes_it_else_its_fallback() {
    let req = |media: ContentBlock| ChatRequest {
        system: None,
        messages: vec![Message::new(gray_core::Role::User, vec![media])],
        tools: Vec::new(),
        max_tokens: None,
    };
    let video = || {
        ContentBlock::media(
            "video/mp4",
            "QUJD",
            vec![
                ContentBlock::text("(contact sheet of clip.mp4)"),
                ContentBlock::image("image/jpeg", "U0hFRVQ="),
            ],
        )
    };
    let pdf = || {
        ContentBlock::media(
            "application/pdf",
            "UERG",
            vec![ContentBlock::text("--- doc.pdf (PDF text) ---\nhello")],
        )
    };
    let audio = || ContentBlock::media("audio/wav", "V0FW", vec![ContentBlock::text("(no audio)")]);
    let chat = |b, model| {
        serde_json::to_value(map_chat_request(req(b), model, None).expect("maps")).unwrap()
            ["messages"][0]["content"]
            .clone()
    };

    let parts = chat(video(), "gemini-3-pro");
    assert!(
        parts[0]["video_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:video/mp4;base64,")
    );
    let parts = chat(audio(), "gemini-3-pro");
    assert_eq!(
        parts[0]["input_audio"],
        serde_json::json!({"data": "V0FW", "format": "wav"})
    );

    let parts = chat(video(), "step-5-preview");
    assert!(
        parts[0]["video_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:video/mp4;base64,")
    );
    let parts = chat(audio(), "gpt-5.1");
    assert_eq!(
        parts[0]["input_audio"],
        serde_json::json!({"data": "V0FW", "format": "wav"})
    );

    assert!(
        chat(pdf(), "gemini-3-pro")
            .as_str()
            .unwrap()
            .contains("hello")
    );

    let body =
        serde_json::to_value(map_chat_to_responses(req(pdf()), "gpt-5.1", None, None)).unwrap();
    let part = &body["input"][0]["content"][0];
    assert_eq!(part["type"], "input_file");
    assert!(
        part["file_data"]
            .as_str()
            .unwrap()
            .starts_with("data:application/pdf;base64,")
    );
    let body =
        serde_json::to_value(map_chat_to_responses(req(video()), "gpt-5.1", None, None)).unwrap();
    assert_eq!(body["input"][0]["content"][0]["type"], "input_video");
}

#[test]
fn chat_tool_result_fallback_text_follows_all_tool_results() {
    let req = ChatRequest {
        system: None,
        messages: vec![
            Message::new(
                gray_core::Role::Assistant,
                vec![
                    ContentBlock::tool_use("a", "bash", serde_json::json!({})),
                    ContentBlock::tool_use("b", "bash", serde_json::json!({})),
                ],
            ),
            Message::new(
                gray_core::Role::User,
                vec![
                    ContentBlock::tool_result("a", "clip.mp4 (video)", false),
                    ContentBlock::media(
                        "video/mp4",
                        "QUJD",
                        vec![ContentBlock::text("(contact sheet)")],
                    ),
                ],
            ),
            Message::new(
                gray_core::Role::User,
                vec![ContentBlock::tool_result("b", "ok", false)],
            ),
        ],
        tools: Vec::new(),
        max_tokens: None,
    };
    let body = serde_json::to_value(map_chat_request(req, "gpt-5.1", None).unwrap()).unwrap();
    let roles: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(roles, ["assistant", "tool", "tool", "user"]);
}

#[test]
fn http_errors_show_the_providers_sentence_not_its_json() {
    let body = r#"{"error": {"message": "Rate limit reached for requests per min (RPM). Please try again in 1s.", "type": "requests", "code": "rate_limit_exceeded"}}"#;
    let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, None, None);
    let ProviderError::RateLimited(msg) = err.class() else {
        panic!("{err:?}")
    };
    assert_eq!(
        msg,
        "status 429 Too Many Requests: Rate limit reached for requests per min (RPM). Please try again in 1s."
    );

    let quota = r#"{"error": {"message": "slow down", "code": "insufficient_quota"}}"#;
    assert!(matches!(
        classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, quota, None, None).class(),
        ProviderError::Auth(_)
    ));

    let plain = classify_http_error(
        reqwest::StatusCode::BAD_GATEWAY,
        "upstream down",
        None,
        None,
    );
    assert!(plain.to_string().contains("upstream down"), "{plain}");
}

#[test]
fn should_retry_request_allows_one_bare_403_retry() {
    let auth = || ProviderError::Auth("status 403: Authentication failed".to_string());
    assert!(should_retry_request(&auth(), 403, 1));
    assert!(!should_retry_request(&auth(), 403, 2));
    assert!(!should_retry_request(&auth(), 401, 1));

    let quota = ProviderError::Auth("status 403: insufficient quota".to_string());
    assert!(!should_retry_request(&quota, 403, 1));

    let server_err = ProviderError::ServerError("status 500: boom".to_string());
    assert!(should_retry_request(&server_err, 500, 3));
}

#[tokio::test]
async fn transient_403_retries_once_then_surfaces_auth() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(403)
                .set_body_string("Authentication failed. Please check your credentials."),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        received.len(),
        2,
        "one retry then terminal, got: {events:?}"
    );
    let notices = events
        .iter()
        .filter(|r| matches!(r, Ok(StreamEvent::StreamError { .. })))
        .count();
    assert_eq!(notices, 1, "one reconnect notice per burst: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(e)) if matches!(e.class(), ProviderError::Auth(_))),
        "burst ends with terminal Auth: {events:?}"
    );
}

#[tokio::test]
async fn auth_401_is_single_attempt() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(401)
                .set_body_string("Authentication failed. Please check your credentials."),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "401 must not retry: {events:?}");
    let notices = events
        .iter()
        .filter(|r| matches!(r, Ok(StreamEvent::StreamError { .. })))
        .count();
    assert_eq!(notices, 0, "no reconnect notice for 401: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(e)) if matches!(e.class(), ProviderError::Auth(_))),
        "ends with terminal Auth: {events:?}"
    );
}

#[tokio::test]
async fn quota_403_is_single_attempt() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(403).set_body_string("insufficient quota"))
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("key", "test-model", server.uri(), None, None)
        .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "quota 403 must not retry: {events:?}");
}

fn plan_usage_provider(base_url: String) -> OpenAiProvider {
    let profile = OpenAiProviderProfile {
        base_url: format!("{base_url}/v1").parse().unwrap(),
        wire: OpenAiWire::Responses,
        authorization: OpenAiAuthorization::Bearer {
            secret_name: "access_token".into(),
        },
        headers: Vec::new(),
        request: OpenAiRequestPolicy {
            prompt_cache_key: false,
            store: false,
            include_reasoning_encrypted: true,
            previous_response_id: false,
            tool_choice: None,
            parallel_tool_calls: None,
            text_verbosity: None,
        },
        follow_redirects: false,
    };
    OpenAiProvider::new_with_profile(
        "gpt-test",
        None,
        Some("session-test".into()),
        profile,
        std::sync::Arc::new(StaticProviderSource {
            secrets: gray_core::credential::SecretMap::from_iter([("access_token", "test-access")]),
            metadata: Default::default(),
        }),
    )
    .unwrap()
}

#[tokio::test]
async fn plan_usage_limit_429_is_single_attempt() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let body = r#"{"error": {"message": "You have reached your usage limit for this app.", "type": "usage_limit", "code": "subscription_sharing_usage_limit_exceeded"}}"#;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(429).set_body_string(body))
        .mount(&server)
        .await;
    let provider = plan_usage_provider(server.uri());
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "usage limit must not retry: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(e)) if matches!(e.class(), ProviderError::Auth(_))),
        "ends with terminal Auth: {events:?}"
    );
}

#[tokio::test]
async fn plan_usage_limit_mid_stream_is_single_attempt() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let failed = serde_json::json!({
        "type": "response.failed",
        "response": {"id": "resp_1", "error": {
            "code": "subscription_sharing_usage_limit_exceeded",
            "message": "You have reached your usage limit for this app."
        }}
    });
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {failed}\n\n")),
        )
        .mount(&server)
        .await;
    let provider = plan_usage_provider(server.uri());
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "usage limit must not retry: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(e)) if matches!(e.class(), ProviderError::Auth(_))),
        "ends with terminal Auth: {events:?}"
    );
}

#[tokio::test]
async fn relay_quota_429_in_server_error_code_is_terminal() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let failed = serde_json::json!({
        "type": "response.failed",
        "response": {"id": "resp_1", "error": {
            "code": "server_error",
            "message": "native API error 429: Claude usage limit reached (five_hour) · resets in ~227m"
        }}
    });
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {failed}\n\n")),
        )
        .mount(&server)
        .await;
    let provider = plan_usage_provider(server.uri());
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 1, "quota window must not retry: {events:?}");
    assert!(
        matches!(events.last(), Some(Err(e)) if matches!(e.class(), ProviderError::Auth(_))),
        "ends with terminal Auth: {events:?}"
    );
}

#[tokio::test]
async fn responses_transient_403_then_success_delivers_text() {
    use futures::StreamExt;
    let server = wiremock::MockServer::start().await;
    let delta = serde_json::json!({
        "type": "response.output_text.delta",
        "response_id": "resp_1",
        "delta": "hello"
    });
    let body = format!("data: {delta}\n\n");

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(403)
                .set_body_string("Authentication failed. Please check your credentials."),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
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
    let received = server.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 2, "one retry then success: {events:?}");
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Ok(StreamEvent::TextDelta { delta }) => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello", "text delivered after retry: {events:?}");
    assert!(
        events.iter().all(|event| event.is_ok()),
        "transient 403 must not surface an error: {events:?}"
    );
}

static STALL_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn quota_hint_bodies_classify_as_terminal_quota() {
    for body in [
        r#"{"error":{"message":"You exceeded your current quota, please check your plan","code":"insufficient_quota"}}"#,
        "insufficient quota for this request",
        "insufficient credits",
        "insufficient balance",
        "you exceeded your current quota",
        "usage exceeds your allocated quota",
        "quota exceeded",
        "quota_exceeded",
        "GoUsageLimitError: free tier exhausted",
        "available balance is empty",
        "usage limit reached",
        "session limit hit",
        "subscription_sharing_usage_limit_exceeded",
        "usage limit reached, resets in ~2h",
    ] {
        let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, None, None);
        assert!(
            matches!(err.class(), ProviderError::Auth(_)),
            "{body}: {err:?}"
        );
        assert!(!is_retryable_error(&err), "{body}");
        assert!(!should_retry_request(&err, 429, 1), "{body}");
        assert!(is_quota_exhausted(&err), "{body}");
        let code = err
            .detail()
            .and_then(|d| d.vendor_code.as_deref())
            .unwrap_or("");
        assert_eq!(code.split(';').next(), Some(QUOTA_VENDOR_CODE), "{body}");
    }
}

#[test]
fn quota_marker_preserves_vendor_code() {
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}"#,
        None,
        None,
    );
    let code = err
        .detail()
        .and_then(|d| d.vendor_code.as_deref())
        .unwrap_or("");
    assert!(code.contains("code=insufficient_quota"), "{code}");
}

#[test]
fn plain_429_without_quota_hint_stays_retryable() {
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"rate limit exceeded","code":"rate_limit_exceeded"}}"#,
        None,
        None,
    );
    assert!(matches!(err.class(), ProviderError::RateLimited(_)));
    assert!(is_retryable_error(&err));
    assert!(should_retry_request(&err, 429, 1));
    assert!(!is_quota_exhausted(&err));
    assert_eq!(quota_reset_in(&err), None);
}

#[test]
fn quota_resets_in_parses_relative_hints() {
    for (body, secs) in [
        ("resets in ~2h", 7200),
        ("usage limit reached, resets in ~30m", 1800),
        ("reset in 90 seconds", 90),
        ("will reset in about 1h", 3600),
        ("resets in 1.5 hours", 5400),
        ("resets in 2 days", 172800),
    ] {
        assert_eq!(
            parse_quota_reset_delay(body),
            Some(Duration::from_secs(secs)),
            "{body}"
        );
    }
    assert_eq!(parse_quota_reset_delay("rate limit exceeded"), None);
}

#[test]
fn quota_resets_in_parses_absolute_hints() {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let body = format!("usage limit reached, resets at {epoch}");
    let d = parse_quota_reset_delay(&body).expect("epoch parses");
    assert!(
        d <= Duration::from_secs(600) && d > Duration::from_secs(550),
        "{d:?}"
    );
    let d = parse_quota_reset_delay("resets at 2099-12-31T00:00:00Z").expect("iso parses");
    assert!(d > Duration::from_secs(86400 * 365), "{d:?}");
    let d = parse_quota_reset_delay("resets at 23:59 UTC").expect("clock parses");
    assert!(d <= Duration::from_secs(86400) && !d.is_zero(), "{d:?}");
}

#[test]
fn quota_reset_hint_flows_through_classification() {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 7200;
    let body = format!("usage limit reached, resets at {epoch}");
    let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, &body, None, None);
    assert!(is_quota_exhausted(&err));
    let d = quota_reset_in(&err).expect("resets parsed");
    assert!(
        d <= Duration::from_secs(7200) && d > Duration::from_secs(7100),
        "{d:?}"
    );
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "session limit reached, resets in ~2h",
        None,
        None,
    );
    assert_eq!(quota_reset_in(&err), Some(Duration::from_secs(7200)));
    let err = classify_http_error(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "usage limit reached",
        None,
        None,
    );
    assert!(is_quota_exhausted(&err));
    assert_eq!(quota_reset_in(&err), None, "no reset hint -> None");
}

#[test]
fn quota_detection_falls_back_to_message_hints() {
    let err = ProviderError::Auth("insufficient_quota".to_string());
    assert!(is_quota_exhausted(&err));
    let err = ProviderError::Auth("invalid api key".to_string());
    assert!(!is_quota_exhausted(&err));
}

#[test]
fn midstream_quota_event_marks_quota() {
    let event = serde_json::json!({"type":"response.failed","response":{"error":{"code":"insufficient_quota","message":"usage limit reached, resets in ~1h"}}});
    let err = stream_failure(&event, "usage limit reached, resets in ~1h".into());
    assert!(is_quota_exhausted(&err));
    assert_eq!(quota_reset_in(&err), Some(Duration::from_secs(3600)));
    assert!(!is_retryable_error(&err));
    assert!(!should_retry_request(&err, 0, 1));
    assert!(matches!(err.class(), ProviderError::Auth(_)));
}

#[tokio::test]
async fn silent_event_stream_trips_stall_cap() {
    use futures::StreamExt;
    let pending: BoxedEventStream = futures::stream::pending().boxed();
    let mut guarded = stall_guard(pending, Duration::from_millis(50));
    let item = tokio::time::timeout(Duration::from_secs(5), guarded.next())
        .await
        .expect("watchdog fires before outer timeout")
        .expect("stall yields an item");
    match item {
        Err(SseReadError::Stalled(cap)) => assert_eq!(cap, Duration::from_millis(50)),
        other => panic!("expected stall error, got {other:?}"),
    }
}

#[tokio::test]
async fn data_events_pass_through_then_silence_stalls() {
    use futures::StreamExt;
    let ev = eventsource_stream::Event {
        event: "message".into(),
        data: "x".into(),
        ..Default::default()
    };
    let s: BoxedEventStream = futures::stream::iter(vec![Ok(ev)])
        .chain(futures::stream::pending())
        .boxed();
    let mut guarded = stall_guard(s, Duration::from_millis(50));
    let first = guarded.next().await.expect("event delivered");
    assert!(first.is_ok(), "{first:?}");
    let second = tokio::time::timeout(Duration::from_secs(5), guarded.next())
        .await
        .expect("watchdog fires")
        .expect("item");
    assert!(
        matches!(second, Err(SseReadError::Stalled(_))),
        "{second:?}"
    );
}

#[tokio::test]
async fn hung_sse_response_surfaces_retryable_stall() {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    let _lock = STALL_ENV.lock().unwrap();
    unsafe { std::env::set_var("GRAY_PROVIDER_STALL_SECS", "1") };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
                    .await;
                tokio::time::sleep(Duration::from_secs(300)).await;
            });
        }
    });
    let provider =
        OpenAiProvider::new("key", "test-model", format!("http://{addr}/v1"), None, None)
            .expect("provider builds");
    let started = std::time::Instant::now();
    let mut s = provider.stream(empty_chat_req());
    let item = tokio::time::timeout(Duration::from_secs(30), s.next())
        .await
        .expect("watchdog fires before outer timeout")
        .expect("stream yields an item");
    let elapsed = started.elapsed();
    unsafe { std::env::remove_var("GRAY_PROVIDER_STALL_SECS") };
    server.abort();
    match item {
        Err(err) => {
            assert!(matches!(err.class(), ProviderError::Stream(_)), "{err:?}");
            assert!(is_retryable_error(&err), "stall stays retryable: {err:?}");
            assert!(err.to_string().contains("stalled"), "{err}");
        }
        Ok(ev) => panic!("expected stall error, got {ev:?}"),
    }
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(15),
        "failed at stall cap, not instantly or after hanging: {elapsed:?}"
    );
}

#[tokio::test]
async fn dynamic_stream_silent_turn_completes_without_stall() {
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = STALL_ENV.lock().unwrap();
    unsafe { std::env::set_var("GRAY_PROVIDER_STALL_SECS", "1") };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        socket.flush().await.unwrap();
        // Turn-settled transport: silence past the stall cap is the contract,
        // not a dead connection — the turn arrives whole when it settles.
        tokio::time::sleep(Duration::from_secs(2)).await;
        socket
            .write_all(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\ndata: [DONE]\n\n",
            )
            .await
            .unwrap();
        socket.flush().await.unwrap();
    });
    let profile = OpenAiProviderProfile {
        base_url: format!("http://{addr}/v1").parse().unwrap(),
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
        std::sync::Arc::new(StaticProviderSource {
            secrets: gray_core::credential::SecretMap::from_iter([("access_token", "t")]),
            metadata: Default::default(),
        }),
    )
    .unwrap();
    let mut stream = provider.stream(empty_chat_req());
    let mut saw_text = false;
    let mut completed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(StreamEvent::TextDelta { delta }) if delta == "hi" => saw_text = true,
            Ok(StreamEvent::MessageComplete { .. }) => completed = true,
            Ok(_) => {}
            Err(e) => panic!("silent turn must not surface a stream error: {e:?}"),
        }
    }
    unsafe { std::env::remove_var("GRAY_PROVIDER_STALL_SECS") };
    server.abort();
    assert!(
        saw_text && completed,
        "silent-then-complete turn must deliver its events"
    );
}

#[test]
fn a_recent_stall_shrinks_the_cap_for_the_next_stream() {
    let _lock = STALL_ENV.lock().unwrap();
    unsafe {
        std::env::remove_var("GRAY_PROVIDER_STALL_SECS");
        std::env::remove_var("GRAY_PROVIDER_RETRY_STALL_SECS");
    }
    LAST_STREAM_STALL_SECS.store(0, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(stream_stall_cap(), DEFAULT_STREAM_STALL);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    LAST_STREAM_STALL_SECS.store(now, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(stream_stall_cap(), RETRY_STREAM_STALL);
    LAST_STREAM_STALL_SECS.store(
        now - STREAM_STALL_MEMORY_SECS - 1,
        std::sync::atomic::Ordering::Relaxed,
    );
    assert_eq!(stream_stall_cap(), DEFAULT_STREAM_STALL);
    LAST_STREAM_STALL_SECS.store(0, std::sync::atomic::Ordering::Relaxed);
}

async fn accept_and_drop(listener: &tokio::net::TcpListener) {
    use tokio::io::AsyncReadExt;
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut buf = [0u8; 8192];
    let _ = socket.read(&mut buf).await;
}

#[tokio::test]
async fn dynamic_relay_retries_a_send_dropped_before_any_response() {
    use futures::StreamExt;
    use gray_core::message::{ChatRequest, ContentBlock, Message, Role};
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits2 = hits.clone();
    let server = tokio::spawn(async move {
        hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        accept_and_drop(&listener).await;

        hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 8192];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi-relay\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
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
        authorization: OpenAiAuthorization::None,
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

    let request = ChatRequest {
        system: None,
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )],
        tools: Vec::new(),
        max_tokens: None,
    };
    let events: Vec<_> = provider.stream(request).collect().await;
    server.await.unwrap();

    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Ok(StreamEvent::TextDelta { delta }) if delta == "hi-relay"
        )),
        "a send dropped before any response must retry transparently: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::StreamError { .. }))),
        "send-stage retries stay silent: {events:?}"
    );
}

#[tokio::test]
async fn static_wire_retries_a_send_dropped_before_any_response() {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits2 = hits.clone();
    let server = tokio::spawn(async move {
        hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        accept_and_drop(&listener).await;

        hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 8192];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            sse.len(),
            sse
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });

    let provider = OpenAiProvider::new(
        "key",
        "test-model",
        format!("http://{addr}/v1"),
        None,
        None,
    )
    .expect("provider builds");
    let events: Vec<_> = provider.stream(empty_chat_req()).collect().await;
    server.abort();

    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Ok(StreamEvent::TextDelta { delta }) if delta == "hi"
        )),
        "a send dropped before any response must retry transparently: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::StreamError { .. }))),
        "send-stage retries stay silent: {events:?}"
    );
}

#[tokio::test]
async fn dynamic_relay_surfaces_the_error_after_the_send_budget() {
    use futures::StreamExt;
    use gray_core::message::{ChatRequest, ContentBlock, Message, Role};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits2 = hits.clone();
    let server = tokio::spawn(async move {
        for _ in 0..MAX_SEND_ATTEMPTS {
            hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            accept_and_drop(&listener).await;
        }
    });

    let profile = OpenAiProviderProfile {
        base_url: format!("http://{addr}/v1").parse().unwrap(),
        wire: OpenAiWire::Responses,
        authorization: OpenAiAuthorization::None,
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

    let request = ChatRequest {
        system: None,
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )],
        tools: Vec::new(),
        max_tokens: None,
    };
    let events: Vec<_> = provider.stream(request).collect().await;
    server.await.unwrap();

    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        MAX_SEND_ATTEMPTS
    );
    assert!(
        matches!(events.last(), Some(Err(_))),
        "send failures must surface once the budget is spent: {events:?}"
    );
}
