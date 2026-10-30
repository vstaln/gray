use super::*;

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiChatRequest {
    pub(super) model: String,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<OpenAiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thinking: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_completion_tokens: Option<u32>,
    pub(super) messages: Vec<OpenAiMessageRequest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<OpenAiToolDefRequest>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prompt_cache_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiStreamOptions {
    pub(super) include_usage: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiMessageRequest {
    pub(super) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<OpenAiToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiToolCallRequest {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) call_type: String,
    pub(super) function: OpenAiFunctionCallRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) extra_content: Option<OpenAiToolCallExtraContent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct OpenAiToolCallExtraContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) google: Option<OpenAiGoogleToolCallContent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct OpenAiGoogleToolCallContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) thought_signature: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiFunctionCallRequest {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiToolDefRequest {
    #[serde(rename = "type")]
    pub(super) tool_type: String,
    pub(super) function: OpenAiFunctionDefRequest,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiFunctionDefRequest {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiChunk {
    #[serde(default)]
    pub(super) choices: Vec<OpenAiChoiceChunk>,
    #[serde(default)]
    pub(super) usage: Option<OpenAiUsageChunk>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiChoiceChunk {
    #[serde(default)]
    pub(super) delta: OpenAiDeltaChunk,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct OpenAiDeltaChunk {
    #[serde(default)]
    pub(super) content: Option<String>,

    #[serde(default)]
    pub(super) reasoning_content: Option<String>,

    #[serde(default)]
    pub(super) reasoning: Option<String>,

    #[serde(default)]
    pub(super) thought: Option<String>,
    #[serde(default)]
    pub(super) thoughts: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<OpenAiToolCallChunk>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiToolCallChunk {
    pub(super) index: usize,
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) function: Option<OpenAiFunctionChunk>,
    #[serde(default)]
    pub(super) extra_content: Option<OpenAiToolCallExtraContent>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiFunctionChunk {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiUsageChunk {
    #[serde(default, alias = "input_tokens")]
    pub(super) prompt_tokens: usize,
    #[serde(default, alias = "output_tokens")]
    pub(super) completion_tokens: usize,
    #[serde(default, alias = "output_tokens_details")]
    pub(super) completion_tokens_details: Option<OpenAiCompletionDetails>,
    #[serde(default, alias = "input_tokens_details")]
    pub(super) prompt_tokens_details: Option<OpenAiPromptDetails>,

    #[serde(
        default,
        alias = "prompt_cache_hit_tokens",
        alias = "promptCacheHitTokens"
    )]
    pub(super) cached_tokens: usize,

    #[serde(default, alias = "promptCacheMissTokens")]
    pub(super) prompt_cache_miss_tokens: usize,

    #[serde(
        default,
        alias = "cache_creation_input_tokens",
        alias = "cacheCreationInputTokens"
    )]
    pub(super) cache_creation_input_tokens: usize,
    #[serde(
        default,
        alias = "cache_read_input_tokens",
        alias = "cacheReadInputTokens"
    )]
    pub(super) cache_read_input_tokens: usize,

    #[serde(default, alias = "total_tokens", alias = "totalTokens")]
    pub(super) total_tokens: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiCompletionDetails {
    #[serde(default, alias = "reasoningTokens")]
    pub(super) reasoning_tokens: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiPromptDetails {
    #[serde(default)]
    pub(super) cached_tokens: usize,
    #[serde(
        default,
        alias = "cache_creation_tokens",
        alias = "cacheCreationTokens"
    )]
    pub(super) cache_creation_tokens: usize,
    #[serde(default, alias = "cache_read_tokens", alias = "cacheReadTokens")]
    pub(super) cache_read_tokens: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesRequest {
    pub(super) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) instructions: Option<String>,
    pub(super) input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<ResponsesTool>,
    pub(super) stream: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prompt_cache_key: Option<String>,

    pub(super) store: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) include: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_output_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct ResponsesTool {
    #[serde(rename = "type")]
    pub(super) tool_type: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: Value,
}
