use std::collections::HashMap;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Plan, SessionUpdate, StopReason as AcpStopReason, ToolCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate,
};
use gray_core::event::{AgentEvent, StopReason};

#[derive(Debug, Default)]
struct ToolState {
    ended: bool,
    result_sent: bool,
}

#[derive(Debug, Default)]
pub struct EventMapper {
    tools: HashMap<String, ToolState>,
    started: bool,
}

impl EventMapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self) -> Vec<AgentEvent> {
        if self.started {
            return vec![];
        }
        self.started = true;
        vec![AgentEvent::Start]
    }

    pub fn map_update(&mut self, update: &SessionUpdate) -> Vec<AgentEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(ContentChunk { content, .. }) => {
                vec![AgentEvent::TextDelta {
                    delta: content_text(content),
                }]
            }
            SessionUpdate::AgentThoughtChunk(ContentChunk { content, .. }) => {
                vec![AgentEvent::ThinkingDelta {
                    delta: content_text(content),
                }]
            }
            SessionUpdate::UserMessageChunk(_) => vec![],
            SessionUpdate::ToolCall(tc) => self.map_tool_call(tc),
            SessionUpdate::ToolCallUpdate(u) => self.map_tool_call_update(u),
            SessionUpdate::Plan(plan) => vec![AgentEvent::TextDelta {
                delta: format_plan(plan),
            }],
            SessionUpdate::UsageUpdate(_) => vec![],
            _ => vec![],
        }
    }

    fn map_tool_call(&mut self, tc: &ToolCall) -> Vec<AgentEvent> {
        let id = tc.tool_call_id.0.to_string();
        let name = tool_name(tc);
        self.tools.insert(
            id.clone(),
            ToolState {
                ended: false,
                result_sent: false,
            },
        );
        let mut out = vec![AgentEvent::ToolCallStart {
            id: id.clone(),
            name,
        }];
        out.push(AgentEvent::ToolCallEnd {
            id: id.clone(),
            args: tool_args(tc),
        });
        if let Some(st) = self.tools.get_mut(&id) {
            st.ended = true;
        }
        out
    }

    fn map_tool_call_update(&mut self, u: &ToolCallUpdate) -> Vec<AgentEvent> {
        let id = u.tool_call_id.0.to_string();
        let status = u.fields.status.unwrap_or(ToolCallStatus::Pending);
        match status {
            ToolCallStatus::Completed | ToolCallStatus::Failed => {
                let is_error = matches!(status, ToolCallStatus::Failed);
                let output = u
                    .fields
                    .content
                    .as_deref()
                    .map(flatten_tool_content)
                    .unwrap_or_default();
                if let Some(st) = self.tools.get_mut(&id) {
                    if st.result_sent {
                        return vec![];
                    }
                    st.result_sent = true;
                }
                vec![AgentEvent::ToolResult {
                    id,
                    output,
                    is_error,
                }]
            }
            _ => vec![],
        }
    }

    pub fn map_stop(&self, reason: &AcpStopReason) -> StopReason {
        match reason {
            AcpStopReason::EndTurn => StopReason::EndTurn,
            AcpStopReason::MaxTokens => StopReason::MaxTokens,
            AcpStopReason::MaxTurnRequests => StopReason::EndTurn,
            AcpStopReason::Refusal => StopReason::Error,
            AcpStopReason::Cancelled => StopReason::Cancelled,
            _ => StopReason::EndTurn,
        }
    }
}

fn content_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::Image(_) => "[image]".to_string(),
        _ => "[unsupported content]".to_string(),
    }
}

fn tool_name(tc: &ToolCall) -> String {
    let kind = format!("{:?}", tc.kind);
    if kind == "Other" {
        tc.title.clone()
    } else {
        kind.to_lowercase()
    }
}

fn tool_args(tc: &ToolCall) -> serde_json::Value {
    match &tc.raw_input {
        Some(v) => v.clone(),
        None => serde_json::json!({ "title": tc.title }),
    }
}

fn flatten_tool_content(content: &[ToolCallContent]) -> String {
    let mut parts = Vec::new();
    for c in content {
        match c {
            ToolCallContent::Content(c) => parts.push(content_text(&c.content)),
            ToolCallContent::Diff(d) => {
                parts.push(format!("{}{}", d.path.display(), d.new_text));
            }
            ToolCallContent::Terminal(_) => parts.push("[terminal]".to_string()),
            _ => {}
        }
    }
    parts.join("\n")
}

fn format_plan(plan: &Plan) -> String {
    let mut out = String::from("Plan:\n");
    for e in &plan.entries {
        out.push_str(&format!("- [{:?}] {}\n", e.status, e.content));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::TextContent;

    fn text_update(s: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            s.to_string(),
        ))))
    }

    #[test]
    fn message_chunk_maps_to_text_delta() {
        let mut m = EventMapper::new();
        let evs = m.map_update(&text_update("hello"));
        assert!(matches!(evs[0], AgentEvent::TextDelta { .. }));
    }

    #[test]
    fn thought_chunk_maps_to_thinking_delta() {
        let mut m = EventMapper::new();
        let u = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("hmm".to_string()),
        )));
        let evs = m.map_update(&u);
        assert!(matches!(evs[0], AgentEvent::ThinkingDelta { .. }));
    }

    #[test]
    fn tool_call_emits_start_then_end() {
        let mut m = EventMapper::new();
        let tc = ToolCall::new("t1", "Read foo");
        let evs = m.map_update(&SessionUpdate::ToolCall(tc));
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], AgentEvent::ToolCallStart { .. }));
        assert!(matches!(evs[1], AgentEvent::ToolCallEnd { .. }));
    }

    #[test]
    fn stop_reasons_map() {
        let m = EventMapper::new();
        assert!(matches!(
            m.map_stop(&AcpStopReason::EndTurn),
            StopReason::EndTurn
        ));
        assert!(matches!(
            m.map_stop(&AcpStopReason::MaxTokens),
            StopReason::MaxTokens
        ));
        assert!(matches!(
            m.map_stop(&AcpStopReason::Cancelled),
            StopReason::Cancelled
        ));
        assert!(matches!(
            m.map_stop(&AcpStopReason::Refusal),
            StopReason::Error
        ));
    }
}
