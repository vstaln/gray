use serde_json::{Value, json};

pub const TOOL_PREFIX: &str = "mcp__gray__";
const MAX_TRANSCRIPT_CHARS: usize = 400_000;

pub struct PreparedTurn {
    pub system: String,
    pub input: Vec<Value>,
    pub tail: Vec<Value>,
    pub outputs: Vec<(String, String)>,
    pub tail_prompt: Vec<Value>,
    pub tools: Vec<Value>,
    pub model: String,
    pub effort: Option<String>,
    pub overrides: Vec<(String, String)>,
}

pub fn split_model_overrides(model: &str) -> (String, Vec<(String, String)>) {
    let Some(open) = model.find('[') else {
        return (model.to_string(), Vec::new());
    };
    if !model.ends_with(']') {
        return (model.to_string(), Vec::new());
    }
    let base = model[..open].trim().to_string();
    let inner = &model[open + 1..model.len() - 1];
    let mut out = Vec::new();
    for pair in inner.split(',') {
        if let Some((k, v)) = pair.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    (base, out)
}

fn item_type(item: &Value) -> &str {
    match item.get("type").and_then(Value::as_str) {
        Some(t) => t,
        None if item.get("role").is_some() => "message",
        None => "",
    }
}

fn text_of_content(content: &Value) -> String {
    let mut out = String::new();
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        for part in arr {
            match part.get("type").and_then(Value::as_str).unwrap_or("") {
                "input_text" | "output_text" | "text" => {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        out.push_str(t);
                    }
                }
                "input_image" | "image_url" => out.push_str("[image]"),
                _ => {}
            }
        }
    }
    out
}

fn message_text(item: &Value) -> String {
    item.get("content").map(text_of_content).unwrap_or_default()
}

fn is_slash_command(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return false;
    };
    rest.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric())
}

pub fn item_to_transcript_pub(item: &Value) -> Option<String> {
    item_to_transcript(item)
}

fn item_to_transcript(item: &Value) -> Option<String> {
    match item_type(item) {
        "message" => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let text = message_text(item);
            if text.trim().is_empty() {
                return None;
            }
            let tag = match role {
                "assistant" => "Assistant",
                "system" | "developer" => "System",
                _ => "User",
            };
            Some(format!("{tag}: {text}"))
        }
        "function_call" => {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
            let args = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            Some(format!(
                "Assistant called tool {name} with arguments: {args}"
            ))
        }
        "function_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("?");
            let output = item
                .get("output")
                .map(|o| match o {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(format!("Tool result for {call_id}: {output}"))
        }
        _ => None,
    }
}

pub fn prepare_turn(body: &Value, model: &str) -> Result<PreparedTurn, String> {
    let input = body
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut system = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    for item in &input {
        if item_type(item) == "message" {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "system" || role == "developer" {
                let text = message_text(item);
                if !text.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
            }
        }
    }

    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let (base, overrides) = split_model_overrides(if model.is_empty() {
        body.get("model").and_then(Value::as_str).unwrap_or("")
    } else {
        model
    });
    let effort = overrides
        .iter()
        .find(|(k, _)| k == "effort")
        .map(|(_, v)| v.clone())
        .or_else(|| {
            body.pointer("/reasoning/effort")
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    Ok(PreparedTurn {
        system,
        input,
        tail: Vec::new(),
        outputs: Vec::new(),
        tail_prompt: Vec::new(),
        tools,
        model: base,
        effort,
        overrides,
    })
}

pub fn assistant_side(item: &Value) -> bool {
    match item_type(item) {
        "function_call" | "reasoning" => true,
        "message" => item.get("role").and_then(Value::as_str) == Some("assistant"),
        _ => false,
    }
}

pub fn continuation<'a>(
    absorbed: &[Value],
    reply_call_ids: &[String],
    reply_text: &str,
    input: &'a [Value],
) -> Result<(&'a [Value], bool), &'static str> {
    if input.len() <= absorbed.len() || !input.starts_with(absorbed) {
        return Err("prefix");
    }
    let rest = &input[absorbed.len()..];
    let mut i = 0;
    let mut call_ids: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    while i < rest.len() && assistant_side(&rest[i]) {
        let item = &rest[i];
        match item_type(item) {
            "function_call" => call_ids.push(
                item.get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "message" => texts.push(message_text(item)),
            _ => {}
        }
        i += 1;
    }
    let undelivered = i == 0 && (!reply_call_ids.is_empty() || !reply_text.trim().is_empty());
    if !undelivered
        && (call_ids.as_slice() != reply_call_ids || texts.join("\n").trim() != reply_text.trim())
    {
        return Err("echo");
    }
    let tail = &rest[i..];
    if tail.is_empty() || tail.iter().any(assistant_side) {
        return Err("empty_delta");
    }
    Ok((tail, undelivered))
}

pub fn reply_signature(r: &TurnResult) -> (Vec<String>, String) {
    let mut call_ids = Vec::new();
    let mut texts = Vec::new();
    for item in &r.items {
        match item {
            OutItem::FunctionCall { call_id, .. } => call_ids.push(call_id.clone()),
            OutItem::Message { text } => texts.push(text.clone()),
            _ => {}
        }
    }
    (call_ids, texts.join("\n"))
}

pub fn split_tail(turn: &mut PreparedTurn) {
    split_tail_with(turn, false);
}

fn image_blocks(item: &Value) -> Vec<Value> {
    let Some(parts) = item.get("content").and_then(Value::as_array) else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|p| {
            matches!(
                p.get("type").and_then(Value::as_str),
                Some("input_image" | "image_url")
            )
        })
        .filter_map(|p| {
            let url = p
                .get("image_url")
                .and_then(|u| u.as_str().or_else(|| u.get("url").and_then(Value::as_str)))?;
            let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
            let mime = head.strip_suffix(";base64")?;
            if !matches!(
                mime,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) {
                return None;
            }
            Some(json!({"type": "image", "mimeType": mime, "data": data}))
        })
        .collect()
}

pub fn split_tail_with(turn: &mut PreparedTurn, images: bool) {
    let mut outputs = Vec::new();
    let mut prompt = Vec::new();
    for item in &turn.tail {
        if item_type(item) == "function_call_output" {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let output = item
                .get("output")
                .map(|o| match o {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            outputs.push((call_id, output));
        } else if let Some(text) = if item_type(item) == "message"
            && matches!(
                item.get("role").and_then(Value::as_str),
                None | Some("user")
            ) {
            let raw = message_text(item);
            if is_slash_command(&raw) {
                Some(raw.trim_start().to_string())
            } else {
                item_to_transcript(item)
            }
        } else {
            item_to_transcript(item)
        } {
            let mut t = text;
            if t.len() > MAX_TRANSCRIPT_CHARS {
                let mut end = MAX_TRANSCRIPT_CHARS;
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                t.truncate(end);
                t.push_str("\n[truncated]");
            }
            prompt.push(json!({"type": "text", "text": t}));
            if images && item_type(item) == "message" {
                prompt.extend(image_blocks(item));
            }
        }
    }
    turn.outputs = outputs;
    turn.tail_prompt = prompt;
}

#[derive(Default)]
pub struct TurnResult {
    pub items: Vec<OutItem>,
    pub usage: Option<Usage>,
    pub stop_reason: String,
    pub suspended: bool,
}

#[derive(Clone)]
pub enum OutItem {
    Message {
        text: String,
    },
    Reasoning {
        text: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
}

#[derive(Default, Clone)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub extra: Value,
}

impl Usage {
    pub fn from_acp(v: &Value) -> Option<Usage> {
        let get = |k1: &str, k2: &str| {
            v.get(k1)
                .or_else(|| v.get(k2))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let input = get("inputTokens", "input_tokens");
        let output = get("outputTokens", "output_tokens");
        let total = get("totalTokens", "total_tokens");
        if input == 0 && output == 0 && total == 0 {
            return None;
        }
        Some(Usage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: if total != 0 { total } else { input + output },
            extra: v.clone(),
        })
    }
}

pub fn rand_hex(n: usize) -> String {
    let bytes = uuid::Uuid::new_v4();
    let mut s = String::with_capacity(n);
    for b in bytes.as_bytes().iter().cycle().take(n) {
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(n);
    s
}

fn sse(event: &Value) -> Vec<u8> {
    format!("data: {event}\n\n").into_bytes()
}

pub fn fold_result(r: &TurnResult, model: &str, response_id: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&sse(&json!({
        "type": "response.created",
        "response": {"id": response_id, "object": "response",
            "created_at": now_secs(), "status": "in_progress",
            "model": model, "output": []}
    })));
    for (index, item) in r.items.iter().enumerate() {
        match item {
            OutItem::Message { text } => {
                let item_id = format!("msg_{response_id}_{index}");
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.added", "output_index": index,
                    "item": {"id": item_id, "type": "message", "role": "assistant",
                        "status": "in_progress", "content": [
                            {"type": "output_text", "text": "", "annotations": []}]}
                })));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_text.delta", "item_id": item_id,
                    "output_index": index, "content_index": 0, "delta": text}
                )));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_text.done", "item_id": item_id,
                    "output_index": index, "content_index": 0, "text": text}
                )));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.done", "output_index": index,
                    "item": {"id": item_id, "type": "message", "role": "assistant",
                        "status": "completed", "content": [
                            {"type": "output_text", "text": text, "annotations": []}]}
                })));
            }
            OutItem::Reasoning { text } => {
                let item_id = format!("rs_{response_id}_{index}");
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.added", "output_index": index,
                    "item": {"id": item_id, "type": "reasoning", "summary": []}
                })));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.done", "output_index": index,
                    "item": {"id": item_id, "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": text}]}
                })));
            }
            OutItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                let item_id = format!("fc_{response_id}_{index}");
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.added", "output_index": index,
                    "item": {"id": item_id, "type": "function_call",
                        "call_id": call_id, "name": name, "arguments": "",
                        "status": "in_progress"}
                })));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.function_call_arguments.done", "item_id": item_id,
                    "output_index": index, "arguments": arguments}
                )));
                out.extend_from_slice(&sse(&json!({
                    "type": "response.output_item.done", "output_index": index,
                    "item": {"id": item_id, "type": "function_call",
                        "call_id": call_id, "name": name, "arguments": arguments,
                        "status": "completed"}
                })));
            }
        }
    }
    let usage = r.usage.as_ref().map(|u| {
        let mut usage = json!({"input_tokens": u.input_tokens, "output_tokens": u.output_tokens,
            "total_tokens": u.total_tokens});
        if let Some(cached) = u.extra.get("cached_tokens").and_then(Value::as_u64) {
            usage["input_tokens_details"] = json!({"cached_tokens": cached});
        }
        usage
    });
    let status = if r.suspended {
        "incomplete"
    } else {
        "completed"
    };
    let mut response = json!({
        "id": response_id, "object": "response",
        "created_at": now_secs(), "status": status, "model": model,
        "output": serde_json::Value::Array(items_json(r)),
    });
    if let Some(u) = usage {
        response["usage"] = u;
    }
    out.extend_from_slice(&sse(
        &json!({"type": "response.completed", "response": response}),
    ));
    out.extend_from_slice(b"data: [DONE]\n\n");
    out
}

fn items_json(r: &TurnResult) -> Vec<Value> {
    r.items
        .iter()
        .map(|item| match item {
            OutItem::Message { text } => json!({
                "type": "message", "role": "assistant", "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": []}]}),
            OutItem::Reasoning { text } => json!({
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": text}]}),
            OutItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => json!({
                "type": "function_call", "call_id": call_id, "name": name,
                "arguments": arguments, "status": "completed"}),
        })
        .collect()
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
