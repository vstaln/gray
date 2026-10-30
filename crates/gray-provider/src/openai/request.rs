use super::*;

pub(super) fn names_cap_max_completion_tokens(base_url: &Url) -> bool {
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

pub(super) fn is_deepseek_model(model: &str) -> bool {
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

pub(super) fn image_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

pub(super) fn input_audio_format(media_type: &str) -> Option<&'static str> {
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

pub(super) fn is_valid_tool_name(name: &str, id: &str) -> bool {
    if name.trim().is_empty() {
        log::warn!(target: "gray_provider", "dropping assistant tool call {id} with empty name");
        false
    } else {
        true
    }
}

pub(super) fn chat_thinking_budget(effort: &str) -> u32 {
    match effort {
        "low" => 1024,
        "medium" => 4096,
        "max" => 32768,
        _ => 16384,
    }
}

pub(super) fn map_chat_request(
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

pub(super) fn serialize_body<T: Serialize>(
    body: &T,
    what: &'static str,
) -> Result<Value, ProviderError> {
    serde_json::to_value(body)
        .map_err(|e| ProviderError::BadRequest(format!("failed to serialize {what}: {e}")))
}

pub(super) fn map_finish_reason(reason: &str) -> Option<StopReason> {
    match reason {
        "stop" => Some(StopReason::EndTurn),
        "tool_calls" => Some(StopReason::ToolUse),
        "length" => Some(StopReason::MaxTokens),
        "cancelled" | "canceled" => Some(StopReason::Cancelled),
        "error" => Some(StopReason::Error),
        _ => None,
    }
}

pub(super) fn map_usage(u: &OpenAiUsageChunk) -> Usage {
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

pub(super) fn chat_completions_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url_str = base_url.as_str().trim_end_matches('/').to_string();
    url_str.push_str("/chat/completions");
    Url::parse(&url_str)
        .map_err(|e| ProviderError::BadRequest(format!("invalid base URL '{base_url}': {e}")))
}

pub(super) fn responses_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url_str = base_url.as_str().trim_end_matches('/').to_string();
    url_str.push_str("/responses");
    Url::parse(&url_str)
        .map_err(|e| ProviderError::BadRequest(format!("invalid base URL '{base_url}': {e}")))
}

pub(super) fn map_chat_to_responses(
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

pub(super) fn apply_dynamic_policy(
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

pub(super) fn map_responses_reasoning(reasoning_effort: Option<&str>) -> Option<Value> {
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
