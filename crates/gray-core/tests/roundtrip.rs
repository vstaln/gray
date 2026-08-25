use gray_core::{
    AgentEvent, ChatRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolDef, Usage,
};
use serde_json::json;

#[test]
fn test_full_chat_request_with_all_content_blocks_and_tools_roundtrip() {
    let tools = vec![
        ToolDef::new(
            "bash",
            "Execute a bash command in a subprocess",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run"
                    }
                },
                "required": ["command"]
            }),
        ),
        ToolDef::new(
            "read",
            "Read file contents with offset/limit support",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative file path"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line offset to start reading from"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of lines to read"
                    }
                },
                "required": ["path"]
            }),
        ),
    ];

    // Message containing all ContentBlock variants
    let composite_message = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::text("Let me inspect the file and run a command."),
            ContentBlock::tool_use(
                "tool_use_1",
                "read",
                json!({ "path": "src/lib.rs", "limit": 20 }),
            ),
            ContentBlock::tool_result(
                "tool_use_1",
                "pub mod message;\npub mod event;",
                false,
            ),
            ContentBlock::tool_use(
                "tool_use_2",
                "bash",
                json!({ "command": "cargo check" }),
            ),
            ContentBlock::tool_result(
                "tool_use_2",
                "error: failed to compile",
                true,
            ),
            ContentBlock::text("Encountered an error, fixing now."),
        ],
    );

    let messages = vec![
        Message::system("You are gray, a minimal agent running on the user's machine."),
        Message::user("Please build and test the project."),
        composite_message,
        Message::assistant("Done! All tests are passing."),
    ];

    let request = ChatRequest::new(messages)
        .with_system("Global system prompt configuration")
        .with_tools(tools);

    // Serialize to JSON string
    let json_output = serde_json::to_string_pretty(&request)
        .expect("Serialization of full ChatRequest must succeed");

    // Deserialize back
    let roundtripped: ChatRequest = serde_json::from_str(&json_output)
        .expect("Deserialization of full ChatRequest must succeed");

    // Assert full equality
    assert_eq!(request, roundtripped);

    // Validate structural fields in JSON
    let parsed_json: serde_json::Value = serde_json::from_str(&json_output)
        .expect("Must parse into serde_json::Value");

    assert_eq!(
        parsed_json["system"],
        json!("Global system prompt configuration")
    );
    assert_eq!(parsed_json["tools"].as_array().map(Vec::len), Some(2));
    assert_eq!(parsed_json["messages"].as_array().map(Vec::len), Some(4));

    // Validate ContentBlock tagging in serialized JSON
    let blocks = &parsed_json["messages"][2]["content"];
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "Let me inspect the file and run a command.");

    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["id"], "tool_use_1");
    assert_eq!(blocks[1]["name"], "read");
    assert_eq!(blocks[1]["args"]["path"], "src/lib.rs");

    assert_eq!(blocks[2]["type"], "tool_result");
    assert_eq!(blocks[2]["id"], "tool_use_1");
    assert_eq!(blocks[2]["content"], "pub mod message;\npub mod event;");
    assert_eq!(blocks[2]["is_error"], false);

    assert_eq!(blocks[3]["type"], "tool_use");
    assert_eq!(blocks[3]["id"], "tool_use_2");
    assert_eq!(blocks[3]["name"], "bash");

    assert_eq!(blocks[4]["type"], "tool_result");
    assert_eq!(blocks[4]["id"], "tool_use_2");
    assert_eq!(blocks[4]["is_error"], true);
}

#[test]
fn test_agent_events_stream_roundtrip_integration() {
    let events = vec![
        AgentEvent::Start,
        AgentEvent::text_delta("Searching files..."),
        AgentEvent::tool_call_start("call_001", "bash"),
        AgentEvent::tool_call_end("call_001", json!({"command": "cargo test"})),
        AgentEvent::tool_result("call_001", "test result: ok. 5 passed", false),
        AgentEvent::turn_end(
            StopReason::EndTurn,
            Usage {
                input_tokens: 320,
                output_tokens: 85,
                ..Default::default()
            },
        ),
    ];

    let json_lines: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).expect("serialize event"))
        .collect();

    let deserialized: Vec<AgentEvent> = json_lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("deserialize event"))
        .collect();

    assert_eq!(events, deserialized);
}

#[test]
fn test_stream_events_roundtrip_integration() {
    let events = vec![
        StreamEvent::text_delta("Hello"),
        StreamEvent::tool_call_delta(
            0,
            Some("call_1".to_string()),
            Some("read".to_string()),
            "{\"path\":",
        ),
        StreamEvent::tool_call_delta(0, None, None, "\"Cargo.toml\"}"),
        StreamEvent::message_complete(
            Some(StopReason::ToolUse),
            Some(Usage::new(400, 50)),
        ),
    ];

    let json_lines: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).expect("serialize stream event"))
        .collect();

    let deserialized: Vec<StreamEvent> = json_lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("deserialize stream event"))
        .collect();

    assert_eq!(events, deserialized);
}
