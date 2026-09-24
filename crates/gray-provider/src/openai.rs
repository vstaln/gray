//! OpenAI-compatible streaming LLM provider.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, BoxStream, StreamExt};
use gray_core::agent::{Provider, ProviderError};
use gray_core::credential::{CredentialLease, CredentialSource};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{ChatRequest, ContentBlock, Role};
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::openai_profile::{
    OpenAiAuthorization, OpenAiHeaderSource, OpenAiProviderProfile, OpenAiRequestPolicy, OpenAiWire,
};

/// Default API base URL pointing to OpenRouter.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Maximum retry attempts for transient errors.
const MAX_ATTEMPTS: usize = 5;

/// Initial retry backoff (exponential, jittered, `Retry-After`-floored).
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Exponential backoff cap: growth stops here, `Retry-After` still wins via max.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

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
    reasoning_effort: Option<String>,
    /// Optional sampling passthroughs (None = leave the server default).
    temperature: Option<f32>,
    top_p: Option<f32>,
    /// Stable per-process id sent as `prompt_cache_key` (Responses API and
    /// chat completions alike) so callers pin one cache shard per session for
    /// prompt caching. Also sent as the `x-opencode-session` header (Console Go
    /// routes on it; required).
    session_id: Option<String>,
    /// Declared dynamic profile; `None` preserves the built-in provider path.
    profile: Option<OpenAiProviderProfile>,
    /// Lease source for dynamic profiles. The agent keeps it alive for the
    /// whole sidecar lifecycle.
    credential_source: Option<Arc<dyn CredentialSource>>,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    /// Constructs an `OpenAiProvider`. An empty `base_url` selects
    /// [`DEFAULT_BASE_URL`]; `session_id` pins the prompt-cache shard.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<String>,
        session_id: Option<String>,
    ) -> Result<Self, String> {
        let base_url_str = base_url.into();
        let base_url_str = if base_url_str.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base_url_str
        };
        let base_url = Url::parse(&base_url_str)
            .map_err(|e| format!("invalid base_url '{base_url_str}': {e}"))?;

        // default client with a 120s idle-read timeout so a stalled server
        // (finish_reason then silence, hung proxy) can't freeze a turn
        // forever. Total timeout stays off: long generations are legal.
        let http = Self::http_client(true);

        Ok(Self {
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
            session_id,
            profile: None,
            credential_source: None,
        })
    }

    fn http_client(follow_redirects: bool) -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .redirect(if follow_redirects {
                Policy::default()
            } else {
                Policy::none()
            })
            .build()
            .expect("reqwest client with timeouts")
    }

    /// Build a profile-driven provider. Gray owns the credential source, so
    /// this path never stores a static API key.
    pub fn new_with_profile(
        model: impl Into<String>,
        reasoning_effort: Option<String>,
        session_id: Option<String>,
        profile: OpenAiProviderProfile,
        credential_source: Arc<dyn CredentialSource>,
    ) -> Result<Self, String> {
        let base_url = profile.base_url.clone();
        let http = Self::http_client(profile.follow_redirects);
        Ok(Self {
            base_url,
            api_key: String::new(),
            model: model.into(),
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
            session_id,
            profile: Some(profile),
            credential_source: Some(credential_source),
        })
    }

    /// Sampling passthrough: providers that accept `temperature`/`top_p` get
    /// them on every chat request; `None` keeps whatever the server defaults
    /// to (the pre-existing behaviour).
    pub fn with_sampling(mut self, temperature: Option<f32>, top_p: Option<f32>) -> Self {
        self.temperature = temperature;
        self.top_p = top_p;
        self
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiChatRequest {
    model: String,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
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

fn image_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

/// Raw video bytes past this never go on the wire: a base64 mp4 is ~1.33x
/// the file, and a 15s clip is already a few MB of prompt. The fallback is
/// `gray view <file>`, which turns the same file into a tiled contact sheet.
pub const MAX_NATIVE_VIDEO_BYTES: usize = 8 * 1024 * 1024;

/// Does this model take a native video part? Deliberately a name check, not
/// a capability table: only the Gemini line documents video input, and a
/// wrong answer here either 400s at the provider or burns a huge prompt, so
/// an unlisted model gets the universal contact sheet instead.
pub fn model_accepts_video(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.contains("gemini") || id.contains("gemma")
}

fn video_rejected(model: &str) -> ProviderError {
    ProviderError::BadRequest(format!(
        "{model} has no native video input; use `gray view <file>` for a contact sheet"
    ))
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
                        ContentBlock::Image { .. } | ContentBlock::Video { .. } => {}
                        ContentBlock::StructuredInput { .. } => {}
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
                });

                for (id, content, is_error) in tool_results {
                    messages.push(OpenAiMessageRequest {
                        role: "tool".to_string(),
                        content: Some(Value::String(wire_tool_output(&content, is_error))),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: Some(id),
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
                // At most one video per turn: two clips in one prompt is a
                // budgeting question the caller should answer, not us.
                let mut video_part: Option<(String, String)> = None;
                let mut tool_results = Vec::new();

                for block in msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text);
                            }
                        }
                        block @ ContentBlock::StructuredInput { .. } => {
                            if let Some(text) = block.provider_text() {
                                text_parts.push(text);
                            }
                        }
                        ContentBlock::Image { media_type, data } => {
                            image_parts.push((media_type, data));
                        }
                        ContentBlock::Video { media_type, data } => {
                            if !model_accepts_video(model) {
                                return Err(video_rejected(model));
                            }
                            if data.len() > MAX_NATIVE_VIDEO_BYTES {
                                return Err(ProviderError::BadRequest(format!(
                                    "video too large for a native part ({} bytes, cap {MAX_NATIVE_VIDEO_BYTES}); use `gray view <file>` for a contact sheet",
                                    data.len()
                                )));
                            }
                            video_part = Some((media_type, data));
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
                    });
                }

                let has_media = !image_parts.is_empty() || video_part.is_some();
                if !text_parts.is_empty() || has_media || messages.is_empty() {
                    let content = if has_media {
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
                        if let Some((media_type, data)) = &video_part {
                            // Gemini's OpenAI-compatible surface takes video
                            // as a video_url part carrying a data URL, the
                            // same shape as image_url above.
                            arr.push(serde_json::json!({
                                "type":"video_url",
                                "video_url":{"url": image_data_url(media_type, data)},
                            }));
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
                    });
                }
            }
        }
    }

    // Tool-result images must follow the entire contiguous result run.
    let mut ordered = Vec::with_capacity(messages.len());
    let mut deferred_images = Vec::new();
    let mut in_results = false;
    for message in messages {
        let image_only = message.role == "user"
            && message
                .content
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    !parts.is_empty() && parts.iter().all(|p| p["type"] == "image_url")
                });
        if in_results && image_only {
            deferred_images.push(message);
            continue;
        }
        if message.role != "tool" {
            ordered.append(&mut deferred_images);
            in_results = false;
        } else {
            in_results = true;
        }
        ordered.push(message);
    }
    ordered.append(&mut deferred_images);
    let messages = ordered;

    // 3. Map tools — drop empty names that would trigger 400 `name` must be non-empty
    let tools: Vec<OpenAiToolDefRequest> = filter_valid_tools(req.tools)
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
        // Sampling passthroughs are stamped by the provider after mapping
        // (`with_sampling`); the mapper itself has no config to read.
        temperature: None,
        top_p: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<Value>,
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
                        block @ ContentBlock::StructuredInput { .. } => {
                            if let Some(text) = block.provider_text() {
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
                        ContentBlock::Video { media_type, data } => {
                            // A Video block only reaches here after the
                            // composer checked the model and the byte cap;
                            // re-check anyway so a hand-built ChatRequest
                            // cannot smuggle a huge base64 blob onto the wire.
                            if !model_accepts_video(model) {
                                text_parts
                                    .push(format!("(video omitted: {})", video_rejected(model)));
                                continue;
                            }
                            if data.len() > MAX_NATIVE_VIDEO_BYTES {
                                text_parts.push(format!(
                                    "(video omitted: {} bytes exceeds the {MAX_NATIVE_VIDEO_BYTES}-byte native cap; use `gray view` for a contact sheet)",
                                    data.len()
                                ));
                                continue;
                            }
                            let url = image_data_url(&media_type, &data);
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            input.push(serde_json::json!({"role": "user", "content": [{"type":"input_video","video_url": url}]}));
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
                        ContentBlock::Image { .. } | ContentBlock::Video { .. } => {}
                        ContentBlock::StructuredInput { .. } => {}
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
        tool_choice: None,
        parallel_tool_calls: None,
        text: None,
    }
}

fn apply_dynamic_policy(
    body: &mut ResponsesRequest,
    policy: &OpenAiRequestPolicy,
    session_id: Option<&str>,
) {
    body.prompt_cache_key = if policy.prompt_cache_key {
        session_id.filter(|s| !s.is_empty()).map(str::to_string)
    } else {
        None
    };
    body.store = policy.store;
    body.previous_response_id = if policy.previous_response_id {
        body.previous_response_id.clone()
    } else {
        None
    };
    if policy.include_reasoning_encrypted {
        body.include = body
            .reasoning
            .as_ref()
            .map(|_| vec!["reasoning.encrypted_content".to_string()]);
    } else {
        body.include = None;
    }
    body.tool_choice = policy.tool_choice.clone().map(Value::String);
    body.parallel_tool_calls = policy.parallel_tool_calls;
    body.text = policy
        .text_verbosity
        .clone()
        .map(|verbosity| serde_json::json!({"verbosity": verbosity}));
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
        500..=599 => ProviderError::ServerError(format!("provider-side {msg} (request was valid)")),
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

/// True for the body-level transport failures `eventsource-stream` reports
/// once the response has started: a truncated chunked body, or a connection
/// reset mid-body. reqwest has no decompression enabled here (features are
/// `json,stream,rustls-tls`), so these are framing/connection failures —
/// always transient, never the model's output.
///
/// Deliberately narrow: `Transport` only. `Parse` (a malformed SSE line) and
/// `InvalidContentType` are the server's own shape and must stay terminal.
fn is_midstream_transport(err: &eventsource_stream::EventStreamError<reqwest::Error>) -> bool {
    matches!(err, eventsource_stream::EventStreamError::Transport(_))
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

/// Exponential backoff with jitter, capped at `MAX_BACKOFF`, floored by
/// `Retry-After` when present (server-asked delay still wins via max).
fn backoff_delay(initial: Duration, attempt: usize, retry_after: Option<Duration>) -> Duration {
    let exp_factor = 1u64 << (attempt.saturating_sub(1).min(10));
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
    let base = Duration::from_millis(backoff_ms + jitter_ms).min(MAX_BACKOFF);
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
/// (their ~1s+jitter backoff is correct for blips).
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

/// Session-affinity header for one POST (empty without a session id).
/// `x-opencode-session`: Console Go (opencode.ai/zen) routes on it and 400s
/// MissingSessionID without it; unknown `x-` headers are ignored elsewhere.
/// Prompt-cache affinity is carried by the request body's `prompt_cache_key`,
/// so no per-host sticky-routing header is needed.
fn session_affinity_headers(session_id: Option<&str>) -> Vec<(&'static str, &str)> {
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    vec![("x-opencode-session", sid)]
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
    let base = session_affinity_headers(session_id)
        .into_iter()
        .fold(base, |b, (name, value)| b.header(name, value));
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

/// One provider stream acquired through a host-owned credential lease.
#[derive(Default)]
struct DynamicToolCall {
    index: usize,
    name: String,
    args: String,
    done: bool,
}

enum DynamicState {
    Init {
        client: reqwest::Client,
        url: Url,
        profile: Box<OpenAiProviderProfile>,
        lease: CredentialLease,
        body: Box<ResponsesRequest>,
        session_id: Option<String>,
    },
    Streaming {
        event_stream: BoxedEventStream,
        tools: BTreeMap<String, DynamicToolCall>,
        next_index: usize,
        pending: VecDeque<StreamEvent>,
        last_usage: Option<Usage>,
        completed: bool,
    },
    Done,
}

fn redact_lease(mut snippet: String, bearer: Option<&str>) -> String {
    if let Some(bearer) = bearer.filter(|value| !value.is_empty()) {
        snippet = snippet.replace(bearer, "[redacted]");
    }
    snippet
}

fn dynamic_headers(
    profile: &OpenAiProviderProfile,
    lease: &CredentialLease,
    session_id: Option<&str>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    for header in &profile.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| ProviderError::BadRequest("invalid provider header name".into()))?;
        if header.name.len() > 128 {
            return Err(ProviderError::BadRequest(
                "provider header name too long".into(),
            ));
        }
        let value = match &header.source {
            OpenAiHeaderSource::Static(value) => value.clone(),
            OpenAiHeaderSource::Metadata(key) => {
                lease.metadata.get(key).cloned().ok_or_else(|| {
                    ProviderError::BadRequest(format!("missing provider metadata header {key}"))
                })?
            }
            OpenAiHeaderSource::SessionId => session_id.unwrap_or_default().to_string(),
        };
        if value.is_empty() {
            if header.required {
                return Err(ProviderError::BadRequest(format!(
                    "missing provider header {}",
                    header.name
                )));
            }
            continue;
        }
        if value.len() > 4096 || value.contains('\r') || value.contains('\n') {
            return Err(ProviderError::BadRequest(
                "invalid provider header value".into(),
            ));
        }
        let value = HeaderValue::from_str(&value)
            .map_err(|_| ProviderError::BadRequest("invalid provider header value".into()))?;
        headers.insert(name, value);
    }
    if let OpenAiAuthorization::Bearer { secret_name } = &profile.authorization {
        let bearer = lease
            .secrets
            .get(secret_name)
            .ok_or_else(|| ProviderError::Auth("provider bearer credential missing".into()))?;
        let value = HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| ProviderError::Auth("invalid provider bearer credential".into()))?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

async fn send_dynamic_json_once(
    client: &reqwest::Client,
    url: &Url,
    profile: &OpenAiProviderProfile,
    lease: &CredentialLease,
    body: &ResponsesRequest,
    session_id: Option<&str>,
    attempt: usize,
) -> Result<reqwest::Response, ProviderError> {
    let headers = dynamic_headers(profile, lease, session_id)?;
    let bearer = match &profile.authorization {
        OpenAiAuthorization::Bearer { secret_name } => {
            lease.secrets.get(secret_name).map(str::to_owned)
        }
        OpenAiAuthorization::None => None,
    };
    let send = client
        .post(url.clone())
        .headers(headers)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/event-stream")
        .json(body);
    let response = send.send().await.map_err(map_send_error)?;
    let status = response.status();
    if !status.is_success() {
        let mut snippet = String::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ProviderError::Stream(e.to_string()))?
        {
            snippet.push_str(&String::from_utf8_lossy(&chunk));
            if snippet.len() > MAX_DYNAMIC_ERROR_BYTES {
                snippet.truncate(MAX_DYNAMIC_ERROR_BYTES);
                break;
            }
        }
        let snippet = redact_lease(snippet, bearer.as_deref());
        return Err(classify_http_error(status, &snippet, None, None));
    }
    log::debug!(target: "gray_provider", "dynamic Responses attempt {attempt} returned {status}");
    Ok(response)
}

fn map_send_error(error: reqwest::Error) -> ProviderError {
    if error.is_connect() {
        ProviderError::Connection(error.to_string())
    } else if error.is_timeout() {
        ProviderError::Timeout(error.to_string())
    } else {
        ProviderError::Stream(error.to_string())
    }
}

fn process_dynamic_event(
    value: Value,
    pending: &mut VecDeque<StreamEvent>,
    tools: &mut BTreeMap<String, DynamicToolCall>,
    next_index: &mut usize,
    last_usage: &mut Option<Usage>,
) -> Result<bool, ProviderError> {
    let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && !delta.is_empty()
            {
                pending.push_back(StreamEvent::text_delta(delta));
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && !delta.is_empty()
            {
                pending.push_back(StreamEvent::thinking_delta(delta));
            }
        }
        "response.output_item.added" => {
            let item = value.get("item");
            if item.and_then(|i| i.get("type")).and_then(|t| t.as_str()) == Some("function_call") {
                let id = item
                    .and_then(|i| i.get("call_id"))
                    .and_then(|v| v.as_str())
                    .or_else(|| item.and_then(|i| i.get("id")).and_then(|v| v.as_str()))
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .and_then(|i| i.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !id.is_empty() {
                    let index = *next_index;
                    *next_index += 1;
                    tools.insert(
                        id.clone(),
                        DynamicToolCall {
                            index,
                            name: name.clone(),
                            ..Default::default()
                        },
                    );
                    pending.push_back(StreamEvent::tool_call_delta(
                        index,
                        Some(id),
                        Some(name),
                        "",
                    ));
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let id = value
                .get("item_id")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("call_id").and_then(|v| v.as_str()))
                .unwrap_or_default();
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && let Some(call) = tools.get_mut(id)
            {
                call.args.push_str(delta);
            }
        }
        "response.output_item.done" => {
            let item = value.get("item");
            let item_type = item.and_then(|i| i.get("type")).and_then(|t| t.as_str());
            if item_type == Some("reasoning") {
                let id = item
                    .and_then(|i| i.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let encrypted = item
                    .and_then(|i| i.get("encrypted_content"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !id.is_empty() && !encrypted.is_empty() {
                    pending.push_back(StreamEvent::reasoning_item(id, encrypted));
                }
            } else if item_type == Some("function_call") {
                let id = item
                    .and_then(|i| i.get("call_id"))
                    .and_then(|v| v.as_str())
                    .or_else(|| item.and_then(|i| i.get("id")).and_then(|v| v.as_str()))
                    .unwrap_or_default()
                    .to_string();
                if let Some(arguments) = item
                    .and_then(|i| i.get("arguments"))
                    .and_then(|v| v.as_str())
                    && let Some(call) = tools.get_mut(&id)
                {
                    call.args.push_str(arguments);
                }
                if let Some(call) = tools.get_mut(&id)
                    && !call.done
                {
                    call.done = true;
                    let index = call.index;
                    let name = call.name.clone();
                    let args = call.args.clone();
                    pending.push_back(StreamEvent::tool_call_delta(
                        index,
                        Some(id),
                        Some(name),
                        args,
                    ));
                }
            }
        }
        "response.completed" => {
            if let Some(uval) = value
                .get("response")
                .and_then(|r| r.get("usage"))
                .or_else(|| value.get("usage"))
                && let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(uval.clone())
            {
                *last_usage = Some(map_usage(&u));
            }
            let stop = if tools.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            };
            pending.push_back(StreamEvent::message_complete(Some(stop), *last_usage));
            return Ok(true);
        }
        "response.incomplete" => {
            let reason = value
                .get("response")
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(|v| v.as_str());
            let stop = if reason == Some("max_output_tokens") {
                StopReason::MaxTokens
            } else {
                StopReason::Error
            };
            pending.push_back(StreamEvent::message_complete(Some(stop), *last_usage));
            return Ok(true);
        }
        "response.failed" | "error" => {
            let detail = value
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| value.get("message").and_then(|v| v.as_str()))
                .unwrap_or("provider stream failed");
            return Err(ProviderError::Stream(detail.to_string()));
        }
        _ => {}
    }
    Ok(false)
}

async fn dynamic_step(
    initial: DynamicState,
) -> Option<(Result<StreamEvent, ProviderError>, DynamicState)> {
    let mut state = initial;
    loop {
        match state {
            DynamicState::Init {
                client,
                url,
                profile,
                lease,
                body,
                session_id,
            } => {
                match send_dynamic_json_once(
                    &client,
                    &url,
                    &profile,
                    &lease,
                    &body,
                    session_id.as_deref(),
                    1,
                )
                .await
                {
                    Ok(response) => {
                        let event_stream = response.bytes_stream().eventsource().boxed();
                        state = DynamicState::Streaming {
                            event_stream,
                            tools: BTreeMap::new(),
                            next_index: 0,
                            pending: VecDeque::new(),
                            last_usage: None,
                            completed: false,
                        };
                    }
                    Err(error) => return Some((Err(error), DynamicState::Done)),
                }
            }
            DynamicState::Streaming {
                mut event_stream,
                mut tools,
                mut next_index,
                mut pending,
                mut last_usage,
                mut completed,
            } => {
                if completed && pending.is_empty() {
                    return None;
                }
                let next = event_stream.next().await;
                match next {
                    None => {
                        return Some((
                            Err(ProviderError::Stream(
                                "dynamic Responses stream ended before response.completed".into(),
                            )),
                            DynamicState::Done,
                        ));
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(ProviderError::Stream(error.to_string())),
                            DynamicState::Done,
                        ));
                    }
                    Some(Ok(event)) => {
                        let data = event.data.trim();
                        if data.is_empty() {
                            state = DynamicState::Streaming {
                                event_stream,
                                tools,
                                next_index,
                                pending,
                                last_usage,
                                completed,
                            };
                            continue;
                        }
                        if data == "[DONE]" {
                            return Some((
                                Err(ProviderError::Stream(
                                    "dynamic Responses stream ended before response.completed"
                                        .into(),
                                )),
                                DynamicState::Done,
                            ));
                        }
                        let value: Value = match serde_json::from_str(data) {
                            Ok(value) => value,
                            Err(e) => {
                                return Some((
                                    Err(ProviderError::Stream(format!(
                                        "failed to parse Responses SSE chunk: {e}: {data}"
                                    ))),
                                    DynamicState::Done,
                                ));
                            }
                        };
                        match process_dynamic_event(
                            value,
                            &mut pending,
                            &mut tools,
                            &mut next_index,
                            &mut last_usage,
                        ) {
                            Ok(finished) => completed |= finished,
                            Err(error) => return Some((Err(error), DynamicState::Done)),
                        }
                        if !pending.is_empty() {
                            let event = pending.pop_front().expect("pending event");
                            state = DynamicState::Streaming {
                                event_stream,
                                tools,
                                next_index,
                                pending,
                                last_usage,
                                completed,
                            };
                            return Some((Ok(event), state));
                        }
                        if completed {
                            return None;
                        }
                        state = DynamicState::Streaming {
                            event_stream,
                            tools,
                            next_index,
                            pending,
                            last_usage,
                            completed,
                        };
                    }
                }
            }
            DynamicState::Done => return None,
        }
    }
}

const MAX_DYNAMIC_ERROR_BYTES: usize = 4096;

impl OpenAiProvider {
    fn dynamic_stream(
        &self,
        request: ChatRequest,
        profile: OpenAiProviderProfile,
        credential_source: Arc<dyn CredentialSource>,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let mut body = map_chat_to_responses(
            request,
            &self.model,
            self.session_id.as_deref(),
            self.reasoning_effort.as_deref(),
        );
        apply_dynamic_policy(&mut body, &profile.request, self.session_id.as_deref());
        let url = match responses_url(&profile.base_url) {
            Ok(url) => url,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        let client = self.http.clone();
        let session_id = self.session_id.clone();
        let started = stream::once(async move {
            credential_source
                .acquire()
                .await
                .map_err(|_| ProviderError::Auth("provider credential unavailable".into()))
                .map(|lease| (lease, body))
        });
        started
            .flat_map(move |result| match result {
                Ok((lease, body)) => stream::unfold(
                    DynamicState::Init {
                        client: client.clone(),
                        url: url.clone(),
                        profile: Box::new(profile.clone()),
                        lease,
                        body: Box::new(body),
                        session_id: session_id.clone(),
                    },
                    dynamic_step,
                )
                .boxed(),
                Err(error) => stream::once(async move { Err(error) }).boxed(),
            })
            .boxed()
    }
}

enum StreamState {
    Init {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        session_id: Option<String>,
        attempt: usize,
        retry_after: Option<Duration>,
    },
    ResponsesInit {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        attempt: usize,
        retry_after: Option<Duration>,
        stream_attempt: usize,
        // Tool-arg prefix inherited from the interrupted stream: carried into
        // the fresh `ResponsesStreaming` on POST success so suffix deltas
        // append (empty on fresh turns and `None`-id full replays).
        tools_by_call_id: BTreeMap<String, (usize, String, String)>,
        index_to_call_id: BTreeMap<usize, String>,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, (String, String, String)>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,
        // Request state, carried so a body-level transport failure can
        // re-POST instead of ending the turn.
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        attempt: usize,
        /// True once a delta has been yielded to the caller. A retry is
        /// only invisible while this is false: the agent loop commits
        /// streamed text to history as it arrives, so replaying after a
        /// visible delta would duplicate what the user already read.
        emitted: bool,
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
        // True once output text deltas have been yielded. A provider that
        // closes SSE without `response.completed` after text has still
        // produced a usable text turn; unconfirmed tool calls stay fatal.
        saw_output_text: bool,
        // Re-POST context for mid-stream resume (Responses only): a
        // retryable transport failure rebuilds ResponsesInit from these
        // with `previous_response_id = last_response_id`.
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        stream_attempt: usize,
        last_response_id: Option<String>,
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
    // Upstream provider rejects previous_response_id when encrypted reasoning is used.
    let has_reasoning = body.reasoning.is_some()
        || body
            .include
            .as_ref()
            .is_some_and(|inc| inc.iter().any(|s| s == "reasoning.encrypted_content"));
    let continuing = last_response_id.is_some() && !has_reasoning;
    body.previous_response_id = if continuing { last_response_id } else { None };
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
                    attempt,
                    retry_after,
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
                            INITIAL_BACKOFF,
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
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted: false,
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
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                };
                                continue;
                            }
                            if is_retryable_error(&err) && attempt < MAX_ATTEMPTS {
                                log::warn!(target: "gray_provider", "retrying (attempt {attempt}) after error: {err}");
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    session_id,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                };
                                // One notice per burst: the transcript is
                                // append-only, so retries 2+ stay silent
                                // instead of stacking `Reconnecting...` cells.
                                if attempt == 1 {
                                    let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
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
                    attempt,
                    retry_after,
                    stream_attempt,
                    tools_by_call_id,
                    index_to_call_id,
                } => {
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(
                            INITIAL_BACKOFF,
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
                                stream_attempt,
                                last_response_id,
                                saw_output_text: false,
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
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    stream_attempt,
                                    // Full replay: stale tool-arg prefix no
                                    // longer has a server-side continuation.
                                    tools_by_call_id: BTreeMap::new(),
                                    index_to_call_id: BTreeMap::new(),
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
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                };
                                continue;
                            }
                            if is_retryable_error(&err) && attempt < MAX_ATTEMPTS {
                                log::warn!(target: "gray_provider", "retrying responses (attempt {attempt}) after error: {err}");
                                let next = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                };
                                // One notice per burst (see Init above).
                                if attempt == 1 {
                                    let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
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
                    client,
                    url,
                    api_key,
                    body,
                    attempt,
                    mut emitted,
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
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted,
                            },
                        ));
                    }

                    if completed {
                        return None;
                    }

                    let next = event_stream.next().await;
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
                                                    emitted = true;
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
                                                emitted = true;
                                            }

                                            if let Some(tool_calls) = choice.delta.tool_calls {
                                                for tc in tool_calls {
                                                    // ponytail: no index cap here — the agent
                                                    // owns the guard (hard error). A silent
                                                    // drop loses model intent with a clean
                                                    // EndTurn; forwarding keeps one policy.
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
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted,
                            };
                        }
                        Some(Err(err)) => {
                            log::error!(target: "gray_provider", "stream error: {err}");
                            // A body-level transport failure — a truncated
                            // chunked body, or a connection reset mid-body —
                            // is transient, and reqwest can only report it as
                            // "error decoding response body". While nothing
                            // has been yielded the re-POST is invisible, so
                            // retry exactly like a pre-stream failure. Once a
                            // delta has reached the caller the agent loop has
                            // already committed it to history, so a replay
                            // would duplicate what the user read: say so
                            // instead.
                            if !emitted && attempt < MAX_ATTEMPTS && is_midstream_transport(&err) {
                                log::warn!(
                                    target: "gray_provider",
                                    "stream body failed before any delta; retrying (attempt {attempt}): {err}"
                                );
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    session_id: None,
                                    attempt: attempt + 1,
                                    retry_after: None,
                                };
                                // One notice per burst, same rule as the
                                // pre-stream path: the transcript is
                                // append-only, so attempts 2+ stay silent.
                                if attempt == 1 {
                                    let notice = retry_notice_event(
                                        attempt,
                                        MAX_ATTEMPTS,
                                        &ProviderError::Stream(err.to_string()),
                                    );
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            let terminal = if emitted {
                                // Name the point of failure: the partial text
                                // was salvaged into history by the caller.
                                ProviderError::Stream(format!(
                                    "{err} (the connection dropped mid-response after the response started;                                      the partial text was kept — retry the turn)"
                                ))
                            } else {
                                ProviderError::Stream(err.to_string())
                            };
                            return Some((Err(terminal), StreamState::Done));
                        }
                        None => {
                            if last_finish_reason.is_none() {
                                // Providers (seen on opencode zen) can close the
                                // stream right after the last delta with no finish
                                // chunk. A complete tool call is still usable, so
                                // keep it and warn; a truncated call must never
                                // execute, so anything else stays retryable.
                                let complete = accumulated_tools.values().all(|(_, _, args)| {
                                    let trimmed = args.trim();
                                    !trimmed.is_empty()
                                        && serde_json::from_str::<Value>(trimmed).is_ok()
                                });
                                if completed || accumulated_tools.is_empty() || !complete {
                                    return Some((
                                        Err(ProviderError::Stream(
                                            "Chat stream ended without a finish reason".into(),
                                        )),
                                        StreamState::Done,
                                    ));
                                }
                                match emit_tool_calls_and_completion(
                                    &mut accumulated_tools,
                                    Some(StopReason::ToolUse),
                                    last_usage,
                                    &mut pending_events,
                                ) {
                                    Ok(()) => {
                                        log::warn!(
                                            target: "gray_provider",
                                            "stream ended without a finish reason; keeping complete tool calls"
                                        );
                                        completed = true;
                                        last_finish_reason = Some(StopReason::ToolUse);
                                        state = StreamState::Streaming {
                                            event_stream,
                                            accumulated_tools,
                                            last_finish_reason,
                                            last_usage,
                                            pending_events,
                                            completed,
                                            client,
                                            url,
                                            api_key,
                                            body,
                                            attempt,
                                            emitted: true,
                                        };
                                    }
                                    Err(_) => {
                                        return Some((
                                            Err(ProviderError::Stream(
                                                "Chat stream ended without a finish reason".into(),
                                            )),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                            } else if !completed {
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
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    attempt,
                                    emitted: true,
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
                    mut saw_output_text,
                    client,
                    url,
                    api_key,
                    body,
                    stream_attempt,
                    mut last_response_id,
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
                                stream_attempt,
                                last_response_id,
                                saw_output_text,
                            },
                        ));
                    }
                    if completed {
                        return None;
                    }
                    let next = event_stream.next().await;
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
                                    stream_attempt,
                                    last_response_id,
                                    saw_output_text,
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
                                        saw_output_text = true;
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
                                "response.incomplete" => {
                                    let reason = value["response"]["incomplete_details"]["reason"]
                                        .as_str()
                                        .unwrap_or("unknown");
                                    if reason == "max_output_tokens" {
                                        if let Some(usage) = value["response"]
                                            .get("usage")
                                            .or_else(|| value.get("usage"))
                                            && let Ok(usage) =
                                                serde_json::from_value::<OpenAiUsageChunk>(
                                                    usage.clone(),
                                                )
                                        {
                                            last_usage = Some(map_usage(&usage));
                                        }
                                        pending_events.push_back(StreamEvent::MessageComplete {
                                            stop_reason: Some(StopReason::MaxTokens),
                                            usage: last_usage,
                                        });
                                        tools_by_call_id.clear();
                                        index_to_call_id.clear();
                                        completed = true;
                                    } else {
                                        return Some((
                                            Err(ProviderError::Stream(format!(
                                                "Responses incomplete: {reason}"
                                            ))),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                                "response.failed" | "error" => {
                                    // Terminal provider failure: surfacing it as
                                    // an error (never a clean EndTurn) keeps a
                                    // truncated turn from persisting as success.
                                    let detail = value
                                        .get("response")
                                        .and_then(|r| r.get("error"))
                                        .or_else(|| value.get("error"))
                                        .or(Some(&value))
                                        .map(|e| {
                                            let code = e
                                                .get("code")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            let msg = e
                                                .get("message")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            format!("{code} {msg}")
                                        })
                                        .unwrap_or_else(|| typ.to_string());
                                    return Some((
                                        Err(ProviderError::Stream(format!(
                                            "responses stream failed: {detail}"
                                        ))),
                                        StreamState::Done,
                                    ));
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
                                "response.output_text.done" => {
                                    saw_output_text = true;
                                }
                                "ping"
                                | "response.created"
                                | "response.in_progress"
                                | "response.content_part.added"
                                | "response.content_part.done"
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
                                stream_attempt,
                                last_response_id,
                                saw_output_text,
                            };
                        }
                        Some(Err(err)) => {
                            // Retryable transport failure: re-POST with
                            // `previous_response_id = last_response_id`
                            // (`None` degrades to today's full replay). The
                            // tool-arg prefix rides along on `Some(id)` so
                            // suffix deltas append; already-yielded deltas
                            // stay out and the server continues the prefix.
                            if stream_attempt < MAX_ATTEMPTS {
                                log::warn!(target: "gray_provider", "responses stream error, resuming (stream attempt {stream_attempt}): {err}");
                                // Throttle the re-POST like pre-stream
                                // retries (transport errors carry no
                                // Retry-After).
                                tokio::time::sleep(backoff_delay(
                                    INITIAL_BACKOFF,
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
                                    attempt: 1,
                                    retry_after: None,
                                    stream_attempt: stream_attempt + 1,
                                    tools_by_call_id: resume_tools,
                                    index_to_call_id: resume_index,
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
                            // Some Responses-compatible gateways close SSE
                            // after the final text event without ever sending
                            // `response.completed`. That is still a complete
                            // text turn. Unconfirmed tool calls remain fatal:
                            // an argument fragment is not proof the provider
                            // finished the call.
                            if tools_by_call_id.is_empty() && saw_output_text {
                                log::warn!(
                                    target: "gray_provider",
                                    "responses stream ended without response.completed; completing text output"
                                );
                                completed = true;
                                pending_events.push_back(StreamEvent::MessageComplete {
                                    stop_reason: Some(StopReason::EndTurn),
                                    usage: last_usage,
                                });
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    saw_output_text,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    stream_attempt,
                                    last_response_id,
                                };
                                continue;
                            }
                            return Some((
                                Err(ProviderError::Stream(
                                    "Responses stream ended before response.completed".into(),
                                )),
                                StreamState::Done,
                            ));
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
        if let (Some(profile), Some(credential_source)) =
            (self.profile.clone(), self.credential_source.clone())
        {
            if !matches!(profile.wire, OpenAiWire::Responses | OpenAiWire::Auto) {
                return stream::once(async move {
                    Err(ProviderError::BadRequest(
                        "dynamic OpenAI chat completions profiles are not implemented".into(),
                    ))
                })
                .boxed();
            }
            return self.dynamic_stream(req, profile, credential_source);
        }
        if is_muse_model(&self.model)
            && (self.base_url.as_str().contains("opencode.ai/zen")
                || self.base_url.as_str().contains("commandcode.ai"))
        {
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
                attempt: 1,
                retry_after: None,
                stream_attempt: 1,
                tools_by_call_id: BTreeMap::new(),
                index_to_call_id: BTreeMap::new(),
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
        body.temperature = self.temperature;
        body.top_p = self.top_p;
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            session_id: self.session_id.clone(),
            attempt: 1,
            retry_after: None,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

#[path = "openai_tests.rs"]
#[cfg(test)]
mod tests;
