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
///
/// `Debug` is redacted by hand: the struct carries a plaintext API key,
/// so the derived impl would leak it into any log that formats the provider.
#[derive(Clone)]
pub struct OpenAiProvider {
    base_url: Url,
    api_key: String,
    model: String,
    http: reqwest::Client,
    initial_backoff: Duration,
    reasoning_effort: Option<String>,
    /// Stable per-process id sent as `prompt_cache_key` (Responses API and
    /// chat completions alike) so callers pin one cache shard per session for
    /// prompt caching. Also sent as the `x-opencode-session` header (Console Go
    /// routes on it; required).
    session_id: Option<String>,
    /// Pre-stream POST retry bound (replaces the single `MAX_ATTEMPTS` gate in
    /// `Init`/`ResponsesInit`). Default 3 reproduces today's behavior.
    request_max_retries: usize,
    /// Mid-stream retry bound (Responses resume arms; chat path has no
    /// resume primitive and stays terminal).
    stream_max_retries: usize,
    /// Per-SSE-event idle deadline. `None` (default) leaves the 120s
    /// reqwest `read_timeout` governing. When set, a stalled Responses event
    /// poll resumes with `previous_response_id`; exhausted stays terminal.
    stream_idle_timeout: Option<Duration>,
}

/// Builder for constructing an `OpenAiProvider`.
///
/// `Debug` is redacted by hand (see [`OpenAiProvider`]).
#[derive(Clone)]
pub struct OpenAiProviderBuilder {
    base_url: Option<String>,
    api_key: String,
    model: String,
    reasoning_effort: Option<String>,
    session_id: Option<String>,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for OpenAiProviderBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProviderBuilder")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl OpenAiProviderBuilder {
    /// Creates a new builder with the given API key and model name.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: None,
            api_key: api_key.into(),
            model: model.into(),
            reasoning_effort: None,
            session_id: None,
        }
    }

    /// Sets a stable session id sent as `prompt_cache_key` on the Responses
    /// API and chat completions alike so consecutive requests hit the same
    /// cache shard.
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Sets the base URL for the OpenAI-compatible API endpoint.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
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

        // default client with a 120s idle-read timeout so a stalled server
        // (finish_reason then silence, hung proxy) can't freeze a turn
        // forever. Total timeout stays off: long generations are legal.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client with timeouts");

        Ok(OpenAiProvider {
            base_url,
            api_key: self.api_key,
            model: self.model,
            http,
            initial_backoff: Duration::from_millis(50),
            reasoning_effort: self.reasoning_effort,
            session_id: self.session_id,
            request_max_retries: MAX_ATTEMPTS,
            stream_max_retries: MAX_ATTEMPTS,
            stream_idle_timeout: None,
        })
    }
}

impl OpenAiProvider {
    /// Returns a builder to configure and construct an `OpenAiProvider`.
    pub fn builder(api_key: impl Into<String>, model: impl Into<String>) -> OpenAiProviderBuilder {
        OpenAiProviderBuilder::new(api_key, model)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiChatRequest {
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
    /// Cache-shard affinity (the session id goes here, mirroring the
    /// Responses `prompt_cache_key`); without it chat turns rotate shards.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
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
    #[serde(
        default,
        alias = "prompt_cache_hit_tokens",
        alias = "promptCacheHitTokens"
    )]
    cached_tokens: usize,
    /// DeepSeek explicit miss count — preferred over prompt-minus-cached
    #[serde(default, alias = "promptCacheMissTokens")]
    prompt_cache_miss_tokens: usize,

    /// Anthropic native breakdown (when not via OpenAI compat)
    #[serde(
        default,
        alias = "cache_creation_input_tokens",
        alias = "cacheCreationInputTokens"
    )]
    cache_creation_input_tokens: usize,
    #[serde(
        default,
        alias = "cache_read_input_tokens",
        alias = "cacheReadInputTokens"
    )]
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
    #[serde(
        default,
        alias = "cache_creation_tokens",
        alias = "cacheCreationTokens"
    )]
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

fn is_deepseek_model(model: &str) -> bool {
    model.to_lowercase().contains("deepseek")
}

/// Wire encoding for a tool result: neither the chat `tool` message nor the
/// Responses `function_call_output` item has an `is_error` slot, so an error
/// result is prefixed (model-visible, strict-provider-safe — no unknown
/// fields). Success content passes through byte-identical.
fn wire_tool_output(content: &str, is_error: bool) -> String {
    if is_error {
        format!("Error: {content}")
    } else {
        content.to_string()
    }
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
    tools
        .into_iter()
        .filter(|t| {
            if t.name.trim().is_empty() {
                log::warn!(target: "gray_provider", "dropping tool def with empty name");
                false
            } else {
                true
            }
        })
        .collect()
}

fn is_valid_tool_name(name: &str, id: &str) -> bool {
    if name.trim().is_empty() {
        log::warn!(target: "gray_provider", "dropping assistant tool call {id} with empty name");
        false
    } else {
        true
    }
}

fn map_chat_request(
    req: ChatRequest,
    model: &str,
    reasoning_effort: Option<&str>,
) -> Result<OpenAiChatRequest, ProviderError> {
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
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => {
                            tool_results.push((id, content, is_error));
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
                    // DeepSeek expects the reasoning field on every assistant
                    // message: send it (possibly empty) for deepseek models,
                    // omit it elsewhere.
                    is_deepseek_model(model).then(String::new)
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

                for (id, content, is_error) in tool_results {
                    messages.push(OpenAiMessageRequest {
                        role: "tool".to_string(),
                        content: Some(Value::String(wire_tool_output(&content, is_error))),
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
                    // A novel wire role landing in this arm (outer pattern
                    // extended without updating this map) errors instead of
                    // panicking the turn.
                    other => {
                        return Err(ProviderError::BadRequest(format!(
                            "unsupported message role for chat completions: {other}"
                        )));
                    }
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
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => {
                            tool_results.push((id, wire_tool_output(&content, is_error)));
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
                            arr.push(
                                serde_json::json!({"type":"image_url","image_url":{"url": url}}),
                            );
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

    Ok(OpenAiChatRequest {
        model: model.to_string(),
        stream: true,
        stream_options: Some(OpenAiStreamOptions {
            include_usage: true,
        }),
        reasoning_effort: reasoning_effort_val,
        reasoning: reasoning_val,
        thinking: thinking_val,
        messages,
        tools,
        prompt_cache_key: None,
    })
}

/// `serde_json::to_value` on the send hot path: a failure is a client-side
/// bug, surfaced as `BadRequest` instead of panicking the turn.
fn serialize_body<T: Serialize>(body: &T, what: &'static str) -> Result<Value, ProviderError> {
    serde_json::to_value(body)
        .map_err(|e| ProviderError::BadRequest(format!("failed to serialize {what}: {e}")))
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
        // miss count — prefer it over subtraction.
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
pub(crate) struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ResponsesTool>,
    stream: bool,
    /// Cache-shard affinity (the session id goes here).
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    /// Server-side retention off: with store enabled the backend
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
    /// Resume a previously started response after a retryable mid-stream
    /// failure (Codex parity): the server continues the prefix instead of
    /// gray replaying the turn. A strict backend may 400 on the unknown id
    /// (`previous_response_not_found`) — that strips the id and replays once
    /// (see `should_retry_without_previous_response`) — or ignore the id and
    /// replay from scratch (duplicating already-yielded text).
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
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
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => {
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            let output = wire_tool_output(&content, is_error);
                            input.push(serde_json::json!({"type":"function_call_output","call_id": id, "output": output}));
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
                    input.push(
                        serde_json::json!({"role": role_str, "content": text_parts.join("\n")}),
                    );
                }
            }
            Role::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut reasoning_items: Vec<Value> = Vec::new();
                let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
                let mut tool_results: Vec<(String, String, bool)> = Vec::new();
                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Thinking {
                            encrypted_content: Some(ec),
                            item_id: Some(id),
                            model: Some(m),
                            ..
                        } if m == model => {
                            // Replay the raw reasoning item verbatim so the
                            // server keeps its cache shard warm (pi_agent_rust
                            // parity). Same-model only: a foreign model cannot
                            // decrypt the blob and strict servers 400 on it.
                            reasoning_items.push(serde_json::json!({"type": "reasoning", "id": id, "summary": [], "encrypted_content": ec}));
                        }
                        ContentBlock::Thinking { .. } => {}
                        ContentBlock::ToolUse { id, name, args } => {
                            tool_uses.push((id, name, args))
                        }
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => tool_results.push((id, content, is_error)),
                    }
                }
                for item in reasoning_items {
                    input.push(item);
                }
                if !text_parts.is_empty() {
                    input.push(
                        serde_json::json!({"role":"assistant","content": text_parts.join("\n")}),
                    );
                }
                for (id, name, args) in tool_uses {
                    if !is_valid_tool_name(&name, &id) {
                        continue;
                    }
                    input.push(serde_json::json!({"type":"function_call","call_id": id, "name": name, "arguments": args.to_string()}));
                }
                for (id, content, is_error) in tool_results {
                    let output = wire_tool_output(&content, is_error);
                    input.push(serde_json::json!({"type":"function_call_output","call_id": id, "output": output}));
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
        previous_response_id: None,
    }
}

/// `reasoning: {effort, summary: "auto"}` for the Responses API.
/// `None`/missing effort omits the field (backend default); `"off"` omits it
/// too (display is suppressed separately via hide_thinking). `max` is a
/// documented Responses-API value on newer models (e.g. GPT-5.6) — forward it
/// verbatim; a model that rejects it 400s and the strip-and-retry path drops
/// reasoning.
/// https://developers.openai.com/api/docs/guides/reasoning
fn map_responses_reasoning(reasoning_effort: Option<&str>) -> Option<Value> {
    match reasoning_effort {
        None | Some("off") => None,
        Some(eff) => Some(serde_json::json!({"effort": eff, "summary": "auto"})),
    }
}

/// True when a chat-completions body carries any reasoning wire params
/// (`reasoning_effort` / `reasoning` / `thinking`). Pure: drives the
/// strip-and-retry decision without touching the network.
pub(crate) fn chat_has_reasoning_params(body: &OpenAiChatRequest) -> bool {
    body.reasoning_effort.is_some() || body.reasoning.is_some() || body.thinking.is_some()
}

/// Drops all three reasoning wire params so a provider that rejects them
/// (e.g. glm-5.2 400ing `Extra inputs are not permitted, field: 'reasoning'`,
/// or a thinking-only model 400ing `thinking: disabled`) falls back to its
/// defaults. The requested effort label is untouched (no cosmetic switch).
pub(crate) fn strip_chat_reasoning_params(body: &mut OpenAiChatRequest) {
    body.reasoning_effort = None;
    body.reasoning = None;
    body.thinking = None;
}

/// True when a Responses body carries reasoning (`include` rides with it).
pub(crate) fn responses_has_reasoning(body: &ResponsesRequest) -> bool {
    body.reasoning.is_some()
}

/// Drops Responses reasoning + its encrypted-content include.
pub(crate) fn strip_responses_reasoning(body: &mut ResponsesRequest) {
    body.reasoning = None;
    body.include = None;
}

/// Retry predicate for a rejected resume id: the server 400/404s naming
/// `previous_response` because the id is unknown/expired (restart, eviction,
/// `store: false` backends that never kept it). With an id actually sent,
/// retry ONCE with it stripped (true full replay). Pure, no network.
pub(crate) fn should_retry_without_previous_response(
    status: u16,
    snippet: &str,
    had_previous_response_id: bool,
) -> bool {
    if (status != 400 && status != 404) || !had_previous_response_id {
        return false;
    }
    let lower = snippet.to_lowercase();
    if !lower.contains("previous_response") && !lower.contains("previous response") {
        return false;
    }
    const NOT_FOUND_HINTS: [&str; 7] = [
        "not_found",
        "not found",
        "unknown",
        "expired",
        "invalid",
        "no longer",
        "does not exist",
    ];
    NOT_FOUND_HINTS.iter().any(|h| lower.contains(h))
}

/// Retry predicate for the glm-5.2 catch-22: at max/low the wire `reasoning`
/// field 400s (`Extra inputs are not permitted`); at off the `thinking:
/// disabled` 400s on a thinking-only model. On a 400 naming reasoning params
/// with params actually sent, retry ONCE with them omitted. Pure, no network.
pub(crate) fn should_retry_without_reasoning(
    status: u16,
    snippet: &str,
    had_reasoning_params: bool,
) -> bool {
    if status != 400 || !had_reasoning_params {
        return false;
    }
    let lower = snippet.to_lowercase();
    let names_reasoning = lower.contains("reasoning")
        || lower.contains("thinking")
        || lower.contains("reasoning_effort")
        || lower.contains("effort");
    if !names_reasoning {
        return false;
    }
    const REJECTION_HINTS: [&str; 13] = [
        "extra",
        "permitted",
        "not allowed",
        "unsupported",
        "unknown",
        "unexpected",
        "additional",
        "unrecognized",
        "invalid",
        "only",
        "required",
        "must",
        "not permitted",
    ];
    REJECTION_HINTS.iter().any(|h| lower.contains(h))
}

/// Clear terminal error naming the conflict (model + rejected field + fix).
/// Appends to the classified message so status/cf-ray/request-id survive.
pub(crate) fn reasoning_conflict_hint(model: &str, snippet: &str) -> String {
    format!(
        "{snippet} [model '{model}' rejected reasoning params — try /thinking off (or another effort); no setting was changed]"
    )
}

/// Wraps a terminal `BadRequest` that names reasoning params with the
/// conflict hint. Non-reasoning errors pass through untouched.
pub(crate) fn maybe_annotate_reasoning_conflict(err: ProviderError, model: &str) -> ProviderError {
    match err {
        ProviderError::BadRequest(msg) => {
            let lower = msg.to_lowercase();
            let names = lower.contains("reasoning")
                || lower.contains("thinking")
                || lower.contains("effort");
            if names {
                ProviderError::BadRequest(reasoning_conflict_hint(model, &msg))
            } else {
                ProviderError::BadRequest(msg)
            }
        }
        other => other,
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
    // Taxonomy lite: context exhaustion compacts + retries once (never retried
    // blindly); content filters are terminal bad requests. Narrowed to avoid
    // false positives: `max_tokens` needs a context qualifier, bare
    // violates/flagged/safety are not filters.
    const CONTEXT_OVERFLOW_HINTS: [&str; 4] = [
        "context length",
        "context window",
        "too many tokens",
        "prompt is too long",
    ];
    let max_tokens_qualified = lower.contains("max_tokens")
        && (lower.contains("context") || lower.contains("too long") || lower.contains("exceeded"));
    if CONTEXT_OVERFLOW_HINTS.iter().any(|h| lower.contains(h)) || max_tokens_qualified {
        return ProviderError::ContextOverflow(format!(
            "context exhausted — start /new or compact ({msg})"
        ));
    }
    const CONTENT_FILTER_HINTS: [&str; 3] = ["content_filter", "content-filter", "usage policies"];
    if CONTENT_FILTER_HINTS.iter().any(|h| lower.contains(h)) {
        return ProviderError::BadRequest(msg);
    }
    let is_unsupported = lower.contains("not supported")
        || lower.contains("unsupported")
        || lower.contains("model not found")
        || lower.contains("unknown model");
    // Billing/quota exhaustion must surface immediately: re-hitting an
    // exhausted balance burns the full request body against zero quota (and
    // the 429 floor would stall the turn for nothing). Narrow phrases only —
    // bare "billing"/"plan" also appear in legit rate-limit upsell copy.
    const QUOTA_EXHAUSTED_HINTS: [&str; 10] = [
        "insufficient_quota",
        "insufficient quota",
        "insufficient credits",
        "insufficient balance",
        "exceeded your current quota",
        "exceeds your allocated quota",
        "quota exceeded",
        "quota_exceeded",
        "gousagelimiterror",
        "available balance",
    ];
    let quota_exhausted = QUOTA_EXHAUSTED_HINTS.iter().any(|h| lower.contains(h));
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(msg),
        // 402 is definitionally billing; a 429 naming quota exhaustion is not
        // a rate limit. Both surface immediately (Auth is terminal).
        402 => ProviderError::Auth(msg),
        429 if quota_exhausted => ProviderError::Auth(msg),
        429 => ProviderError::RateLimited(msg),
        // 413 is a too-large payload, not a blip: compact once via the
        // ContextOverflow path, never generic-retry.
        413 => ProviderError::ContextOverflow(format!(
            "context exhausted — start /new or compact ({msg})"
        )),
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

/// Server-asked delay cap: a `Retry-After: 3600` must not park a turn for an
/// hour (Esc still aborts via stream-drop, but drains without a cancel branch
/// would stall out the full delay).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// `Retry-After` (delay-seconds or HTTP-date) plus the de-facto
/// `retry-after-ms` millis header; server-asked delays are capped at 60s.
/// Garbage and absent headers stay `None` (callers fall back to the
/// synthesized floor).
pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Some(MAX_RETRY_AFTER.min(Duration::from_millis(ms)));
    }
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(MAX_RETRY_AFTER.min(Duration::from_secs(secs)));
    }
    parse_http_date_delay(raw).map(|d| d.min(MAX_RETRY_AFTER))
}

/// IMF-fixdate `Retry-After` (`Sun, 06 Nov 1994 08:49:37 GMT`) as a delay
/// from now; past dates are zero (retry now). Anything else is `None`.
fn parse_http_date_delay(raw: &str) -> Option<Duration> {
    let rest = raw
        .strip_suffix(" GMT")
        .or_else(|| raw.strip_suffix(" UTC"))?;
    let date = rest.split_once(", ")?.1;
    let mut parts = date.split_whitespace();
    let day: u64 = parts.next()?.parse().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }
    let month: i64 = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let clock = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let mut clock_parts = clock.split(':');
    let hour: u64 = clock_parts.next()?.parse().ok()?;
    let min: u64 = clock_parts.next()?.parse().ok()?;
    let sec: u64 = clock_parts.next()?.parse().ok()?;
    if clock_parts.next().is_some() || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    // Days-from-civil (Hinnant): one header form doesn't justify a date dep.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let target = days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Duration::from_secs((target - now).max(0) as u64))
}

/// Exponential backoff with jitter, floored by `Retry-After` when present.
fn backoff_delay(initial: Duration, attempt: usize, retry_after: Option<Duration>) -> Duration {
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
    let base = Duration::from_millis(backoff_ms + jitter_ms);
    retry_after.map(|floor| base.max(floor)).unwrap_or(base)
}

/// 429 floor when the server sends no `Retry-After`: re-hitting an exhausted
/// quota ~1s later re-burns the full request body against zero quota.
/// Floor of several seconds with exponential growth per failed attempt; a
/// present `Retry-After` still wins via `backoff_delay`'s max.
fn rate_limit_floor(failed_attempt: usize) -> Duration {
    Duration::from_secs(5 * (1u64 << failed_attempt.saturating_sub(1).min(6)))
}

/// 429-aware retry floor: keep the server's `Retry-After` when present, else
/// synthesize the escalating 429 floor. Non-429 errors pass through untouched
/// (their ~50ms+jitter backoff is correct for blips).
fn retry_floor(
    err: &ProviderError,
    floor: Option<Duration>,
    failed_attempt: usize,
) -> Option<Duration> {
    if floor.is_none() && matches!(err, ProviderError::RateLimited(_)) {
        return Some(rate_limit_floor(failed_attempt));
    }
    floor
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
    session_id: Option<&str>,
) -> Result<reqwest::Response, (ProviderError, Option<Duration>, u16)> {
    let base = if api_key.is_empty() {
        client.post(url.clone())
    } else {
        client
            .post(url.clone())
            .header("Authorization", format!("Bearer {api_key}"))
    };
    // Console Go (opencode.ai/zen) routes on this; without it inference
    // 400s MissingSessionID. Unknown `x-` headers are ignored elsewhere.
    let base = match session_id.filter(|s| !s.is_empty()) {
        Some(sid) => base.header("x-opencode-session", sid),
        None => base,
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
            let retry_after = parse_retry_after(res.headers());
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
            // 4000-char classifier input: quota/context signals can sit past a
            // long JSON prefix. The transcript notice stays capped at 200
            // (retry_notice_event) so blobs never reach the UI.
            let snippet: String = text.chars().take(4000).collect();
            Err((
                classify_http_error(status, &snippet, cf_ray.as_deref(), req_id.as_deref()),
                retry_after,
                status.as_u16(),
            ))
        }
        Err(e) => Err((
            if e.is_connect() {
                ProviderError::Connection(e.to_string())
            } else if e.is_timeout() {
                ProviderError::Timeout(e.to_string())
            } else {
                ProviderError::Stream(e.to_string())
            },
            None,
            0,
        )),
    }
}

type BoxedEventStream = BoxStream<
    'static,
    Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>,
>;

enum StreamState {
    Init {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        session_id: Option<String>,
        initial_backoff: Duration,
        attempt: usize,
        retry_after: Option<Duration>,
        request_max_retries: usize,
        stream_idle_timeout: Option<Duration>,
    },
    ResponsesInit {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        initial_backoff: Duration,
        attempt: usize,
        retry_after: Option<Duration>,
        request_max_retries: usize,
        stream_max_retries: usize,
        stream_attempt: usize,
        // Tool-arg prefix inherited from the interrupted stream: carried into
        // the fresh `ResponsesStreaming` on POST success so suffix deltas
        // append (empty on fresh turns and `None`-id full replays).
        tools_by_call_id: BTreeMap<String, (usize, String, String)>,
        index_to_call_id: BTreeMap<usize, String>,
        stream_idle_timeout: Option<Duration>,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, (String, String, String)>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,
        stream_idle_timeout: Option<Duration>,
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
        // Re-POST context for mid-stream resume (Responses only): a
        // retryable transport/idle failure rebuilds ResponsesInit from these
        // with `previous_response_id = last_response_id`.
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        initial_backoff: Duration,
        request_max_retries: usize,
        stream_max_retries: usize,
        stream_attempt: usize,
        last_response_id: Option<String>,
        stream_idle_timeout: Option<Duration>,
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

    pending_events.push_back(StreamEvent::MessageComplete { stop_reason, usage });
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
            id: if call_id.is_empty() {
                None
            } else {
                Some(call_id)
            },
            name: if name.is_empty() { None } else { Some(name) },
            arguments_delta: args_fixed,
        });
    }
    let stop_reason = if pending_events
        .iter()
        .any(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
    {
        Some(StopReason::ToolUse)
    } else {
        Some(StopReason::EndTurn)
    };
    pending_events.push_back(StreamEvent::MessageComplete { stop_reason, usage });
    Ok(())
}

/// Accumulated Responses tool-call fragments, keyed by call id.
/// (Type alias: keeps the resume helper's signature under clippy's
/// complexity threshold.)
type ResponsesToolsByCallId = BTreeMap<String, (usize, String, String)>;
/// Index → call id ordering for the accumulated fragments.
type ResponsesIndexToCallId = BTreeMap<usize, String>;

/// Resume rebuild (pure): stamp `previous_response_id` on the body and decide
/// the tool-arg prefix's fate. `Some(id)` continues the server-side prefix —
/// the server will NOT re-emit prefix deltas, so the maps are carried for
/// suffix appends. `None` is a true full replay — stale prefix is dropped.
fn resume_body_and_tool_prefix(
    mut body: ResponsesRequest,
    tools_by_call_id: ResponsesToolsByCallId,
    index_to_call_id: ResponsesIndexToCallId,
    last_response_id: Option<String>,
) -> (
    ResponsesRequest,
    ResponsesToolsByCallId,
    ResponsesIndexToCallId,
) {
    let continuing = last_response_id.is_some();
    body.previous_response_id = last_response_id;
    if continuing {
        (body, tools_by_call_id, index_to_call_id)
    } else {
        (body, BTreeMap::new(), BTreeMap::new())
    }
}

fn stream_unfold_step(
    mut state: StreamState,
) -> futures::future::BoxFuture<'static, Option<(Result<StreamEvent, ProviderError>, StreamState)>>
{
    Box::pin(async move {
        loop {
            match state {
                StreamState::Init {
                    client,
                    url,
                    api_key,
                    body,
                    session_id,
                    initial_backoff,
                    attempt,
                    retry_after,
                    request_max_retries,
                    stream_idle_timeout,
                } => {
                    // Backoff for attempts 2+ runs here so the previous
                    // `Reconnecting...` notice is already on screen (Codex:
                    // notify then sleep, not sleep then notify). Abort
                    // contract: this sleep lives inside the polled
                    // `stream.next()` future, so the consumer's cancel
                    // `select!` (agent_loop) drops it instantly — Esc never
                    // waits out the delay. Applies to every backoff sleep in
                    // this function; server-asked delays are additionally
                    // capped at 60s in `parse_retry_after` for drains without
                    // a cancel branch.
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(
                            initial_backoff,
                            attempt - 1,
                            retry_after,
                        ))
                        .await;
                    }
                    let body_value = match serialize_body(&body, "chat request") {
                        Ok(v) => v,
                        Err(err) => {
                            log::error!(target: "gray_provider", "dropping unsendable chat request: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    };
                    match send_json_once(
                        &client,
                        &url,
                        &api_key,
                        &body_value,
                        attempt,
                        session_id.as_deref(),
                    )
                    .await
                    {
                        Ok(response) => {
                            let event_stream: BoxedEventStream =
                                response.bytes_stream().eventsource().boxed();
                            state = StreamState::Streaming {
                                event_stream,
                                accumulated_tools: BTreeMap::new(),
                                last_finish_reason: None,
                                last_usage: None,
                                pending_events: VecDeque::new(),
                                completed: false,
                                stream_idle_timeout,
                            };
                        }
                        Err((err, floor, http_status)) => {
                            // glm-5.2 catch-22: a 400 naming reasoning params
                            // retries ONCE with them omitted (provider
                            // defaults apply; the effort label is unchanged).
                            if attempt == 1
                                && chat_has_reasoning_params(&body)
                                && should_retry_without_reasoning(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "retrying without reasoning params after 400: {err}");
                                let mut stripped = body;
                                let model = stripped.model.clone();
                                strip_chat_reasoning_params(&mut stripped);
                                log::warn!(target: "gray_provider", "stripped reasoning params for model {model}");
                                state = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    session_id,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    request_max_retries,
                                    stream_idle_timeout,
                                };
                                continue;
                            }
                            if is_retryable_error(&err) && attempt < request_max_retries {
                                log::warn!(target: "gray_provider", "retrying (attempt {attempt}) after error: {err}");
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    session_id,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                    request_max_retries,
                                    stream_idle_timeout,
                                };
                                // One notice per burst: the transcript is
                                // append-only, so retries 2+ stay silent
                                // instead of stacking `Reconnecting...` cells.
                                if attempt == 1 {
                                    let notice =
                                        retry_notice_event(attempt, request_max_retries, &err);
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            let model = body.model.clone();
                            let err = maybe_annotate_reasoning_conflict(err, &model);
                            log::error!(target: "gray_provider", "stream request failed: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    }
                }
                StreamState::ResponsesInit {
                    client,
                    url,
                    api_key,
                    body,
                    initial_backoff,
                    attempt,
                    retry_after,
                    request_max_retries,
                    stream_max_retries,
                    stream_attempt,
                    tools_by_call_id,
                    index_to_call_id,
                    stream_idle_timeout,
                } => {
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(
                            initial_backoff,
                            attempt - 1,
                            retry_after,
                        ))
                        .await;
                    }
                    let body_value = match serialize_body(&body, "responses request") {
                        Ok(v) => v,
                        Err(err) => {
                            log::error!(target: "gray_provider", "dropping unsendable responses request: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    };
                    match send_json_once(
                        &client,
                        &url,
                        &api_key,
                        &body_value,
                        attempt,
                        body.prompt_cache_key.as_deref(),
                    )
                    .await
                    {
                        Ok(response) => {
                            let event_stream: BoxedEventStream =
                                response.bytes_stream().eventsource().boxed();
                            // Seed from the request: a resumed stream that
                            // fails before yielding an id keeps chaining from
                            // the id it resumed with (else it would degrade
                            // to a full replay); fresh streams seed None.
                            let last_response_id = body.previous_response_id.clone();
                            state = StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
                                last_usage: None,
                                pending_events: VecDeque::new(),
                                completed: false,
                                client,
                                url,
                                api_key,
                                body,
                                initial_backoff,
                                request_max_retries,
                                stream_max_retries,
                                stream_attempt,
                                last_response_id,
                                stream_idle_timeout,
                            };
                        }
                        Err((err, floor, http_status)) => {
                            // Unknown/expired resume id: strip it and replay
                            // once from scratch instead of ending the turn
                            // with a terminal BadRequest.
                            if attempt == 1
                                && body.previous_response_id.is_some()
                                && should_retry_without_previous_response(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "previous_response_id rejected, stripping and replaying once: {err}");
                                let mut stripped = body;
                                stripped.previous_response_id = None;
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt,
                                    // Full replay: stale tool-arg prefix no
                                    // longer has a server-side continuation.
                                    tools_by_call_id: BTreeMap::new(),
                                    index_to_call_id: BTreeMap::new(),
                                    stream_idle_timeout,
                                };
                                continue;
                            }
                            if attempt == 1
                                && responses_has_reasoning(&body)
                                && should_retry_without_reasoning(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "retrying responses without reasoning after 400: {err}");
                                let mut stripped = body;
                                let model = stripped.model.clone();
                                strip_responses_reasoning(&mut stripped);
                                log::warn!(target: "gray_provider", "stripped responses reasoning for model {model}");
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    stream_idle_timeout,
                                };
                                continue;
                            }
                            if is_retryable_error(&err) && attempt < request_max_retries {
                                log::warn!(target: "gray_provider", "retrying responses (attempt {attempt}) after error: {err}");
                                let next = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    initial_backoff,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    stream_idle_timeout,
                                };
                                // One notice per burst (see Init above).
                                if attempt == 1 {
                                    let notice =
                                        retry_notice_event(attempt, request_max_retries, &err);
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            let model = body.model.clone();
                            let err = maybe_annotate_reasoning_conflict(err, &model);
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
                    stream_idle_timeout,
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
                                stream_idle_timeout,
                            },
                        ));
                    }

                    if completed {
                        return None;
                    }

                    // Per-SSE idle deadline: a configured timeout fires
                    // before read_timeout would. Chat has no resume
                    // primitive, so this arm stays terminal here; the
                    // Responses arm resumes instead.
                    let next = if let Some(idle) = stream_idle_timeout {
                        match tokio::time::timeout(idle, event_stream.next()).await {
                            Ok(next) => next,
                            Err(_) => {
                                let err = ProviderError::Timeout(format!(
                                    "stream stalled: no SSE event within {idle:?}"
                                ));
                                log::error!(target: "gray_provider", "stream idle timeout: {err}");
                                return Some((Err(err), StreamState::Done));
                            }
                        }
                    } else {
                        event_stream.next().await
                    };
                    match next {
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
                                                Some(delta_text) if !delta_text.is_empty() => {
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
                                            if let Some(reasoning) = reasoning
                                                && !reasoning.is_empty()
                                            {
                                                pending_events.push_back(
                                                    StreamEvent::ThinkingDelta { delta: reasoning },
                                                );
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
                                                            (
                                                                String::new(),
                                                                String::new(),
                                                                String::new(),
                                                            )
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
                                stream_idle_timeout,
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
                                    stream_idle_timeout,
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
                    client,
                    url,
                    api_key,
                    body,
                    initial_backoff,
                    request_max_retries,
                    stream_max_retries,
                    stream_attempt,
                    mut last_response_id,
                    stream_idle_timeout,
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
                                client,
                                url,
                                api_key,
                                body,
                                initial_backoff,
                                request_max_retries,
                                stream_max_retries,
                                stream_attempt,
                                last_response_id,
                                stream_idle_timeout,
                            },
                        ));
                    }
                    if completed {
                        return None;
                    }
                    // Idle arm: a stalled event poll resumes with
                    // `previous_response_id` while stream attempts remain
                    // (fresh full retry when no id was seen yet); exhausted
                    // stays terminal exactly as before. Chat path untouched.
                    let next = if let Some(idle) = stream_idle_timeout {
                        match tokio::time::timeout(idle, event_stream.next()).await {
                            Ok(next) => next,
                            Err(_) => {
                                if stream_attempt < stream_max_retries {
                                    log::warn!(target: "gray_provider", "responses stream stalled, resuming (stream attempt {stream_attempt})");
                                    // Throttle the re-POST like pre-stream
                                    // retries (no Retry-After on a stall).
                                    tokio::time::sleep(backoff_delay(
                                        initial_backoff,
                                        stream_attempt,
                                        None,
                                    ))
                                    .await;
                                    let (resumed, resume_tools, resume_index) =
                                        resume_body_and_tool_prefix(
                                            body,
                                            tools_by_call_id,
                                            index_to_call_id,
                                            last_response_id,
                                        );
                                    state = StreamState::ResponsesInit {
                                        client,
                                        url,
                                        api_key,
                                        body: resumed,
                                        initial_backoff,
                                        attempt: 1,
                                        retry_after: None,
                                        request_max_retries,
                                        stream_max_retries,
                                        stream_attempt: stream_attempt + 1,
                                        tools_by_call_id: resume_tools,
                                        index_to_call_id: resume_index,
                                        stream_idle_timeout,
                                    };
                                    continue;
                                }
                                let err = ProviderError::Timeout(format!(
                                    "stream stalled: no SSE event within {idle:?}"
                                ));
                                log::error!(target: "gray_provider", "responses stream idle timeout: {err}");
                                return Some((Err(err), StreamState::Done));
                            }
                        }
                    } else {
                        event_stream.next().await
                    };
                    match next {
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
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    initial_backoff,
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt,
                                    last_response_id,
                                    stream_idle_timeout,
                                };
                                continue;
                            }
                            let value: Value = match serde_json::from_str(data) {
                                Ok(v) => v,
                                Err(e) => {
                                    return Some((
                                        Err(ProviderError::Stream(format!(
                                            "failed to parse Responses SSE chunk: {e}: {data}"
                                        ))),
                                        StreamState::Done,
                                    ));
                                }
                            };
                            let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            // Retain the response envelope id for resume:
                            // `response.created`/`response.completed` carry
                            // `response.id`, deltas carry `response_id`.
                            if let Some(rid) = value
                                .get("response_id")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    value
                                        .get("response")
                                        .and_then(|r| r.get("id"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                })
                            {
                                last_response_id = Some(rid.to_string());
                            }
                            match typ {
                                "response.output_text.delta" => {
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                                        && !delta.is_empty()
                                    {
                                        pending_events.push_back(StreamEvent::TextDelta {
                                            delta: delta.to_string(),
                                        });
                                    }
                                }
                                "response.reasoning_text.delta"
                                | "response.reasoning_summary_text.delta" => {
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                                        && !delta.is_empty()
                                    {
                                        pending_events.push_back(StreamEvent::ThinkingDelta {
                                            delta: delta.to_string(),
                                        });
                                    }
                                }
                                "response.output_item.added" => {
                                    if let Some(item) = value.get("item") {
                                        let item_type =
                                            item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                        if item_type == "function_call" {
                                            let call_id = item
                                                .get("call_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let item_id = item
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| call_id.clone());
                                            let name = item
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if !call_id.is_empty()
                                                && !tools_by_call_id.contains_key(&call_id)
                                            {
                                                let idx = tools_by_call_id.len();
                                                tools_by_call_id.insert(
                                                    call_id.clone(),
                                                    (idx, name.clone(), String::new()),
                                                );
                                                index_to_call_id.insert(idx, call_id.clone());
                                                if item_id != call_id && !item_id.is_empty() {
                                                    tools_by_call_id.insert(
                                                        item_id.clone(),
                                                        (idx, name, String::new()),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    let item_id =
                                        value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
                                    let delta =
                                        value.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                                    if !delta.is_empty() && !item_id.is_empty() {
                                        if let Some(entry) = tools_by_call_id.get_mut(item_id) {
                                            entry.2.push_str(delta);
                                        } else {
                                            let idx = tools_by_call_id.len();
                                            tools_by_call_id.insert(
                                                item_id.to_string(),
                                                (idx, String::new(), delta.to_string()),
                                            );
                                            index_to_call_id.insert(idx, item_id.to_string());
                                        }
                                    }
                                }
                                "response.function_call_arguments.done" => {
                                    if let Some(args) =
                                        value.get("arguments").and_then(|v| v.as_str())
                                    {
                                        let item_id = value
                                            .get("item_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !item_id.is_empty()
                                            && let Some(entry) = tools_by_call_id.get_mut(item_id)
                                            && entry.2.is_empty()
                                        {
                                            entry.2 = args.to_string();
                                        }
                                        if let Some(name) =
                                            value.get("name").and_then(|v| v.as_str())
                                            && let Some(entry) = tools_by_call_id.get_mut(item_id)
                                            && entry.1.is_empty()
                                        {
                                            entry.1 = name.to_string();
                                        }
                                    }
                                }
                                "response.completed" => {
                                    if !completed {
                                        completed = true;
                                        let usage_val = value
                                            .get("response")
                                            .and_then(|r| r.get("usage"))
                                            .or_else(|| value.get("usage"));
                                        if let Some(uval) = usage_val {
                                            if let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(
                                                uval.clone(),
                                            ) {
                                                last_usage = Some(map_usage(&u));
                                            } else {
                                                let input = uval
                                                    .get("input_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let output = uval
                                                    .get("output_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let cached = uval
                                                    .get("input_tokens_details")
                                                    .and_then(|d| d.get("cached_tokens"))
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let reasoning = uval
                                                    .get("output_tokens_details")
                                                    .and_then(|d| d.get("reasoning_tokens"))
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let total = uval
                                                    .get("total_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .map(|v| v as usize)
                                                    .unwrap_or(input + output);
                                                let mut usage = Usage {
                                                    input_tokens: input,
                                                    output_tokens: output,
                                                    reasoning_tokens: reasoning,
                                                    cached_tokens: cached,
                                                    non_cached_input_tokens: input
                                                        .saturating_sub(cached),
                                                    cache_read_input_tokens: cached,
                                                    cache_write_input_tokens: 0,
                                                    total_tokens: total,
                                                };
                                                usage.normalize();
                                                last_usage = Some(usage);
                                            }
                                        }
                                        let mut dedup: BTreeMap<usize, (String, String, String)> =
                                            BTreeMap::new();
                                        for (k, (idx, name, args)) in
                                            std::mem::take(&mut tools_by_call_id)
                                        {
                                            let entry = dedup.entry(idx).or_insert((
                                                k.clone(),
                                                String::new(),
                                                String::new(),
                                            ));
                                            if !name.is_empty() {
                                                entry.1 = name;
                                            }
                                            if !args.is_empty() {
                                                entry.2 = args.clone();
                                            }
                                            if entry.0.is_empty() || k.starts_with("call_") {
                                                entry.0 = k;
                                            }
                                        }
                                        for (idx, (call_id, name, args)) in dedup {
                                            tools_by_call_id
                                                .insert(call_id.clone(), (idx, name, args));
                                        }
                                        index_to_call_id.clear();
                                        for (call_id, (idx, _, _)) in &tools_by_call_id {
                                            index_to_call_id.insert(*idx, call_id.clone());
                                        }
                                        if let Err(err) = emit_responses_tool_calls_and_completion(
                                            &mut tools_by_call_id,
                                            &mut index_to_call_id,
                                            last_usage,
                                            &mut pending_events,
                                        ) {
                                            return Some((Err(err), StreamState::Done));
                                        }
                                    }
                                }
                                "response.output_item.done" => {
                                    // Capture completed reasoning items (id +
                                    // encrypted blob) for verbatim replay next
                                    // turn. Other item types need no action.
                                    if value
                                        .get("item")
                                        .and_then(|i| i.get("type"))
                                        .and_then(|t| t.as_str())
                                        == Some("reasoning")
                                    {
                                        let item = &value["item"];
                                        let id = item
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        let ec = item
                                            .get("encrypted_content")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        if !id.is_empty() && !ec.is_empty() {
                                            pending_events
                                                .push_back(StreamEvent::reasoning_item(id, ec));
                                        }
                                    }
                                }
                                "ping"
                                | "response.created"
                                | "response.in_progress"
                                | "response.content_part.added"
                                | "response.content_part.done"
                                | "response.output_text.done"
                                | "response.reasoning_text.done" => {}
                                _ => {
                                    if let Some(uval) = value
                                        .get("response")
                                        .and_then(|r| r.get("usage"))
                                        .or_else(|| value.get("usage"))
                                        && let Ok(u) =
                                            serde_json::from_value::<OpenAiUsageChunk>(uval.clone())
                                    {
                                        last_usage = Some(map_usage(&u));
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
                                client,
                                url,
                                api_key,
                                body,
                                initial_backoff,
                                request_max_retries,
                                stream_max_retries,
                                stream_attempt,
                                last_response_id,
                                stream_idle_timeout,
                            };
                        }
                        Some(Err(err)) => {
                            // Retryable transport failure: re-POST with
                            // `previous_response_id = last_response_id`
                            // (`None` degrades to today's full replay). The
                            // tool-arg prefix rides along on `Some(id)` so
                            // suffix deltas append; already-yielded deltas
                            // stay out and the server continues the prefix.
                            if stream_attempt < stream_max_retries {
                                log::warn!(target: "gray_provider", "responses stream error, resuming (stream attempt {stream_attempt}): {err}");
                                // Throttle the re-POST like pre-stream
                                // retries (transport errors carry no
                                // Retry-After).
                                tokio::time::sleep(backoff_delay(
                                    initial_backoff,
                                    stream_attempt,
                                    None,
                                ))
                                .await;
                                let (resumed, resume_tools, resume_index) =
                                    resume_body_and_tool_prefix(
                                        body,
                                        tools_by_call_id,
                                        index_to_call_id,
                                        last_response_id,
                                    );
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: resumed,
                                    initial_backoff,
                                    attempt: 1,
                                    retry_after: None,
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt: stream_attempt + 1,
                                    tools_by_call_id: resume_tools,
                                    index_to_call_id: resume_index,
                                    stream_idle_timeout,
                                };
                                continue;
                            }
                            log::error!(target: "gray_provider", "responses stream error: {err}");
                            return Some((
                                Err(ProviderError::Stream(err.to_string())),
                                StreamState::Done,
                            ));
                        }
                        None => {
                            if !completed {
                                completed = true;
                                let mut dedup: BTreeMap<usize, (String, String, String)> =
                                    BTreeMap::new();
                                for (k, (idx, name, args)) in std::mem::take(&mut tools_by_call_id)
                                {
                                    dedup.entry(idx).or_insert((k, name, args));
                                }
                                for (idx, (call_id, name, args)) in dedup {
                                    tools_by_call_id.insert(call_id.clone(), (idx, name, args));
                                }
                                index_to_call_id.clear();
                                for (call_id, (idx, _, _)) in &tools_by_call_id {
                                    index_to_call_id.insert(*idx, call_id.clone());
                                }
                                if let Err(err) = emit_responses_tool_calls_and_completion(
                                    &mut tools_by_call_id,
                                    &mut index_to_call_id,
                                    last_usage,
                                    &mut pending_events,
                                ) {
                                    return Some((Err(err), StreamState::Done));
                                }
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    initial_backoff,
                                    request_max_retries,
                                    stream_max_retries,
                                    stream_attempt,
                                    last_response_id,
                                    stream_idle_timeout,
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

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        if is_muse_model(&self.model) && self.base_url.as_str().contains("opencode.ai/zen") {
            let url = match responses_url(&self.base_url) {
                Ok(u) => u,
                Err(e) => return stream::once(async move { Err(e) }).boxed(),
            };
            let body = map_chat_to_responses(
                req,
                &self.model,
                self.session_id.as_deref(),
                self.reasoning_effort.as_deref(),
            );
            log::debug!(target: "gray_provider", "using Responses API for model {}", self.model);
            let init_state = StreamState::ResponsesInit {
                client: self.http.clone(),
                url,
                api_key: self.api_key.clone(),
                body,
                initial_backoff: self.initial_backoff,
                attempt: 1,
                retry_after: None,
                request_max_retries: self.request_max_retries,
                stream_max_retries: self.stream_max_retries,
                stream_attempt: 1,
                tools_by_call_id: BTreeMap::new(),
                index_to_call_id: BTreeMap::new(),
                stream_idle_timeout: self.stream_idle_timeout,
            };
            return stream::unfold(init_state, stream_unfold_step).boxed();
        }
        let url = match chat_completions_url(&self.base_url) {
            Ok(u) => u,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };

        let mut body = match map_chat_request(req, &self.model, self.reasoning_effort.as_deref()) {
            Ok(b) => b,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };
        // Session affinity (Responses parity): the `x-opencode-session`
        // header alone leaves chat turns rotating cache shards — stamp the
        // body key too so consecutive chat turns pin one shard.
        body.prompt_cache_key = self.session_id.clone().filter(|s| !s.is_empty());
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            session_id: self.session_id.clone(),
            initial_backoff: self.initial_backoff,
            attempt: 1,
            retry_after: None,
            request_max_retries: self.request_max_retries,
            stream_idle_timeout: self.stream_idle_timeout,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_contains_api_key() {
        let p = OpenAiProviderBuilder::new("sk-sentinel-secret", "m")
            .build()
            .unwrap();
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
        let provider = OpenAiProvider::builder("key", "test-model")
            .base_url(server.uri())
            .build()
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
        let provider = OpenAiProvider::builder("key", "test-model")
            .base_url(server.uri())
            .session_id("sess-123")
            .build()
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
            3,
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

    #[tokio::test]
    async fn stream_idle_timeout_fires_on_stalled_stream() {
        // Unfold-step level: wiremock cannot stage "headers, one SSE event,
        // then silence", so drive a never-resolving event stream directly.
        // A configured 50ms idle must surface a fast terminal Err (the same
        // terminal shape a transport stall gets today); Task 2 turns this
        // arm into resume instead of terminal.
        let pending: BoxedEventStream = stream::pending().boxed();
        let state = StreamState::Streaming {
            event_stream: pending,
            accumulated_tools: BTreeMap::new(),
            last_finish_reason: None,
            last_usage: None,
            pending_events: VecDeque::new(),
            completed: false,
            stream_idle_timeout: Some(Duration::from_millis(50)),
        };
        let start = std::time::Instant::now();
        let out = stream_unfold_step(state).await;
        let elapsed = start.elapsed();
        match out {
            Some((Err(ProviderError::Timeout(_)), StreamState::Done)) => {}
            Some((Err(_), _)) => panic!("expected Timeout terminal Err"),
            _ => panic!("expected terminal Err + Done"),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "idle timeout must fire fast: {elapsed:?}"
        );
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
        let (tools, index) = prefix();
        let (body, kept_tools, kept_index) = resume_body_and_tool_prefix(
            map_chat_to_responses(empty_chat_req(), "m1", Some("sess"), Some("high")),
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
            let err =
                classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
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
            let err =
                classify_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body, None, None);
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
        let mut body =
            map_chat_request(empty_chat_req(), "zai/glm-5.2", Some("low")).expect("maps");
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
        let provider = OpenAiProvider::builder("key", "test-model")
            .base_url(server.uri())
            .build()
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
        let provider = OpenAiProvider::builder("key", "test-model")
            .base_url(server.uri())
            .build()
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
        let provider = OpenAiProvider::builder("key", "test-model")
            .base_url(server.uri())
            .session_id("sess-123")
            .build()
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
            3,
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
            should_retry_without_previous_response(
                404,
                "previous response not found: resp_9",
                true
            ),
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
}
