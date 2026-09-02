//! Context compaction and summarization for Gray conversations.
//!
//! Ported from the gold standard compaction architecture in pi / prime-agent:
//! serializes conversation history to plain text with truncated tool outputs,
//! prompts the LLM to produce a structured summary (Goal, Constraints, Progress Done/In Progress,
//! Key Decisions, Next Steps, Critical Context), and condenses the active conversation history.

use gray_core::event::Usage;
use gray_core::message::{ContentBlock, Message, Role};

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured, concise summary.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

const BASE_SUMMARIZATION_PROMPT: &str = r#"The messages below are conversation messages from the current session.

Produce a structured summary of the conversation and work completed so far.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple goals or tasks.]

## Constraints & Preferences
- [Any constraints, rules, user preferences, or requirements mentioned]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks, modifications, files written, bugs fixed]

### In Progress
- [ ] [Current active work or pending tasks]

### Blocked / Issues
- [Issues or errors encountered, if any]

## Key Decisions & Architecture
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Key file paths, function/struct names, URLs, port numbers, or error details needed to continue]

Keep each section concise. Preserve exact file paths, symbol names, and error messages."#;

pub fn build_summarization_prompt(transcript: &str, custom_instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "{BASE_SUMMARIZATION_PROMPT}\n\n<conversation-transcript>\n{transcript}\n</conversation-transcript>"
    );
    if let Some(instructions) = custom_instructions {
        if !instructions.trim().is_empty() {
            prompt.push_str(&format!(
                "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above:\n{}\n</user-instructions>",
                instructions.trim()
            ));
        }
    }
    prompt
}

/// Serializes LLM messages into plain-text transcript suitable for context compaction.
/// Tool results are capped at 1500 chars so massive outputs do not blow up the summarization request.
pub fn serialize_conversation(messages: &[Message]) -> String {
    const MAX_TOOL_CHARS: usize = 1500;
    let mut parts = Vec::new();

    for msg in messages {
        match msg.role {
            Role::User => {
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Role::Assistant => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.trim().is_empty() {
                                text_parts.push(text.as_str());
                            }
                        }
                        ContentBlock::ToolUse { name, args, .. } => {
                            let args_str = serde_json::to_string(args).unwrap_or_default();
                            tool_calls.push(format!("{name}({args_str})"));
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Role::System => {
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(format!("[System]: {text}"));
                }
            }
        }

        // Also check if any tool result blocks were stored in the message
        for block in &msg.content {
            if let ContentBlock::ToolResult { id, content, is_error } = block {
                let text = content.as_str();
                let truncated = if text.chars().count() > MAX_TOOL_CHARS {
                    let s: String = text.chars().take(MAX_TOOL_CHARS).collect();
                    format!("{s}... [truncated]")
                } else {
                    text.to_string()
                };
                let err_tag = if *is_error { " (error)" } else { "" };
                parts.push(format!("[Tool result {id}{err_tag}]: {truncated}"));
            }
        }
    }

    parts.join("\n\n")
}

pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

pub fn should_compact(tokens: usize, window: usize, s: &CompactionSettings) -> bool {
    s.enabled && tokens > window.saturating_sub(s.reserve_tokens)
}

pub fn calculate_context_tokens(u: &Usage) -> usize {
    if u.total() > 0 { u.total() } else { u.input_tokens + u.output_tokens }
}

pub fn estimate_tokens(msg: &Message) -> usize {
    (msg.text_content().len() as f64 / 4.0).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_compact_threshold() {
        let s = CompactionSettings {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(100_000, 128_000, &s));
        assert!(should_compact(115_000, 128_000, &s)); // 115k > 128k-16k
        assert!(!should_compact(
            200_000,
            128_000,
            &CompactionSettings { enabled: false, ..s }
        ));
    }
}

