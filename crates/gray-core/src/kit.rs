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
    "request_user_input",
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

pub fn is_direct(t: &ToolDef) -> bool {
    if bash_only() {
        return t.name == "bash";
    }
    t.direct || DIRECT.contains(&t.name.as_str())
}

pub fn def() -> ToolDef {
    ToolDef::new(
        NAME,
        "Run plugin, MCP, and host tools that are not listed directly. The <kit> menu in the \
         conversation names them. Prefer running them from bash as `kit <name> --flag value` \
         (`kit <name> --help` prints the flags), or call this tool with action=search|open|run.",
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
    let mut out: Vec<ToolDef> = all.iter().filter(|t| is_direct(t)).cloned().collect();
    out.push(def());
    out
}

pub fn hidden(all: &[ToolDef]) -> impl Iterator<Item = &ToolDef> {
    all.iter()
        .filter(|t| !is_direct(t) && !is_kit_name(&t.name))
}

fn short_line(name: &str, description: &str) -> String {
    let budget = LINE_BYTES.saturating_sub(name.len() + 4);
    let end = description
        .match_indices(". ")
        .map(|(i, _)| i)
        .find(|&i| {
            let word = description[..i]
                .rsplit(char::is_whitespace)
                .next()
                .unwrap_or("");
            let word = word.trim_start_matches(|c: char| !c.is_alphanumeric());
            !word.contains('.') && !matches!(word, "etc" | "vs" | "approx")
        })
        .unwrap_or(description.len());
    let first = description[..end]
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

fn effective_schema(spec: &Value) -> &Value {
    for key in ["anyOf", "oneOf"] {
        if let Some(alts) = spec.get(key).and_then(Value::as_array)
            && let Some(first) = alts
                .iter()
                .find(|a| a.get("type").and_then(Value::as_str) != Some("null"))
        {
            return first;
        }
    }
    spec
}

fn type_of(spec: &Value) -> &str {
    match effective_schema(spec).get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("value"),
        _ => "value",
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn placeholder(spec: &Value) -> String {
    let spec = effective_schema(spec);
    if let Some(choices) = spec.get("enum").and_then(Value::as_array) {
        let words: Vec<String> = choices.iter().map(scalar_text).collect();
        return words.join("|");
    }
    match type_of(spec) {
        "integer" => "n".into(),
        "number" => "number".into(),
        "boolean" => String::new(),
        "array" => match spec.get("items").map(type_of) {
            Some("object") | Some("array") => "json-list".into(),
            _ => "item".into(),
        },
        "object" => "json".into(),
        _ => "text".into(),
    }
}

fn flag_usage(key: &str, spec: &Value) -> String {
    let flag = key.replace('_', "-");
    let ph = placeholder(spec);
    if ph.is_empty() {
        format!("--{flag}")
    } else {
        format!("--{flag} <{ph}>")
    }
}

fn first_paragraph(text: &str) -> String {
    text.split("\n\n")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn format_help(t: &ToolDef) -> String {
    let required: Vec<&str> = t
        .parameters
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let empty = Map::new();
    let props = t
        .parameters
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut usage = format!("kit {}", t.name);
    for k in &required {
        if let Some(spec) = props.get(*k) {
            usage.push(' ');
            usage.push_str(&flag_usage(k, spec));
        }
    }
    if props.keys().any(|k| !required.contains(&k.as_str())) {
        usage.push_str(" [options]");
    }
    let mut out = format!("{}\n\n{}\n\nUsage: {usage}\n", t.name, t.description.trim());
    let mut nested = false;
    for (title, want_required) in [("Required", true), ("Options", false)] {
        let rows: Vec<(&String, &Value)> = props
            .iter()
            .filter(|(k, _)| required.contains(&k.as_str()) == want_required)
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{title}:\n"));
        for (k, spec) in rows {
            let eff = effective_schema(spec);
            let mut notes: Vec<String> = Vec::new();
            let desc = spec
                .get("description")
                .or_else(|| eff.get("description"))
                .and_then(Value::as_str)
                .map(first_paragraph)
                .unwrap_or_default();
            if !desc.is_empty() {
                notes.push(desc);
            }
            match type_of(spec) {
                "array" => {
                    if matches!(eff.get("items").map(type_of), Some("object" | "array")) {
                        nested = true;
                        notes.push("a JSON list; easiest through stdin (see below)".into());
                    } else {
                        notes.push("repeat the flag once per item".into());
                    }
                }
                "object" => {
                    nested = true;
                    notes.push("a JSON object; easiest through stdin (see below)".into());
                }
                "boolean" => {
                    let flag = k.replace('_', "-");
                    notes.push(format!("on/off switch; --no-{flag} turns it off"));
                }
                _ => {}
            }
            if let Some(d) = spec.get("default").or_else(|| eff.get("default"))
                && !d.is_null()
            {
                notes.push(format!("default {}", scalar_text(d)));
            }
            let head = format!("  {}", flag_usage(k, spec));
            if notes.is_empty() {
                out.push_str(&format!("{head}\n"));
            } else {
                out.push_str(&format!("{head}\n      {}\n", notes.join("; ")));
            }
        }
    }
    if nested {
        out.push_str(&format!(
            "\nNested input: pipe the whole input as one JSON object on stdin, flags still apply on top:\n  kit {} <<'EOF'\n  {{\"key\": \"value\"}}\n  EOF\n",
            t.name
        ));
    }
    out
}

pub fn resolve_target(all: &[ToolDef], argv: &[String]) -> Option<(String, usize)> {
    resolve_target_with(argv, |name| all.iter().any(|t| t.name == name))
}

pub fn resolve_target_with(
    argv: &[impl AsRef<str>],
    exists: impl Fn(&str) -> bool,
) -> Option<(String, usize)> {
    let first = argv.first()?.as_ref();
    if exists(first) {
        return Some((first.to_string(), 1));
    }
    let first_under = first.replace('-', "_");
    if exists(&first_under) {
        return Some((first_under, 1));
    }
    if let Some(second) = argv.get(1).map(|arg| arg.as_ref()) {
        let srv = first.replace('-', "_");
        let tool = second.replace('-', "_");
        for cand in [
            format!("mcp__{first}__{second}"),
            format!("mcp__{srv}__{tool}"),
            format!("{first}_{second}"),
            format!("{srv}_{tool}"),
        ] {
            if exists(&cand) {
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

fn insert_flag(obj: &mut Map<String, Value>, key: String, raw: &str, schema: Option<&Value>) {
    let is_list = schema.map(type_of) == Some("array");
    if !is_list {
        obj.insert(key, coerce_value(raw, schema.map(effective_schema)));
        return;
    }
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) {
        obj.insert(key, Value::Array(items));
        return;
    }
    let item_schema = schema.map(effective_schema).and_then(|s| s.get("items"));
    let item = coerce_value(raw, item_schema);
    match obj.get_mut(&key) {
        Some(Value::Array(items)) => items.push(item),
        _ => {
            obj.insert(key, Value::Array(vec![item]));
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
    let open = def
        .parameters
        .get("additionalProperties")
        .is_some_and(|v| v != &Value::Bool(false))
        || props.is_none_or(Map::is_empty);
    let known = |key: &str| -> Result<(), String> {
        match props {
            Some(p) if !open && !p.contains_key(key) => {
                let flags: Vec<String> = p
                    .keys()
                    .map(|k| format!("--{}", k.replace('_', "-")))
                    .collect();
                Err(format!(
                    "unknown flag --{}; {} takes: {}",
                    key.replace('_', "-"),
                    def.name,
                    flags.join(" ")
                ))
            }
            _ => Ok(()),
        }
    };
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
            known(&key)?;
            obj.insert(key, Value::Bool(false));
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((raw_k, raw_v)) = rest.split_once('=') {
                let key = match_prop_key(props, raw_k);
                known(&key)?;
                let schema = props.and_then(|p| p.get(&key));
                insert_flag(&mut obj, key, raw_v, schema);
                i += 1;
            } else {
                let key = match_prop_key(props, rest);
                known(&key)?;
                let schema = props.and_then(|p| p.get(&key));
                let is_bool = schema.map(type_of) == Some("boolean");
                match args.get(i + 1) {
                    Some(next) if !next.starts_with("--") && !is_bool => {
                        insert_flag(&mut obj, key, next, schema);
                        i += 2;
                    }
                    Some(next) if is_bool && matches!(next.as_str(), "true" | "false") => {
                        obj.insert(key, coerce_value(next, schema));
                        i += 2;
                    }
                    _ if is_bool || schema.is_none() => {
                        obj.insert(key, Value::Bool(true));
                        i += 1;
                    }
                    _ => {
                        return Err(format!("--{rest} needs a value: --{rest} <value>"));
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
                "unexpected positional arguments: {} (use --flag value; `kit <tool> --help` lists the flags)",
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
            "<kit>\nThese are commands: run them from bash as `kit <name> --flag value`. `kit <name> --help` prints a tool's flags, `kit search <words>` finds more.\n{}</kit>",
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
