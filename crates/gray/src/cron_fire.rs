//! Cron fire helpers: pure prompt assembly, wake-gate/silence parsing,
//! transcript rendering. No I/O here except via `run_pre_script`'s caller.

use std::path::PathBuf;

/// Cap for injected pre-script stdout (hermes `_MAX_CONTEXT_CHARS`).
pub const SCRIPT_OUTPUT_CAP: usize = 8000;

/// Wake gate: false only when the last non-empty stdout line is JSON
/// `{"wakeAgent": false}`; anything else (empty, non-JSON, missing flag,
/// `true`) wakes normally.
pub fn parse_wake_gate(output: &str) -> bool {
    let last = output.lines().rev().find(|l| !l.trim().is_empty());
    let Some(line) = last else { return true };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return true;
    };
    !matches!(
        v.get("wakeAgent"),
        Some(serde_json::Value::Bool(false))
    )
}

/// `[SILENT]` prefix (case-insensitive, leading whitespace tolerated)
/// suppresses delivery; the fire still records `ok`.
pub fn is_silent_response(text: &str) -> bool {
    text.trim_start()
        .get(..8)
        .is_some_and(|h| h.eq_ignore_ascii_case("[silent]"))
}

/// Final prompt: optional `## skills` block (exact `<location>` paths, one
/// per line) + optional fenced `## script-output` block + base prompt.
/// Empty sections are omitted (byte-stable for the common prompt-only job).
pub fn assemble_fire_prompt(
    base: &str,
    skill_paths: &[PathBuf],
    script_output: Option<&str>,
) -> String {
    let mut out = String::new();
    if !skill_paths.is_empty() {
        out.push_str("## skills\nThe following skills apply to this task; read the SKILL.md at each <location> with bash before starting:\n\n");
        for p in skill_paths {
            out.push_str(&format!("- <location>{}</location>\n", p.display()));
        }
        out.push('\n');
    }
    if let Some(body) = script_output {
        let capped: String = body.chars().take(SCRIPT_OUTPUT_CAP).collect();
        out.push_str("## script-output\nPre-run data collection (stdout, capped):\n\n```\n");
        out.push_str(&capped);
        out.push_str("\n```\n\n");
    }
    out.push_str(base);
    out
}

/// Render collected `AgentEvent`s into a storable transcript: text deltas
/// concatenated verbatim; tool calls noted by name so silent-but-active
/// runs still leave a trace.
pub fn transcript_text(events: &[gray_core::event::AgentEvent]) -> String {
    use gray_core::event::AgentEvent;
    let mut text = String::new();
    for ev in events {
        match ev {
            AgentEvent::TextDelta { delta } => text.push_str(delta),
            AgentEvent::ToolCallStart { name, .. } => {
                text.push_str(&format!("\n[tool:{name}]\n"));
            }
            AgentEvent::ToolResult {
                output, is_error, ..
            } => {
                let tag = if *is_error { "result-err" } else { "result" };
                let s = output.chars().take(2000).collect::<String>();
                text.push_str(&format!("[{tag}:{s}]\n"));
            }
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;

    #[test]
    fn wake_gate_shapes() {
        assert!(parse_wake_gate("some output\n"));
        assert!(parse_wake_gate(""));
        assert!(parse_wake_gate("data\n{\"wakeAgent\": true}\n"));
        assert!(!parse_wake_gate("data\n{\"wakeAgent\": false}\n"));
        assert!(!parse_wake_gate("  {\"wakeAgent\": false}  \n\n"));
        assert!(parse_wake_gate("not json\n"));
        assert!(parse_wake_gate("{\"other\": 1}\n"));
    }

    #[test]
    fn silent_prefix_shapes() {
        assert!(is_silent_response("[SILENT] nothing to report"));
        assert!(is_silent_response("  [silent]  x"));
        assert!(!is_silent_response("loud report"));
        assert!(!is_silent_response(""));
    }

    #[test]
    fn assembly_orders_skills_then_script_then_prompt() {
        let out = assemble_fire_prompt(
            "check CI",
            &[std::path::PathBuf::from("/s/SKILL.md")],
            Some("build ok"),
        );
        let si = out.find("## skills").unwrap();
        let oi = out.find("## script-output").unwrap();
        assert!(si < oi);
        assert!(out.ends_with("check CI"));
        assert!(out.contains("/s/SKILL.md"));
        assert!(out.contains("build ok"));
    }

    #[test]
    fn assembly_omits_empty_sections() {
        let out = assemble_fire_prompt("hi", &[], None);
        assert_eq!(out, "hi");
    }

    #[test]
    fn transcript_collects_text_and_tool_names() {
        use gray_core::event::AgentEvent;
        let events = vec![
            AgentEvent::TextDelta {
                delta: "hello ".to_string(),
            },
            AgentEvent::TextDelta {
                delta: "world".to_string(),
            },
        ];
        let t = transcript_text(&events);
        assert!(t.contains("hello world"));
    }
}
