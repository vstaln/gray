//! Outbound side of the stdio protocol: the `emit` envelope plus the
//! `AgentEvent` → row translator wired into `dispatch_agent_event` when
//! stdio mode is active (see repl/session.rs).

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{Value, json};

use gray_core::event::{AgentEvent, Usage};
use gray_core::redaction::redact_for_disclosure;

use super::{CURRENT_REQ, CURRENT_SESSION, LAST_USAGE, OUT, SEQ, current_turn_id, lock};

const DETAIL_CAP: usize = 4_000;
const OUTPUT_CAP: usize = 8_000;

/// Attach protocol envelope fields and write one NDJSON row. `req` and
/// `turn_id` are filled from the current context unless already present.
pub(crate) fn emit(mut row: Value) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let obj = row.as_object_mut().expect("emit takes a JSON object");
    obj.insert("protocol".into(), json!(2));
    obj.insert("seq".into(), json!(seq));
    obj.entry("session_id").or_insert_with(|| {
        lock(&CURRENT_SESSION)
            .clone()
            .map_or(Value::Null, Value::String)
    });
    obj.entry("req")
        .or_insert_with(|| lock(&CURRENT_REQ).clone().unwrap_or(Value::Null));
    obj.entry("turn_id")
        .or_insert_with(|| current_turn_id().map_or(Value::Null, Value::String));
    let _guard = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{row}");
    let _ = out.flush();
}

/// SGR/cursor sequences from `say` callers (e.g. the resume banner) must not
/// leak into the JSON stream.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Command output / notices the REPL would print via `say`/`say_err`.
pub(crate) fn emit_say(level: &str, text: &str) {
    emit(json!({"type": "say", "level": level, "text": strip_ansi(text)}));
}

/// `say_lines` callers hand us styled ratatui lines (host say queue, pager
/// fallbacks); flatten to plain text, one line per source line.
pub(crate) fn emit_say_lines(lines: &[ratatui::text::Line<'static>]) {
    let text = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    emit_say("info", &text);
}

/// Approval/question request from the ask service (`approve` answers it).
pub(crate) fn emit_ask(ask_id: u64, questions: &[crate::ask::AskQuestion]) {
    emit(json!({
        "type": "ask",
        "ask_id": ask_id,
        "questions": questions,
    }));
}

fn disclose(text: &str, cap: usize) -> String {
    let redaction = redact_for_disclosure(text);
    let text = if redaction.has_secret() {
        redaction.into_text()
    } else {
        text.to_string()
    };
    if text.chars().count() > cap {
        let kept: String = text.chars().take(cap).collect();
        format!("{kept}…[truncated]")
    } else {
        text
    }
}

/// The stdio branch of `repl::session::dispatch_agent_event`: same
/// bookkeeping as the non-interactive renderer (pending_tools, turn_usage,
/// totals, duration) plus a JSON row per event.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_agent_event(
    ev: &AgentEvent,
    pending_tools: &mut HashMap<String, (String, Option<Value>)>,
    turn_usage: &mut Option<Usage>,
    model: &str,
    totals: &mut super::SessionTotals,
    turn_start: Instant,
    turn_duration_ms: &mut Option<u64>,
) {
    let elapsed_ms = || turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    match ev {
        AgentEvent::Start | AgentEvent::MessageBoundary => {}
        AgentEvent::TextDelta { delta } => {
            emit(json!({"type": "text", "delta": delta}));
        }
        AgentEvent::ThinkingDelta { delta } => {
            emit(json!({"type": "thinking", "delta": delta}));
        }
        AgentEvent::ToolCallStart { id, name } => {
            pending_tools.insert(id.clone(), (name.clone(), None));
            emit(json!({"type": "tool_start", "id": id, "name": name}));
        }
        AgentEvent::ToolCallProgress {
            id,
            name,
            args_so_far,
        } => {
            pending_tools
                .entry(id.clone())
                .or_insert((name.clone(), None));
            emit(json!({
                "type": "tool_progress",
                "id": id,
                "name": name,
                "args_so_far": disclose(args_so_far, DETAIL_CAP),
            }));
        }
        AgentEvent::ToolCallEnd { id, args } => {
            let entry = pending_tools
                .entry(id.clone())
                .or_insert((String::new(), None));
            entry.1 = Some(args.clone());
            let name = if entry.0.is_empty() {
                "tool".to_string()
            } else {
                entry.0.clone()
            };
            emit(json!({
                "type": "tool_call",
                "id": id,
                "name": name,
                "args": args,
            }));
        }
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
        } => {
            let (name, args) = pending_tools
                .remove(id)
                .map(|(n, a)| (if n.is_empty() { "tool".to_string() } else { n }, a))
                .unwrap_or_else(|| ("tool".to_string(), None));
            emit(json!({
                "type": "tool_result",
                "id": id,
                "name": name,
                "args": args,
                "output": disclose(output, OUTPUT_CAP),
                "is_error": is_error,
            }));
        }
        AgentEvent::StepUsage { usage } => {
            emit(json!({"type": "usage", "step": true, "usage": usage}));
        }
        AgentEvent::PrefixChanged { cause } => {
            emit(json!({"type": "notice", "kind": "prefix_changed", "cause": cause}));
        }
        AgentEvent::StreamError { message, details } => {
            emit(json!({
                "type": "say",
                "level": "warn",
                "kind": "provider_retry",
                "text": disclose(message, DETAIL_CAP),
                "details": disclose(details, DETAIL_CAP),
            }));
        }
        AgentEvent::Steered { text } => {
            emit(json!({"type": "steered", "text": disclose(text, DETAIL_CAP)}));
        }
        AgentEvent::Retract => {
            emit(json!({"type": "retract"}));
        }
        AgentEvent::Compacted {
            tokens_before,
            tokens_after,
            messages_before,
            messages_after,
            reason,
        } => {
            emit(json!({
                "type": "compacted",
                "tokens_before": tokens_before,
                "tokens_after": tokens_after,
                "messages_before": messages_before,
                "messages_after": messages_after,
                "reason": reason,
            }));
        }
        AgentEvent::TurnEnd { usage, stop_reason } => {
            *turn_usage = Some(*usage);
            let ms = elapsed_ms();
            *turn_duration_ms = Some(ms);
            if usage.total() > 0 {
                totals.add(usage, model, Some(ms));
            }
            *lock(&LAST_USAGE) = Some(*usage);
            emit(json!({
                "type": "turn_usage",
                "usage": usage,
                "stop_reason": stop_reason,
            }));
        }
    }
}
