//! Native Anthropic Messages API provider (`POST /v1/messages`).
//!
//! The OpenAI-compatible endpoint Anthropic also serves does not support
//! prompt caching, so a direct Claude key re-billed the whole prompt on every
//! request. This speaks the native wire: `cache_control` breakpoints on the
//! system prompt, the last tool and the last message (pi
//! `applyAnthropicCacheControl`), thinking blocks replayed with their
//! signatures, and the Messages SSE stream mapped onto gray's `StreamEvent`s.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::stream::{self, BoxStream, StreamExt};
use gray_core::agent::{Provider, ProviderError};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{ChatRequest, ContentBlock, Role};
use reqwest::Url;
use serde_json::{Value, json};

use crate::openai::{
    backoff_delay, classify_http_error, filter_valid_tools, parse_retry_after, video_rejected,
    wire_tool_output,
};

/// Default base URL; `/messages` is appended.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const API_VERSION: &str = "2023-06-01";
const MAX_ATTEMPTS: usize = 4;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// `ReasoningItem::item_id` marking an Anthropic thinking replay: the
/// `encrypted_content` is the response's thinking blocks as a JSON array,
/// signatures included, so they go back byte for byte.
pub const THINKING_ITEM_ID: &str = "anthropic";

/// True when `base_url` is Anthropic's own API (the native wire applies).
pub fn is_anthropic_base_url(base_url: &str) -> bool {
    Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h == "api.anthropic.com"))
        .unwrap_or(false)
}

/// Anthropic Messages provider. `Debug` is hand-written: the struct carries
/// a plaintext API key.
#[derive(Clone)]
pub struct AnthropicProvider {
    base_url: Url,
    api_key: String,
    model: String,
    http: reqwest::Client,
    reasoning_effort: Option<String>,
    temperature: Option<f32>,
    top_p: Option<f32>,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<String>,
    ) -> Result<Self, String> {
        let raw = base_url.into();
        let raw = if raw.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            raw
        };
        let base_url = Url::parse(&raw).map_err(|e| format!("invalid base_url '{raw}': {e}"))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client with timeouts");
        Ok(Self {
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
        })
    }

    pub fn with_sampling(mut self, temperature: Option<f32>, top_p: Option<f32>) -> Self {
        self.temperature = temperature;
        self.top_p = top_p;
        self
    }

    fn messages_url(&self) -> Result<Url, ProviderError> {
        let base = self.base_url.as_str().trim_end_matches('/');
        Url::parse(&format!("{base}/messages"))
            .map_err(|e| ProviderError::BadRequest(format!("invalid messages url: {e}")))
    }
}

/// Output cap when the request names none. Claude 3.x tops out at 8192;
/// everything newer accepts 32000.
fn default_max_tokens(model: &str) -> u32 {
    if model.to_lowercase().contains("claude-3") && !model.contains("3-7") {
        8192
    } else {
        32_000
    }
}

/// Thinking budget for an effort level (the chat mapper's table).
fn thinking_budget(effort: Option<&str>) -> Option<u32> {
    match effort? {
        "off" => None,
        "low" => Some(1024),
        "medium" => Some(4096),
        "max" => Some(32768),
        _ => Some(16384),
    }
}

fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

/// Maps a gray request onto the Messages API body.
pub(crate) fn map_request(
    req: ChatRequest,
    model: &str,
    reasoning_effort: Option<&str>,
    temperature: Option<f32>,
    top_p: Option<f32>,
) -> Result<Value, ProviderError> {
    let max_tokens = req.max_tokens.unwrap_or_else(|| default_max_tokens(model));
    // A budget must leave room below `max_tokens` (min 1024): a request that
    // cannot fit one sends no thinking rather than a 400.
    let budget = thinking_budget(reasoning_effort)
        .map(|b| b.min(max_tokens.saturating_sub(1024)))
        .filter(|b| *b >= 1024);

    let mut messages: Vec<(Role, Vec<Value>)> = Vec::new();
    for msg in req.messages {
        let mut blocks = Vec::new();
        for block in msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                block @ ContentBlock::StructuredInput { .. } => {
                    if let Some(text) = block.provider_text() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                ContentBlock::Image { media_type, data } => blocks.push(json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": media_type, "data": data},
                })),
                ContentBlock::Video { .. } => return Err(video_rejected(model)),
                ContentBlock::ToolUse { id, name, args } => {
                    let input = if args.is_object() { args } else { json!({}) };
                    blocks
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                }
                ContentBlock::ToolResult {
                    id,
                    content,
                    is_error,
                } => {
                    let text = wire_tool_output(&content, is_error);
                    let text = if text.is_empty() {
                        "(no output)".to_string()
                    } else {
                        text
                    };
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": text,
                        "is_error": is_error,
                    }));
                }
                // Only this model's own signed blocks can go back; anything
                // else (another provider's reasoning, a display-only run) is
                // dropped, which Anthropic accepts for earlier turns.
                ContentBlock::Thinking {
                    item_id,
                    encrypted_content,
                    model: from,
                    ..
                } => {
                    if item_id.as_deref() == Some(THINKING_ITEM_ID)
                        && from.as_deref() == Some(model)
                        && let Some(Ok(Value::Array(saved))) = encrypted_content
                            .as_deref()
                            .map(serde_json::from_str::<Value>)
                    {
                        blocks.extend(saved);
                    }
                }
            }
        }
        // A mid-conversation system note has no Messages slot: it rides as
        // user text (the system prompt itself stays fixed for the cache).
        let role = match msg.role {
            Role::Assistant => Role::Assistant,
            Role::User | Role::System => Role::User,
        };
        if !blocks.is_empty() {
            messages.push((role, blocks));
        }
    }
    pair_tool_results(&mut messages);
    if messages.first().is_some_and(|(r, _)| *r == Role::Assistant) {
        messages.insert(
            0,
            (
                Role::User,
                vec![json!({"type": "text", "text": "(continue)"})],
            ),
        );
    }

    // Breakpoint 3: the last cacheable block of the last message.
    if let Some((_, blocks)) = messages.last_mut()
        && let Some(block) = blocks.iter_mut().rev().find(|b| {
            !matches!(
                b.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking")
            )
        })
    {
        block["cache_control"] = ephemeral();
    }

    let mut tools: Vec<Value> = filter_valid_tools(req.tools)
        .into_iter()
        .map(
            |t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}),
        )
        .collect();
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = ephemeral();
    }

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages
            .into_iter()
            .map(|(role, content)| json!({
                "role": if role == Role::Assistant { "assistant" } else { "user" },
                "content": content,
            }))
            .collect::<Vec<_>>(),
    });
    if let Some(system) = req.system.filter(|s| !s.is_empty()) {
        body["system"] = json!([{"type": "text", "text": system, "cache_control": ephemeral()}]);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    match budget {
        Some(b) => body["thinking"] = json!({"type": "enabled", "budget_tokens": b}),
        // Sampling knobs are rejected alongside thinking.
        None => {
            if let Some(t) = temperature {
                body["temperature"] = json!(t.min(1.0));
            }
            if let Some(p) = top_p {
                body["top_p"] = json!(p);
            }
        }
    }
    Ok(body)
}

/// Anthropic rejects a `tool_use` whose result is not in the very next user
/// message, and a `tool_result` with no `tool_use` before it. Missing results
/// get a stub, strays become text, and results lead their message.
fn pair_tool_results(messages: &mut Vec<(Role, Vec<Value>)>) {
    let ids = |blocks: &[Value], kind: &str, key: &str| -> Vec<String> {
        blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some(kind))
            .filter_map(|b| b.get(key).and_then(Value::as_str).map(str::to_string))
            .collect()
    };
    let mut i = 0;
    while i < messages.len() {
        if messages[i].0 == Role::User {
            let open: HashSet<String> = match i.checked_sub(1).map(|p| &messages[p]) {
                Some((Role::Assistant, prev)) => ids(prev, "tool_use", "id").into_iter().collect(),
                _ => HashSet::new(),
            };
            let blocks = std::mem::take(&mut messages[i].1);
            let (mut results, rest): (Vec<Value>, Vec<Value>) = blocks
                .into_iter()
                .partition(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"));
            let mut text = Vec::new();
            results.retain(|r| {
                let known = r
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| open.contains(id));
                if !known {
                    let body = r.get("content").and_then(Value::as_str).unwrap_or("");
                    text.push(json!({"type": "text", "text": format!("[tool result] {body}")}));
                }
                known
            });
            results.extend(text);
            results.extend(rest);
            messages[i].1 = results;
        }
        if messages[i].0 == Role::Assistant {
            let calls = ids(&messages[i].1, "tool_use", "id");
            if !calls.is_empty() {
                let next_is_user = messages.get(i + 1).is_some_and(|(r, _)| *r == Role::User);
                if !next_is_user {
                    messages.insert(i + 1, (Role::User, Vec::new()));
                }
                let answered: HashSet<String> =
                    ids(&messages[i + 1].1, "tool_result", "tool_use_id")
                        .into_iter()
                        .collect();
                let stubs: Vec<Value> = calls
                    .iter()
                    .filter(|id| !answered.contains(*id))
                    .map(|id| {
                        log::warn!(target: "gray_provider", "synthesizing missing tool output for orphaned call {id}");
                        json!({"type": "tool_result", "tool_use_id": id, "content": "(no output: the call was interrupted)", "is_error": true})
                    })
                    .collect();
                messages[i + 1].1.splice(0..0, stubs);
            }
        }
        i += 1;
    }
}

/// Messages `usage` onto gray's inclusive [`Usage`].
fn map_usage(u: &Value, into: &mut Usage) {
    let get = |k: &str| u.get(k).and_then(Value::as_u64).map(|n| n as usize);
    if let Some(n) = get("input_tokens") {
        into.non_cached_input_tokens = n;
    }
    if let Some(n) = get("cache_read_input_tokens") {
        into.cache_read_input_tokens = n;
        into.cached_tokens = n;
    }
    if let Some(n) = get("cache_creation_input_tokens") {
        into.cache_write_input_tokens = n;
    }
    if let Some(n) = get("output_tokens") {
        into.output_tokens = n;
    }
    into.input_tokens =
        into.non_cached_input_tokens + into.cache_read_input_tokens + into.cache_write_input_tokens;
    into.total_tokens = into.input_tokens + into.output_tokens;
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

/// One streamed content block, accumulated for the thinking replay.
#[derive(Default)]
struct Block {
    kind: String,
    thinking: String,
    signature: String,
    data: String,
}

/// Folds Messages SSE events into `StreamEvent`s.
#[derive(Default)]
pub(crate) struct Decoder {
    usage: Usage,
    stop: Option<StopReason>,
    blocks: BTreeMap<usize, Block>,
    pub(crate) finished: bool,
}

impl Decoder {
    pub(crate) fn feed(
        &mut self,
        data: &str,
        out: &mut VecDeque<Result<StreamEvent, ProviderError>>,
    ) {
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let index = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(u) = ev.pointer("/message/usage") {
                    map_usage(u, &mut self.usage);
                }
            }
            "content_block_start" => {
                let cb = ev.get("content_block").cloned().unwrap_or(Value::Null);
                let kind = cb
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if kind == "tool_use" {
                    out.push_back(Ok(StreamEvent::ToolCallDelta {
                        index,
                        id: cb.get("id").and_then(Value::as_str).map(str::to_string),
                        name: cb.get("name").and_then(Value::as_str).map(str::to_string),
                        arguments_delta: String::new(),
                    }));
                }
                let data = cb
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.blocks.insert(
                    index,
                    Block {
                        kind,
                        data,
                        ..Block::default()
                    },
                );
            }
            "content_block_delta" => {
                let delta = ev.get("delta").cloned().unwrap_or(Value::Null);
                let text = |k: &str| {
                    delta
                        .get(k)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => out.push_back(Ok(StreamEvent::TextDelta {
                        delta: text("text"),
                    })),
                    "thinking_delta" => {
                        let t = text("thinking");
                        if let Some(b) = self.blocks.get_mut(&index) {
                            b.thinking.push_str(&t);
                        }
                        out.push_back(Ok(StreamEvent::ThinkingDelta { delta: t }));
                    }
                    "signature_delta" => {
                        if let Some(b) = self.blocks.get_mut(&index) {
                            b.signature.push_str(&text("signature"));
                        }
                    }
                    "input_json_delta" => out.push_back(Ok(StreamEvent::ToolCallDelta {
                        index,
                        id: None,
                        name: None,
                        arguments_delta: text("partial_json"),
                    })),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = ev.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(map_stop_reason(r));
                }
                if let Some(u) = ev.get("usage") {
                    map_usage(u, &mut self.usage);
                }
            }
            "message_stop" => {
                let saved: Vec<Value> = self
                    .blocks
                    .values()
                    .filter_map(|b| match b.kind.as_str() {
                        "thinking" if !b.signature.is_empty() => Some(json!({
                            "type": "thinking", "thinking": b.thinking, "signature": b.signature,
                        })),
                        "redacted_thinking" => {
                            Some(json!({"type": "redacted_thinking", "data": b.data}))
                        }
                        _ => None,
                    })
                    .collect();
                if !saved.is_empty() {
                    out.push_back(Ok(StreamEvent::ReasoningItem {
                        item_id: THINKING_ITEM_ID.to_string(),
                        encrypted_content: Value::Array(saved).to_string(),
                    }));
                }
                out.push_back(Ok(StreamEvent::MessageComplete {
                    stop_reason: self.stop.or(Some(StopReason::EndTurn)),
                    usage: Some(self.usage),
                }));
                self.finished = true;
            }
            "error" => {
                let kind = ev
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let msg = ev
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("stream error")
                    .to_string();
                out.push_back(Err(match kind {
                    "overloaded_error" | "api_error" => ProviderError::ServerError(msg),
                    "rate_limit_error" => ProviderError::RateLimited(msg),
                    _ => classify_http_error(reqwest::StatusCode::BAD_REQUEST, &msg, None, None),
                }));
                self.finished = true;
            }
            _ => {}
        }
    }
}

type Sse = BoxStream<
    'static,
    Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>,
>;

struct State {
    http: reqwest::Client,
    url: Url,
    api_key: String,
    body: Value,
    sse: Option<Sse>,
    out: VecDeque<Result<StreamEvent, ProviderError>>,
    decoder: Decoder,
    done: bool,
}

/// POST with pre-stream retries on rate limits and server errors.
async fn connect(
    http: &reqwest::Client,
    url: &Url,
    api_key: &str,
    body: &Value,
) -> Result<Sse, ProviderError> {
    let mut attempt = 1;
    loop {
        let sent = http
            .post(url.clone())
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await;
        let (err, retry_after) = match sent {
            Ok(resp) if resp.status().is_success() => {
                return Ok(resp.bytes_stream().eventsource().boxed());
            }
            Ok(resp) => {
                let status = resp.status();
                let retry_after = parse_retry_after(resp.headers());
                let req_id = resp
                    .headers()
                    .get("request-id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let snippet: String = resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(800)
                    .collect();
                // 529 is Anthropic's "overloaded": a retryable server error.
                let err = if status.as_u16() == 529 {
                    ProviderError::ServerError(format!("status 529 overloaded: {snippet}"))
                } else {
                    classify_http_error(status, &snippet, None, req_id.as_deref())
                };
                (err, retry_after)
            }
            Err(e) if e.is_timeout() => (ProviderError::Timeout(e.to_string()), None),
            Err(e) => (ProviderError::Connection(e.to_string()), None),
        };
        if !err.retryable() || attempt >= MAX_ATTEMPTS {
            return Err(err);
        }
        log::warn!(target: "gray_provider", "anthropic retrying ({attempt}/{MAX_ATTEMPTS}): {err}");
        tokio::time::sleep(backoff_delay(INITIAL_BACKOFF, attempt, retry_after)).await;
        attempt += 1;
    }
}

async fn step(mut st: State) -> Option<(Result<StreamEvent, ProviderError>, State)> {
    loop {
        if let Some(ev) = st.out.pop_front() {
            return Some((ev, st));
        }
        if st.done {
            return None;
        }
        let Some(sse) = st.sse.as_mut() else {
            match connect(&st.http, &st.url, &st.api_key, &st.body).await {
                Ok(sse) => st.sse = Some(sse),
                Err(e) => {
                    st.done = true;
                    st.out.push_back(Err(e));
                }
            }
            continue;
        };
        match sse.next().await {
            Some(Ok(event)) => {
                st.decoder.feed(&event.data, &mut st.out);
                st.done = st.decoder.finished;
            }
            Some(Err(e)) => {
                st.done = true;
                st.out.push_back(Err(ProviderError::Stream(e.to_string())));
            }
            None => {
                st.done = true;
                st.out.push_back(Err(ProviderError::Stream(
                    "stream ended before message_stop".into(),
                )));
            }
        }
    }
}

impl Provider for AnthropicProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let prepared = self.messages_url().and_then(|url| {
            map_request(
                req,
                &self.model,
                self.reasoning_effort.as_deref(),
                self.temperature,
                self.top_p,
            )
            .map(|body| (url, body))
        });
        let (url, body) = match prepared {
            Ok(p) => p,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };
        let state = State {
            http: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            sse: None,
            out: VecDeque::new(),
            decoder: Decoder::default(),
            done: false,
        };
        stream::unfold(state, step).boxed()
    }
}

#[path = "anthropic_tests.rs"]
#[cfg(test)]
mod tests;
