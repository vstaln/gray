use std::collections::{BTreeMap, HashSet, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, BoxStream, Stream, StreamExt};
use gray_core::agent::{Provider, ProviderError, current_isolated_stream};
use gray_core::credential::{CredentialLease, CredentialSource};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{
    ChatRequest, ContentBlock, IMAGE_OMITTED, Role, TOOL_IMAGE_OMITTED, resolve_media,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::openai_profile::{
    OpenAiAuthorization, OpenAiHeaderSource, OpenAiProviderProfile, OpenAiRequestPolicy, OpenAiWire,
};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

const MAX_ATTEMPTS: usize = 5;

const MAX_SEND_ATTEMPTS: usize = 3;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

const MAX_BACKOFF: Duration = Duration::from_secs(30);

const DEFAULT_STREAM_STALL: Duration = Duration::from_secs(300);

const RETRY_STREAM_STALL: Duration = Duration::from_secs(75);

const STREAM_STALL_MEMORY_SECS: i64 = 15 * 60;

static LAST_STREAM_STALL_SECS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[derive(Clone)]
pub struct OpenAiProvider {
    base_url: Url,
    api_key: String,
    model: String,
    http: reqwest::Client,
    reasoning_effort: Option<String>,

    temperature: Option<f32>,
    top_p: Option<f32>,

    session_id: Option<String>,

    profile: Option<OpenAiProviderProfile>,

    caps: ModelCaps,

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

        let http = Self::http_client(true);
        let model = model.into();

        Ok(Self {
            caps: ModelCaps::for_host(&base_url),
            base_url,
            api_key: api_key.into(),
            model,
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

    pub fn new_with_profile(
        model: impl Into<String>,
        reasoning_effort: Option<String>,
        session_id: Option<String>,
        profile: OpenAiProviderProfile,
        credential_source: Arc<dyn CredentialSource>,
    ) -> Result<Self, String> {
        let base_url = profile.base_url.clone();
        let http = Self::http_client(profile.follow_redirects);
        let model = model.into();
        Ok(Self {
            caps: ModelCaps::for_host(&base_url),
            base_url,
            api_key: String::new(),
            model,
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
            session_id,
            profile: Some(profile),
            credential_source: Some(credential_source),
        })
    }

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

    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    messages: Vec<OpenAiMessageRequest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolDefRequest>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_content: Option<OpenAiToolCallExtraContent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiToolCallExtraContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    google: Option<OpenAiGoogleToolCallContent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiGoogleToolCallContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
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

    #[serde(default)]
    reasoning_content: Option<String>,

    #[serde(default)]
    reasoning: Option<String>,

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
    #[serde(default)]
    extra_content: Option<OpenAiToolCallExtraContent>,
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

    #[serde(
        default,
        alias = "prompt_cache_hit_tokens",
        alias = "promptCacheHitTokens"
    )]
    cached_tokens: usize,

    #[serde(default, alias = "promptCacheMissTokens")]
    prompt_cache_miss_tokens: usize,

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

fn names_cap_max_completion_tokens(base_url: &Url) -> bool {
    base_url.host_str() == Some("api.openai.com")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CapField {
    #[default]
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ModelCaps {
    pub prefer_responses: bool,

    pub cap_field: CapField,
}

impl ModelCaps {
    pub(crate) fn for_host(base_url: &Url) -> Self {
        Self {
            prefer_responses: base_url.as_str().contains("opencode.ai/zen")
                || base_url.as_str().contains("commandcode.ai"),
            cap_field: if names_cap_max_completion_tokens(base_url) {
                CapField::MaxCompletionTokens
            } else {
                CapField::MaxTokens
            },
        }
    }
}

fn is_deepseek_model(model: &str) -> bool {
    model.to_lowercase().contains("deepseek")
}

pub(crate) fn chat_accepts(media_type: &str) -> bool {
    media_type.starts_with("image/")
        || media_type.starts_with("video/")
        || input_audio_format(media_type).is_some()
}

pub(crate) fn responses_accepts(media_type: &str) -> bool {
    media_type == "application/pdf"
        || media_type.starts_with("image/")
        || media_type.starts_with("video/")
}

pub(crate) fn wire_tool_output(content: &str, is_error: bool) -> String {
    if is_error {
        format!("Error: {content}")
    } else {
        content.to_string()
    }
}

fn image_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

fn input_audio_format(media_type: &str) -> Option<&'static str> {
    Some(match media_type {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/aiff" | "audio/x-aiff" => "aiff",
        "audio/aac" => "aac",
        "audio/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => return None,
    })
}

pub(crate) fn filter_valid_tools(
    tools: Vec<gray_core::message::ToolDef>,
) -> Vec<gray_core::message::ToolDef> {
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

fn chat_thinking_budget(effort: &str) -> u32 {
    match effort {
        "low" => 1024,
        "medium" => 4096,
        "max" => 32768,
        _ => 16384,
    }
}

fn map_chat_request(
    req: ChatRequest,
    model: &str,
    reasoning_effort: Option<&str>,
) -> Result<OpenAiChatRequest, ProviderError> {
    let max_tokens = req.max_tokens;
    let mut messages = Vec::new();

    if let Some(system) = req.system {
        messages.push(OpenAiMessageRequest {
            role: "system".to_string(),
            content: Some(Value::String(system)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
    }

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
                        ContentBlock::Image { .. } | ContentBlock::Media { .. } => {}
                        ContentBlock::StructuredInput { .. } => {}
                        ContentBlock::Thinking { text, .. } => {
                            if !text.is_empty() {
                                thinking_parts.push(text);
                            }
                        }
                        ContentBlock::ToolUse {
                            id,
                            name,
                            args,
                            thought_signature,
                        } => {
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
                                extra_content: thought_signature.filter(|s| s.model == model).map(
                                    |s| OpenAiToolCallExtraContent {
                                        google: Some(OpenAiGoogleToolCallContent {
                                            thought_signature: Some(s.signature),
                                        }),
                                    },
                                ),
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

                let reasoning_content = if is_deepseek_model(model) {
                    Some(thinking_parts.join("\n"))
                } else {
                    None
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

                    other => {
                        return Err(ProviderError::BadRequest(format!(
                            "unsupported message role for chat completions: {other}"
                        )));
                    }
                };

                let mut text_parts = Vec::new();

                let mut media_parts = Vec::new();
                let mut tool_results = Vec::new();

                let tool_images = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
                let content = resolve_media(
                    msg.content,
                    chat_accepts,
                    if tool_images {
                        TOOL_IMAGE_OMITTED
                    } else {
                        IMAGE_OMITTED
                    },
                );
                for block in content {
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
                            let url = image_data_url(&media_type, &data);
                            media_parts.push(
                                serde_json::json!({"type":"image_url","image_url":{"url": url}}),
                            );
                        }
                        ContentBlock::Media {
                            media_type, data, ..
                        } => match input_audio_format(&media_type) {
                            Some(format) => media_parts.push(serde_json::json!({
                                "type":"input_audio",
                                "input_audio":{"data": data, "format": format},
                            })),

                            None => media_parts.push(serde_json::json!({
                                "type":"video_url",
                                "video_url":{"url": image_data_url(&media_type, &data)},
                            })),
                        },
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => {
                            tool_results.push((id, wire_tool_output(&content, is_error)));
                        }
                        ContentBlock::ToolUse { id, name, args, .. } => {
                            tool_results.push((id, format!("{name}: {args}")));
                        }

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

                let has_media = !media_parts.is_empty();
                if !text_parts.is_empty() || has_media || messages.is_empty() {
                    let content = if has_media {
                        let mut arr = Vec::new();
                        for text in text_parts {
                            arr.push(serde_json::json!({"type":"text","text": text}));
                        }
                        arr.append(&mut media_parts);
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

    let mut ordered = Vec::with_capacity(messages.len());
    let mut deferred_images = Vec::new();
    let mut in_results = false;
    for message in messages {
        if in_results && message.role == "user" {
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
            let budget = chat_thinking_budget(eff);

            let thinking = max_tokens
                .is_none_or(|cap| budget < cap)
                .then(|| serde_json::json!({ "type": "enabled", "budget_tokens": budget }));
            (
                Some(eff.to_string()),
                Some(serde_json::json!({ "effort": eff })),
                thinking,
            )
        }
        None => (None, None, None),
    };

    Ok(OpenAiChatRequest {
        model: model.to_string(),
        stream: true,

        temperature: None,
        top_p: None,
        stream_options: Some(OpenAiStreamOptions {
            include_usage: true,
        }),
        reasoning_effort: reasoning_effort_val,
        reasoning: reasoning_val,
        thinking: thinking_val,
        max_tokens,
        max_completion_tokens: None,
        messages,
        tools,
        prompt_cache_key: None,
    })
}

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
    let mut reasoning = 0usize;
    if let Some(details) = &u.completion_tokens_details {
        reasoning = details.reasoning_tokens;
    }

    let mut cache_read = 0usize;
    let mut cache_write = 0usize;
    if let Some(details) = &u.prompt_tokens_details {
        cache_read = details.cached_tokens.max(details.cache_read_tokens);
        cache_write = details.cache_creation_tokens;
    }

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
        let inclusive = u.prompt_tokens + cache_read + cache_write;
        (inclusive, u.prompt_tokens)
    } else {
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

    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,

    store: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    include: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,

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
    let max_output_tokens = req.max_tokens.map(|n| n.max(16));
    let mut input: Vec<Value> = Vec::new();
    for msg in req.messages {
        match msg.role {
            Role::User | Role::System => {
                let role_str = match msg.role {
                    Role::System => "system",
                    _ => "user",
                };
                let mut text_parts: Vec<String> = Vec::new();
                let tool_images = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
                let content = resolve_media(
                    msg.content,
                    responses_accepts,
                    if tool_images {
                        TOOL_IMAGE_OMITTED
                    } else {
                        IMAGE_OMITTED
                    },
                );
                for block in content {
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
                            let url = image_data_url(&media_type, &data);
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            input.push(serde_json::json!({"role": "user", "content": [{"type":"input_image","image_url": url}]}));
                        }
                        ContentBlock::Media {
                            media_type, data, ..
                        } => {
                            let url = image_data_url(&media_type, &data);
                            if !text_parts.is_empty() {
                                input.push(serde_json::json!({"role": role_str, "content": text_parts.join("\n")}));
                                text_parts.clear();
                            }
                            let part = if media_type == "application/pdf" {
                                serde_json::json!({"type":"input_file","filename":"document.pdf","file_data": url})
                            } else {
                                serde_json::json!({"type":"input_video","video_url": url})
                            };
                            input.push(serde_json::json!({"role": "user", "content": [part]}));
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
                        ContentBlock::ToolUse { id, name, args, .. } => {
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
                        ContentBlock::Image { .. } | ContentBlock::Media { .. } => {}
                        ContentBlock::StructuredInput { .. } => {}
                        ContentBlock::Thinking {
                            encrypted_content: Some(ec),
                            item_id: Some(id),
                            model: Some(m),
                            ..
                        } if m == model => {
                            reasoning_items.push(serde_json::json!({"type": "reasoning", "id": id, "summary": [], "encrypted_content": ec}));
                        }
                        ContentBlock::Thinking {
                            text,
                            item_id,
                            model: m,
                            ..
                        } if !text.is_empty() && m.as_deref().is_none_or(|mm| mm == model) => {
                            let mut item = serde_json::json!({"type": "reasoning",
                                "summary": [{"type": "summary_text", "text": text}]});
                            if let Some(id) = item_id {
                                item["id"] = Value::String(id);
                            }
                            reasoning_items.push(item);
                        }
                        ContentBlock::Thinking { .. } => {}
                        ContentBlock::ToolUse { id, name, args, .. } => {
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

    let include = reasoning
        .as_ref()
        .map(|_| vec!["reasoning.encrypted_content".to_string()]);
    ResponsesRequest {
        model: model.to_string(),
        instructions,
        input,
        tools,
        stream: true,
        prompt_cache_key: session_id.filter(|s| !s.is_empty()).map(str::to_string),
        store: false,
        reasoning,
        include,
        max_output_tokens,
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

fn map_responses_reasoning(reasoning_effort: Option<&str>) -> Option<Value> {
    match reasoning_effort {
        None | Some("off") => None,
        Some(eff) => Some(serde_json::json!({"effort": eff, "summary": "auto"})),
    }
}

pub(crate) fn chat_has_reasoning_params(body: &OpenAiChatRequest) -> bool {
    body.reasoning_effort.is_some() || body.reasoning.is_some() || body.thinking.is_some()
}

pub(crate) fn strip_chat_reasoning_params(body: &mut OpenAiChatRequest) {
    body.reasoning_effort = None;
    body.reasoning = None;
    body.thinking = None;
}

pub(crate) fn responses_has_reasoning(body: &ResponsesRequest) -> bool {
    body.reasoning.is_some()
        || body
            .input
            .iter()
            .any(|i| i.get("type").and_then(Value::as_str) == Some("reasoning"))
}

pub(crate) fn strip_responses_reasoning(body: &mut ResponsesRequest) {
    body.reasoning = None;
    body.include = None;
    body.input
        .retain(|i| i.get("type").and_then(Value::as_str) != Some("reasoning"));
}

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

pub(crate) fn reasoning_conflict_hint(model: &str, snippet: &str) -> String {
    format!(
        "{snippet} [model '{model}' rejected reasoning params — set effort off in /model; no setting was changed]"
    )
}

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

fn error_body_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    let msg = v
        .pointer("/error/message")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)?
        .trim();
    (!msg.is_empty()).then(|| msg.to_string())
}

const QUOTA_EXHAUSTED_HINTS: [&str; 14] = [
    "insufficient_quota",
    "subscription_sharing_usage_limit_exceeded",
    "insufficient quota",
    "insufficient credits",
    "insufficient balance",
    "exceeded your current quota",
    "exceeds your allocated quota",
    "quota exceeded",
    "quota_exceeded",
    "gousagelimiterror",
    "available balance",
    "usage limit reached",
    "session limit",
    "resets in ~",
];

pub const QUOTA_VENDOR_CODE: &str = "quota_exhausted";

fn is_quota_code(code: &str) -> bool {
    code.split(';').next() == Some(QUOTA_VENDOR_CODE)
}

fn quota_hinted(text: &str) -> bool {
    let lower = text.to_lowercase();
    QUOTA_EXHAUSTED_HINTS.iter().any(|h| lower.contains(h))
}

fn quota_vendor_code(orig: Option<String>, resets_in: Option<Duration>) -> String {
    let mut code = String::from(QUOTA_VENDOR_CODE);
    if let Some(orig) = orig.filter(|s| !s.is_empty()) {
        code.push_str(&format!(";code={}", orig.replace([';', '='], "_")));
    }
    if let Some(d) = resets_in {
        code.push_str(&format!(";reset_in={}", d.as_secs()));
    }
    code
}

pub fn is_quota_exhausted(err: &ProviderError) -> bool {
    if let Some(code) = err.detail().and_then(|d| d.vendor_code.as_deref())
        && is_quota_code(code)
    {
        return true;
    }
    matches!(err.class(), ProviderError::Auth(msg) | ProviderError::BadRequest(msg) | ProviderError::RateLimited(msg) if quota_hinted(msg))
}

pub fn quota_reset_in(err: &ProviderError) -> Option<Duration> {
    if let Some(code) = err.detail().and_then(|d| d.vendor_code.as_deref())
        && is_quota_code(code)
    {
        return code
            .split(';')
            .skip(1)
            .find_map(|p| p.strip_prefix("reset_in="))
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
    }
    if is_quota_exhausted(err) {
        return parse_quota_reset_delay(&err.to_string());
    }
    None
}

fn error_body_code(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    v.pointer("/error/code")
        .or_else(|| v.pointer("/error/type"))
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|s| !s.is_empty())
}

pub(crate) fn classify_http_error(
    status: reqwest::StatusCode,
    snippet: &str,
    cf_ray: Option<&str>,
    req_id: Option<&str>,
) -> ProviderError {
    let class = classify_http_error_inner(status, snippet, cf_ray, req_id);
    let vendor_message = error_body_message(snippet);
    let mut vendor_code = error_body_code(snippet);
    let mut resets_in = None;
    if matches!(
        class.class(),
        ProviderError::Auth(_) | ProviderError::BadRequest(_)
    ) && quota_hinted(snippet)
    {
        resets_in = vendor_message
            .as_deref()
            .and_then(parse_quota_reset_delay)
            .or_else(|| parse_quota_reset_delay(snippet));
        vendor_code = Some(quota_vendor_code(vendor_code, resets_in));
    }
    ProviderError::Detailed {
        class: Box::new(class),
        detail: Box::new(gray_core::error::ProviderDetail {
            status: Some(status.as_u16()),
            vendor_code,
            vendor_message,
            cf_ray: cf_ray.map(str::to_string),
            request_id: req_id.map(str::to_string),
            retry_after_ms: resets_in.map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64),
        }),
    }
}

fn classify_http_error_inner(
    status: reqwest::StatusCode,
    snippet: &str,
    cf_ray: Option<&str>,
    req_id: Option<&str>,
) -> ProviderError {
    let shown = error_body_message(snippet).unwrap_or_else(|| snippet.to_string());
    let mut msg = if shown.is_empty() {
        format!("status {status}")
    } else {
        format!("status {status}: {shown}")
    };
    if let Some(ray) = cf_ray {
        msg.push_str(&format!(", cf-ray: {ray}"));
    }
    if let Some(rid) = req_id {
        msg.push_str(&format!(", request-id: {rid}"));
    }
    let lower = snippet.to_lowercase();

    if status == reqwest::StatusCode::FORBIDDEN
        && cf_ray.is_some()
        && lower.trim_start().starts_with("error code: 1")
    {
        return ProviderError::ServerError(format!("provider edge blocked the request ({msg})"));
    }

    const CONTEXT_OVERFLOW_HINTS: [&str; 4] = [
        "context length",
        "context window",
        "too many tokens",
        "prompt is too long",
    ];
    let max_tokens_qualified = lower.contains("max_tokens")
        && (lower.contains("context") || lower.contains("too long") || lower.contains("exceeded"));
    if !gray_core::overflow::is_non_overflow_message(snippet)
        && (CONTEXT_OVERFLOW_HINTS.iter().any(|h| lower.contains(h))
            || max_tokens_qualified
            || gray_core::overflow::is_context_overflow_message(snippet))
    {
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

    if is_unsupported {
        return ProviderError::BadRequest(msg);
    }
    let quota_exhausted = QUOTA_EXHAUSTED_HINTS.iter().any(|h| lower.contains(h));
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(msg),

        402 => ProviderError::Auth(msg),
        429 if quota_exhausted => ProviderError::Auth(msg),
        429 => ProviderError::RateLimited(msg),

        413 => ProviderError::ContextOverflow(format!(
            "context exhausted — start /new or compact ({msg})"
        )),
        400 | 404 => ProviderError::BadRequest(msg),

        500..=599 => ProviderError::ServerError(format!("provider-side {msg} (request was valid)")),
        _ => ProviderError::Stream(msg),
    }
}

pub(crate) fn is_retryable_error(err: &ProviderError) -> bool {
    err.retryable()
}

fn should_retry_request(err: &ProviderError, http_status: u16, attempt: usize) -> bool {
    if is_quota_exhausted(err) {
        return false;
    }
    if is_retryable_error(err) {
        return true;
    }
    http_status == 403 && attempt == 1 && matches!(err.class(), ProviderError::Auth(_))
}

fn is_midstream_transport(err: &SseReadError) -> bool {
    matches!(
        err,
        SseReadError::Upstream(eventsource_stream::EventStreamError::Transport(_))
    )
}

pub(crate) fn retry_notice_event(attempt: usize, max: usize, err: &ProviderError) -> StreamEvent {
    let details: String = err.to_string().chars().take(200).collect();
    StreamEvent::stream_error(format!("Reconnecting... {attempt}/{max}"), details)
}

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

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

    let target = days_from_civil(year, month, day) * 86400
        + hour as i64 * 3600
        + min as i64 * 60
        + sec as i64;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Duration::from_secs((target - now).max(0) as u64))
}

fn days_from_civil(year: i64, month: i64, day: u64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_quota_reset_delay(text: &str) -> Option<Duration> {
    let lower = text.to_lowercase();
    let mut offset = 0;
    while let Some(at) = lower[offset..].find("reset") {
        let tail = lower[offset + at + 5..].trim_start();
        offset += at + 5;
        let tail = tail.strip_prefix('s').map(str::trim_start).unwrap_or(tail);
        let tail = tail
            .strip_prefix("back")
            .map(str::trim_start)
            .unwrap_or(tail);
        for prep in ["in", "at", "on"] {
            let Some(rest) = tail.strip_prefix(prep).map(str::trim_start) else {
                continue;
            };
            let rest = rest.trim_start_matches(['~', '\u{2248}', ' ']);
            let rest = rest
                .strip_prefix("about")
                .map(str::trim_start)
                .unwrap_or(rest);
            if let Some(d) = parse_reset_duration(rest)
                .or_else(|| parse_reset_timestamp(rest))
                .or_else(|| parse_http_date_delay(rest))
            {
                return Some(d);
            }
        }
    }
    None
}

fn parse_reset_duration(s: &str) -> Option<Duration> {
    let nlen = s
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    if nlen == 0 {
        return None;
    }
    let num: f64 = s[..nlen].parse().ok()?;
    let unit = s[nlen..].trim_start();
    let ulen = unit.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
    let secs: u64 = match &unit[..ulen] {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 604800,
        _ => return None,
    };
    Some(Duration::from_secs_f64(num * secs as f64))
}

fn parse_reset_timestamp(s: &str) -> Option<Duration> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits >= 10 {
        let raw: i64 = s[..digits].parse().ok()?;
        let epoch = if digits >= 13 { raw / 1000 } else { raw };
        return Some(Duration::from_secs(epoch.saturating_sub(now).max(0) as u64));
    }
    if let Some(epoch) = parse_iso_reset_epoch(s) {
        return Some(Duration::from_secs(epoch.saturating_sub(now).max(0) as u64));
    }
    let (clock, _) = parse_reset_clock(s)?;
    let today = now - now.rem_euclid(86400);
    let mut target = today + clock;
    if target <= now {
        target += 86400;
    }
    Some(Duration::from_secs((target - now) as u64))
}

fn parse_iso_reset_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: u64 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let rest = s[10..].trim_start_matches(['T', ' ']);
    let (clock, used) = if rest.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        parse_reset_clock(rest)?
    } else {
        (0, 0)
    };
    let offset = parse_reset_tz_offset(rest[used..].trim_start());
    Some(days_from_civil(year, month, day) * 86400 + clock - offset)
}

fn parse_reset_clock(s: &str) -> Option<(i64, usize)> {
    let b = s.as_bytes();
    if b.len() < 5 || b[2] != b':' {
        return None;
    }
    let hour: i64 = s[0..2].parse().ok()?;
    let min: i64 = s[3..5].parse().ok()?;
    if hour > 23 || min > 59 {
        return None;
    }
    let mut sec = 0i64;
    let mut used = 5;
    if b.get(5) == Some(&b':') {
        sec = s.get(6..8)?.parse().ok()?;
        if sec > 60 {
            return None;
        }
        used = 8;
        if b.get(8) == Some(&b'.') {
            used += 1 + s[9..].bytes().take_while(|b| b.is_ascii_digit()).count();
        }
    }
    Some((hour * 3600 + min * 60 + sec, used))
}

fn parse_reset_tz_offset(s: &str) -> i64 {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1i64, &s[1..]),
        Some(b'-') => (-1i64, &s[1..]),
        _ => return 0,
    };
    let raw_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b':')
        .count();
    let raw = &rest[..raw_len];
    let (hour, min) = match raw.split_once(':') {
        Some((h, m)) => (h.parse().unwrap_or(0), m.parse().unwrap_or(0)),
        None if raw.len() > 2 => (
            raw[..raw.len() - 2].parse().unwrap_or(0),
            raw[raw.len() - 2..].parse().unwrap_or(0),
        ),
        None => (raw.parse().unwrap_or(0), 0),
    };
    sign * (hour * 3600 + min * 60)
}

pub(crate) fn backoff_delay(
    initial: Duration,
    attempt: usize,
    retry_after: Option<Duration>,
) -> Duration {
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

fn rate_limit_floor(failed_attempt: usize) -> Duration {
    Duration::from_secs(5 * (1u64 << failed_attempt.saturating_sub(1).min(6)))
}

fn retry_floor(
    err: &ProviderError,
    floor: Option<Duration>,
    failed_attempt: usize,
) -> Option<Duration> {
    if floor.is_none() && matches!(err.class(), ProviderError::RateLimited(_)) {
        return Some(rate_limit_floor(failed_attempt));
    }
    floor
}

fn session_affinity_headers(session_id: Option<&str>) -> Vec<(&'static str, &str)> {
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    vec![("x-opencode-session", sid)]
}

fn send_stage_retryable(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_request()
}

#[allow(clippy::result_large_err)]
async fn send_json_once(
    client: &reqwest::Client,
    url: &Url,
    api_key: &str,
    body: &Value,
    attempt: usize,
    session_id: Option<&str>,
) -> Result<reqwest::Response, (ProviderError, Option<Duration>, u16)> {
    let mut res_result = None;
    for send_attempt in 1..=MAX_SEND_ATTEMPTS {
        let base = if api_key.is_empty() {
            client.post(url.clone())
        } else {
            client
                .post(url.clone())
                .header("Authorization", format!("Bearer {api_key}"))
        };
        let base = session_affinity_headers(if url.as_str().contains("opencode.ai/zen") {
            session_id
        } else {
            None
        })
        .into_iter()
        .fold(base, |b, (name, value)| b.header(name, value));
        let req = base.header("Content-Type", "application/json").json(body);
        match req.send().await {
            Err(err)
                if send_attempt < MAX_SEND_ATTEMPTS && send_stage_retryable(&err) =>
            {
                log::warn!(target: "gray_provider", "send failed before a response ({err}); retrying on a fresh connection {send_attempt}/{MAX_SEND_ATTEMPTS}");
            }
            result => {
                res_result = Some(result);
                break;
            }
        }
    }
    let res_result = res_result.expect("send loop always yields a result");
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

type BoxedEventStream = BoxStream<'static, Result<eventsource_stream::Event, SseReadError>>;

#[derive(Debug)]
enum SseReadError {
    Upstream(eventsource_stream::EventStreamError<reqwest::Error>),
    Stalled(Duration),
}

impl std::fmt::Display for SseReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upstream(e) => write!(f, "{e}"),
            Self::Stalled(cap) => {
                write!(f, "stream stalled: no output for {}s", cap.as_secs())
            }
        }
    }
}

struct StallGuard {
    inner: BoxedEventStream,
    cap: Duration,
    timer: Pin<Box<tokio::time::Sleep>>,
    fired: bool,
}

impl Stream for StallGuard {
    type Item = Result<eventsource_stream::Event, SseReadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.fired {
            return self.inner.as_mut().poll_next(cx);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(item) => {
                if !matches!(&item, Some(Ok(ev)) if ev.event == "ping" || ev.data.trim().is_empty())
                {
                    let deadline = tokio::time::Instant::now() + self.cap;
                    self.timer.as_mut().reset(deadline);
                }
                Poll::Ready(item)
            }
            Poll::Pending => {
                if self.timer.as_mut().poll(cx).is_ready() {
                    self.fired = true;
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    LAST_STREAM_STALL_SECS.store(now, std::sync::atomic::Ordering::Relaxed);
                    return Poll::Ready(Some(Err(SseReadError::Stalled(self.cap))));
                }
                Poll::Pending
            }
        }
    }
}

fn env_secs(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(Duration::from_secs)
}

fn stream_stall_cap() -> Duration {
    let base = env_secs("GRAY_PROVIDER_STALL_SECS").unwrap_or(DEFAULT_STREAM_STALL);
    let last = LAST_STREAM_STALL_SECS.load(std::sync::atomic::Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < STREAM_STALL_MEMORY_SECS {
        base.min(env_secs("GRAY_PROVIDER_RETRY_STALL_SECS").unwrap_or(RETRY_STREAM_STALL))
    } else {
        base
    }
}

fn stall_guard(inner: BoxedEventStream, cap: Duration) -> BoxedEventStream {
    StallGuard {
        inner,
        cap,
        timer: Box::pin(tokio::time::sleep(cap)),
        fired: false,
    }
    .boxed()
}

fn sse_event_stream(response: reqwest::Response) -> BoxedEventStream {
    response
        .bytes_stream()
        .eventsource()
        .map(|item| item.map_err(SseReadError::Upstream))
        .boxed()
}

fn guarded_sse_event_stream(response: reqwest::Response) -> BoxedEventStream {
    stall_guard(sse_event_stream(response), stream_stall_cap())
}

#[derive(Default)]
struct DynamicToolCall {
    index: usize,
    name: String,
    args: String,
    done: bool,
}

#[derive(Default)]
struct EventTrail {
    count: usize,
    tail: VecDeque<String>,
}

impl EventTrail {
    const TAIL_CAP: usize = 8;

    fn push(&mut self, typ: &str) {
        self.count += 1;
        if self.tail.len() == Self::TAIL_CAP {
            self.tail.pop_front();
        }
        self.tail.push_back(typ.to_string());
    }

    fn tail_joined(&self) -> String {
        self.tail.iter().cloned().collect::<Vec<_>>().join(",")
    }
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
        trail: EventTrail,
        reasoning_deltas: HashSet<String>,
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
    let mut response = None;
    for send_attempt in 1..=MAX_SEND_ATTEMPTS {
        let send = client
            .post(url.clone())
            .headers(headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "text/event-stream")
            .json(body);
        match send.send().await {
            Err(err)
                if send_attempt < MAX_SEND_ATTEMPTS && send_stage_retryable(&err) =>
            {
                log::warn!(target: "gray_provider", "dynamic send failed before a response ({err}); retrying on a fresh connection {send_attempt}/{MAX_SEND_ATTEMPTS}");
            }
            Err(err) => return Err(map_send_error(err)),
            Ok(res) => {
                response = Some(res);
                break;
            }
        }
    }
    let response = response.expect("send loop always yields a response");
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

fn reasoning_summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn process_dynamic_event(
    value: Value,
    pending: &mut VecDeque<StreamEvent>,
    tools: &mut BTreeMap<String, DynamicToolCall>,
    next_index: &mut usize,
    last_usage: &mut Option<Usage>,
    reasoning_deltas: &mut HashSet<String>,
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
            if typ.ends_with("summary_text.delta")
                && let Some(id) = value.get("item_id").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                reasoning_deltas.insert(id.to_string());
            }
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
                let summary = item.map(reasoning_summary_text).unwrap_or_default();
                if !summary.is_empty() && !reasoning_deltas.contains(id) {
                    pending.push_back(StreamEvent::thinking_delta(summary));
                }
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
            return Err(stream_failure(&value, detail.to_string()));
        }
        _ => {}
    }
    Ok(false)
}

fn stream_failure(event: &Value, detail: String) -> ProviderError {
    let code = event
        .get("response")
        .and_then(|r| r.get("error"))
        .or_else(|| event.get("error"))
        .unwrap_or(event)
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let lower = detail.to_lowercase();
    if QUOTA_EXHAUSTED_HINTS
        .iter()
        .any(|h| code.contains(h) || lower.contains(h))
    {
        let resets_in = parse_quota_reset_delay(&detail);
        return ProviderError::Detailed {
            class: Box::new(ProviderError::Auth(detail.clone())),
            detail: Box::new(gray_core::error::ProviderDetail {
                status: None,
                vendor_code: Some(quota_vendor_code(
                    (!code.is_empty()).then_some(code),
                    resets_in,
                )),
                vendor_message: Some(detail),
                cf_ray: None,
                request_id: None,
                retry_after_ms: resets_in.map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64),
            }),
        };
    }
    ProviderError::Stream(detail)
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
                        // No stall guard: plugin relays are turn-settled — silence
                        // until the turn completes is the contract, not a dead
                        // connection. The sidecar's own turn deadline bounds it.
                        let event_stream = sse_event_stream(response);
                        state = DynamicState::Streaming {
                            event_stream,
                            tools: BTreeMap::new(),
                            next_index: 0,
                            pending: VecDeque::new(),
                            last_usage: None,
                            completed: false,
                            trail: EventTrail::default(),
                            reasoning_deltas: HashSet::new(),
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
                mut trail,
                mut reasoning_deltas,
            } => {
                if completed && pending.is_empty() {
                    return None;
                }
                let next = event_stream.next().await;
                match next {
                    None => {
                        log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={}",
                            trail.count,
                            trail.tail_joined(),
                            tools.values().filter(|c| !c.done).count());
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
                                trail,
                                reasoning_deltas,
                            };
                            continue;
                        }
                        if data == "[DONE]" {
                            log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={}",
                                trail.count,
                                trail.tail_joined(),
                                tools.values().filter(|c| !c.done).count());
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
                        trail.push(value.get("type").and_then(|v| v.as_str()).unwrap_or(""));
                        match process_dynamic_event(
                            value,
                            &mut pending,
                            &mut tools,
                            &mut next_index,
                            &mut last_usage,
                            &mut reasoning_deltas,
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
                                trail,
                                reasoning_deltas,
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
                            trail,
                            reasoning_deltas,
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
        session_id: Option<String>,
        cache_off: bool,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let mut body = map_chat_to_responses(
            request,
            &self.model,
            session_id.as_deref(),
            self.reasoning_effort.as_deref(),
        );
        apply_dynamic_policy(&mut body, &profile.request, session_id.as_deref());
        if cache_off {
            body.prompt_cache_key = None;
        }

        let profile_url = match responses_url(&profile.base_url) {
            Ok(url) => url,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        let client = self.http.clone();
        let started = stream::once(async move {
            credential_source
                .acquire()
                .await
                .map_err(|e| ProviderError::Auth(format!("provider credential unavailable: {e:#}")))
                .and_then(|lease| {
                    let url = match lease.metadata.get("relay_url") {
                        Some(relay) => relay.parse::<Url>().map_err(|e| {
                            ProviderError::BadRequest(format!("invalid relay URL: {e}"))
                        }),
                        None => Ok(profile_url.clone()),
                    }?;
                    Ok((lease, body, url))
                })
        });
        started
            .flat_map(move |result| match result {
                Ok((lease, body, url)) => stream::unfold(
                    DynamicState::Init {
                        client: client.clone(),
                        url,
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

#[allow(clippy::large_enum_variant)]
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

        tools_by_call_id: BTreeMap<String, (usize, String, String)>,
        index_to_call_id: BTreeMap<usize, String>,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, AccumulatedToolCall>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,

        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        attempt: usize,

        emitted: bool,
    },
    ResponsesStreaming {
        event_stream: BoxedEventStream,

        tools_by_call_id: BTreeMap<String, (usize, String, String)>,

        index_to_call_id: BTreeMap<usize, String>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,

        saw_output_text: bool,

        last_text_part: Option<(u64, u64)>,

        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        stream_attempt: usize,
        last_response_id: Option<String>,
        trail: EventTrail,
        reasoning_deltas: HashSet<String>,
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

#[derive(Default)]
struct AccumulatedToolCall {
    id: String,
    name: String,
    args: String,
    thought_signature: Option<String>,
}

fn emit_tool_calls_and_completion(
    accumulated_tools: &mut BTreeMap<usize, AccumulatedToolCall>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    pending_events: &mut VecDeque<StreamEvent>,
) -> Result<(), ProviderError> {
    for (index, call) in std::mem::take(accumulated_tools) {
        if call.name.is_empty() && call.args.trim().is_empty() {
            log::debug!(target: "gray_provider", "skipping ghost tool-call fragment at index {index} (no name, no args)");
            continue;
        }
        let args_fixed = normalize_tool_args(&call.args, index)?;
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index,
            id: (!call.id.is_empty()).then_some(call.id),
            name: (!call.name.is_empty()).then_some(call.name),
            arguments_delta: args_fixed,
            thought_signature: call.thought_signature,
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
    let mut ordered: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for (call_id, (idx, name, args)) in std::mem::take(tools_by_call_id) {
        ordered.insert(idx, (call_id, name, args));
    }
    index_to_call_id.clear();
    for (idx, (call_id, name, args)) in ordered {
        if name.is_empty() && args.trim().is_empty() {
            log::debug!(target: "gray_provider", "skipping ghost Responses tool-call fragment at index {idx} (no name, no args)");
            continue;
        }
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
            thought_signature: None,
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

type ResponsesToolsByCallId = BTreeMap<String, (usize, String, String)>;

type ResponsesIndexToCallId = BTreeMap<usize, String>;

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
                            let event_stream: BoxedEventStream = guarded_sse_event_stream(response);
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
                            if should_retry_request(&err, http_status, attempt)
                                && attempt < MAX_ATTEMPTS
                            {
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
                            let event_stream: BoxedEventStream = guarded_sse_event_stream(response);

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

                                last_text_part: None,
                                trail: EventTrail::default(),
                                reasoning_deltas: HashSet::new(),
                            };
                        }
                        Err((err, floor, http_status)) => {
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
                            if should_retry_request(&err, http_status, attempt)
                                && attempt < MAX_ATTEMPTS
                            {
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
                                                    let signature = tc
                                                        .extra_content
                                                        .and_then(|extra| extra.google)
                                                        .and_then(|google| google.thought_signature)
                                                        .filter(|s| !s.is_empty());
                                                    let carries_anything = tc.id.is_some()
                                                        || signature.is_some()
                                                        || tc.function.as_ref().is_some_and(|f| {
                                                            f.name.is_some()
                                                                || f.arguments.is_some()
                                                        });
                                                    if !carries_anything {
                                                        continue;
                                                    }
                                                    let entry = accumulated_tools
                                                        .entry(tc.index)
                                                        .or_default();
                                                    if let Some(signature) = signature {
                                                        if entry
                                                            .thought_signature
                                                            .as_ref()
                                                            .is_some_and(|s| s != &signature)
                                                        {
                                                            return Some((
                                                                Err(ProviderError::Stream(
                                                                    format!(
                                                                        "provider changed thought signature for tool-call index {}",
                                                                        tc.index
                                                                    ),
                                                                )),
                                                                StreamState::Done,
                                                            ));
                                                        }
                                                        entry.thought_signature = Some(signature);
                                                    }
                                                    if let Some(id) = tc.id {
                                                        entry.id.push_str(&id);
                                                    }
                                                    if let Some(func) = tc.function {
                                                        if let Some(name) = func.name {
                                                            entry.name.push_str(&name);
                                                        }
                                                        if let Some(args) = func.arguments {
                                                            entry.args.push_str(&args);
                                                        }
                                                    }
                                                }
                                            }

                                            if let Some(reason_str) = choice.finish_reason {
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
                            if emitted && accumulated_tools.is_empty() {
                                let details = err.to_string().chars().take(200).collect::<String>();
                                pending_events.push_back(StreamEvent::stream_error(
                                    "Connection interrupted; partial answer kept",
                                    format!("{details}; send another message to retry"),
                                ));
                                pending_events.push_back(StreamEvent::MessageComplete {
                                    stop_reason: Some(StopReason::EndTurn),
                                    usage: last_usage,
                                });
                                completed = true;
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
                                continue;
                            }
                            return Some((
                                Err(ProviderError::Stream(err.to_string())),
                                StreamState::Done,
                            ));
                        }
                        None => {
                            if last_finish_reason.is_none() {
                                let complete = accumulated_tools.values().all(|call| {
                                    let trimmed = call.args.trim();
                                    !trimmed.is_empty()
                                        && serde_json::from_str::<Value>(trimmed).is_ok()
                                });
                                if completed || accumulated_tools.is_empty() || !complete {
                                    let err = ProviderError::Stream(
                                        "Chat stream ended without a finish reason".into(),
                                    );

                                    if !emitted && !completed && attempt < MAX_ATTEMPTS {
                                        log::warn!(
                                            target: "gray_provider",
                                            "stream ended without a finish reason before any delta; retrying (attempt {attempt})"
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

                                        if attempt == 1 {
                                            let notice =
                                                retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                            return Some((Ok(notice), next));
                                        }
                                        state = next;
                                        continue;
                                    }
                                    return Some((Err(err), StreamState::Done));
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
                    mut last_text_part,
                    client,
                    url,
                    api_key,
                    body,
                    stream_attempt,
                    mut last_response_id,
                    mut trail,
                    mut reasoning_deltas,
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
                                last_text_part,
                                trail,
                                reasoning_deltas,
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
                                    last_text_part,
                                    trail,
                                    reasoning_deltas,
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
                            trail.push(typ);

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

                                        let part = (
                                            value
                                                .get("output_index")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            value
                                                .get("content_index")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                        );
                                        let mut delta = delta.to_string();
                                        if last_text_part.is_some_and(|p| p != part) {
                                            delta.insert_str(0, "\n\n");
                                        }
                                        last_text_part = Some(part);
                                        pending_events.push_back(StreamEvent::TextDelta { delta });
                                    }
                                }
                                "response.reasoning_text.delta"
                                | "response.reasoning_summary_text.delta" => {
                                    if typ.ends_with("summary_text.delta")
                                        && let Some(id) =
                                            value.get("item_id").and_then(|v| v.as_str())
                                        && !id.is_empty()
                                    {
                                        reasoning_deltas.insert(id.to_string());
                                    }
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
                                        Err(stream_failure(
                                            &value,
                                            format!("responses stream failed: {detail}"),
                                        )),
                                        StreamState::Done,
                                    ));
                                }
                                "response.output_item.done" => {
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
                                        let summary = reasoning_summary_text(item);
                                        if !summary.is_empty() && !reasoning_deltas.contains(id) {
                                            pending_events
                                                .push_back(StreamEvent::thinking_delta(summary));
                                        }
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
                                last_text_part,
                                trail,
                                reasoning_deltas,
                            };
                        }
                        Some(Err(err)) => {
                            if stream_attempt < MAX_ATTEMPTS
                                && !matches!(err, SseReadError::Stalled(_))
                            {
                                log::warn!(target: "gray_provider", "responses stream error, resuming (stream attempt {stream_attempt}): {err}");

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
                                    last_text_part,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    stream_attempt,
                                    last_response_id,
                                    trail,
                                    reasoning_deltas,
                                };
                                continue;
                            }

                            let pending_tool_calls = tools_by_call_id
                                .values()
                                .map(|(idx, _, _)| *idx)
                                .collect::<std::collections::BTreeSet<_>>()
                                .len();
                            log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={} saw_output_text={} response_id={}",
                                trail.count,
                                trail.tail_joined(),
                                pending_tool_calls,
                                saw_output_text,
                                if last_response_id.is_some() { "some" } else { "none" });
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

    fn warm_output_cap(&self, _req: &ChatRequest) -> u32 {
        1
    }

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let iso = current_isolated_stream().unwrap_or_default();
        let session_id = iso.session_id.or_else(|| self.session_id.clone());
        let cache_off = iso.cache_off;
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
            return self.dynamic_stream(req, profile, credential_source, session_id, cache_off);
        }
        if self.caps.prefer_responses {
            let url = match responses_url(&self.base_url) {
                Ok(u) => u,
                Err(e) => return stream::once(async move { Err(e) }).boxed(),
            };
            let mut body = map_chat_to_responses(
                req,
                &self.model,
                session_id.as_deref(),
                self.reasoning_effort.as_deref(),
            );
            if cache_off {
                body.prompt_cache_key = None;
            }
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

        body.prompt_cache_key = if cache_off {
            None
        } else {
            session_id.clone().filter(|s| !s.is_empty())
        };
        body.temperature = self.temperature;
        body.top_p = self.top_p;
        if self.caps.cap_field == CapField::MaxCompletionTokens {
            body.max_completion_tokens = body.max_tokens.take();
        }
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            session_id: session_id.clone(),
            attempt: 1,
            retry_after: None,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

#[path = "openai_tests.rs"]
#[cfg(test)]
mod tests;

#[path = "openai_signature_tests.rs"]
#[cfg(test)]
mod signature_tests;
