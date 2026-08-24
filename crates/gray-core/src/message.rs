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

    /// Extracts the text content if this block is a text variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_role_serde_roundtrip() {
        for role in [Role::System, Role::User, Role::Assistant] {
            let json_str = serde_json::to_string(&role).expect("serialize role");
            let deserialized: Role =
                serde_json::from_str(&json_str).expect("deserialize role");
            assert_eq!(role, deserialized);
        }
    }

    #[test]
    fn test_content_block_text_serde() {
        let block = ContentBlock::text("Hello, world!");
        let json_str = serde_json::to_string(&block).expect("serialize text block");
        assert!(json_str.contains(r#""type":"text""#));
        assert!(json_str.contains(r#""text":"Hello, world!""#));

        let deserialized: ContentBlock =
            serde_json::from_str(&json_str).expect("deserialize text block");
        assert_eq!(block, deserialized);
    }

    #[test]
    fn test_content_block_tool_use_serde() {
        let block = ContentBlock::tool_use(
            "call_abc123",
            "bash",
            json!({ "command": "cargo test" }),
        );
        let json_str = serde_json::to_string(&block).expect("serialize tool use block");
        assert!(json_str.contains(r#""type":"tool_use""#));
        assert!(json_str.contains(r#""id":"call_abc123""#));
        assert!(json_str.contains(r#""name":"bash""#));

        let deserialized: ContentBlock =
            serde_json::from_str(&json_str).expect("deserialize tool use block");
        assert_eq!(block, deserialized);
    }

    #[test]
    fn test_content_block_tool_result_serde() {
        let block = ContentBlock::tool_result("call_abc123", "success output", false);
        let json_str = serde_json::to_string(&block).expect("serialize tool result block");
        assert!(json_str.contains(r#""type":"tool_result""#));
        assert!(json_str.contains(r#""id":"call_abc123""#));
        assert!(json_str.contains(r#""content":"success output""#));
        assert!(json_str.contains(r#""is_error":false"#));

        let deserialized: ContentBlock =
            serde_json::from_str(&json_str).expect("deserialize tool result block");
        assert_eq!(block, deserialized);

        // Error result
        let err_block = ContentBlock::tool_result("call_abc123", "command failed", true);
        let err_json = serde_json::to_string(&err_block).expect("serialize error result");
        let deserialized_err: ContentBlock =
            serde_json::from_str(&err_json).expect("deserialize error result");
        assert_eq!(err_block, deserialized_err);
    }

    #[test]
    fn test_message_serde_roundtrip() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("Running command now..."),
                ContentBlock::tool_use(
                    "call_1",
                    "bash",
                    json!({ "command": "ls -la" }),
                ),
            ],
        };

        let json_str = serde_json::to_string(&msg).expect("serialize message");
        let deserialized: Message =
            serde_json::from_str(&json_str).expect("deserialize message");
        assert_eq!(msg, deserialized);
        assert_eq!(msg.text_content(), "Running command now...");
    }

    #[test]
    fn test_tool_def_serde_roundtrip() {
        let tool = ToolDef::new(
            "bash",
            "Execute a bash command in the terminal",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to run"
                    }
                },
                "required": ["command"]
            }),
        );

        let json_str = serde_json::to_string(&tool).expect("serialize tool def");
        let deserialized: ToolDef =
            serde_json::from_str(&json_str).expect("deserialize tool def");
        assert_eq!(tool, deserialized);
    }

    #[test]
    fn test_chat_request_serde_roundtrip() {
        let req = ChatRequest::new(vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello!"),
        ])
        .with_system("Global system prompt")
        .with_tools(vec![ToolDef::new(
            "read",
            "Read file",
            json!({ "type": "object" }),
        )]);

        let json_str = serde_json::to_string(&req).expect("serialize chat request");
        let deserialized: ChatRequest =
            serde_json::from_str(&json_str).expect("deserialize chat request");
        assert_eq!(req, deserialized);
    }
}
