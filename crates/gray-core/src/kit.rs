use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::agent::ToolOutput;
use crate::message::ToolDef;

pub const NAME: &str = "kit";
pub const LEGACY_NAME: &str = "toolbox";

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

pub fn is_kit_name(name: &str) -> bool {
    name == NAME || name == LEGACY_NAME
}

pub fn bash_only() -> bool {
    std::env::var("GRAY_KIT_BASH_ONLY")
        .ok()
        .as_deref()
        .map(str::trim)
        == Some("1")
}

pub fn is_direct(name: &str) -> bool {
    if bash_only() {
        return name == "bash";
    }
    DIRECT.contains(&name)
}

pub fn def() -> ToolDef {
    ToolDef::new(
        NAME,
        "Run plugin, MCP, and host tools that are not listed directly. The <kit> menu in the \
         conversation names them. Prefer calling them from bash with `kit <name> --help` or \
         `kit <name> [flags | --json '{...}']`, or call this tool directly with action=search|open|run.",
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
    if bash_only() {
        return all
            .iter()
            .find(|t| t.name == "bash")
            .cloned()
            .into_iter()
            .collect();
    }
    let mut out: Vec<ToolDef> = DIRECT
        .iter()
        .filter_map(|n| all.iter().find(|t| t.name == *n).cloned())
        .collect();
    out.push(def());
    out
}

pub fn hidden(all: &[ToolDef]) -> impl Iterator<Item = &ToolDef> {
    all.iter()
        .filter(|t| !is_direct(&t.name) && !is_kit_name(&t.name))
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
            out.push_str("- … more: run `kit search <query>`\n");
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn list_text(all: &[ToolDef]) -> String {
    let items: Vec<&ToolDef> = all
        .iter()
        .filter(|t| t.name != "bash" && !is_kit_name(&t.name))
        .collect();
    if items.is_empty() {
        return "No kit tools available.".to_string();
    }
    lines(items.into_iter())
}

pub fn search_text(all: &[ToolDef], query: &str, limit: usize) -> String {
    let limit = limit.clamp(1, MAX_LIMIT);
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let mut scored: Vec<(usize, &ToolDef)> = all
        .iter()
        .filter(|t| t.name != "bash" && !is_kit_name(&t.name))
        .map(|t| {
            let name = t.name.to_ascii_lowercase();
            let desc = t.description.to_ascii_lowercase();
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
        return format!("No tools match '{query}'.");
    }
    lines(scored.into_iter().take(limit).map(|(_, t)| t))
}

pub fn format_help(t: &ToolDef) -> String {
    let mut out = format!(
        "{} — {}\n\nUsage:\n  kit {} [FLAGS] [--json '{{...}}']\n",
        t.name, t.description, t.name
    );
    let required: BTreeSet<&str> = t
        .parameters
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(props) = t.parameters.get("properties").and_then(Value::as_object)
        && !props.is_empty()
    {
        out.push_str("\nFlags:\n");
        for (k, spec) in props {
            let ty = spec.get("type").and_then(Value::as_str).unwrap_or("value");
            let req = if required.contains(k.as_str()) {
                " (required)"
            } else {
                ""
            };
            let desc = spec
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let flag = k.replace('_', "-");
            if desc.is_empty() {
                out.push_str(&format!("  --{flag} <{ty}>{req}\n"));
            } else {
                out.push_str(&format!("  --{flag} <{ty}>{req}  {desc}\n"));
            }
        }
    }
    let schema = serde_json::to_string_pretty(&t.parameters).unwrap_or_else(|_| "{}".into());
    out.push_str(&format!("\nSchema:\n{schema}"));
    out
}

pub fn resolve_target(all: &[ToolDef], argv: &[String]) -> Option<(String, usize)> {
    let first = argv.first()?.as_str();
    if all.iter().any(|t| t.name == first) {
        return Some((first.to_string(), 1));
    }
    let first_under = first.replace('-', "_");
    if all.iter().any(|t| t.name == first_under) {
        return Some((first_under, 1));
    }
    if let Some(second) = argv.get(1) {
        let srv = first.replace('-', "_");
        let tool = second.replace('-', "_");
        for cand in [
            format!("mcp__{first}__{second}"),
            format!("mcp__{srv}__{tool}"),
            format!("{first}_{second}"),
            format!("{srv}_{tool}"),
        ] {
            if all.iter().any(|t| t.name == cand) {
                return Some((cand, 2));
            }
        }
    }
    None
}

fn coerce_value(raw: &str, schema: Option<&Value>) -> Value {
    let ty = schema.and_then(|s| s.get("type")).and_then(Value::as_str);
    match ty {
        Some("boolean") => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Value::Bool(true),
            "false" | "0" | "no" => Value::Bool(false),
            _ => Value::String(raw.to_string()),
        },
        Some("integer") => raw
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        Some("number") => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(raw.to_string())),
        Some("array") | Some("object") => {
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        }
        Some("string") => Value::String(raw.to_string()),
        _ => {
            if raw == "true" {
                Value::Bool(true)
            } else if raw == "false" {
                Value::Bool(false)
            } else if raw == "null" {
                Value::Null
            } else if let Ok(n) = raw.parse::<i64>() {
                Value::from(n)
            } else if (raw.starts_with('{') && raw.ends_with('}'))
                || (raw.starts_with('[') && raw.ends_with(']'))
            {
                serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
            } else {
                Value::String(raw.to_string())
            }
        }
    }
}

fn match_prop_key(props: Option<&Map<String, Value>>, raw_key: &str) -> String {
    if let Some(props) = props {
        if props.contains_key(raw_key) {
            return raw_key.to_string();
        }
        let underscored = raw_key.replace('-', "_");
        if props.contains_key(&underscored) {
            return underscored;
        }
    }
    raw_key.replace('-', "_")
}

pub fn parse_cli_args(
    def: &ToolDef,
    args: &[String],
    stdin_json: Option<&str>,
) -> Result<Value, String> {
    let mut obj = Map::new();
    if let Some(raw) = stdin_json.map(str::trim).filter(|s| !s.is_empty()) {
        let parsed: Value =
            serde_json::from_str(raw).map_err(|e| format!("invalid JSON on stdin: {e}"))?;
        let map = parsed
            .as_object()
            .ok_or_else(|| "stdin JSON must be an object".to_string())?;
        for (k, v) in map {
            obj.insert(k.clone(), v.clone());
        }
    }
    let props = def.parameters.get("properties").and_then(Value::as_object);
    let mut i = 0;
    let mut positionals: Vec<String> = Vec::new();
    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            let next = args
                .get(i + 1)
                .ok_or_else(|| "--json requires a JSON object argument".to_string())?;
            let parsed: Value =
                serde_json::from_str(next).map_err(|e| format!("invalid --json: {e}"))?;
            let map = parsed
                .as_object()
                .ok_or_else(|| "--json value must be a JSON object".to_string())?;
            for (k, v) in map {
                obj.insert(k.clone(), v.clone());
            }
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--json=") {
            let parsed: Value =
                serde_json::from_str(rest).map_err(|e| format!("invalid --json: {e}"))?;
            let map = parsed
                .as_object()
                .ok_or_else(|| "--json value must be a JSON object".to_string())?;
            for (k, v) in map {
                obj.insert(k.clone(), v.clone());
            }
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--no-") {
            let key = match_prop_key(props, rest);
            obj.insert(key, Value::Bool(false));
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((raw_k, raw_v)) = rest.split_once('=') {
                let key = match_prop_key(props, raw_k);
                let schema = props.and_then(|p| p.get(&key));
                obj.insert(key, coerce_value(raw_v, schema));
                i += 1;
            } else {
                let key = match_prop_key(props, rest);
                let schema = props.and_then(|p| p.get(&key));
                let is_bool =
                    schema.and_then(|s| s.get("type")).and_then(Value::as_str) == Some("boolean");
                match args.get(i + 1) {
                    Some(next) if !next.starts_with("--") => {
                        obj.insert(key, coerce_value(next, schema));
                        i += 2;
                    }
                    _ if is_bool => {
                        obj.insert(key, Value::Bool(true));
                        i += 1;
                    }
                    _ => {
                        obj.insert(key, Value::Bool(true));
                        i += 1;
                    }
                }
            }
            continue;
        }
        positionals.push(arg.clone());
        i += 1;
    }
    if positionals.len() == 1
        && obj.is_empty()
        && positionals[0].trim().starts_with('{')
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&positionals[0])
    {
        return Ok(Value::Object(map));
    }
    if !positionals.is_empty() {
        let required: Vec<&str> = def
            .parameters
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if required.len() == 1 && !obj.contains_key(required[0]) {
            let key = required[0].to_string();
            let schema = props.and_then(|p| p.get(&key));
            obj.insert(key, coerce_value(&positionals.join(" "), schema));
        } else {
            return Err(format!(
                "unexpected positional arguments: {} (use --flag value or --json '{{...}}')",
                positionals.join(" ")
            ));
        }
    }
    Ok(Value::Object(obj))
}

pub fn announcement(all: &[ToolDef], announced: &BTreeSet<String>) -> Option<String> {
    let now: BTreeSet<String> = hidden(all).map(|t| t.name.clone()).collect();
    if &now == announced {
        return None;
    }
    if announced.is_empty() {
        return Some(format!(
            "<kit>\nCall these from bash via `kit <name> --help` or `kit <name> [flags | --json '{{...}}']` (or `kit search <query>`).\n{}</kit>",
            lines(hidden(all))
        ));
    }
    let added = lines(hidden(all).filter(|t| !announced.contains(&t.name)));
    let removed: Vec<&str> = announced
        .iter()
        .filter(|n| !now.contains(*n))
        .map(String::as_str)
        .collect();
    let mut text = String::from("<kit-update>\n");
    if !added.is_empty() {
        text.push_str("Added:\n");
        text.push_str(&added);
    }
    if !removed.is_empty() {
        text.push_str(&format!("Removed: {}\n", removed.join(", ")));
    }
    text.push_str("</kit-update>");
    Some(text)
}

pub fn hidden_names(all: &[ToolDef]) -> BTreeSet<String> {
    hidden(all).map(|t| t.name.clone()).collect()
}

pub fn run_request(args: &Value) -> Option<(String, Value)> {
    if args.get("action").and_then(Value::as_str)? != "run" {
        return None;
    }
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let inner = args
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    Some((target.to_string(), inner))
}

pub fn run_target(all: &[ToolDef], args: &Value) -> Option<(String, Value)> {
    let (target, inner) = run_request(args)?;
    if !all.iter().any(|t| t.name == target) {
        return None;
    }
    Some((target, inner))
}

pub fn unknown_tool_message(target: &str) -> String {
    format!("kit: unknown tool '{target}' — run `kit search <query>` to find it.")
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
            Some(t) => ToolOutput::ok(format_help(t)),
            None => ToolOutput::error(unknown_tool_message(target)),
        },
        "search" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, MAX_LIMIT))
                .unwrap_or(DEFAULT_LIMIT);
            ToolOutput::ok(search_text(all, target, limit))
        }
        "run" => ToolOutput::error(unknown_tool_message(target)),
        _ => ToolOutput::error("kit: action must be search, open or run".to_string()),
    }
}
