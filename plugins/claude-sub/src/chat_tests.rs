use super::*;

fn body(items: Vec<Value>) -> Value {
    json!({"model": "sonnet", "instructions": "sys", "input": items, "stream": true})
}

#[test]
fn user_message_becomes_native_frame() {
    let b = body(vec![json!({"type": "message", "role": "user",
        "content": [{"type": "input_text", "text": "hi"}]})]);
    let turn = prepare_turn(&b, "sonnet").unwrap();
    assert_eq!(turn.system, "sys");
    assert_eq!(turn.frames.len(), 1);
    assert_eq!(turn.frames[0]["type"], "user");
    assert_eq!(turn.native_model, "claude-sonnet-5[1m]");
}

#[test]
fn empty_history_is_rejected() {
    let b = body(vec![]);
    assert!(prepare_turn(&b, "sonnet").is_err());
}

#[test]
fn fold_emits_valid_responses_sse() {
    let lines = vec![
        json!({"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": "hello"}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false,
            "usage": {"input_tokens": 10, "output_tokens": 3}}),
    ];
    let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|_| {});
    let (sse, natives, text, calls, usage, stop) = fold_lines(&lines, &[], &say).unwrap();
    assert_eq!(text, "hello");
    assert!(calls.is_empty());
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(stop, "completed");
    assert_eq!(natives.len(), 1);
    let s = String::from_utf8(sse).unwrap();
    assert!(s.contains("response.created"));
    assert!(s.contains("response.output_text.done"));
    assert!(s.contains("response.output_item.done"));
    assert!(s.contains("response.completed"));
    assert!(s.ends_with("data: [DONE]\n\n"));
}

#[test]
fn fold_rejects_foreign_tools() {
    let lines = vec![
        json!({"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "tool_use", "id": "t1", "name": "rm_rf", "input": {}}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false,
            "usage": {"input_tokens": 1, "output_tokens": 1}}),
    ];
    let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|_| {});
    assert!(fold_lines(&lines, &["bash".to_string()], &say).is_err());
}
