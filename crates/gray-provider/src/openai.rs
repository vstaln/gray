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
    reasoning_effort: Option<String>,
    /// Stable per-process id sent as `prompt_cache_key` (Responses API) so the
    /// gateway pins one cache shard — pi parity for prompt caching.
    session_id: Option<String>,
}

/// Builder for constructing an `OpenAiProvider`.
#[derive(Debug, Clone)]
pub struct OpenAiProviderBuilder {
    base_url: Option<String>,
    api_key: String,
    model: String,
    http: Option<reqwest::Client>,
    initial_backoff: Option<Duration>,
    reasoning_effort: Option<String>,
    session_id: Option<String>,
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
            reasoning_effort: None,
            session_id: None,
        }
    }

    /// Sets a stable session id sent as `prompt_cache_key` on the Responses
    /// API so consecutive requests hit the same cache shard (pi parity).
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
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

    /// Sets reasoning effort (e.g. "low", "medium", "high", "off").
    pub fn reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// Builds the `OpenAiProvider` instance.
    pub fn build(self) -> Result<OpenAiProvider, String> {
        let base_url_str = self
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = Url::parse(&base_url_str)
            .map_err(|e| format!("invalid base_url '{base_url_str}': {e}"))?;

        // default client when the caller doesn't inject one — with a
        // 120s idle-read timeout so a stalled server (finish_reason then silence,
        // hung proxy) can't freeze a turn forever. Total timeout stays off:
        // long generations are legal.
        let http = self.http.unwrap_or_else(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
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
            reasoning_effort: self.reasoning_effort,
            session_id: self.session_id,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    messages: Vec<OpenAiMessageRequest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolDefRequest>,
}

#[derive(Debug, Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<Value>,
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
    /// Ollama / Gemini / Qwen style.
    #[serde(default)]
    thought: Option<String>,
    #[serde(default)]
    thoughts: Option<String>,
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
    #[serde(default, alias = "output_tokens_details")]
    completion_tokens_details: Option<OpenAiCompletionDetails>,
    #[serde(default, alias = "input_tokens_details")]
    prompt_tokens_details: Option<OpenAiPromptDetails>,
    /// DeepSeek / OpenRouter / Kimi top-level fields
    #[serde(default, alias = "prompt_cache_hit_tokens", alias = "promptCacheHitTokens")]
    cached_tokens: usize,
    /// DeepSeek explicit miss count — preferred over prompt-minus-cached (pi parity)
    #[serde(default, alias = "promptCacheMissTokens")]
    prompt_cache_miss_tokens: usize,

    /// Anthropic native breakdown (when not via OpenAI compat)
    #[serde(default, alias = "cache_creation_input_tokens", alias = "cacheCreationInputTokens")]
    cache_creation_input_tokens: usize,
    #[serde(default, alias = "cache_read_input_tokens", alias = "cacheReadInputTokens")]
    cache_read_input_tokens: usize,
    /// Provider total if supplied (OpenAI `total_tokens`, Anthropic not)
    #[serde(default, alias = "total_tokens", alias = "totalTokens")]
    total_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompletionDetails {
    #[serde(default, alias = "reasoningTokens")]
    reasoning_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAiPromptDetails {
    #[serde(default)]
    cached_tokens: usize,
    #[serde(default, alias = "cache_creation_tokens", alias = "cacheCreationTokens")]
    cache_creation_tokens: usize,
    #[serde(default, alias = "cache_read_tokens", alias = "cacheReadTokens")]
    cache_read_tokens: usize,
}

fn is_anthropic_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("claude") || lower.contains("anthropic")
}

fn is_muse_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("muse") || lower.contains("spark") || lower.contains("glimmer")
}

/// Anthropic prompt-caching: matching Pi's applyAnthropicCacheControl.
/// Attaches cache_control breakpoints to:
/// 1. System prompt
/// 2. Last tool definition
/// 3. Last conversation message
fn apply_anthropic_cache_control(
    messages: &mut [OpenAiMessageRequest],
    tools: &mut [OpenAiToolDefRequest],
) {
    let cache_control = serde_json::json!({"type": "ephemeral"});

    // 1. Add cache control to system prompt
    for m in messages.iter_mut() {
        if m.role == "system" || m.role == "developer" {
            m.cache_control = Some(cache_control.clone());
            break;
        }
    }

    // 2. Add cache control to last tool
    if let Some(last_tool) = tools.last_mut() {
        last_tool.cache_control = Some(cache_control.clone());
    }

    // 3. Add cache control to last conversation message
    for m in messages.iter_mut().rev() {
        if m.role == "user" || m.role == "assistant" || m.role == "tool" {
            m.cache_control = Some(cache_control.clone());
            break;
        }
    }
}

fn image_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

fn filter_valid_tools(tools: Vec<gray_core::message::ToolDef>) -> Vec<gray_core::message::ToolDef> {
    tools.into_iter().filter(|t| {
        if t.name.trim().is_empty() {
            log::warn!(target: "gray_provider", "dropping tool def with empty name");
            false
        } else { true }
    }).collect()
}

fn is_valid_tool_name(name: &str, id: &str) -> bool {
    if name.trim().is_empty() {
        log::warn!(target: "gray_provider", "dropping assistant tool call {id} with empty name");
        false
    } else { true }
}

fn map_chat_request(req: ChatRequest, model: &str, reasoning_effort: Option<&str>) -> OpenAiChatRequest {
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
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Thinking { text, .. } => {
                            if !text.is_empty() {
                                thinking_parts.push(text);
                            }
                        }
                        ContentBlock::ToolUse { id, name, args } => {
                            if !is_valid_tool_name(&name, &id) {
                                continue;
                            }
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
                let mut image_parts = Vec::new();
                let mut tool_results = Vec::new();

                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Image { media_type, data } => {
                            image_parts.push((media_type, data));
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

                let has_images = !image_parts.is_empty();
                if !text_parts.is_empty() || has_images || messages.is_empty() {
                    let content = if has_images {
                        let mut arr = Vec::new();
                        for text in text_parts {
                            arr.push(serde_json::json!({"type":"text","text": text}));
                        }
                        for (media_type, data) in &image_parts {
                            let url = image_data_url(media_type, data);
                            arr.push(serde_json::json!({"type":"image_url","image_url":{"url": url}}));
                        }
                        Some(Value::Array(arr))
                    } else {
                        Some(Value::String(text_parts.join("\n")))
                    };
                    messages.push(OpenAiMessageRequest {
                        role: role_str.to_string(),
                        content,
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                        cache_control: None,
                    });
                }
            }
        }
    }

    // 3. Map tools — drop empty names that would trigger 400 `name` must be non-empty
    let mut tools: Vec<OpenAiToolDefRequest> = filter_valid_tools(req.tools)
        .into_iter()
        .map(|tool| OpenAiToolDefRequest {
            tool_type: "function".to_string(),
            function: OpenAiFunctionDefRequest {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters,
            },
            cache_control: None,
        })
        .collect();

    // Anthropic prompt caching (Pi-matching): only applied for Anthropic/Claude models
    if is_anthropic_model(model) {
        apply_anthropic_cache_control(&mut messages, &mut tools);
    }

    // Orphan guard (same class as the Responses mapper): an assistant
    // tool_call with no following tool result 400s on strict providers —
    // synthesize a stub result when a non-tool message (or end of history)
    // arrives with calls still unanswered.
    let mut fixed: Vec<OpenAiMessageRequest> = Vec::with_capacity(messages.len());
    let mut outstanding: Vec<String> = Vec::new();
    let stub = |id: &str| OpenAiMessageRequest {
        role: "tool".to_string(),
        content: Some(Value::String(
            "[no tool output — call was interrupted]".to_string(),
        )),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(id.to_string()),
        cache_control: None,
    };
    for m in messages {
        match m.role.as_str() {
            "assistant" => {
                for id in outstanding.drain(..) {
                    log::warn!(target: "gray_provider", "synthesizing missing tool output for orphaned call {id}");
                    fixed.push(stub(&id));
                }
                if let Some(calls) = &m.tool_calls {
                    outstanding.extend(calls.iter().map(|c| c.id.clone()));
                }
                fixed.push(m);
            }
            "tool" => {
                if let Some(id) = &m.tool_call_id {
                    outstanding.retain(|o| o != id);
                }
                fixed.push(m);
            }
            _ => {
                for id in outstanding.drain(..) {
                    log::warn!(target: "gray_provider", "synthesizing missing tool output for orphaned call {id}");
                    fixed.push(stub(&id));
                }
                fixed.push(m);
            }
        }
    }
    for id in outstanding.drain(..) {
        log::warn!(target: "gray_provider", "synthesizing missing tool output for orphaned call {id}");
        fixed.push(stub(&id));
    }
    let messages = fixed;

    let (reasoning_effort_val, reasoning_val, thinking_val) = match reasoning_effort {
        Some("off") => (None, None, Some(serde_json::json!({ "type": "disabled" }))),
        Some(eff) => {
            let budget = match eff {
                "low" => 1024,
                "medium" => 4096,
                "max" => 32768,
                _ => 16384, // high / default
            };
            (
                Some(eff.to_string()),
                Some(serde_json::json!({ "effort": eff })),
                Some(serde_json::json!({ "type": "enabled", "budget_tokens": budget })),
            )
        }
        None => (None, None, None),
    };

    OpenAiChatRequest {
        model: model.to_string(),
        stream: true,
        stream_options: Some(OpenAiStreamOptions { include_usage: true }),
        reasoning_effort: reasoning_effort_val,
        reasoning: reasoning_val,
        thinking: thinking_val,
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
    // Opencode v2 logic: inclusive totals + non-overlapping breakdown with clamping.
    // OpenAI: prompt_tokens is inclusive, cached is subset -> non_cached = subtract(inclusive, cached)
    // Anthropic: prompt_tokens is non-cached only, plus read/write -> inclusive = sum(non_cached, read, write)
    let mut reasoning = 0usize;
    if let Some(details) = &u.completion_tokens_details {
        reasoning = details.reasoning_tokens;
    }

    // Extract cache fields from all possible shapes
    let mut cache_read = 0usize;
    let mut cache_write = 0usize;
    if let Some(details) = &u.prompt_tokens_details {
        cache_read = details.cached_tokens.max(details.cache_read_tokens);
        cache_write = details.cache_creation_tokens;
    }
    // Top-level fallbacks (Anthropic native or OpenRouter)
    if cache_read == 0 {
        cache_read = u.cached_tokens.max(u.cache_read_input_tokens);
    } else if u.cache_read_input_tokens != 0 {
        cache_read = cache_read.max(u.cache_read_input_tokens);
    }
    if cache_write == 0 {
        cache_write = u.cache_creation_input_tokens;
    }

    let is_anthropic_shape = u.cache_creation_input_tokens != 0 || u.cache_read_input_tokens != 0;

    let (input_inclusive, non_cached) = if is_anthropic_shape {
        // Anthropic: prompt_tokens = non-cached only
        let inclusive = u.prompt_tokens + cache_read + cache_write;
        (inclusive, u.prompt_tokens)
    } else {
        // OpenAI: prompt_tokens is inclusive. DeepSeek reports an explicit
        // miss count — prefer it over subtraction (pi parity).
        let non_cached = if u.prompt_cache_miss_tokens != 0 {
            u.prompt_cache_miss_tokens
        } else {
            u.prompt_tokens.saturating_sub(cache_read + cache_write)
        };
        (u.prompt_tokens, non_cached)
    };

    let total = if u.total_tokens != 0 {
        u.total_tokens
    } else {
        input_inclusive + u.completion_tokens
    };

    let mut usage = Usage {
        input_tokens: input_inclusive,
        output_tokens: u.completion_tokens,
        reasoning_tokens: reasoning,
        cached_tokens: cache_read,
        non_cached_input_tokens: non_cached,
        cache_read_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
        total_tokens: total,
    };
    usage.normalize();
    usage
}

fn chat_completions_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url_str = base_url.as_str().trim_end_matches('/').to_string();
    url_str.push_str("/chat/completions");
    Url::parse(&url_str)
        .map_err(|e| ProviderError::BadRequest(format!("invalid base URL '{base_url}': {e}")))
}

fn responses_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url_str = base_url.as_str().trim_end_matches('/').to_string();
    url_str.push_str("/responses");
    Url::parse(&url_str)
        .map_err(|e| ProviderError::BadRequest(format!("invalid base URL '{base_url}': {e}")))
}

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ResponsesTool>,
    stream: bool,
    /// Cache-shard affinity (pi sends the session id here).
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    /// Server-side retention off (pi parity): with store enabled the backend
    /// folds generated reasoning into its cached prompt, which breaks the
    /// exact-prefix match on later turns (measured: turn 3+ misses at 0%).
    store: bool,
    /// Reasoning summaries (`summary: "auto"`, opencode parity): without this
    /// the backend returns encrypted-only thinking and there is nothing to
    /// display — no `response.reasoning_summary_text.delta` events arrive.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    /// Ask for `reasoning.encrypted_content` back (pi_agent_rust parity):
    /// without this there is nothing replayable and every turn re-pays full
    /// prefix processing. Only sent when `reasoning` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    include: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct ResponsesTool {
    #[serde(rename = "type")]
    tool_type: String,
    name: String,
    description: String,
    parameters: Value,
}

fn map_chat_to_responses(
    req: ChatRequest,
    model: &str,
    session_id: Option<&str>,
    reasoning_effort: Option<&str>,
) -> ResponsesRequest {
    let instructions = req.system;
    let mut input: Vec<Value> = Vec::new();
    for msg in req.messages {
        match msg.role {
            Role::User | Role::System => {
                let role_str = match msg.role {
                    Role::System => "system",
                    _ => "user",
                };
                let mut text_parts: Vec<String> = Vec::new();
                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Image { media_type, data } => {
                            // Responses supports image input as content part; send as data URL text placeholder
                            let url = image_data_url(&media_type, &data);
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            input.push(serde_json::json!({"role": "user", "content": [{"type":"input_image","image_url": url}]}));
                        }
                        ContentBlock::ToolResult { id, content, is_error: _ } => {
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            input.push(serde_json::json!({"type":"function_call_output","call_id": id, "output": content}));
                        }
                        ContentBlock::ToolUse { id, name, args } => {
                            if !is_valid_tool_name(&name, &id) {
                                continue;
                            }
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            input.push(serde_json::json!({"type":"function_call","call_id": id, "name": name, "arguments": args.to_string()}));
                        }
                        ContentBlock::Thinking { .. } => {}
                    }
                }
                if !text_parts.is_empty() {
                    input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                }
            }
            Role::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut reasoning_items: Vec<Value> = Vec::new();
                let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
                let mut tool_results: Vec<(String, String)> = Vec::new();
                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Thinking { encrypted_content: Some(ec), item_id: Some(id), model: Some(m), .. } if m == model => {
                            // Replay the raw reasoning item verbatim so the
                            // server keeps its cache shard warm (pi_agent_rust
                            // parity). Same-model only: a foreign model cannot
                            // decrypt the blob and strict servers 400 on it.
                            reasoning_items.push(serde_json::json!({"type": "reasoning", "id": id, "summary": [], "encrypted_content": ec}));
                        }
                        ContentBlock::Thinking { .. } => {}
                        ContentBlock::ToolUse { id, name, args } => tool_uses.push((id, name, args)),
                        ContentBlock::ToolResult { id, content, is_error: _ } => tool_results.push((id, content)),
                    }
                }
                for item in reasoning_items {
                    input.push(item);
                }
                if !text_parts.is_empty() {
                    input.push(serde_json::json!({"role":"assistant","content": text_parts.join("\n")}));
                }
                for (id, name, args) in tool_uses {
                    if !is_valid_tool_name(&name, &id) {
                        continue;
                    }
                    input.push(serde_json::json!({"type":"function_call","call_id": id, "name": name, "arguments": args.to_string()}));
                }
                for (id, content) in tool_results {
                    input.push(serde_json::json!({"type":"function_call_output","call_id": id, "output": content}));
                }
            }
        }
    }
    if input.is_empty() {
        input.push(serde_json::json!({"role":"user","content":""}));
    }
    // Transcripts can contain a `function_call` whose output never followed
    // (turn cancelled mid-tool round before this guard existed). Strict
    // providers 400 on the orphan, bricking the session forever — synthesize
    // a stub output for any unanswered call.
    let answered: std::collections::HashSet<&str> = input
        .iter()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .filter_map(|v| v.get("call_id").and_then(Value::as_str))
        .collect();
    let orphan_ids: Vec<String> = input
        .iter()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("function_call"))
        .filter_map(|v| v.get("call_id").and_then(Value::as_str))
        .filter(|id| !answered.contains(*id))
        .map(|id| id.to_string())
        .collect();
    for id in orphan_ids {
        log::warn!(target: "gray_provider", "synthesizing missing tool output for orphaned function call {id}");
        input.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": id,
            "output": "[no tool output — call was interrupted]",
        }));
    }
    let tools: Vec<ResponsesTool> = filter_valid_tools(req.tools)
        .into_iter()
        .map(|t| ResponsesTool {
            tool_type: "function".to_string(),
            name: t.name,
            description: t.description,
            parameters: t.parameters,
        })
        .collect();
    let reasoning = map_responses_reasoning(reasoning_effort);
    // Encrypted-content include rides with reasoning (nothing to replay
    // without it; omitted entirely when reasoning is off).
    let include = reasoning
        .as_ref()
        .map(|_| vec!["reasoning.encrypted_content".to_string()]);
    ResponsesRequest {
        model: model.to_string(),
        instructions,
        input,
        tools,
        stream: true,
        prompt_cache_key: session_id.map(str::to_string),
        store: false,
        reasoning,
        include,
    }
}

/// `reasoning: {effort, summary: "auto"}` for the Responses API.
/// `None`/missing effort omits the field (backend default); `"off"` omits it
/// too (display is suppressed separately via hide_thinking). `"max"` is not a
/// Responses-API value — closest is `"xhigh"`.
fn map_responses_reasoning(reasoning_effort: Option<&str>) -> Option<Value> {
    match reasoning_effort {
        None | Some("off") => None,
        Some("max") => Some(serde_json::json!({"effort": "xhigh", "summary": "auto"})),
        Some(eff) => Some(serde_json::json!({"effort": eff, "summary": "auto"})),
    }
}

pub(crate) fn classify_http_error(
    status: reqwest::StatusCode,
    snippet: &str,
    cf_ray: Option<&str>,
    req_id: Option<&str>,
) -> ProviderError {
    let mut msg = if snippet.is_empty() {
        format!("status {status}")
    } else {
        format!("status {status}: {snippet}")
    };
    if let Some(ray) = cf_ray {
        msg.push_str(&format!(", cf-ray: {ray}"));
    }
    if let Some(rid) = req_id {
        msg.push_str(&format!(", request-id: {rid}"));
    }
    let lower = snippet.to_lowercase();
    let is_unsupported = lower.contains("not supported")
        || lower.contains("unsupported")
        || lower.contains("model not found")
        || lower.contains("unknown model");
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(msg),
        429 => ProviderError::RateLimited(msg),
        400 | 404 => ProviderError::BadRequest(msg),
        500..=599 if is_unsupported => ProviderError::BadRequest(msg),
        500..=599 => ProviderError::ServerError(msg),
        _ => ProviderError::Stream(msg),
    }
}

pub(crate) fn is_retryable_error(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::RateLimited(_)
            | ProviderError::ServerError(_)
            | ProviderError::Stream(_)
            | ProviderError::Timeout(_)
            | ProviderError::Connection(_)
    )
}

/// Codex steal (`notify_stream_error`): `Reconnecting... n/m` + short cause.
/// Details are capped so a multi-KB upstream blob never reaches the transcript.
pub(crate) fn retry_notice_event(attempt: usize, max: usize, err: &ProviderError) -> StreamEvent {
    let details: String = err.to_string().chars().take(200).collect();
    StreamEvent::stream_error(format!("Reconnecting... {attempt}/{max}"), details)
}

/// Exponential backoff with jitter, matching the existing retry cadence.
fn backoff_delay(initial: Duration, attempt: usize) -> Duration {
    let exp_factor = 1u64 << (attempt.saturating_sub(1));
    let backoff_ms = (initial.as_millis() as u64).saturating_mul(exp_factor);
    let max_jitter = backoff_ms / 2;
    let jitter_ms = if max_jitter > 0 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        nanos % (max_jitter + 1)
    } else {
        0
    };
    Duration::from_millis(backoff_ms + jitter_ms)
}

/// Single POST attempt (no retry). Retry + `Reconnecting...` notices live in
/// `stream_unfold_step` so each attempt surfaces to the UI like Codex's
/// `notify_stream_error` instead of stalling silently in a sleep loop.
async fn send_json_once(
    client: &reqwest::Client,
    url: &Url,
    api_key: &str,
    body: &Value,
    attempt: usize,
) -> Result<reqwest::Response, ProviderError> {
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
    match res_result {
        Ok(res) => {
            let status = res.status();
            if status.is_success() {
                return Ok(res);
            }
            let cf_ray = res
                .headers()
                .get("cf-ray")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let req_id = res
                .headers()
                .get("x-request-id")
                .or_else(|| res.headers().get("request-id"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(500).collect();
            Err(classify_http_error(status, &snippet, cf_ray.as_deref(), req_id.as_deref()))
        }
        Err(e) => Err(if e.is_connect() {
            ProviderError::Connection(e.to_string())
        } else if e.is_timeout() {
            ProviderError::Timeout(e.to_string())
        } else {
            ProviderError::Stream(e.to_string())
        }),
    }
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
        attempt: usize,
    },
    ResponsesInit {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        initial_backoff: Duration,
        attempt: usize,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, (String, String, String)>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,
    },
    ResponsesStreaming {
        event_stream: BoxedEventStream,
        // keyed by call_id -> (index, name, args)
        tools_by_call_id: BTreeMap<String, (usize, String, String)>,
        // index -> call_id for ordering at completion
        index_to_call_id: BTreeMap<usize, String>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,
    },
    Done,
}

fn normalize_tool_args(args: &str, index: usize) -> Result<String, ProviderError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return Ok(trimmed.to_string());
    }
    let repaired = if trimmed.starts_with('{') && !trimmed.ends_with('}') {
        format!("{trimmed}}}")
    } else {
        trimmed.to_string()
    };
    if serde_json::from_str::<Value>(&repaired).is_ok() {
        return Ok(repaired);
    }
    log::warn!(target: "gray_provider", "tool call {index} malformed args: {}", args.chars().take(300).collect::<String>());
    Err(ProviderError::Stream(format!(
        "tool call at index {index} has malformed JSON arguments: {}",
        args.chars().take(500).collect::<String>()
    )))
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
        let args_fixed = normalize_tool_args(&args, index)?;
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index,
            id: if id.is_empty() { None } else { Some(id) },
            name: if name.is_empty() { None } else { Some(name) },
            arguments_delta: args_fixed,
        });
    }

    pending_events.push_back(StreamEvent::MessageComplete {
        stop_reason,
        usage,
    });
    Ok(())
}

fn emit_responses_tool_calls_and_completion(
    tools_by_call_id: &mut BTreeMap<String, (usize, String, String)>,
    index_to_call_id: &mut BTreeMap<usize, String>,
    usage: Option<Usage>,
    pending_events: &mut VecDeque<StreamEvent>,
) -> Result<(), ProviderError> {
    // Emit in index order for deterministic tool ordering
    let mut ordered: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for (call_id, (idx, name, args)) in std::mem::take(tools_by_call_id) {
        ordered.insert(idx, (call_id, name, args));
    }
    index_to_call_id.clear();
    for (idx, (call_id, name, args)) in ordered {
        let args_fixed = normalize_tool_args(&args, idx)?;
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index: idx,
            id: if call_id.is_empty() { None } else { Some(call_id) },
            name: if name.is_empty() { None } else { Some(name) },
            arguments_delta: args_fixed,
        });
    }
    let stop_reason = if pending_events.iter().any(|e| matches!(e, StreamEvent::ToolCallDelta { .. })) {
        Some(StopReason::ToolUse)
    } else {
        Some(StopReason::EndTurn)
    };
    pending_events.push_back(StreamEvent::MessageComplete { stop_reason, usage });
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
                    attempt,
                } => {
                    // Backoff for attempts 2+ runs here so the previous
                    // `Reconnecting...` notice is already on screen (Codex:
                    // notify then sleep, not sleep then notify).
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(initial_backoff, attempt - 1)).await;
                    }
                    let body_value =
                        serde_json::to_value(&body).expect("OpenAiChatRequest serialization");
                    match send_json_once(&client, &url, &api_key, &body_value, attempt).await {
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
                            if is_retryable_error(&err) && attempt < MAX_ATTEMPTS {
                                log::warn!(target: "gray_provider", "retrying (attempt {attempt}) after error: {err}");
                                let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                };
                                return Some((Ok(notice), next));
                            }
                            log::error!(target: "gray_provider", "stream request failed: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    }
                }
                StreamState::ResponsesInit { client, url, api_key, body, initial_backoff, attempt } => {
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(initial_backoff, attempt - 1)).await;
                    }
                    let body_value =
                        serde_json::to_value(&body).expect("ResponsesRequest serialization");
                    match send_json_once(&client, &url, &api_key, &body_value, attempt).await {
                        Ok(response) => {
                            let event_stream: BoxedEventStream = response.bytes_stream().eventsource().boxed();
                            state = StreamState::ResponsesStreaming { event_stream, tools_by_call_id: BTreeMap::new(), index_to_call_id: BTreeMap::new(), last_usage: None, pending_events: VecDeque::new(), completed: false };
                        }
                        Err(err) => {
                            if is_retryable_error(&err) && attempt < MAX_ATTEMPTS {
                                log::warn!(target: "gray_provider", "retrying responses (attempt {attempt}) after error: {err}");
                                let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                let next = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                };
                                return Some((Ok(notice), next));
                            }
                            log::error!(target: "gray_provider", "responses request failed: {err}");
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
                                                .or(choice.delta.reasoning)
                                                .or(choice.delta.thought)
                                                .or(choice.delta.thoughts);
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
                                                    // cap wire-controlled indices so a broken/
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
                StreamState::ResponsesStreaming {
                    mut event_stream,
                    mut tools_by_call_id,
                    mut index_to_call_id,
                    mut last_usage,
                    mut pending_events,
                    mut completed,
                } => {
                    if let Some(event) = pending_events.pop_front() {
                        return Some((
                            Ok(event),
                            StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
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
                            if data.is_empty() {
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    last_usage,
                                    pending_events,
                                    completed,
                                };
                                continue;
                            }
                            let value: Value = match serde_json::from_str(data) {
                                Ok(v) => v,
                                Err(e) => {
                                    return Some((
                                        Err(ProviderError::Stream(format!("failed to parse Responses SSE chunk: {e}: {data}"))),
                                        StreamState::Done,
                                    ));
                                }
                            };
                            let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match typ {
                                "response.output_text.delta" => {
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                                        if !delta.is_empty() {
                                            pending_events.push_back(StreamEvent::TextDelta { delta: delta.to_string() });
                                        }
                                    }
                                }
                                "response.reasoning_text.delta"
                                | "response.reasoning_summary_text.delta" => {
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                                        if !delta.is_empty() {
                                            pending_events.push_back(StreamEvent::ThinkingDelta { delta: delta.to_string() });
                                        }
                                    }
                                }
                                "response.output_item.added" => {
                                    if let Some(item) = value.get("item") {
                                        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                        if item_type == "function_call" {
                                            let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let item_id = item.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| call_id.clone());
                                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            if !call_id.is_empty() && !tools_by_call_id.contains_key(&call_id) {
                                                let idx = tools_by_call_id.len();
                                                tools_by_call_id.insert(call_id.clone(), (idx, name.clone(), String::new()));
                                                index_to_call_id.insert(idx, call_id.clone());
                                                if item_id != call_id && !item_id.is_empty() {
                                                    tools_by_call_id.insert(item_id.clone(), (idx, name, String::new()));
                                                }
                                            }
                                        }
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    let item_id = value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
                                    let delta = value.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                                    if !delta.is_empty() && !item_id.is_empty() {
                                        if let Some(entry) = tools_by_call_id.get_mut(item_id) {
                                            entry.2.push_str(delta);
                                        } else {
                                            let idx = tools_by_call_id.len();
                                            tools_by_call_id.insert(item_id.to_string(), (idx, String::new(), delta.to_string()));
                                            index_to_call_id.insert(idx, item_id.to_string());
                                        }
                                    }
                                }
                                "response.function_call_arguments.done" => {
                                    if let Some(args) = value.get("arguments").and_then(|v| v.as_str()) {
                                        let item_id = value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
                                        if !item_id.is_empty() {
                                            if let Some(entry) = tools_by_call_id.get_mut(item_id) {
                                                if entry.2.is_empty() { entry.2 = args.to_string(); }
                                            }
                                        }
                                        if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                                            if let Some(entry) = tools_by_call_id.get_mut(item_id) {
                                                if entry.1.is_empty() { entry.1 = name.to_string(); }
                                            }
                                        }
                                    }
                                }
                                "response.completed" => {
                                    if !completed {
                                        completed = true;
                                        let usage_val = value.get("response").and_then(|r| r.get("usage")).or_else(|| value.get("usage"));
                                        if let Some(uval) = usage_val {
                                            if let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(uval.clone()) {
                                                last_usage = Some(map_usage(&u));
                                            } else {
                                                let input = uval.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                                let output = uval.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                                let cached = uval.get("input_tokens_details").and_then(|d| d.get("cached_tokens")).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                                let reasoning = uval.get("output_tokens_details").and_then(|d| d.get("reasoning_tokens")).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                                let total = uval.get("total_tokens").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(input + output);
                                                let mut usage = Usage { input_tokens: input, output_tokens: output, reasoning_tokens: reasoning, cached_tokens: cached, non_cached_input_tokens: input.saturating_sub(cached), cache_read_input_tokens: cached, cache_write_input_tokens: 0, total_tokens: total };
                                                usage.normalize();
                                                last_usage = Some(usage);
                                            }
                                        }
                                        let mut dedup: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
                                        for (k, (idx, name, args)) in std::mem::take(&mut tools_by_call_id) {
                                            let entry = dedup.entry(idx).or_insert((k.clone(), String::new(), String::new()));
                                            if !name.is_empty() { entry.1 = name; }
                                            if !args.is_empty() { entry.2 = args.clone(); }
                                            if entry.0.is_empty() || k.starts_with("call_") { entry.0 = k; }
                                        }
                                        for (idx, (call_id, name, args)) in dedup {
                                            tools_by_call_id.insert(call_id.clone(), (idx, name, args));
                                        }
                                        index_to_call_id.clear();
                                        for (call_id, (idx, _, _)) in &tools_by_call_id {
                                            index_to_call_id.insert(*idx, call_id.clone());
                                        }
                                        if let Err(err) = emit_responses_tool_calls_and_completion(&mut tools_by_call_id, &mut index_to_call_id, last_usage.clone(), &mut pending_events) {
                                            return Some((Err(err), StreamState::Done));
                                        }
                                    }
                                }
                                "response.output_item.done" => {
                                    // Capture completed reasoning items (id +
                                    // encrypted blob) for verbatim replay next
                                    // turn. Other item types need no action.
                                    if value.get("item").and_then(|i| i.get("type")).and_then(|t| t.as_str()) == Some("reasoning") {
                                        let item = &value["item"];
                                        let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                                        let ec = item.get("encrypted_content").and_then(|v| v.as_str()).unwrap_or_default();
                                        if !id.is_empty() && !ec.is_empty() {
                                            pending_events.push_back(StreamEvent::reasoning_item(id, ec));
                                        }
                                    }
                                }
                                "ping" | "response.created" | "response.in_progress" | "response.content_part.added" | "response.content_part.done" | "response.output_text.done" | "response.reasoning_text.done" => {}
                                _ => {
                                    if let Some(uval) = value.get("response").and_then(|r| r.get("usage")).or_else(|| value.get("usage")) {
                                        if let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(uval.clone()) {
                                            last_usage = Some(map_usage(&u));
                                        }
                                    }
                                }
                            }
                            state = StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
                                last_usage,
                                pending_events,
                                completed,
                            };
                        }
                        Some(Err(err)) => {
                            log::error!(target: "gray_provider", "responses stream error: {err}");
                            return Some((Err(ProviderError::Stream(err.to_string())), StreamState::Done));
                        }
                        None => {
                            if !completed {
                                completed = true;
                                let mut dedup: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
                                for (k, (idx, name, args)) in std::mem::take(&mut tools_by_call_id) {
                                    if !dedup.contains_key(&idx) { dedup.insert(idx, (k, name, args)); }
                                }
                                for (idx, (call_id, name, args)) in dedup { tools_by_call_id.insert(call_id.clone(), (idx, name, args)); }
                                index_to_call_id.clear();
                                for (call_id, (idx, _, _)) in &tools_by_call_id { index_to_call_id.insert(*idx, call_id.clone()); }
                                if let Err(err) = emit_responses_tool_calls_and_completion(&mut tools_by_call_id, &mut index_to_call_id, last_usage.clone(), &mut pending_events) {
                                    return Some((Err(err), StreamState::Done));
                                }
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
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
    fn model_id(&self) -> &str {
        &self.model
    }

    fn stream(
        &self,
        req: ChatRequest,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        if is_muse_model(&self.model) && self.base_url.as_str().contains("opencode.ai/zen") {
            let url = match responses_url(&self.base_url) {
                Ok(u) => u,
                Err(e) => return stream::once(async move { Err(e) }).boxed(),
            };
            let body = map_chat_to_responses(req, &self.model, self.session_id.as_deref(), self.reasoning_effort.as_deref());
            log::debug!(target: "gray_provider", "using Responses API for model {}", self.model);
            let init_state = StreamState::ResponsesInit {
                client: self.http.clone(),
                url,
                api_key: self.api_key.clone(),
                body,
                initial_backoff: self.initial_backoff,
                attempt: 1,
            };
            return stream::unfold(init_state, stream_unfold_step).boxed();
        }
        let url = match chat_completions_url(&self.base_url) {
            Ok(u) => u,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };

        let body = map_chat_request(req, &self.model, self.reasoning_effort.as_deref());
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            initial_backoff: self.initial_backoff,
            attempt: 1,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "rate limit", None, None);
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
        let err = classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "internal error", None, None);
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

    fn responses_req_with_thinking(model: &str) -> ChatRequest {
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
        let body = map_chat_to_responses(responses_req_with_thinking("m1"), "m1", Some("sess"), Some("high"));
        let v = serde_json::to_value(&body).expect("serializes");
        // encrypted-content include rides with reasoning
        let include = v.get("include").and_then(|i| i.as_array()).expect("include");
        assert!(include.iter().any(|s| s.as_str() == Some("reasoning.encrypted_content")));
        assert!(v.get("reasoning").and_then(|r| r.get("summary")).and_then(|s| s.as_str()) == Some("auto"));
        // same-model reasoning item replayed verbatim ahead of text
        let input = v.get("input").and_then(|i| i.as_array()).expect("input");
        let reason = input.iter().find(|i| i.get("type").and_then(|t| t.as_str()) == Some("reasoning")).expect("reasoning item");
        assert_eq!(reason.get("id").and_then(|v| v.as_str()), Some("rs_1"));
        assert_eq!(reason.get("encrypted_content").and_then(|v| v.as_str()), Some("blob"));
    }

    #[test]
    fn responses_replay_drops_foreign_model_thinking() {
        let body = map_chat_to_responses(responses_req_with_thinking("m1"), "m2", Some("sess"), Some("high"));
        let v = serde_json::to_value(&body).expect("serializes");
        let input = v.get("input").and_then(|i| i.as_array()).expect("input");
        assert!(input.iter().all(|i| i.get("type").and_then(|t| t.as_str()) != Some("reasoning")));
        // prose still sent
        assert!(input.iter().any(|i| i.get("role").and_then(|r| r.as_str()) == Some("assistant")));
    }

    #[test]
    fn responses_include_omitted_when_reasoning_off() {
        let body = map_chat_to_responses(responses_req_with_thinking("m1"), "m1", Some("sess"), Some("off"));
        let v = serde_json::to_value(&body).expect("serializes");
        assert!(v.get("include").is_none());
        assert!(v.get("reasoning").is_none());
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
        assert_eq!(usage.non_cached_input_tokens, 150, "miss preferred: {usage:?}");
        assert_eq!(usage.cache_read_input_tokens, 800, "read kept: {usage:?}");
    }
}

