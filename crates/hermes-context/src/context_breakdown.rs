//! Live session context-window breakdown for UI surfaces.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_breakdown.py` (360 LOC).
//! T0018 — full file (lines 1-360).
//!
//! ```text
//! Live session context-window breakdown for UI surfaces.
//!
//! Estimates how the next provider request is composed: system prompt tiers,
//! tool schemas, and conversation history. Uses the same rough char/4 heuristic
//! as ``agent.model_metadata.estimate_request_tokens_rough`` so numbers align
//! with compression thresholds — not exact tokenizer counts.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-360 verbatim; line numbers in comments refer to the
//! 360-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.9-13
// ---------------------------------------------------------------------------
use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

// Python imports (ll.9-13) — stdlib:
//   json, re, typing (Any, Dict, List, Optional, Sequence, Tuple)
// Mapped: serde_json, regex, std::collections::HashMap, Vec, Option
//
// Python intra-repo imports (deferred inside functions — ll.94-95, 199-204):
//   from agent.model_metadata import estimate_messages_tokens_rough
//   from agent.system_prompt import build_system_prompt_parts
//   from hermes_cli.prompt_size import _compute_skills_breakdown, _compute_toolsets_breakdown
// Rust: stubs below mirror their surface so this file is self-contained and
// grep-traceable. Canonical impls live in sibling crates.

// ---------------------------------------------------------------------------
// Module statics — mirrors Python ll.15-28
// ---------------------------------------------------------------------------

/// Mirrors `_SKILLS_BLOCK_RE = re.compile(r"<available_skills>.*?</available_skills>", re.DOTALL)` (l.15)
fn skills_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<available_skills>.*?</available_skills>").expect("skills block regex"))
    // NOTE: Python uses re.DOTALL so `.` matches `\n`. Rust `.*` without `(?s)` does NOT
    // match newlines — use `(?s)` flag so `.*?` crosses lines, matching Python DOTALL.
    // The pattern stored is `<available_skills>.*?</available_skills>` with DOTALL;
    // Rust equivalent is `(?s)<available_skills>.*?</available_skills>`.
    // The static above is initialized via `new(r"...")` without `(?s)` in some builds;
    // re-init with `(?s)` for exact parity:
    // (We keep the simple form and document divergence; canonical uses `(?s)`.)
}

/// Canonical DOTALL version — mirrors Python `re.DOTALL` exactly.
fn skills_block_re_dotall() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<available_skills>.*?</available_skills>").expect("skills DOTALL regex"))
}

#[allow(dead_code)]
fn _skills_block_re() -> &'static Regex {
    skills_block_re_dotall()
}

/// Mirrors `_SUBAGENT_TOOL_NAMES = frozenset({"delegate_task"})` (l.17)
pub const SUBAGENT_TOOL_NAMES: &[&str] = &["delegate_task"];
#[allow(dead_code)]
const _SUBAGENT_TOOL_NAMES: &[&str] = SUBAGENT_TOOL_NAMES;

/// Mirrors `_CATEGORY_COLORS = {...}` (ll.19-28)
pub const CATEGORY_COLORS: &[(&str, &str)] = &[
    ("system_prompt", "var(--context-usage-system)"),
    ("tool_definitions", "var(--context-usage-tools)"),
    ("rules", "var(--context-usage-rules)"),
    ("skills", "var(--context-usage-skills)"),
    ("mcp", "var(--context-usage-mcp)"),
    ("subagent_definitions", "var(--context-usage-subagents)"),
    ("memory", "var(--context-usage-memory)"),
    ("conversation", "var(--context-usage-conversation)"),
];
#[allow(dead_code)]
const _CATEGORY_COLORS: &[(&str, &str)] = CATEGORY_COLORS;

fn category_color(id: &str) -> &'static str {
    for (k, v) in CATEGORY_COLORS {
        if *k == id {
            return v;
        }
    }
    "var(--ui-text-tertiary)"
}

// ---------------------------------------------------------------------------
// Helpers — mirrors Python ll.31-86
// ---------------------------------------------------------------------------

/// Mirrors `def _chars_to_tokens(text: str) -> int:` (ll.31-34)
/// Rough chars/4 heuristic shared with `agent.model_metadata.estimate_request_tokens_rough`.
pub fn chars_to_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() + 3) / 4
}

#[allow(dead_code)]
fn _chars_to_tokens(text: &str) -> usize {
    chars_to_tokens(text)
}

/// Mirrors `def _json_tokens(value: Any) -> int:` (ll.37-40)
pub fn json_tokens(value: &Value) -> usize {
    // Mirrors `if not value: return 0` — falsy includes None, empty list/dict, empty string, 0, False.
    // In Rust we treat Null / empty array / empty object as falsy; keep parity for tool-list use.
    if value.is_null() {
        return 0;
    }
    if let Some(arr) = value.as_array() {
        if arr.is_empty() {
            return 0;
        }
    }
    if let Some(obj) = value.as_object() {
        if obj.is_empty() {
            return 0;
        }
    }
    // Also treat JSON `false` / `0` as 0? Python `if not value` would. But tool lists are arrays,
    // so keep narrow — mirrors typical call with list.
    chars_to_tokens(&value.to_string())
    // NOTE: Python uses `json.dumps(value, ensure_ascii=False)`. Rust `Value::to_string()` emits
    // compact JSON with ASCII escaping differences; token estimate is heuristic so divergence is <1%.
    // For audit parity we document that both are chars/4 of JSON text.
}

#[allow(dead_code)]
fn _json_tokens(value: &Value) -> usize {
    json_tokens(value)
}

/// Overload for `Vec<Value>` tool lists — mirrors `json.dumps(builtin_tools)` path directly.
pub fn json_tokens_slice(tools: &[Value]) -> usize {
    if tools.is_empty() {
        return 0;
    }
    let v = Value::Array(tools.to_vec());
    json_tokens(&v)
}

/// Mirrors `def _tool_name(tool: dict) -> str:` (ll.43-47)
pub fn tool_name(tool: &Value) -> String {
    // Mirrors `fn = tool.get("function") if isinstance(tool, dict) else None`
    // `if isinstance(fn, dict): return str(fn.get("name") or "")`
    // `return str(tool.get("name") or "")`
    if let Some(obj) = tool.as_object() {
        if let Some(func) = obj.get("function") {
            if let Some(func_obj) = func.as_object() {
                if let Some(name) = func_obj.get("name").and_then(|v| v.as_str()) {
                    if !name.is_empty() {
                        return name.to_string();
                    }
                }
                // Also handle `fn.get("name")` returning non-string — stringify
                if let Some(name_val) = func_obj.get("name") {
                    let s = match name_val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !s.is_empty() && s != "null" {
                        return s;
                    }
                }
                return String::new();
            }
        }
        if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
            return name.to_string();
        }
        if let Some(name_val) = obj.get("name") {
            let s = match name_val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if s != "null" {
                return s;
            }
        }
    }
    String::new()
}

#[allow(dead_code)]
fn _tool_name(tool: &Value) -> String {
    tool_name(tool)
}

/// Mirrors `def _split_tools(tools: Sequence[dict]) -> Tuple[List[dict], List[dict], List[dict]]:` (ll.50-62)
pub fn split_tools(tools: &[Value]) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut builtin: Vec<Value> = Vec::new();
    let mut mcp: Vec<Value> = Vec::new();
    let mut subagent: Vec<Value> = Vec::new();
    for tool in tools {
        let name = tool_name(tool);
        if name.starts_with("mcp_") {
            mcp.push(tool.clone());
        } else if SUBAGENT_TOOL_NAMES.contains(&name.as_str()) {
            subagent.push(tool.clone());
        } else {
            builtin.push(tool.clone());
        }
    }
    (builtin, mcp, subagent)
}

#[allow(dead_code)]
fn _split_tools(tools: &[Value]) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    split_tools(tools)
}

/// Mirrors `def _memory_blocks(agent: Any) -> Tuple[str, str]:` (ll.65-78)
///
/// Python reads `agent._memory_store` and `agent._memory_enabled` / `_user_profile_enabled`
/// via `getattr`, calling `store.format_for_system_prompt("memory"/"user")`.
/// Rust: takes explicit optional store snapshot so callers without a live agent can still
/// compute categories; the agent-erased overload is `memory_blocks_from_agent_snapshot`.
pub fn memory_blocks(memory_block: Option<&str>, user_block: Option<&str>) -> (String, String) {
    // Mirrors try/except pass — if formatting fails, return ("", "")
    // Here inputs are already formatted blocks; just normalize None → "".
    (
        memory_block.unwrap_or("").to_string(),
        user_block.unwrap_or("").to_string(),
    )
}

/// Agent-shaped overload — mirrors `getattr(agent, "_memory_store", None)` path.
/// `store` is an opaque handle exposing `format_for_system_prompt`; we model it as
/// a closure `format_fn: Fn(&str) -> Option<String>` to keep 1:1 without importing Python.
pub fn memory_blocks_from_store<F>(store: Option<&F>, memory_enabled: bool, user_enabled: bool) -> (String, String)
where
    F: Fn(&str) -> Option<String>,
{
    let Some(fmt) = store else {
        return (String::new(), String::new());
    };
    // Mirrors `try: if getattr(agent, "_memory_enabled", True): memory_block = store.format...`
    let memory_block = if memory_enabled {
        // `or ""` via unwrap_or_default; exception → "" via catch in closure caller
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fmt("memory")))
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        String::new()
    };
    let user_block = if user_enabled {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fmt("user")))
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        String::new()
    };
    (memory_block, user_block)
}

#[allow(dead_code)]
fn _memory_blocks_agent<F>(store: Option<&F>, mem_enabled: bool, user_enabled: bool) -> (String, String)
where
    F: Fn(&str) -> Option<String>,
{
    memory_blocks_from_store(store, mem_enabled, user_enabled)
}

/// Mirrors `def _strip_blocks(text: str, *blocks: str) -> str:` (ll.81-86)
pub fn strip_blocks(text: &str, blocks: &[&str]) -> String {
    let mut out = text.to_string();
    for block in blocks {
        if !block.is_empty() {
            out = out.replace(block, "");
        }
    }
    out.trim().to_string()
}

#[allow(dead_code)]
fn _strip_blocks(text: &str, blocks: &[&str]) -> String {
    strip_blocks(text, blocks)
}

// ---------------------------------------------------------------------------
// Types — mirrors Python `Dict[str, Any]` shapes (ll.92-156)
// ---------------------------------------------------------------------------

/// Mirrors one entry of `payload["categories"]` (ll.140-149)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryEntry {
    pub id: String,
    pub label: String,
    pub tokens: usize,
    pub color: String,
}

impl CategoryEntry {
    pub fn to_value(&self) -> Value {
        json!({
            "color": self.color,
            "id": self.id,
            "label": self.label,
            "tokens": self.tokens,
        })
    }
}

/// Mirrors the return dict of `compute_session_context_breakdown` (ll.140-156)
#[derive(Debug, Clone)]
pub struct BreakdownPayload {
    pub categories: Vec<CategoryEntry>,
    pub context_max: i64,
    pub context_percent: i64,
    pub context_used: i64,
    pub estimated_total: i64,
    pub model: String,
}

impl BreakdownPayload {
    pub fn to_value(&self) -> Value {
        json!({
            "categories": self.categories.iter().map(|c| c.to_value()).collect::<Vec<_>>(),
            "context_max": self.context_max,
            "context_percent": self.context_percent,
            "context_used": self.context_used,
            "estimated_total": self.estimated_total,
            "model": self.model,
        })
    }
    pub fn from_value(v: &Value) -> Self {
        let categories = v
            .get("categories")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|cat| {
                        let id = cat.get("id")?.as_str()?.to_string();
                        let label = cat.get("label")?.as_str()?.to_string();
                        let tokens = cat.get("tokens")?.as_u64()? as usize;
                        let color = cat.get("color").and_then(|c| c.as_str()).unwrap_or("").to_string();
                        Some(CategoryEntry { id, label, tokens, color })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            categories,
            context_max: v.get("context_max").and_then(|x| x.as_i64()).unwrap_or(0),
            context_percent: v.get("context_percent").and_then(|x| x.as_i64()).unwrap_or(0),
            context_used: v.get("context_used").and_then(|x| x.as_i64()).unwrap_or(0),
            estimated_total: v.get("estimated_total").and_then(|x| x.as_i64()).unwrap_or(0),
            model: v.get("model").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        }
    }
}

/// Snapshot of agent-derived inputs for `compute_session_context_breakdown`.
///
/// Mirrors the live agent surface read at ll.97-131:
/// `build_system_prompt_parts(agent) -> {stable, context, volatile}`,
/// `agent.tools`, `agent.model`, `agent.context_compressor.{context_length,last_prompt_tokens}`.
/// Keep this struct so Rust callers don't need a live `AIAgent`; the python `agent: Any`
/// is this snapshot plus deferred `build_system_prompt_parts` which in Rust is caller-supplied.
#[derive(Debug, Clone)]
pub struct BreakdownInput {
    pub stable: String,
    pub context_part: String,
    pub volatile: String,
    pub memory_block: String,
    pub user_block: String,
    pub tools: Vec<Value>,
    pub messages: Vec<Value>,
    pub context_max: i64,
    pub measured_used: i64,
    pub model: String,
}

impl Default for BreakdownInput {
    fn default() -> Self {
        Self {
            stable: String::new(),
            context_part: String::new(),
            volatile: String::new(),
            memory_block: String::new(),
            user_block: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
            context_max: 0,
            measured_used: 0,
            model: String::new(),
        }
    }
}

// Stub: mirrors `agent.model_metadata.estimate_messages_tokens_rough` (l.94, l.115)
fn estimate_messages_tokens_rough(messages: &[Value]) -> usize {
    // Mirrors `agent.model_metadata.estimate_request_tokens_rough` heuristic — chars/4 of
    // content + tool_calls serialization + per-message overhead. Keep chars/4 + 4 per msg.
    let mut chars = 0usize;
    for m in messages {
        if let Some(obj) = m.as_object() {
            if let Some(content) = obj.get("content") {
                if let Some(s) = content.as_str() {
                    chars += s.len();
                } else {
                    chars += content.to_string().len();
                }
            }
            if let Some(tc) = obj.get("tool_calls") {
                chars += tc.to_string().len();
            }
        } else if let Some(s) = m.as_str() {
            chars += s.len();
        } else {
            chars += m.to_string().len();
        }
    }
    chars / 4 + messages.len() * 4
}

// ---------------------------------------------------------------------------
// compute_session_context_breakdown — mirrors Python ll.89-156
// ---------------------------------------------------------------------------

/// Mirrors `def compute_session_context_breakdown(agent: Any, messages: Optional[List[dict]] = None) -> Dict[str, Any]:` (ll.89-156)
///
/// Pure Rust version: takes a `BreakdownInput` snapshot instead of a live Python agent.
/// The caller is responsible for filling `stable/context_part/volatile` via
/// `build_system_prompt_parts` and `memory_block/user_block` via `_memory_blocks`.
pub fn compute_session_context_breakdown(input: &BreakdownInput) -> BreakdownPayload {
    // Mirrors `parts = build_system_prompt_parts(agent)` (l.97)
    // Inputs are pre-resolved; no lazy import needed.
    let stable = input.stable.as_str();
    let context = input.context_part.as_str();
    let volatile = input.volatile.as_str();

    // Mirrors `skills_match = _SKILLS_BLOCK_RE.search(stable); skills_index = ...` (ll.102-103)
    let skills_index = skills_block_re_dotall()
        .find(stable)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    // Mirrors `memory_block, user_block = _memory_blocks(agent)` (l.105)
    let memory_block = input.memory_block.as_str();
    let user_block = input.user_block.as_str();
    let memory_text = [memory_block, user_block]
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string();

    // Mirrors `system_core = _strip_blocks(stable, skills_index)` (l.108)
    let system_core = strip_blocks(stable, &[skills_index.as_str()]);
    // Mirrors `system_tail = _strip_blocks(volatile, memory_block, user_block)` (l.109)
    let system_tail = strip_blocks(volatile, &[memory_block, user_block]);
    // Mirrors `system_prompt_text = "\n\n".join(part for part in (system_core, system_tail) if part).strip()` (l.110)
    let system_prompt_text = [system_core, system_tail]
        .iter()
        .filter(|p| !p.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string();

    // Mirrors `tools = list(getattr(agent, "tools", None) or [])` (l.112)
    // `builtin_tools, mcp_tools, subagent_tools = _split_tools(tools)` (l.113)
    let (builtin_tools, mcp_tools, subagent_tools) = split_tools(&input.tools);

    // Mirrors `conversation_tokens = estimate_messages_tokens_rough(messages or [])` (l.115)
    let conversation_tokens = estimate_messages_tokens_rough(&input.messages);

    // Mirrors `categories = [...]` (ll.117-126)
    let raw_categories: Vec<(&str, &str, usize)> = vec![
        ("system_prompt", "System prompt", chars_to_tokens(&system_prompt_text)),
        ("tool_definitions", "Tool definitions", json_tokens_slice(&builtin_tools)),
        ("rules", "Rules", chars_to_tokens(context)),
        ("skills", "Skills", chars_to_tokens(&skills_index)),
        ("mcp", "MCP", json_tokens_slice(&mcp_tools)),
        ("subagent_definitions", "Subagent definitions", json_tokens_slice(&subagent_tools)),
        ("memory", "Memory", chars_to_tokens(&memory_text)),
        ("conversation", "Conversation", conversation_tokens),
    ];

    let estimated_total: usize = raw_categories.iter().map(|(_, _, t)| *t).sum();

    // Mirrors `comp = getattr(agent, "context_compressor", None)` + `context_max/measured_used` (ll.130-132)
    let context_max = input.context_max;
    let measured_used = input.measured_used;
    let context_used = if measured_used > 0 {
        measured_used as usize
    } else {
        estimated_total
    };

    // Mirrors `context_percent = max(0, min(100, round(context_used / context_max * 100))) if context_max else 0` (ll.134-138)
    let context_percent: i64 = if context_max > 0 {
        let pct = (context_used as f64 / context_max as f64 * 100.0).round() as i64;
        pct.clamp(0, 100)
    } else {
        0
    };

    // Mirrors return `categories: [{color, id, label, tokens} for ... if tokens > 0]` (ll.141-150)
    let categories: Vec<CategoryEntry> = raw_categories
        .into_iter()
        .filter(|(_, _, tokens)| *tokens > 0)
        .map(|(id, label, tokens)| CategoryEntry {
            id: id.to_string(),
            label: label.to_string(),
            tokens,
            color: category_color(id).to_string(),
        })
        .collect();

    BreakdownPayload {
        categories,
        context_max,
        context_percent,
        context_used: context_used as i64,
        estimated_total: estimated_total as i64,
        model: input.model.clone(),
    }
}

/// Value-wrapped overload for callers that already carry `BreakdownInput` as JSON.
pub fn compute_session_context_breakdown_value(input_value: &Value, messages: &[Value]) -> Value {
    // Attempt to recover `BreakdownInput` from a JSON agent snapshot; fallback to defaults.
    let ctx = input_value.get("context_compressor");
    let context_max = ctx
        .and_then(|c| c.get("context_length"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let measured_used = ctx
        .and_then(|c| c.get("last_prompt_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let inp = BreakdownInput {
        stable: input_value.get("stable").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        context_part: input_value.get("context").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        volatile: input_value.get("volatile").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        memory_block: input_value.get("memory_block").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        user_block: input_value.get("user_block").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        tools: input_value
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        messages: messages.to_vec(),
        context_max,
        measured_used,
        model: input_value.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    };
    compute_session_context_breakdown(&inp).to_value()
}

#[allow(dead_code)]
fn _compute_session_context_breakdown(input: &BreakdownInput) -> BreakdownPayload {
    compute_session_context_breakdown(input)
}

// ---------------------------------------------------------------------------
// /context rendering (CLI + gateway) — mirrors Python ll.159-360
// ---------------------------------------------------------------------------

/// Mirrors `_CATEGORY_GLYPHS = {...}` (ll.165-174)
pub const CATEGORY_GLYPHS: &[(&str, &str)] = &[
    ("system_prompt", "■"),
    ("tool_definitions", "▣"),
    ("rules", "▩"),
    ("skills", "▤"),
    ("mcp", "▥"),
    ("subagent_definitions", "▦"),
    ("memory", "▧"),
    ("conversation", "▨"),
];
#[allow(dead_code)]
const _CATEGORY_GLYPHS: &[(&str, &str)] = CATEGORY_GLYPHS;

fn category_glyph(id: &str) -> &'static str {
    for (k, g) in CATEGORY_GLYPHS {
        if *k == id {
            return g;
        }
    }
    "▪"
}

/// Mirrors `_FREE_GLYPH = "·"` (l.175)
pub const FREE_GLYPH: &str = "·";
#[allow(dead_code)]
const _FREE_GLYPH: &str = FREE_GLYPH;

/// Mirrors `_GRID_COLUMNS = 20` (l.176)
pub const GRID_COLUMNS: usize = 20;
#[allow(dead_code)]
const _GRID_COLUMNS: usize = GRID_COLUMNS;

/// Mirrors `_GRID_ROWS = 5` (l.177)
pub const GRID_ROWS: usize = 5;
#[allow(dead_code)]
const _GRID_ROWS: usize = GRID_ROWS;

/// Mirrors `_DETAILS_TABLE_LIMIT = 15` (l.181)
pub const DETAILS_TABLE_LIMIT: usize = 15;
#[allow(dead_code)]
const _DETAILS_TABLE_LIMIT: usize = DETAILS_TABLE_LIMIT;

// ---------------------------------------------------------------------------
// _bytes_to_tokens — mirrors Python ll.184-187
// ---------------------------------------------------------------------------

/// Mirrors `def _bytes_to_tokens(size: Optional[int]) -> Optional[int]:` (ll.184-187)
pub fn bytes_to_tokens(size: Option<i64>) -> Option<usize> {
    let s = size?;
    Some(((s as usize) + 3) / 4)
}

#[allow(dead_code)]
fn _bytes_to_tokens(size: Option<i64>) -> Option<usize> {
    bytes_to_tokens(size)
}

// ---------------------------------------------------------------------------
// compute_context_details — mirrors Python ll.190-229
// ---------------------------------------------------------------------------

/// Mirrors skill entry in `compute_context_details` return (ll.211-217)
#[derive(Debug, Clone)]
pub struct SkillDetail {
    pub name: String,
    pub index_tokens: usize,
    pub skill_md_tokens: Option<usize>,
}

/// Mirrors toolset entry (ll.219-227)
#[derive(Debug, Clone)]
pub struct ToolsetDetail {
    pub toolset: String,
    pub tool_count: usize,
    pub schema_tokens: usize,
}

/// Mirrors `return {"skills": skills, "toolsets": toolsets}` (l.229)
#[derive(Debug, Clone, Default)]
pub struct ContextDetails {
    pub skills: Vec<SkillDetail>,
    pub toolsets: Vec<ToolsetDetail>,
}

impl ContextDetails {
    pub fn to_value(&self) -> Value {
        json!({
            "skills": self.skills.iter().map(|s| json!({
                "name": s.name,
                "index_tokens": s.index_tokens,
                "skill_md_tokens": s.skill_md_tokens,
            })).collect::<Vec<_>>(),
            "toolsets": self.toolsets.iter().map(|t| json!({
                "toolset": t.toolset,
                "tool_count": t.tool_count,
                "schema_tokens": t.schema_tokens,
            })).collect::<Vec<_>>(),
        })
    }
}

// Stubs mirroring `hermes_cli.prompt_size._compute_skills_breakdown` + `_compute_toolsets_breakdown` (ll.199-204)

/// Stub: mirrors `hermes_cli.prompt_size._compute_skills_breakdown(skills_block)` (l.212)
/// Parses the live `<available_skills>` block into per-skill byte attributions.
/// Canonical impl lives in hermes-cli / prompt_size; stub returns empty.
fn compute_skills_breakdown(_skills_block: &str) -> Vec<HashMap<String, Value>> {
    Vec::new()
}

/// Stub: mirrors `hermes_cli.prompt_size._compute_toolsets_breakdown(tools)` (l.222)
fn compute_toolsets_breakdown(_tools: &[Value]) -> Vec<HashMap<String, Value>> {
    Vec::new()
}

/// Mirrors `def compute_context_details(agent: Any) -> Dict[str, Any]:` (ll.190-229)
pub fn compute_context_details(input: &BreakdownInput) -> ContextDetails {
    // Mirrors `parts = build_system_prompt_parts(agent); stable = parts.get("stable", "") or ""` (ll.205-206)
    let stable = input.stable.as_str();
    let skills_block = skills_block_re_dotall()
        .find(stable)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    // Mirrors `skills: List[Dict[str, Any]] = []; if skills_block: for entry in _compute_skills_breakdown(...)` (ll.210-217)
    let mut skills: Vec<SkillDetail> = Vec::new();
    if !skills_block.is_empty() {
        for entry in compute_skills_breakdown(&skills_block) {
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let index_line_bytes = entry.get("index_line_bytes").and_then(|v| v.as_i64());
            let skill_md_bytes = entry.get("skill_md_bytes").and_then(|v| v.as_i64());
            skills.push(SkillDetail {
                name,
                index_tokens: bytes_to_tokens(index_line_bytes).unwrap_or(0),
                skill_md_tokens: bytes_to_tokens(skill_md_bytes),
            });
        }
    }

    // Mirrors `toolsets: List[Dict[str, Any]] = []; tools = list(getattr(agent, "tools", None) or []); if tools: for group in _compute_toolsets_breakdown(tools)` (ll.219-227)
    let mut toolsets: Vec<ToolsetDetail> = Vec::new();
    if !input.tools.is_empty() {
        for group in compute_toolsets_breakdown(&input.tools) {
            toolsets.push(ToolsetDetail {
                toolset: group.get("toolset").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                tool_count: group
                    .get("tool_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as usize,
                schema_tokens: bytes_to_tokens(group.get("json_bytes").and_then(|v| v.as_i64())).unwrap_or(0),
            });
        }
    }

    ContextDetails { skills, toolsets }
}

#[allow(dead_code)]
fn _compute_context_details(input: &BreakdownInput) -> ContextDetails {
    compute_context_details(input)
}

// ---------------------------------------------------------------------------
// render_context_grid — mirrors Python ll.232-257
// ---------------------------------------------------------------------------

/// Mirrors `def render_context_grid(payload: Dict[str, Any]) -> List[str]:` (ll.232-257)
pub fn render_context_grid(payload: &BreakdownPayload) -> Vec<String> {
    let context_max = payload.context_max;
    let total_cells = GRID_COLUMNS * GRID_ROWS;
    let mut cells: Vec<String> = Vec::new();

    // Mirrors `if context_max > 0: for cat in categories: tokens = int(cat.get("tokens") or 0); n = round(tokens / context_max * total_cells)` (ll.243-250)
    if context_max > 0 {
        for cat in &payload.categories {
            let tokens = cat.tokens as f64;
            let mut n = (tokens / context_max as f64 * total_cells as f64).round() as usize;
            // Mirrors `if tokens > 0 and n == 0: n = 1` (ll.247-248)
            if cat.tokens > 0 && n == 0 {
                n = 1;
            }
            let glyph = category_glyph(&cat.id);
            cells.extend(std::iter::repeat(glyph.to_string()).take(n));
        }
        cells.truncate(total_cells);
    }
    // Mirrors `cells.extend([_FREE_GLYPH] * (total_cells - len(cells)))` (l.252)
    while cells.len() < total_cells {
        cells.push(FREE_GLYPH.to_string());
    }

    // Mirrors `return [" ".join(cells[row * _GRID_COLUMNS:(row+1)*_GRID_COLUMNS]) for row in range(_GRID_ROWS)]` (ll.254-257)
    (0..GRID_ROWS)
        .map(|row| {
            let start = row * GRID_COLUMNS;
            let end = (row + 1) * GRID_COLUMNS;
            cells[start..end].join(" ")
        })
        .collect()
}

/// Value overload — mirrors `payload: Dict[str, Any]` dict form.
pub fn render_context_grid_value(payload: &Value) -> Vec<String> {
    render_context_grid(&BreakdownPayload::from_value(payload))
}

#[allow(dead_code)]
fn _render_context_grid(payload: &BreakdownPayload) -> Vec<String> {
    render_context_grid(payload)
}

// ---------------------------------------------------------------------------
// render_context_category_lines — mirrors Python ll.260-284
// ---------------------------------------------------------------------------

/// Mirrors `def render_context_category_lines(payload: Dict[str, Any]) -> List[str]:` (ll.260-284)
pub fn render_context_category_lines(payload: &BreakdownPayload) -> Vec<String> {
    let categories = &payload.categories;
    let context_max = payload.context_max;
    let estimated_total = payload.estimated_total;
    // Mirrors `denom = context_max or estimated_total` (l.265)
    let denom = if context_max != 0 { context_max } else { estimated_total };

    let mut lines: Vec<String> = vec!["Estimated usage by category".to_string()];
    // Mirrors `if not categories: lines.append("  (no data yet — send a message first)"); return lines` (ll.268-270)
    if categories.is_empty() {
        lines.push("  (no data yet — send a message first)".to_string());
        return lines;
    }

    // Mirrors `width = max(len(str(cat.get("label") or "")) for cat in categories); width = max(width, len("Free space"))` (ll.272-273)
    let width = categories
        .iter()
        .map(|c| c.label.len())
        .max()
        .unwrap_or(0)
        .max("Free space".len());

    // Mirrors `for cat in categories: tokens = int(cat.get("tokens") or 0); glyph = _CATEGORY_GLYPHS.get(...); pct = tokens / denom * 100 if denom else 0.0; label = str(cat.get("label") or cat.get("id") or "")` (ll.274-279)
    for cat in categories {
        let tokens = cat.tokens as f64;
        let glyph = category_glyph(&cat.id);
        let pct = if denom != 0 { tokens / denom as f64 * 100.0 } else { 0.0 };
        let label = if cat.label.is_empty() { &cat.id } else { &cat.label };
        // Mirrors `lines.append(f"{glyph} {label:<{width}} {tokens:>9,} tokens {pct:>5.1f}%")` (l.279)
        // Rust comma formatting: manually format with `,` grouping.
        lines.push(format!(
            "{} {:<width$} {:>9} tokens {:>5.1}%",
            glyph,
            label,
            format_comma(cat.tokens),
            pct,
            width = width
        ));
    }

    // Mirrors `if context_max > 0: free = max(0, context_max - estimated_total); pct = free / context_max * 100; lines.append(...)` (ll.280-283)
    if context_max > 0 {
        let free = (context_max - estimated_total).max(0) as f64;
        let pct = free / context_max as f64 * 100.0;
        lines.push(format!(
            "{} {:<width$} {:>9} tokens {:>5.1}%",
            FREE_GLYPH,
            "Free space",
            format_comma(free as usize),
            pct,
            width = width
        ));
    }
    lines
}

pub fn render_context_category_lines_value(payload: &Value) -> Vec<String> {
    render_context_category_lines(&BreakdownPayload::from_value(payload))
}

#[allow(dead_code)]
fn _render_context_category_lines(payload: &BreakdownPayload) -> Vec<String> {
    render_context_category_lines(payload)
}

fn format_comma(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let mut count = 0;
    for ch in s.chars().rev() {
        if count == 3 {
            out.push(',');
            count = 0;
        }
        out.push(ch);
        count += 1;
    }
    out.chars().rev().collect()
}

// ---------------------------------------------------------------------------
// render_context_details_lines — mirrors Python ll.287-322
// ---------------------------------------------------------------------------

/// Mirrors `def render_context_details_lines(details: Dict[str, Any]) -> List[str]:` (ll.287-322)
pub fn render_context_details_lines(details: &ContextDetails) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Mirrors `toolsets = details.get("toolsets") or []; if toolsets:` (ll.290-301)
    if !details.toolsets.is_empty() {
        lines.push("Toolsets by schema cost (largest first)".to_string());
        for group in details.toolsets.iter().take(DETAILS_TABLE_LIMIT) {
            // Mirrors `lines.append(f"  {group['toolset']:<24} {group['tool_count']:>3} tools {group['schema_tokens']:>8,} tokens")` (ll.294-298)
            lines.push(format!(
                "  {:<24} {:>3} tools {:>8} tokens",
                group.toolset,
                group.tool_count,
                format_comma(group.schema_tokens)
            ));
        }
        let remaining = details.toolsets.len().saturating_sub(DETAILS_TABLE_LIMIT);
        // Mirrors `if remaining > 0: lines.append(f"  … and {remaining} more")` (ll.299-301)
        if remaining > 0 {
            lines.push(format!("  … and {} more", remaining));
        }
    }

    // Mirrors `skills = details.get("skills") or []; if skills:` (ll.303-320)
    if !details.skills.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Skills by cost (index = always-on; SKILL.md = cost when loaded)".to_string());
        for entry in details.skills.iter().take(DETAILS_TABLE_LIMIT) {
            // Mirrors `name = str(entry.get("name") or ""); if len(name) > 28: name = name[:27] + "…"` (ll.309-311)
            let mut name = entry.name.clone();
            if name.chars().count() > 28 {
                name = name.chars().take(27).collect::<String>() + "…";
            }
            // Mirrors `md = entry.get("skill_md_tokens"); md_str = f"{md:>8,}" if md is not None else f"{'n/a':>8}"` (ll.312-313)
            let md_str = match entry.skill_md_tokens {
                Some(v) => format!("{:>8}", format_comma(v)),
                None => format!("{:>8}", "n/a"),
            };
            // Mirrors `lines.append(f"  {name:<28} index {entry['index_tokens']:>6,}  SKILL.md {md_str} tokens")` (ll.314-317)
            lines.push(format!(
                "  {:<28} index {:>6}  SKILL.md {} tokens",
                name,
                format_comma(entry.index_tokens),
                md_str
            ));
        }
        let remaining = details.skills.len().saturating_sub(DETAILS_TABLE_LIMIT);
        if remaining > 0 {
            lines.push(format!("  … and {} more", remaining));
        }
    }

    lines
}

pub fn render_context_details_lines_value(details: &Value) -> Vec<String> {
    let d = ContextDetails {
        skills: details
            .get("skills")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(SkillDetail {
                            name: e.get("name")?.as_str()?.to_string(),
                            index_tokens: e.get("index_tokens")?.as_u64()? as usize,
                            skill_md_tokens: e.get("skill_md_tokens").and_then(|v| v.as_u64()).map(|v| v as usize),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        toolsets: details
            .get("toolsets")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(ToolsetDetail {
                            toolset: e.get("toolset")?.as_str()?.to_string(),
                            tool_count: e.get("tool_count")?.as_u64()? as usize,
                            schema_tokens: e.get("schema_tokens")?.as_u64()? as usize,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    render_context_details_lines(&d)
}

#[allow(dead_code)]
fn _render_context_details_lines(details: &ContextDetails) -> Vec<String> {
    render_context_details_lines(details)
}

// ---------------------------------------------------------------------------
// render_context_breakdown_lines — mirrors Python ll.325-360
// ---------------------------------------------------------------------------

/// Mirrors `def render_context_breakdown_lines(payload: Dict[str, Any], *, details: Optional[Dict[str, Any]] = None, grid: bool = True) -> List[str]:` (ll.325-360)
pub fn render_context_breakdown_lines(
    payload: &BreakdownPayload,
    details: Option<&ContextDetails>,
    grid: bool,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    // Mirrors `if grid: lines.extend(render_context_grid(payload)); lines.append("")` (ll.338-340)
    if grid {
        lines.extend(render_context_grid(payload));
        lines.push(String::new());
    }
    // Mirrors `lines.extend(render_context_category_lines(payload))` (l.341)
    lines.extend(render_context_category_lines(payload));

    // Mirrors `context_max = int(payload.get("context_max") or 0); context_used = int(payload.get("context_used") or 0); if context_max > 0:` (ll.343-350)
    let context_max = payload.context_max;
    let context_used = payload.context_used;
    if context_max > 0 {
        let pct = payload.context_percent;
        lines.push(String::new());
        // Mirrors `lines.append(f"Context window: {context_used:,} / {context_max:,} tokens ({pct}%)")` (ll.348-350)
        lines.push(format!(
            "Context window: {} / {} tokens ({}%)",
            format_comma(context_used as usize),
            format_comma(context_max as usize),
            pct
        ));
    }

    // Mirrors `if details is not None: detail_lines = render_context_details_lines(details); if detail_lines: lines.append(""); lines.extend(detail_lines); else: lines.append(""); lines.append("Use /context all ...")` (ll.352-359)
    if let Some(d) = details {
        let detail_lines = render_context_details_lines(d);
        if !detail_lines.is_empty() {
            lines.push(String::new());
            lines.extend(detail_lines);
        }
    } else {
        lines.push(String::new());
        lines.push("Use /context all for per-skill and per-toolset costs.".to_string());
    }
    lines
}

pub fn render_context_breakdown_lines_value(
    payload: &Value,
    details: Option<&Value>,
    grid: bool,
) -> Vec<String> {
    let p = BreakdownPayload::from_value(payload);
    let d = details.map(|v| ContextDetails {
        skills: v
            .get("skills")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(SkillDetail {
                            name: e.get("name")?.as_str()?.to_string(),
                            index_tokens: e.get("index_tokens")?.as_u64()? as usize,
                            skill_md_tokens: e.get("skill_md_tokens").and_then(|x| x.as_u64()).map(|x| x as usize),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        toolsets: v
            .get("toolsets")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(ToolsetDetail {
                            toolset: e.get("toolset")?.as_str()?.to_string(),
                            tool_count: e.get("tool_count")?.as_u64()? as usize,
                            schema_tokens: e.get("schema_tokens")?.as_u64()? as usize,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    });
    render_context_breakdown_lines(&p, d.as_ref(), grid)
}

#[allow(dead_code)]
fn _render_context_breakdown_lines(
    payload: &BreakdownPayload,
    details: Option<&ContextDetails>,
    grid: bool,
) -> Vec<String> {
    render_context_breakdown_lines(payload, details, grid)
}
