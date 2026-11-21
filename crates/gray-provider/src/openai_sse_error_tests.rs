use super::*;
use gray_core::message::Message;
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> ChatRequest {
    ChatRequest {
        system: None,
        messages: vec![Message::user("hi")],
        tools: Vec::new(),
        max_tokens: None,
    }
}

fn sse(chunks: Vec<Value>) -> ResponseTemplate {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn run(template: ResponseTemplate) -> Vec<Result<StreamEvent, ProviderError>> {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(template)
        .mount(&server)
        .await;
    let provider = OpenAiProvider::new("k", "m", server.uri(), None, None).unwrap();
    provider.stream(request()).collect().await
}

fn first_err(events: &[Result<StreamEvent, ProviderError>]) -> &ProviderError {
    events
        .iter()
        .find_map(|e| e.as_ref().err())
        .expect("expected a stream error")
}

#[tokio::test]
async fn inline_sse_error_chunk_surfaces_the_upstream_message() {
    let events = run(sse(vec![
        json!({"error":{"message":"upstream exploded","code":"bad_gateway"}}),
    ]))
    .await;
    let err = first_err(&events).to_string();
    assert!(err.contains("upstream exploded"), "{err}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::MessageComplete { .. })))
    );
}

#[tokio::test]
async fn inline_sse_string_error_is_surfaced() {
    let events = run(sse(vec![json!({"error":"model overloaded"})])).await;
    assert!(first_err(&events).to_string().contains("model overloaded"));
}

#[tokio::test]
async fn inline_sse_quota_error_is_classified_as_quota() {
    let events = run(sse(vec![
        json!({"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}),
    ]))
    .await;
    assert!(is_quota_exhausted(first_err(&events)));
}

#[tokio::test]
async fn inline_sse_content_filter_error_is_a_bad_request() {
    let events = run(sse(vec![
        json!({"error":{"message":"blocked by content_filter","code":"content_filter"}}),
    ]))
    .await;
    assert!(matches!(
        first_err(&events).class(),
        ProviderError::BadRequest(_)
    ));
}

#[tokio::test]
async fn empty_done_stream_is_an_error_not_an_empty_end_turn() {
    let events = run(sse(Vec::new())).await;
    assert!(
        first_err(&events)
            .to_string()
            .contains("without any content or finish reason")
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::MessageComplete { .. })))
    );
}

#[tokio::test]
async fn content_filter_finish_without_output_is_reported() {
    let events = run(sse(vec![
        json!({"choices":[{"delta":{},"finish_reason":"content_filter"}]}),
    ]))
    .await;
    let err = first_err(&events);
    assert!(matches!(err.class(), ProviderError::BadRequest(_)));
    assert!(err.to_string().contains("content filter"));
}

#[tokio::test]
async fn content_filter_after_partial_text_keeps_the_text() {
    let events = run(sse(vec![
        json!({"choices":[{"delta":{"content":"partial"}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"content_filter"}]}),
    ]))
    .await;
    assert!(events.iter().all(|e| e.is_ok()));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { delta }) if delta == "partial"))
    );
}

#[tokio::test]
async fn normal_stop_and_empty_stop_streams_still_complete() {
    let events = run(sse(vec![
        json!({"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}),
    ]))
    .await;
    assert!(events.iter().all(|e| e.is_ok()));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::MessageComplete { .. })))
    );

    let events = run(sse(vec![
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
    ]))
    .await;
    assert!(events.iter().all(|e| e.is_ok()));
}
