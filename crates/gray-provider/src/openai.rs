//! OpenAI-compatible streaming LLM provider.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, BoxStream, StreamExt};
use gray_core::agent::{Provider, ProviderError};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{ChatRequest, ContentBlock, Role};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default API base URL pointing to OpenRouter.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Maximum retry attempts for transient errors.
const MAX_ATTEMPTS: usize = 3;

/// Upper bound for wire-controlled tool-call indices (hostile-server guard).
const MAX_TOOL_CALL_INDEX: usize = 4096;

/// An OpenAI-compatible LLM provider implementing the `Provider` trait.
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    base_url: Url,
    api_key: String,
    model: String,
    http: reqwest::Client,
    initial_backoff: Duration,
}

/// Builder for constructing an `OpenAiProvider`.
#[derive(Debug, Clone)]
pub struct OpenAiProviderBuilder {
    base_url: Option<String>,
    api_key: String,
    model: String,
    http: Option<reqwest::Client>,
    initial_backoff: Option<Duration>,
}

impl OpenAiProviderBuilder {
    /// Creates a new builder with the given API key and model name.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: None,
            api_key: api_key.into(),
            model: model.into(),
            http: None,
            initial_backoff: None,
        }
    }

    /// Sets the base URL for the OpenAI-compatible API endpoint.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Sets a custom `reqwest::Client`.
    pub fn http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Sets the initial backoff duration used for retrying rate limits and stream errors.
    pub fn initial_backoff(mut self, backoff: Duration) -> Self {
        self.initial_backoff = Some(backoff);
        self
    }

    /// Builds the `OpenAiProvider` instance.
    pub fn build(self) -> Result<OpenAiProvider, String> {
        let base_url_str = self
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = Url::parse(&base_url_str)
            .map_err(|e| format!("invalid base_url '{base_url_str}': {e}"))?;

        // ponytail: default client when the caller doesn't inject one — with a
        // 120s idle-read timeout so a stalled server (finish_reason then silence,
        // hung proxy) can't freeze a turn forever. Total timeout stays off:
        // long generations are legal.
        let http = self.http.unwrap_or_else(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .read_timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client with timeouts")
        });

        Ok(OpenAiProvider {
            base_url,
            api_key: self.api_key,
            model: self.model,
            http,
            initial_backoff: self
                .initial_backoff
                .unwrap_or(Duration::from_millis(50)),
        })
    }
}

impl OpenAiProvider {
    /// Creates a new `OpenAiProvider` with default options.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::builder(api_key, model)
            .build()
            .expect("default OpenAiProvider configuration is valid")
    }

    /// Returns a builder to configure and construct an `OpenAiProvider`.
    pub fn builder(api_key: impl Into<String>, model: impl Into<String>) -> OpenAiProviderBuilder {
        OpenAiProviderBuilder::new(api_key, model)
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Returns the configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Serialize)]
struct OpenAiChatRequest {
    model: String,
    stream: bool,
    messages: Vec<OpenAiMessageRequest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolDefRequest>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessageRequest {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    /// Reasoning from a prior assistant turn, sent back so reasoning models
    /// keep their chain-of-thought in context (deepseek/openai-compat style).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<Value>,
}

#[derive(Debug, Serialize)]
struct OpenAiToolCallRequest {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiFunctionCallRequest,
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionCallRequest {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAiToolDefRequest {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAiFunctionDefRequest,
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionDefRequest {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoiceChunk>,
    #[serde(default)]
    usage: Option<OpenAiUsageChunk>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceChunk {
    #[serde(default)]
    delta: OpenAiDeltaChunk,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDeltaChunk {
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek style.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// OpenRouter style (ox-alpha et al.).
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallChunk>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallChunk {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionChunk>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionChunk {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsageChunk {
    #[serde(default, alias = "input_tokens")]
    prompt_tokens: usize,
    #[serde(default, alias = "output_tokens")]
    completion_tokens: usize,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiCompletionDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptDetails>,
    /// Some providers (OpenRouter) also surface cached_tokens at top level
    #[serde(default)]
    cached_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompletionDetails {
    #[serde(default)]
    reasoning_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAiPromptDetails {
    #[serde(default)]
    cached_tokens: usize,
}

/// Anthropic prompt-caching: put an ephemeral cache breakpoint on the
/// last message so the prefix (system + history) can be cached. Copied
/// from mini-swe-agent's `cache_control.py` (MIT) — they show it cuts
/// input cost/latency for long trajectories.
///
/// Only for Anthropic-family models (Claude etc.) — other providers
/// ignore the field, Anthropic via OpenRouter honors it.
fn set_cache_control(messages: &mut [OpenAiMessageRequest]) {
    // clear any stale marks first (idempotent on retry)
    for m in messages.iter_mut() {
        m.cache_control = None;
        if let Some(Value::Array(arr)) = &mut m.content {
            if arr.len() == 1 {
                if let Some(obj) = arr.get_mut(0).and_then(Value::as_object_mut) {
                    obj.remove("cache_control");
                }
            }
        }
    }
    let Some(last) = messages.last_mut() else {
        return;
    };
    // assistant with only tool_use has no content → top-level cache_control
    if last.content.is_none() {
        last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        return;
    }
    if let Some(content) = &mut last.content {
        match content {
            Value::String(s) => {
                let text = std::mem::take(s);
                // empty string still needs a list wrapper to attach cache_control
                *content = serde_json::json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"}
                }]);
                if last.role == "tool" {
                    if let Value::Array(arr) = content {
                        if let Some(obj) = arr.get_mut(0).and_then(Value::as_object_mut) {
                            obj.remove("cache_control");
                        }
                    }
                    last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
                }
            }
            Value::Array(arr) => {
                if let Some(obj) = arr.get_mut(0).and_then(Value::as_object_mut) {
                    obj.insert("cache_control".to_string(), serde_json::json!({"type": "ephemeral"}));
                }
                if last.role == "tool" {
                    if let Some(obj) = arr.get_mut(0).and_then(Value::as_object_mut) {
                        obj.remove("cache_control");
                    }
                    last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
                }
            }
            _ => {}
        }
    }
}

fn map_chat_request(req: ChatRequest, model: &str) -> OpenAiChatRequest {
    let mut messages = Vec::new();

    // 1. Map system prompt
    if let Some(system) = req.system {
        messages.push(OpenAiMessageRequest {
            role: "system".to_string(),
            content: Some(Value::String(system)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            cache_control: None,
        });
    }

    // 2. Map conversation messages
    for msg in req.messages {
        match msg.role {
            Role::Assistant => {
                let mut text_parts = Vec::new();
                let mut thinking_parts = Vec::new();
                let mut tool_calls = Vec::new();
                let mut tool_results = Vec::new();

                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Thinking { text } => {
                            if !text.is_empty() {
                                thinking_parts.push(text);
                            }
                        }
                        ContentBlock::ToolUse { id, name, args } => {
                            tool_calls.push(OpenAiToolCallRequest {
                                id,
                                call_type: "function".to_string(),
                                function: OpenAiFunctionCallRequest {
                                    name,
                                    arguments: args.to_string(),
                                },
                            });
                        }
                        ContentBlock::ToolResult { id, content, is_error: _ } => {
                            tool_results.push((id, content));
                        }
                    }
                }

                let content = if text_parts.is_empty() {
                    if tool_calls.is_empty() {
                        Some(Value::String(String::new()))
                    } else {
                        None
                    }
                } else {
                    Some(Value::String(text_parts.join("\n")))
                };

                let tool_calls_opt = if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                };

                let reasoning_content = if thinking_parts.is_empty() {
                    None
                } else {
                    Some(thinking_parts.join("\n"))
                };

                messages.push(OpenAiMessageRequest {
                    role: "assistant".to_string(),
                    content,
                    reasoning_content,
                    tool_calls: tool_calls_opt,
                    tool_call_id: None,
                    cache_control: None,
                });

                for (id, content) in tool_results {
                    messages.push(OpenAiMessageRequest {
                        role: "tool".to_string(),
                        content: Some(Value::String(content)),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: Some(id),
                        cache_control: None,
                    });
                }
            }
            Role::User | Role::System => {
                let role_str = match msg.role {
                    Role::User => "user",
                    Role::System => "system",
                    Role::Assistant => unreachable!(),
                };

                let mut text_parts = Vec::new();
                let mut tool_results = Vec::new();

                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::ToolResult { id, content, is_error: _ } => {
                            tool_results.push((id, content));
                        }
                        ContentBlock::ToolUse { id, name, args } => {
                            tool_results.push((id, format!("{name}: {args}")));
                        }
                        // Reasoning from user/system turns isn't a thing; drop
                        // stale assistant thinking here rather than echoing it
                        // into tool results.
                        ContentBlock::Thinking { .. } => {}
                    }
                }

                for (id, content) in tool_results {
                    messages.push(OpenAiMessageRequest {
                        role: "tool".to_string(),
                        content: Some(Value::String(content)),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: Some(id),
                        cache_control: None,
                    });
                }

                if !text_parts.is_empty() || messages.is_empty() {
                    messages.push(OpenAiMessageRequest {
                        role: role_str.to_string(),
                        content: Some(Value::String(text_parts.join("\n"))),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                        cache_control: None,
                    });
                }
            }
        }
    }

    // Anthropic prompt caching — always set, non-Anthropic providers ignore it
    // (mini-swe-agent's default_end does the same when enabled)
    set_cache_control(&mut messages);

    // 3. Map tools
    let tools = req
        .tools
        .into_iter()
        .map(|tool| OpenAiToolDefRequest {
            tool_type: "function".to_string(),
            function: OpenAiFunctionDefRequest {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters,
            },
        })
        .collect();

    OpenAiChatRequest {
        model: model.to_string(),
        stream: true,
        messages,
        tools,
    }
}

fn map_finish_reason(reason: &str) -> Option<StopReason> {
    match reason {
        "stop" => Some(StopReason::EndTurn),
        "tool_calls" => Some(StopReason::ToolUse),
        "length" => Some(StopReason::MaxTokens),
        "cancelled" | "canceled" => Some(StopReason::Cancelled),
        "error" => Some(StopReason::Error),
        _ => None,
    }
}

fn map_usage(u: &OpenAiUsageChunk) -> Usage {
    let mut usage = Usage::new(u.prompt_tokens, u.completion_tokens);
    if let Some(details) = &u.completion_tokens_details {
        usage.reasoning_tokens = details.reasoning_tokens;
    }
    if let Some(details) = &u.prompt_tokens_details {
        usage.cached_tokens = details.cached_tokens;
    }
    // fallback for providers that put cached_tokens at top level
    if usage.cached_tokens == 0 {
        usage.cached_tokens = u.cached_tokens;
    }
    usage
}

fn chat_completions_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url_str = base_url.as_str().trim_end_matches('/').to_string();
    url_str.push_str("/chat/completions");
    Url::parse(&url_str)
        .map_err(|e| ProviderError::BadRequest(format!("invalid base URL '{base_url}': {e}")))
}

async fn send_request_with_retries(
    client: &reqwest::Client,
    url: &Url,
    api_key: &str,
    body: &OpenAiChatRequest,
    initial_backoff: Duration,
) -> Result<reqwest::Response, ProviderError> {
    for attempt in 1..=MAX_ATTEMPTS {
        // Empty key means keyless upstream: send no auth header at all.
        let base = if api_key.is_empty() {
            client.post(url.clone())
        } else {
            client
                .post(url.clone())
                .header("Authorization", format!("Bearer {api_key}"))
        };
        let req = base.header("Content-Type", "application/json").json(body);

        let res_result = req.send().await;
        log::debug!(target: "gray_provider", "request sent to {url} (attempt {attempt})");
        let err = match res_result {
            Ok(res) => {
                let status = res.status();
                if status.is_success() {
                    return Ok(res);
                }

                let text = res.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(500).collect();
                let msg = if snippet.is_empty() {
                    format!("status {status}")
                } else {
                    format!("status {status}: {snippet}")
                };

                match status.as_u16() {
                    401 | 403 => ProviderError::Auth(msg),
                    429 => ProviderError::RateLimited(msg),
                    400 => ProviderError::BadRequest(msg),
                    _ => ProviderError::Stream(msg),
                }
            }
            Err(e) => ProviderError::Stream(e.to_string()),
        };

        let is_retryable = matches!(err, ProviderError::RateLimited(_) | ProviderError::Stream(_));
        if !is_retryable || attempt == MAX_ATTEMPTS {
            log::warn!(target: "gray_provider", "request error after attempt {attempt}: {err}");
            return Err(err);
        }
        log::warn!(target: "gray_provider", "retrying (attempt {attempt}) after error: {err}");

        let exp_factor = 1u64 << (attempt - 1);
        let backoff_ms = (initial_backoff.as_millis() as u64).saturating_mul(exp_factor);
        let max_jitter = backoff_ms / 2;
        // Jitter from system-time nanos; no external RNG dependency.
        let jitter_ms = if max_jitter > 0 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0);
            nanos % (max_jitter + 1)
        } else {
            0
        };
        tokio::time::sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
    }

    Err(ProviderError::Stream("maximum retry attempts exceeded".to_string()))
}

type BoxedEventStream = BoxStream<
    'static,
    Result<
        eventsource_stream::Event,
        eventsource_stream::EventStreamError<reqwest::Error>,
    >,
>;

enum StreamState {
    Init {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        initial_backoff: Duration,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, (String, String, String)>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,
    },
    Done,
}

/// Drains accumulated tool-call fragments and queues the completion event.
///
/// Argument fragments are joined and parsed exactly once here; malformed
/// argument JSON surfaces as a `Stream` error instead of being forwarded.
fn emit_tool_calls_and_completion(
    accumulated_tools: &mut BTreeMap<usize, (String, String, String)>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    pending_events: &mut VecDeque<StreamEvent>,
) -> Result<(), ProviderError> {
    for (index, (id, name, args)) in std::mem::take(accumulated_tools) {
        if serde_json::from_str::<Value>(&args).is_err() {
            return Err(ProviderError::Stream(format!(
                "tool call at index {index} has malformed JSON arguments"
            )));
        }
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index,
            id: if id.is_empty() { None } else { Some(id) },
            name: if name.is_empty() { None } else { Some(name) },
            arguments_delta: args,
        });
    }

    pending_events.push_back(StreamEvent::MessageComplete {
        stop_reason,
        usage,
    });
    Ok(())
}

fn stream_unfold_step(
    mut state: StreamState,
) -> futures::future::BoxFuture<
    'static,
    Option<(Result<StreamEvent, ProviderError>, StreamState)>,
> {
    Box::pin(async move {
        loop {
            match state {
                StreamState::Init {
                    client,
                    url,
                    api_key,
                    body,
                    initial_backoff,
                } => {
                    match send_request_with_retries(&client, &url, &api_key, &body, initial_backoff).await {
                        Ok(response) => {
                            let event_stream: BoxedEventStream = response.bytes_stream().eventsource().boxed();
                            state = StreamState::Streaming {
                                event_stream,
                                accumulated_tools: BTreeMap::new(),
                                last_finish_reason: None,
                                last_usage: None,
                                pending_events: VecDeque::new(),
                                completed: false,
                            };
                        }
                        Err(err) => {
                            log::error!(target: "gray_provider", "stream request failed: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    }
                }
                StreamState::Streaming {
                    mut event_stream,
                    mut accumulated_tools,
                    mut last_finish_reason,
                    mut last_usage,
                    mut pending_events,
                    mut completed,
                } => {
                    if let Some(event) = pending_events.pop_front() {
                        return Some((
                            Ok(event),
                            StreamState::Streaming {
                                event_stream,
                                accumulated_tools,
                                last_finish_reason,
                                last_usage,
                                pending_events,
                                completed,
                            },
                        ));
                    }

                    if completed {
                        return None;
                    }

                    match event_stream.next().await {
                        Some(Ok(sse_event)) => {
                            let data = sse_event.data.trim();
                            if data == "[DONE]" {
                                // Guard against double-completion when
                                // finish_reason already ended the stream.
                                if !completed {
                                    completed = true;
                                    if let Err(err) = emit_tool_calls_and_completion(
                                        &mut accumulated_tools,
                                        last_finish_reason,
                                        last_usage,
                                        &mut pending_events,
                                    ) {
                                        return Some((Err(err), StreamState::Done));
                                    }
                                }
                            } else {
                                match serde_json::from_str::<OpenAiChunk>(data) {
                                    Ok(chunk) => {
                                        if let Some(u) = chunk.usage {
                                            last_usage = Some(map_usage(&u));
                                        }

                                        for choice in chunk.choices {
                                            match choice.delta.content {
                                                Some(delta_text)
                                                    if !delta_text.is_empty() =>
                                                {
                                                    pending_events.push_back(
                                                        StreamEvent::TextDelta {
                                                            delta: delta_text,
                                                        },
                                                    );
                                                }
                                                _ => {}
                                            }

                                            let reasoning = choice
                                                .delta
                                                .reasoning_content
                                                .or(choice.delta.reasoning);
                                            if let Some(reasoning) = reasoning {
                                                if !reasoning.is_empty() {
                                                    pending_events.push_back(
                                                        StreamEvent::ThinkingDelta {
                                                            delta: reasoning,
                                                        },
                                                    );
                                                }
                                            }

                                            if let Some(tool_calls) = choice.delta.tool_calls {
                                                for tc in tool_calls {
                                                    // ponytail: cap wire-controlled indices so a broken/
                                                    // hostile server can't balloon memory; raise if real
                                                    // turns ever need more concurrent tool calls.
                                                    if tc.index >= MAX_TOOL_CALL_INDEX {
                                                        continue;
                                                    }
                                                    let entry = accumulated_tools
                                                        .entry(tc.index)
                                                        .or_insert_with(|| {
                                                            (String::new(), String::new(), String::new())
                                                        });
                                                    if let Some(id) = tc.id {
                                                        entry.0.push_str(&id);
                                                    }
                                                    if let Some(func) = tc.function {
                                                        if let Some(name) = func.name {
                                                            entry.1.push_str(&name);
                                                        }
                                                        if let Some(args) = func.arguments {
                                                            entry.2.push_str(&args);
                                                        }
                                                    }
                                                }
                                            }

                                            if let Some(reason_str) = choice.finish_reason {
                                                // Defer MessageComplete until [DONE]/stream
                                                // end: OpenRouter & co. send usage in a final
                                                // chunk AFTER finish_reason; emitting here would
                                                // drop it and report 0 tokens every turn.
                                                last_finish_reason = map_finish_reason(&reason_str);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        return Some((
                                            Err(ProviderError::Stream(format!(
                                                "failed to parse OpenAI SSE chunk: {e}"
                                            ))),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                            }

                            state = StreamState::Streaming {
                                event_stream,
                                accumulated_tools,
                                last_finish_reason,
                                last_usage,
                                pending_events,
                                completed,
                            };
                        }
                        Some(Err(err)) => {
                            log::error!(target: "gray_provider", "stream error: {err}");
                            return Some((
                                Err(ProviderError::Stream(err.to_string())),
                                StreamState::Done,
                            ));
                        }
                        None => {
                            if !completed {
                                completed = true;
                                if let Err(err) = emit_tool_calls_and_completion(
                                    &mut accumulated_tools,
                                    last_finish_reason,
                                    last_usage,
                                    &mut pending_events,
                                ) {
                                    return Some((Err(err), StreamState::Done));
                                }
                                state = StreamState::Streaming {
                                    event_stream,
                                    accumulated_tools,
                                    last_finish_reason,
                                    last_usage,
                                    pending_events,
                                    completed,
                                };
                            } else {
                                return None;
                            }
                        }
                    }
                }
                StreamState::Done => {
                    return None;
                }
            }
        }
    })
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn stream(
        &self,
        req: ChatRequest,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let url = match chat_completions_url(&self.base_url) {
            Ok(u) => u,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };

        let body = map_chat_request(req, &self.model);
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            initial_backoff: self.initial_backoff,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sse_done, sse_json, text_delta_chunk, tool_call_chunk};
    use gray_core::event::Usage;
    use gray_core::message::Message;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn streams_text_deltas() {
        let server = MockServer::start().await;

        let sse_body = format!(
            "{}{}{}",
            sse_json(&text_delta_chunk("Hello, ", None)),
            sse_json(&text_delta_chunk("world!", Some("stop"))),
            sse_done()
        );

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body, "text/event-stream")
            )
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("test-key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Hi")]);
        let mut stream = provider.stream(req);

        let mut events = Vec::new();
        while let Some(res) = stream.next().await {
            events.push(res.expect("stream event should succeed"));
        }

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    delta: "Hello, ".to_string()
                },
                StreamEvent::TextDelta {
                    delta: "world!".to_string()
                },
                StreamEvent::MessageComplete {
                    stop_reason: Some(StopReason::EndTurn),
                    usage: None,
                }
            ]
        );
    }

    #[tokio::test]
    async fn streams_reasoning_content_as_thinking_deltas() {
        let server = MockServer::start().await;

        // deepseek/openai-compat style: reasoning arrives in delta.reasoning_content
        let think1 = json!({
            "choices": [{"index": 0, "delta": {"reasoning_content": "pondering "}}]
        });
        let think2 = json!({
            "choices": [{"index": 0, "delta": {"reasoning_content": "hard"}}]
        });
        let text = json!({
            "choices": [{"index": 0, "delta": {"content": "answer"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 30,
                "completion_tokens_details": {"reasoning_tokens": 21}
            }
        });
        let sse_body = format!("{}{}{}{}", sse_json(&think1), sse_json(&think2), sse_json(&text), sse_done());

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("test-key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Hi")]);
        let mut stream = provider.stream(req);

        let mut events = Vec::new();
        while let Some(res) = stream.next().await {
            events.push(res.expect("stream event should succeed"));
        }

        assert_eq!(
            events,
            vec![
                StreamEvent::thinking_delta("pondering "),
                StreamEvent::thinking_delta("hard"),
                StreamEvent::TextDelta { delta: "answer".to_string() },
                StreamEvent::message_complete(
                    Some(StopReason::EndTurn),
                    Some(Usage { input_tokens: 10, output_tokens: 30, reasoning_tokens: 21, ..Default::default() }),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn fragmented_tool_call_args_across_chunks_and_indices() {
        let server = MockServer::start().await;

        // >=4 chunks with different indices interleaved out of order
        // Chunk 1: index 0 (call_0, "bash", fragment "{\"cmd")
        let c1 = sse_json(&tool_call_chunk(0, Some("call_0"), Some("bash"), Some("{\"cmd"), None));
        // Chunk 2: index 1 (call_1, "read", fragment "{\"path")
        let c2 = sse_json(&tool_call_chunk(1, Some("call_1"), Some("read"), Some("{\"path"), None));
        // Chunk 3: index 0 (fragment "\":\"ls -la\"}")
        let c3 = sse_json(&tool_call_chunk(0, None, None, Some("\":\"ls -la\"}"), None));
        // Chunk 4: index 1 (fragment "\":\"src/lib.rs\"}", finish_reason "tool_calls")
        let c4 = sse_json(&tool_call_chunk(1, None, None, Some("\":\"src/lib.rs\"}"), Some("tool_calls")));
        let c5 = sse_done();

        let sse_body = format!("{c1}{c2}{c3}{c4}{c5}");

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body, "text/event-stream")
            )
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("test-key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Run tools")]);
        let mut stream = provider.stream(req);

        let mut events = Vec::new();
        while let Some(res) = stream.next().await {
            events.push(res.expect("stream event should succeed"));
        }

        assert_eq!(events.len(), 3);

        // Verify index 0 tool call delta
        match &events[0] {
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                assert_eq!(*index, 0);
                assert_eq!(id.as_deref(), Some("call_0"));
                assert_eq!(name.as_deref(), Some("bash"));
                let parsed: Value = serde_json::from_str(arguments_delta).expect("valid json for index 0");
                assert_eq!(parsed, json!({ "cmd": "ls -la" }));
            }
            other => panic!("expected ToolCallDelta at index 0, got {other:?}"),
        }

        // Verify index 1 tool call delta
        match &events[1] {
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                assert_eq!(*index, 1);
                assert_eq!(id.as_deref(), Some("call_1"));
                assert_eq!(name.as_deref(), Some("read"));
                let parsed: Value = serde_json::from_str(arguments_delta).expect("valid json for index 1");
                assert_eq!(parsed, json!({ "path": "src/lib.rs" }));
            }
            other => panic!("expected ToolCallDelta at index 1, got {other:?}"),
        }

        // Verify MessageComplete
        match &events[2] {
            StreamEvent::MessageComplete { stop_reason, usage } => {
                assert_eq!(*stop_reason, Some(StopReason::ToolUse));
                assert_eq!(*usage, None);
            }
            other => panic!("expected MessageComplete at index 2, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn retries_rate_limited_then_succeeds() {
        let server = MockServer::start().await;

        let sse_body = format!(
            "{}{}",
            sse_json(&text_delta_chunk("Success after retry", Some("stop"))),
            sse_done()
        );

        // Mount successful response first (evaluated after the 429 when LIFO / up_to_n_times expires)
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body, "text/event-stream")
            )
            .mount(&server)
            .await;

        // Mount 429 response to be served once
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Rate limit exceeded"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("test-key", "gpt-4o")
            .base_url(server.uri())
            .initial_backoff(Duration::from_millis(10))
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Retry test")]);
        let mut stream = provider.stream(req);

        let mut events = Vec::new();
        while let Some(res) = stream.next().await {
            events.push(res.expect("stream should succeed on retry"));
        }

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    delta: "Success after retry".to_string()
                },
                StreamEvent::MessageComplete {
                    stop_reason: Some(StopReason::EndTurn),
                    usage: None,
                }
            ]
        );
    }

    #[tokio::test]
    async fn unauthorized_fails_fast() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized key"))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("invalid-key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Test auth")]);
        let mut stream = provider.stream(req);

        let first = stream.next().await;
        match first {
            Some(Err(ProviderError::Auth(msg))) => {
                assert!(msg.contains("401"), "expected status 401 in message: {msg}");
            }
            other => panic!("expected ProviderError::Auth, got {other:?}"),
        }

        assert!(stream.next().await.is_none(), "stream should end after error");
    }

    #[tokio::test]
    async fn bad_request_fails_fast() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Invalid parameter"))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Test bad request")]);
        let mut stream = provider.stream(req);

        match stream.next().await {
            Some(Err(ProviderError::BadRequest(msg))) => {
                assert!(msg.contains("400"), "expected status 400 in message: {msg}");
            }
            other => panic!("expected ProviderError::BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_parsed_correctly() {
        let server = MockServer::start().await;

        let chunk = json!({
            "choices": [
                {
                    "index": 0,
                    "delta": { "content": "Done" },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 17
            }
        });

        let sse_body = format!("{}{}", sse_json(&chunk), sse_done());

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body, "text/event-stream")
            )
            .mount(&server)
            .await;

        let provider = OpenAiProvider::builder("key", "gpt-4o")
            .base_url(server.uri())
            .build()
            .expect("valid provider builder");

        let req = ChatRequest::new(vec![Message::user("Usage test")]);
        let mut stream = provider.stream(req);

        let mut events = Vec::new();
        while let Some(res) = stream.next().await {
            events.push(res.expect("stream event should succeed"));
        }

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    delta: "Done".to_string()
                },
                StreamEvent::MessageComplete {
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Some(Usage::new(42, 17)),
                }
            ]
        );
    }

    #[test]
    fn cache_control_last_user_gets_ephemeral() {
        let req = ChatRequest::new(vec![Message::user("hello")]).with_system("you are helpful");
        let mapped = map_chat_request(req, "anthropic/claude-sonnet-4");
        assert_eq!(mapped.messages.len(), 2);
        // system has no cache_control
        assert!(mapped.messages[0].cache_control.is_none());
        // last user should have ephemeral inside content
        let last = &mapped.messages[1];
        assert_eq!(last.role, "user");
        let content = last.content.as_ref().unwrap();
        assert!(content.is_array(), "last user content should be array with cache_control");
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        assert!(last.cache_control.is_none());
    }

    #[test]
    fn cache_control_clears_previous_and_sets_only_last() {
        // 2 user messages -> only last gets cache_control
        let req = ChatRequest::new(vec![Message::user("first"), Message::user("second")]);
        let mapped = map_chat_request(req, "anthropic/claude");
        assert_eq!(mapped.messages.len(), 2);
        // first user: string content, no cache
        assert!(mapped.messages[0].content.as_ref().unwrap().is_string());
        assert!(mapped.messages[0].cache_control.is_none());
        // second user: array with cache_control
        let last = &mapped.messages[1];
        assert!(last.content.as_ref().unwrap().is_array());
        assert_eq!(last.content.as_ref().unwrap()[0]["cache_control"], json!({"type": "ephemeral"}));
    }

    #[test]
    fn cache_control_tool_last_uses_top_level() {
        // Build a request where last message is a tool result
        let mut req = ChatRequest::new(vec![Message::user("hi")]);
        req.messages.push(Message {
            role: gray_core::message::Role::Assistant,
            content: vec![gray_core::message::ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "bash".into(),
                args: serde_json::json!({"cmd": "ls"}),
            }],
        });
        req.messages.push(Message {
            role: gray_core::message::Role::User,
            content: vec![gray_core::message::ContentBlock::ToolResult {
                id: "call_1".into(),
                content: "output".into(),
                is_error: false,
            }],
        });
        let mapped = map_chat_request(req, "anthropic/claude");
        let last = mapped.messages.last().unwrap();
        assert_eq!(last.role, "tool");
        // tool should have top-level cache_control, not inside content
        assert_eq!(last.cache_control, Some(json!({"type": "ephemeral"})));
        // content should be array without inner cache_control (mini workaround)
        let content = last.content.as_ref().unwrap();
        assert!(content.is_array());
        assert!(content[0].get("cache_control").is_none());
    }

    #[test]
    fn cache_control_set_even_for_openai_models() {
        // A: always-on — non-Anthropic ignores it, so safe to set
        let req = ChatRequest::new(vec![Message::user("hello")]);
        let mapped = map_chat_request(req, "openai/gpt-4o");
        let last = &mapped.messages[0];
        assert!(last.content.as_ref().unwrap().is_array());
        assert_eq!(last.content.as_ref().unwrap()[0]["cache_control"], json!({"type": "ephemeral"}));
    }
}
