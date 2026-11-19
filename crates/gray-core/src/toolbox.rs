use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::agent::ToolOutput;
use crate::message::ToolDef;

pub const NAME: &str = "toolbox";

pub const DIRECT: &[&str] = &[
    "bash",
    "read",
    "edit",
    "write",
    "grep",
    "glob",
    "Read",
    "Edit",
    "Write",
    "Grep",
    "Glob",
    "apply_patch",
    "memory",
    "skill",
    "skills_install",
    "web_search",
    "web_fetch",
    "request_user_input",
    "subagent_spawn",
    "subagent_message",
    "subagent_stop",
];

pub const LINE_BYTES: usize = 100;

pub const MAX_MENU_BYTES: usize = 32 * 1024;

const DEFAULT_LIMIT: usize = 5;

const MAX_LIMIT: usize = 20;

pub fn is_direct(name: &str) -> bool {
    DIRECT.contains(&name)
}

pub fn def() -> ToolDef {
    ToolDef::new(
        NAME,
        "Long-tail tools (plugins, MCP servers) that are not listed directly. The <toolbox> menu in the \
         conversation names them. action=search finds tools by keywords; open returns one tool's full \
         description and parameter schema; run calls a tool by its exact name with `arguments` as a JSON \
         object. A tool you already know can be run without opening it first.",
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["search", "open", "run"]},
                "target": {"type": "string", "description": "search: keywords; open/run: exact tool name"},
                "arguments": {"type": "object", "description": "run: the tool's arguments"},
                "limit": {"type": "integer", "description": "search: max matches (default 5, max 20)"}
            },
            "required": ["action", "target"]
        }),
    )
}

pub fn request_tools(all: &[ToolDef]) -> Vec<ToolDef> {
    let mut out: Vec<ToolDef> = DIRECT
        .iter()
        .filter_map(|n| all.iter().find(|t| t.name == *n).cloned())
        .collect();
    out.push(def());
    out
}

pub fn hidden(all: &[ToolDef]) -> impl Iterator<Item = &ToolDef> {
    all.iter().filter(|t| !is_direct(&t.name) && t.name != NAME)
}

fn short_line(name: &str, description: &str) -> String {
    let budget = LINE_BYTES.saturating_sub(name.len() + 4);
    let first = description
        .split_inclusive(". ")
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('.')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text = if first.len() <= budget {
        first
    } else {
        let mut cut = first.floor_char_boundary(budget.saturating_sub(1));
        if let Some(sp) = first[..cut].rfind(' ') {
            cut = sp;
        }
        format!("{}…", &first[..cut])
    };
    if text.is_empty() {
        format!("- {name}")
    } else {
        format!("- {name}: {text}")
    }
}

fn lines<'a>(defs: impl Iterator<Item = &'a ToolDef>) -> String {
    let mut out = String::new();
    for t in defs {
        let line = short_line(&t.name, &t.description);
        if out.len() + line.len() + 1 > MAX_MENU_BYTES {
            out.push_str("- … more: use action=search\n");
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn announcement(all: &[ToolDef], announced: &BTreeSet<String>) -> Option<String> {
    let now: BTreeSet<String> = hidden(all).map(|t| t.name.clone()).collect();
    if &now == announced {
        return None;
    }
    if announced.is_empty() {
        return Some(format!(
            "<toolbox>\nReach these through the `toolbox` tool (run by exact name, or open for the schema).\n{}</toolbox>",
            lines(hidden(all))
        ));
    }
    let added = lines(hidden(all).filter(|t| !announced.contains(&t.name)));
    let removed: Vec<&str> = announced
        .iter()
        .filter(|n| !now.contains(*n))
        .map(String::as_str)
        .collect();
    let mut text = String::from("<toolbox-update>\n");
    if !added.is_empty() {
        text.push_str("Added:\n");
        text.push_str(&added);
    }
    if !removed.is_empty() {
        text.push_str(&format!("Removed: {}\n", removed.join(", ")));
    }
    text.push_str("</toolbox-update>");
    Some(text)
}

pub fn hidden_names(all: &[ToolDef]) -> BTreeSet<String> {
    hidden(all).map(|t| t.name.clone()).collect()
}

pub fn run_target(all: &[ToolDef], args: &Value) -> Option<(String, Value)> {
    let (target, arguments) = run_request(args)?;
    all.iter()
        .any(|t| t.name == target)
        .then_some((target, arguments))
}

pub fn run_request(args: &Value) -> Option<(String, Value)> {
    if args.get("action").and_then(Value::as_str) != Some("run") {
        return None;
    }
    let target = args.get("target").and_then(Value::as_str)?.trim();
    if target.is_empty() || target == NAME {
        return None;
    }
    let arguments = match args.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        Some(v) => v.clone(),
    };
    Some((target.to_string(), arguments))
}

pub fn unknown_tool_message(target: &str) -> String {
    format!("No tool named '{target}'. Use action=search to find one.")
}

pub fn answer(all: &[ToolDef], args: &Value) -> ToolOutput {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    match action {
        "open" => match all.iter().find(|t| t.name == target) {
            Some(t) => ToolOutput::ok(
                serde_json::to_string_pretty(&json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }))
                .unwrap_or_default(),
            ),
            None => ToolOutput::error(unknown_tool_message(target)),
        },
        "search" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT));
            let words: Vec<String> = target
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect();
            let mut scored: Vec<(usize, &ToolDef)> = all
                .iter()
                .filter(|t| t.name != NAME)
                .map(|t| {
                    let name = t.name.to_lowercase();
                    let desc = t.description.to_lowercase();
                    let score = words
                        .iter()
                        .map(|w| {
                            3 * usize::from(name.contains(w.as_str()))
                                + usize::from(desc.contains(w.as_str()))
                        })
                        .sum();
                    (score, t)
                })
                .filter(|(s, _)| *s > 0 || words.is_empty())
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            if scored.is_empty() {
                return ToolOutput::ok(format!("No tools match '{target}'."));
            }
            ToolOutput::ok(lines(scored.into_iter().take(limit).map(|(_, t)| t)))
        }
        "run" => ToolOutput::error(unknown_tool_message(target)),
        _ => ToolOutput::error("toolbox: action must be search, open or run".to_string()),
    }
}
