//! Test utilities and SSE fixture builders.

use serde_json::{json, Value};

/// Returns the standard OpenAI stream termination chunk (`data: [DONE]\n\n`).
pub fn sse_done() -> String {
    "data: [DONE]\n\n".to_string()
}

/// Formats a JSON value into an SSE data frame.
pub fn sse_json(val: &Value) -> String {
    format!("data: {}\n\n", serde_json::to_string(val).expect("serialize json"))
}

/// Helper to construct a text delta chunk JSON.
pub fn text_delta_chunk(delta_text: &str, finish_reason: Option<&str>) -> Value {
    json!({
        "choices": [
            {
                "index": 0,
                "delta": {
                    "content": delta_text
                },
                "finish_reason": finish_reason
            }
        ]
    })
}

/// Helper to construct a tool call fragment chunk JSON.
pub fn tool_call_chunk(
    index: usize,
    id: Option<&str>,
    name: Option<&str>,
    arguments_delta: Option<&str>,
    finish_reason: Option<&str>,
) -> Value {
    let mut tool_call = json!({
        "index": index,
    });
    if let Some(id_str) = id {
        tool_call["id"] = json!(id_str);
        tool_call["type"] = json!("function");
    }
    if name.is_some() || arguments_delta.is_some() {
        let mut func = json!({});
        if let Some(n) = name {
            func["name"] = json!(n);
        }
        if let Some(args) = arguments_delta {
            func["arguments"] = json!(args);
        }
        tool_call["function"] = func;
    }

    json!({
        "choices": [
            {
                "index": 0,
                "delta": {
                    "tool_calls": [tool_call]
                },
                "finish_reason": finish_reason
            }
        ]
    })
}
