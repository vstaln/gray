use std::collections::{BTreeMap, HashSet};

use crate::agent::ToolOutput;
use crate::message::{ContentBlock, Message, Role, ToolDef};

pub const LOAD_TOOLS: &str = "load_tools";

const SHORT_DESC_CHARS: usize = 80;
const QUERY_HITS: usize = 15;
const MCP_LIST_MAX: usize = 20;

pub fn enabled_from_env() -> bool {
    !matches!(
        std::env::var("GRAY_DEFER_TOOLS").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

fn mcp_server(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mcp__")?;
    rest.split_once("__").map(|(server, _)| server)
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

fn requested_names(args: &serde_json::Value) -> Vec<String> {
    match args.get("names") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(serde_json::Value::String(s)) => s
            .split([',', ' ', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn used_names(messages: &[Message]) -> HashSet<String> {
    let mut used = HashSet::new();
    for m in messages.iter().filter(|m| m.role == Role::Assistant) {
        for b in &m.content {
            if let ContentBlock::ToolUse { name, args, .. } = b {
                if name == LOAD_TOOLS {
                    used.extend(requested_names(args));
                } else {
                    used.insert(name.clone());
                }
            }
        }
    }
    used
}

fn is_live(name: &str, always: &HashSet<String>, used: &HashSet<String>) -> bool {
    always.contains(name) || used.iter().any(|p| matches_pattern(p, name))
}

pub fn request_tools(
    all: &[ToolDef],
    always: &HashSet<String>,
    messages: &[Message],
) -> Vec<ToolDef> {
    let used = used_names(messages);
    let (mut live, deferred): (Vec<&ToolDef>, Vec<&ToolDef>) =
        all.iter().partition(|t| is_live(&t.name, always, &used));
    if deferred.is_empty() {
        return all.to_vec();
    }
    let index = loader_def(all, always);
    live.push(&index);
    live.into_iter().cloned().collect()
}

fn short(desc: &str) -> String {
    let line = desc.trim().lines().next().unwrap_or("").trim();
    let end = line
        .match_indices(". ")
        .find(|(i, _)| line[i + 2..].starts_with(|c: char| c.is_uppercase()))
        .map(|(i, _)| i);
    let first = match end {
        Some(i) => &line[..i],
        None => line.trim_end_matches('.'),
    };
    if first.chars().count() <= SHORT_DESC_CHARS {
        first.to_string()
    } else {
        let mut s: String = first.chars().take(SHORT_DESC_CHARS - 1).collect();
        s.push('…');
        s
    }
}

pub fn loader_def(all: &[ToolDef], always: &HashSet<String>) -> ToolDef {
    let mut lines = Vec::new();
    let mut servers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in all.iter().filter(|t| !always.contains(&t.name)) {
        match mcp_server(&t.name) {
            Some(server) => servers
                .entry(server)
                .or_default()
                .push(&t.name[server.len() + 7..]),
            None => lines.push(format!("- {}: {}", t.name, short(&t.description))),
        }
    }
    for (server, names) in servers {
        if names.len() > MCP_LIST_MAX {
            lines.push(format!(
                "- mcp__{server}__* ({} tools; `query` to find the ones you need)",
                names.len()
            ));
        } else {
            lines.push(format!(
                "- mcp__{server}__* ({} tools): {}",
                names.len(),
                names.join(", ")
            ));
        }
    }
    ToolDef::new(
        LOAD_TOOLS,
        format!(
            "Load more tools. Only the tools you see are loaded; the ones below exist but their \
             schemas are sent only after you load them. Pass `names` (exact names, or a \
             `prefix*` such as `mcp__server__*`) to load them for the rest of the session, \
             or `query` to search names and descriptions first.\n{}",
            lines.join("\n")
        ),
        serde_json::json!({
            "type": "object",
            "properties": {
                "names": {"type": "array", "items": {"type": "string"}},
                "query": {"type": "string"}
            }
        }),
    )
}

pub fn run_loader(all: &[ToolDef], args: &serde_json::Value) -> ToolOutput {
    let mut sections = Vec::new();
    let names = requested_names(args);
    if !names.is_empty() {
        let mut loaded = Vec::new();
        let mut unknown = Vec::new();
        for n in &names {
            let hits: Vec<&str> = all
                .iter()
                .filter(|t| t.name != LOAD_TOOLS && matches_pattern(n, &t.name))
                .map(|t| t.name.as_str())
                .collect();
            if hits.is_empty() {
                unknown.push(n.as_str());
            }
            loaded.extend(hits);
        }
        if !loaded.is_empty() {
            sections.push(format!(
                "Loaded {} tool(s); call them directly now: {}",
                loaded.len(),
                loaded.join(", ")
            ));
        }
        if !unknown.is_empty() {
            sections.push(format!("No tool matches: {}", unknown.join(", ")));
        }
    }
    if let Some(q) = args.get("query").and_then(|v| v.as_str()) {
        let terms: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
        let mut scored: Vec<(usize, &ToolDef)> = all
            .iter()
            .filter(|t| t.name != LOAD_TOOLS)
            .map(|t| {
                let hay = format!("{} {}", t.name, t.description).to_lowercase();
                (terms.iter().filter(|w| hay.contains(w.as_str())).count(), t)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        if scored.is_empty() {
            sections.push(format!("No tools match {q:?}."));
        } else {
            let hits: Vec<String> = scored
                .iter()
                .take(QUERY_HITS)
                .map(|(_, t)| format!("- {}: {}", t.name, short(&t.description)))
                .collect();
            sections.push(format!("Matches (load with `names`):\n{}", hits.join("\n")));
        }
    }
    if sections.is_empty() {
        return ToolOutput::error("Pass `names` to load tools or `query` to search them.");
    }
    ToolOutput::ok(sections.join("\n\n"))
}

#[cfg(test)]
#[path = "tool_deferral_tests.rs"]
mod tests;
