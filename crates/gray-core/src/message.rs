use serde::{Deserialize, Serialize};

/// Role of a message sender in a conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
        }
    }
}

/// A distinct content block within a conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text { text: String },
    /// Image content (base64-encoded).
    Image {
        media_type: String,
        data: String,
    },
    /// A tool invocation requested by the model.
    ToolUse {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// The result of executing a tool invocation.
    ToolResult {
        id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// The model's reasoning / chain-of-thought for an assistant turn.
    /// Displayed dim+italic in the REPL (mirrors pi's thinking blocks).
    /// `encrypted_content`/`item_id` round-trip Responses-API reasoning items
    /// (pi_agent_rust parity) so the server keeps its prompt-cache shard warm;
    /// `model` gates replay to the model that produced them (a foreign model
    /// cannot decrypt the blob). All optional → old session files still parse.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

impl ContentBlock {
    /// Creates a new text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Creates a new tool use block.
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            args,
        }
    }

    /// Creates a new thinking block (no replay data; the provider attaches
    /// that when it captures a reasoning item from the stream).
    pub fn thinking(text: impl Into<String>) -> Self {
        Self::Thinking { text: text.into(), encrypted_content: None, item_id: None, model: None }
    }

    /// Creates a new image block (base64 data).
    pub fn image(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image {
            media_type: media_type.into(),
            data: data.into(),
        }
    }

    /// Creates a new tool result block.
    pub fn tool_result(
        id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            id: id.into(),
            content: content.into(),
            is_error,
        }
    }

}

/// A conversation turn message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Creates a new message with the given role and content blocks.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    /// Helper to create a user message containing a single text block.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Helper to create an assistant message containing a single text block.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Helper to create a system message containing a single text block.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Concatenates all text blocks in the message.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Everything in this message that the provider will bill as context —
    /// not just its prose.
    ///
    /// [`text_content`](Self::text_content) is a *display* accessor: it keeps
    /// `Text` blocks and drops tool results, tool arguments, replayed
    /// reasoning blobs and image payloads. Those are precisely the blocks
    /// that dominate a coding session, so any budgeting code that measures
    /// with `text_content` scores a 50 KiB tool result as zero tokens.
    /// Size-estimation callers must use this instead.
    ///
    /// Concatenation order and separators are irrelevant to callers: only the
    /// resulting length is meaningful. Kept deliberately allocation-simple —
    /// it runs once per message per compaction, not per token.
    pub fn context_text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            let piece = match block {
                ContentBlock::Text { text } => text.clone(),
                ContentBlock::ToolResult { content, .. } => content.clone(),
                ContentBlock::ToolUse { name, args, .. } => {
                    format!("{name}{args}")
                }
                ContentBlock::Thinking { text, encrypted_content, .. } => {
                    // The encrypted blob is replayed verbatim next turn to
                    // keep the provider's cache shard warm, and is billed
                    // like any other input token. It must be counted.
                    match encrypted_content {
                        Some(blob) => format!("{text}{blob}"),
                        None => text.clone(),
                    }
                }
                // Base64 payload length is the only size signal available
                // here; providers re-encode, so this is an approximation in
                // the same spirit as the chars/4 heuristic downstream.
                ContentBlock::Image { data, .. } => data.clone(),
            };
            if !piece.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&piece);
            }
        }
        out
    }
}

/// Definition of a tool available to the agent model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDef {
    /// Creates a new tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Request payload sent to an LLM provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
}

impl ChatRequest {
    /// Creates a new chat request with the given messages.
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            system: None,
            messages,
            tools: Vec::new(),
        }
    }

    /// Sets the optional system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Sets the available tools.
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }
}

