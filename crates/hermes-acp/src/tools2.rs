//! ACP tool-call helpers — maps Hermes tools to ACP ToolKind and builds content.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/acp_adapter/tools.py`
//! (1348 lines, full file — single slice). Covers: `TOOL_KIND_MAP`,
//! `_POLISHED_TOOLS`, `get_tool_kind`, `make_tool_call_id`, `build_tool_title`,
//! `_text` / `_json_loads_maybe` / `_tool_result_failed` / `_truncate_text` /
//! `_fenced_text`, all `_format_*` helpers (todo, read_file, search_files,
//! execute_code, skill_view, skill_manage, web_search, web_extract, process,
//! delegate_task, session_search, memory, edit_result, browser, media/cron,
//! structured_value, generic), `_build_polished_completion_content`,
//! `_strip_diff_prefix`, `_parse_unified_diff_content`,
//! `_build_tool_complete_content`, `build_tool_start` / `_build_tool_start`,
//! `_is_structured_json_result`, `build_tool_complete`, `extract_locations`.
//!
//! Mirrors Python module docstring (line 1):
//! ```text
//! ACP tool-call helpers for mapping hermes tools to ACP ToolKind and building content.
//! ```
//!
//! T0411 — 1:1 port, no cargo (NEVER cargo). All external crates / ACP SDK
//! types are stubbed as local structs for traceability; `uuid` as std-only
//! pseudo-UUID, `json` as minimal manual parser (std only), `acp` helpers as
//! local stubs. `logging` maps to `logger_name` stub / `eprintln!`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors line 1
// ---------------------------------------------------------------------------

/// Mirrors `acp_adapter/tools.py` top-level docstring (line 1).
pub const MODULE_DOC: &str =
    "ACP tool-call helpers for mapping hermes tools to ACP ToolKind and building content.";

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (line 18)
// ---------------------------------------------------------------------------

pub fn logger_name() -> &'static str {
    "acp_adapter.tools"
}

// ---------------------------------------------------------------------------
// ArgVal — models `Dict[str, Any]` values without cargo (NEVER cargo)
// ---------------------------------------------------------------------------

/// Lightweight stand-in for Python's `Any` inside tool `arguments`.
/// Mirrors the varied shapes in `build_tool_title` / format helpers: plain
/// strings, ints, bools, lists of strings/dicts, and nested dicts. Real
/// Python uses `json.loads` structures; this enum covers the shapes touched
/// by the 1:1 branches without pulling `serde_json`.
#[derive(Debug, Clone)]
pub enum ArgVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ArgVal>),
    Dict(HashMap<String, ArgVal>),
}

impl ArgVal {
    pub fn as_str(&self) -> Option<&str> {
        if let ArgVal::Str(s) = self { Some(s.as_str()) } else { None }
    }
    pub fn as_list(&self) -> Option<&Vec<ArgVal>> {
        if let ArgVal::List(v) = self { Some(v) } else { None }
    }
}

impl From<String> for ArgVal {
    fn from(s: String) -> Self { ArgVal::Str(s) }
}
impl From<&str> for ArgVal {
    fn from(s: &str) -> Self { ArgVal::Str(s.to_string()) }
}

pub type Args = HashMap<String, ArgVal>;

fn arg_str(args: &Args, key: &str) -> Option<String> {
    args.get(key).and_then(|v| match v {
        ArgVal::Str(s) => Some(s.clone()),
        ArgVal::Int(n) => Some(n.to_string()),
        ArgVal::Float(f) => Some(f.to_string()),
        ArgVal::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}
fn arg_str_trim(args: &Args, key: &str) -> String {
    arg_str(args, key).unwrap_or_default().trim().to_string()
}
fn arg_list_len(args: &Args, key: &str) -> Option<usize> {
    args.get(key).and_then(|v| v.as_list()).map(|l| l.len())
}

// ---------------------------------------------------------------------------
// ToolKind — mirrors `acp.schema.ToolKind` (lines 11-16, 24-59)
// ---------------------------------------------------------------------------

/// Mirrors `ToolKind` literal union in Python (`"read" | "edit" | "search" | ...`).
/// In Python the map values are string literals; here we keep the same strings
/// so `get_tool_kind` round-trips identically.
pub const TOOL_KIND_READ: &str = "read";
pub const TOOL_KIND_EDIT: &str = "edit";
pub const TOOL_KIND_SEARCH: &str = "search";
pub const TOOL_KIND_EXECUTE: &str = "execute";
pub const TOOL_KIND_FETCH: &str = "fetch";
pub const TOOL_KIND_THINK: &str = "think";
pub const TOOL_KIND_OTHER: &str = "other";

/// Mirrors `TOOL_KIND_MAP: Dict[str, ToolKind]` (lines 24-59).
pub fn tool_kind_map_entries() -> &'static [(&'static str, &'static str)] {
    &[
        // File operations
        ("read_file", TOOL_KIND_READ),
        ("write_file", TOOL_KIND_EDIT),
        ("patch", TOOL_KIND_EDIT),
        ("search_files", TOOL_KIND_SEARCH),
        // Terminal / execution
        ("terminal", TOOL_KIND_EXECUTE),
        ("process", TOOL_KIND_EXECUTE),
        ("execute_code", TOOL_KIND_EXECUTE),
        // Session/meta tools
        ("todo", TOOL_KIND_OTHER),
        ("skill_view", TOOL_KIND_READ),
        ("skills_list", TOOL_KIND_READ),
        ("skill_manage", TOOL_KIND_EDIT),
        // Web / fetch
        ("web_search", TOOL_KIND_FETCH),
        ("web_extract", TOOL_KIND_FETCH),
        // Browser
        ("browser_navigate", TOOL_KIND_FETCH),
        ("browser_click", TOOL_KIND_EXECUTE),
        ("browser_type", TOOL_KIND_EXECUTE),
        ("browser_snapshot", TOOL_KIND_READ),
        ("browser_vision", TOOL_KIND_READ),
        ("browser_scroll", TOOL_KIND_EXECUTE),
        ("browser_press", TOOL_KIND_EXECUTE),
        ("browser_back", TOOL_KIND_EXECUTE),
        ("browser_get_images", TOOL_KIND_READ),
        // Agent internals
        ("delegate_task", TOOL_KIND_EXECUTE),
        ("vision_analyze", TOOL_KIND_READ),
        ("image_generate", TOOL_KIND_EXECUTE),
        ("text_to_speech", TOOL_KIND_EXECUTE),
        // Thinking / meta
        ("_thinking", TOOL_KIND_THINK),
    ]
}

/// Mirrors `_POLISHED_TOOLS` set (lines 62-82).
pub fn polished_tools() -> &'static [&'static str] {
    &[
        "todo", "memory", "session_search", "delegate_task",
        "read_file", "write_file", "patch", "search_files", "terminal", "process", "execute_code",
        "skill_view", "skills_list", "skill_manage", "web_search", "web_extract",
        "browser_navigate", "browser_click", "browser_type", "browser_press", "browser_scroll",
        "browser_back", "browser_snapshot", "browser_console", "browser_get_images", "browser_vision",
        "vision_analyze", "image_generate", "text_to_speech",
        "cronjob", "send_message", "clarify", "discord", "discord_admin",
        "ha_list_entities", "ha_get_state", "ha_list_services", "ha_call_service",
        "feishu_doc_read", "feishu_drive_list_comments", "feishu_drive_list_comment_replies",
        "feishu_drive_reply_comment", "feishu_drive_add_comment",
        "kanban_create", "kanban_show", "kanban_comment", "kanban_complete",
        "kanban_block", "kanban_request_review", "kanban_request_changes",
        "kanban_link", "kanban_heartbeat",
        "yb_query_group_info", "yb_query_group_members", "yb_search_sticker",
        "yb_send_dm", "yb_send_sticker",
    ]
}

fn is_polished(tool_name: &str) -> bool {
    polished_tools().contains(&tool_name)
}

// ---------------------------------------------------------------------------
// ACP stubs — mirrors `acp` / `acp.schema` imports (lines 10-16) + helpers
//   `acp.tool_content`, `acp.text_block`, `acp.tool_diff_content`,
//   `acp.start_tool_call`, `acp.update_tool_call` (used throughout)
//   NEVER cargo: local structs replace the Python SDK.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TextBlock { pub text: String }

#[derive(Debug, Clone)]
pub struct DiffContent {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub enum ToolContent {
    Text(TextBlock),
    Diff(DiffContent),
}

#[derive(Debug, Clone)]
pub struct ToolCallLocation {
    pub path: String,
    pub line: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ToolCallStart {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Vec<ToolCallLocation>,
    pub raw_input: Option<Args>,
}

#[derive(Debug, Clone)]
pub struct ToolCallProgress {
    pub tool_call_id: String,
    pub kind: String,
    pub status: String, // "completed" | "failed"
    pub content: Option<Vec<ToolContent>>,
    pub raw_output: Option<String>,
}

// Mirrors `acp.text_block(content)` — wraps a string as a TextBlock.
pub fn text_block(content: String) -> TextBlock { TextBlock { text: content } }
// Mirrors `acp.tool_content(acp.text_block(...))` — wraps TextBlock as ToolContent.
pub fn tool_content(block: TextBlock) -> ToolContent { ToolContent::Text(block) }
pub fn tool_diff_content(path: String, old_text: Option<String>, new_text: String) -> ToolContent {
    ToolContent::Diff(DiffContent { path, old_text, new_text })
}
// Mirrors `edit_diff` ad-hoc object used in `build_tool_start` (lines 1049-1050).
#[derive(Debug, Clone, Default)]
pub struct EditDiff {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}
pub fn start_tool_call(
    tool_call_id: String,
    title: String,
    kind: String,
    content: Option<Vec<ToolContent>>,
    locations: Vec<ToolCallLocation>,
    raw_input: Option<Args>,
) -> ToolCallStart {
    ToolCallStart { tool_call_id, title, kind, content, locations, raw_input }
}
pub fn update_tool_call(
    tool_call_id: String,
    kind: String,
    status: String,
    content: Option<Vec<ToolContent>>,
    raw_output: Option<String>,
) -> ToolCallProgress {
    ToolCallProgress { tool_call_id, kind, status, content, raw_output }
}

// ---------------------------------------------------------------------------
// get_tool_kind — lines 85-87
// ---------------------------------------------------------------------------

/// Mirrors `def get_tool_kind(tool_name: str) -> ToolKind` (85-87).
pub fn get_tool_kind(tool_name: &str) -> String {
    for (k, v) in tool_kind_map_entries() {
        if *k == tool_name { return v.to_string(); }
    }
    TOOL_KIND_OTHER.to_string()
}

#[allow(dead_code)]
pub fn _get_tool_kind(tool_name: &str) -> String { get_tool_kind(tool_name) }

// ---------------------------------------------------------------------------
// make_tool_call_id — lines 90-92
// ---------------------------------------------------------------------------

static TC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Mirrors `def make_tool_call_id() -> str` (90-92): `f"tc-{uuid.uuid4().hex[:12]}"`.
/// std-only: time-nanos + monotonic counter, hex 12 chars, same `tc-` prefix.
pub fn make_tool_call_id() -> String {
    // Combine time and counter for per-process uniqueness (no real uuid crate — NEVER cargo).
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let c = TC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = nanos.wrapping_add(c.wrapping_mul(0x9E3779B97F4A7C15));
    // Take low 48 bits -> 12 hex chars (`hex[:12]` = 48 bits).
    let low = mixed & 0xFFF_FFFF_FFFFu64;
    format!("tc-{low:012x}")
}

// ---------------------------------------------------------------------------
// build_tool_title — lines 95-189
// ---------------------------------------------------------------------------

/// Mirrors `def build_tool_title(tool_name: str, args: Dict[str, Any]) -> str` (95-189).
/// Every `if tool_name == "..."` branch is preserved with the same truncation
/// and fallback semantics as Python.
pub fn build_tool_title(tool_name: &str, args: &Args) -> String {
    // terminal: 97-101
    if tool_name == "terminal" {
        let cmd = arg_str(args, "command").unwrap_or_default();
        if cmd.len() > 80 {
            return format!("terminal: {}...", &cmd[..77]);
        }
        return format!("terminal: {cmd}");
    }
    if tool_name == "read_file" {
        return format!("read: {}", arg_str(args, "path").unwrap_or_else(|| "?".to_string()));
    }
    if tool_name == "write_file" {
        return format!("write: {}", arg_str(args, "path").unwrap_or_else(|| "?".to_string()));
    }
    if tool_name == "patch" {
        let mode = arg_str(args, "mode").unwrap_or_else(|| "replace".to_string());
        let path = arg_str(args, "path").unwrap_or_else(|| "?".to_string());
        return format!("patch ({mode}): {path}");
    }
    if tool_name == "search_files" {
        return format!("search: {}", arg_str(args, "pattern").unwrap_or_else(|| "?".to_string()));
    }
    if tool_name == "web_search" {
        return format!("web search: {}", arg_str(args, "query").unwrap_or_else(|| "?".to_string()));
    }
    if tool_name == "web_extract" {
        // 114-123 — handle `urls` as list of str-or-dicts.
        if let Some(urls_val) = args.get("urls") {
            if let ArgVal::List(urls) = urls_val {
                if !urls.is_empty() {
                    let first_raw = match &urls[0] {
                        ArgVal::Str(s) => s.clone(),
                        ArgVal::Dict(d) => d.get("url").and_then(|v| v.as_str().map(|s| s.to_string()))
                            .or_else(|| d.get("href").and_then(|v| v.as_str().map(|s| s.to_string())))
                            .unwrap_or_else(|| "?".to_string()),
                        _ => "?".to_string(),
                    };
                    let first = if first_raw.is_empty() { "?".to_string() } else { first_raw };
                    let suffix = if urls.len() > 1 { format!(" (+{})", urls.len() - 1) } else { String::new() };
                    return format!("extract: {first}{suffix}");
                }
            }
        }
        return "web extract".to_string();
    }
    if tool_name == "process" {
        let action = arg_str_trim(args, "action");
        let action = if action.is_empty() { "manage".to_string() } else { action };
        let sid = arg_str_trim(args, "session_id");
        if !sid.is_empty() { return format!("process {action}: {sid}"); }
        return format!("process {action}");
    }
    if tool_name == "delegate_task" {
        // 128-135
        if let Some(tasks) = args.get("tasks") {
            if let ArgVal::List(l) = tasks {
                if !l.is_empty() { return format!("delegate batch ({} tasks)", l.len()); }
            }
        }
        let goal = arg_str(args, "goal").unwrap_or_default();
        if !goal.is_empty() {
            if goal.len() > 60 { return format!("delegate: {}...", &goal[..57]); }
            return format!("delegate: {goal}");
        }
        return "delegate task".to_string();
    }
    if tool_name == "session_search" {
        let query = arg_str_trim(args, "query");
        if !query.is_empty() { return format!("session search: {query}"); }
        return "recent sessions".to_string();
    }
    if tool_name == "memory" {
        let action = {
            let a = arg_str_trim(args, "action");
            if a.is_empty() { "manage".to_string() } else { a }
        };
        let target = {
            let t = arg_str_trim(args, "target");
            if t.is_empty() { "memory".to_string() } else { t }
        };
        return format!("memory {action}: {target}");
    }
    if tool_name == "execute_code" {
        let code = arg_str(args, "code").unwrap_or_default();
        let trimmed = code.trim().to_string();
        if !trimmed.is_empty() {
            // first non-empty line
            let first_line = trimmed.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            if !first_line.is_empty() {
                if first_line.len() > 70 { return format!("python: {}...", &first_line[..67]); }
                return format!("python: {first_line}");
            }
        }
        return "python code".to_string();
    }
    if tool_name == "todo" {
        if let Some(todos) = args.get("todos") {
            if let ArgVal::List(items) = todos {
                let n = items.len();
                let s = if n == 1 { "" } else { "s" };
                return format!("todo ({n} item{s})");
            }
        }
        return "todo".to_string();
    }
    if tool_name == "skill_view" {
        let name = { let n = arg_str_trim(args, "name"); if n.is_empty() { "?".to_string() } else { n } };
        let file_path = arg_str_trim(args, "file_path");
        let suffix = if !file_path.is_empty() { format!("/{file_path}") } else { String::new() };
        return format!("skill view ({name}{suffix})");
    }
    if tool_name == "skills_list" {
        let category = arg_str_trim(args, "category");
        if !category.is_empty() { return format!("skills list ({category})"); }
        return "skills list".to_string();
    }
    if tool_name == "skill_manage" {
        let action = { let a = arg_str_trim(args, "action"); if a.is_empty() { "manage".to_string() } else { a } };
        let name = { let n = arg_str_trim(args, "name"); if n.is_empty() { "?".to_string() } else { n } };
        let file_path = arg_str_trim(args, "file_path");
        let target = if !file_path.is_empty() { format!("{name}/{file_path}") } else { name.clone() };
        let target = if target.len() > 64 { format!("{}...", &target[..61]) } else { target };
        return format!("skill {action}: {target}");
    }
    if tool_name == "browser_navigate" {
        return format!("navigate: {}", arg_str(args, "url").unwrap_or_else(|| "?".to_string()));
    }
    if tool_name == "browser_snapshot" { return "browser snapshot".to_string(); }
    if tool_name == "browser_vision" {
        let q = arg_str(args, "question").unwrap_or_else(|| "?".to_string());
        return format!("browser vision: {}", &q[..q.len().min(50)]);
    }
    if tool_name == "browser_get_images" { return "browser images".to_string(); }
    if tool_name == "vision_analyze" {
        let q = arg_str(args, "question").unwrap_or_else(|| "?".to_string());
        return format!("analyze image: {}", &q[..q.len().min(50)]);
    }
    if tool_name == "image_generate" {
        let prompt = {
            let p = arg_str(args, "prompt").or_else(|| arg_str(args, "description")).unwrap_or_default();
            p.trim().to_string()
        };
        if !prompt.is_empty() {
            return format!("generate image: {}", &prompt[..prompt.len().min(50)]);
        }
        return "generate image".to_string();
    }
    if tool_name == "cronjob" {
        let action = { let a = arg_str_trim(args, "action"); if a.is_empty() { "manage".to_string() } else { a } };
        let job_id = arg_str_trim(args, "job_id");
        let job_id = if job_id.is_empty() { arg_str_trim(args, "id") } else { job_id };
        if !job_id.is_empty() { return format!("cron {action}: {job_id}"); }
        return format!("cron {action}");
    }
    tool_name.to_string()
}

// ---------------------------------------------------------------------------
// _text — lines 192-193
// ---------------------------------------------------------------------------

/// Mirrors `def _text(content: str) -> Any` (192-193): `return acp.tool_content(acp.text_block(content))`.
pub fn _text(content: String) -> ToolContent {
    tool_content(text_block(content))
}
#[allow(dead_code)]
pub fn text_content(content: String) -> ToolContent { _text(content) }

// ---------------------------------------------------------------------------
// Minimal JSON value + parser — mirrors `json` / `_json_loads_maybe` (196-211)
//   NEVER cargo: std-only recursive descent. Covers the shapes actually
//   asserted in `_tool_result_failed` and the `_format_*` helpers.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<JsonVal>),
    Object(HashMap<String, JsonVal>),
}

impl JsonVal {
    pub fn as_object(&self) -> Option<&HashMap<String, JsonVal>> {
        if let JsonVal::Object(m) = self { Some(m) } else { None }
    }
    pub fn as_array(&self) -> Option<&Vec<JsonVal>> {
        if let JsonVal::Array(a) = self { Some(a) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let JsonVal::Str(s) = self { Some(s.as_str()) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let JsonVal::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let JsonVal::Number(n) = self { Some(*n) } else { None }
    }
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|f| f as i64)
    }
    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        self.as_object().and_then(|m| m.get(key))
    }
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| match v {
            JsonVal::Str(s) => Some(s.clone()),
            JsonVal::Number(n) => Some(n.to_string()),
            JsonVal::Bool(b) => Some(b.to_string()),
            _ => None,
        })
    }
}

// Tiny JSON parser — handles strings, numbers, booleans, null, arrays, objects.
// Not a full validator; sufficient for Hermes tool payloads (flat-ish dicts).
struct JsonParser<'a> { input: &'a [u8], pos: usize }

impl<'a> JsonParser<'a> {
    fn new(s: &'a str) -> Self { Self { input: s.as_bytes(), pos: 0 } }
    fn peek(&self) -> Option<u8> { self.input.get(self.pos).copied() }
    fn advance(&mut self) { self.pos += 1; }
    fn skip_ws(&mut self) { while matches!(self.peek(), Some(b' ') | Some(b'\n') | Some(b'\r') | Some(b'\t')) { self.advance(); } }
    fn parse_value(&mut self) -> Option<JsonVal> {
        self.skip_ws();
        match self.peek()? {
            b'n' => self.parse_null(),
            b't' | b'f' => self.parse_bool(),
            b'"' => self.parse_string().map(JsonVal::Str),
            b'[' => self.parse_array(),
            b'{' => self.parse_object(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => None,
        }
    }
    fn parse_null(&mut self) -> Option<JsonVal> {
        if self.input.get(self.pos..self.pos+4) == Some(b"null") { self.pos+=4; Some(JsonVal::Null) } else { None }
    }
    fn parse_bool(&mut self) -> Option<JsonVal> {
        if self.input.get(self.pos..self.pos+4)==Some(b"true") { self.pos+=4; Some(JsonVal::Bool(true)) }
        else if self.input.get(self.pos..self.pos+5)==Some(b"false") { self.pos+=5; Some(JsonVal::Bool(false)) } else { None }
    }
    fn parse_string(&mut self) -> Option<String> {
        // Assumes opening quote already peeked.
        self.advance(); // skip "
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c == b'"' { self.advance(); return Some(out); }
            if c == b'\\' {
                self.advance();
                match self.peek()? {
                    b'"' => { out.push('"'); self.advance(); }
                    b'\\' => { out.push('\\'); self.advance(); }
                    b'/' => { out.push('/'); self.advance(); }
                    b'n' => { out.push('\n'); self.advance(); }
                    b'r' => { out.push('\r'); self.advance(); }
                    b't' => { out.push('\t'); self.advance(); }
                    b'u' => {
                        // \uXXXX
                        self.advance();
                        let hex: String = (0..4).map(|_| { let ch=self.peek().unwrap_or(b'0') as char; self.advance(); ch }).collect();
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) { out.push(ch); }
                        }
                    }
                    other => { out.push(other as char); self.advance(); }
                }
            } else {
                out.push(c as char); self.advance();
            }
        }
        None
    }
    fn parse_number(&mut self) -> Option<JsonVal> {
        let start = self.pos;
        if self.peek()==Some(b'-') { self.advance(); }
        while matches!(self.peek(), Some(b'0'..=b'9')) { self.advance(); }
        if self.peek()==Some(b'.') { self.advance(); while matches!(self.peek(), Some(b'0'..=b'9')) { self.advance(); } }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) { self.advance(); if matches!(self.peek(), Some(b'+')|Some(b'-')) { self.advance(); } while matches!(self.peek(), Some(b'0'..=b'9')) { self.advance(); } }
        let s = std::str::from_utf8(&self.input[start..self.pos]).ok()?;
        s.parse::<f64>().ok().map(JsonVal::Number)
    }
    fn parse_array(&mut self) -> Option<JsonVal> {
        self.advance(); // [
        let mut arr = Vec::new();
        loop {
            self.skip_ws();
            if self.peek()==Some(b']') { self.advance(); break; }
            if let Some(v) = self.parse_value() { arr.push(v); } else { return None; }
            self.skip_ws();
            if self.peek()==Some(b',') { self.advance(); } else if self.peek()==Some(b']') { continue; } else { return None; }
        }
        Some(JsonVal::Array(arr))
    }
    fn parse_object(&mut self) -> Option<JsonVal> {
        self.advance(); // {
        let mut map = HashMap::new();
        loop {
            self.skip_ws();
            if self.peek()==Some(b'}') { self.advance(); break; }
            // key must be string
            if self.peek()!=Some(b'"') { return None; }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek()!=Some(b':') { return None; }
            self.advance();
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            if self.peek()==Some(b',') { self.advance(); } else if self.peek()==Some(b'}') { continue; } else { return None; }
        }
        Some(JsonVal::Object(map))
    }
}

fn json_parse(s: &str) -> Option<JsonVal> {
    let mut p = JsonParser::new(s);
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.input.len() { return None; }
    Some(v)
}

/// Mirrors `def _json_loads_maybe(value: Optional[str]) -> Any` (196-211).
/// Tries `json.loads(value)`, then `JSONDecoder().raw_decode(value.lstrip())`
/// to tolerate the `"\n\n[Hint: ...]"` suffix some Hermes tools append.
/// Returns `None` when the value is not a JSON string or parsing fails.
pub fn json_loads_maybe(value: Option<&str>) -> Option<JsonVal> {
    let s = value?;
    // Python: `if not isinstance(value, str): return value` — in Rust the
    // non-string case is modelled as `None` input, so we return None.
    let s = s.trim();
    if s.is_empty() { return None; }
    // Try full parse first.
    if let Some(v) = json_parse(s) { return Some(v); }
    // Second attempt: `raw_decode(value.lstrip())` — decode first JSON value only.
    // We emulate by trimming leading whitespace and finding the first balanced
    // `{...}` or `[...]` prefix.
    let lstripped = value.unwrap_or("").trim_start();
    if lstripped.is_empty() { return None; }
    // Find first `{` or `[`
    let start = lstripped.find(|c| c=='{' || c=='[')?;
    let slice = &lstripped[start..];
    // Try to parse progressively shorter prefixes until one succeeds (mirrors raw_decode).
    // For speed, just try full slice then trim trailing non-JSON.
    if let Some(v) = json_parse(slice) { return Some(v); }
    // Attempt: find matching closing brace/bracket via depth counting.
    let open = slice.chars().next()?;
    let close = if open=='{' { '}' } else { ']' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (idx, ch) in slice.char_indices() {
        if esc { esc = false; continue; }
        if ch=='\\' && in_str { esc = true; continue; }
        if ch=='"' { in_str = !in_str; continue; }
        if in_str { continue; }
        if ch==open { depth+=1; }
        else if ch==close { depth-=1; if depth==0 { let cand=&slice[..idx+ch.len_utf8()]; if let Some(v)=json_parse(cand) { return Some(v); } break; } }
    }
    None
}

#[allow(dead_code)]
pub fn _json_loads_maybe(value: Option<&str>) -> Option<JsonVal> { json_loads_maybe(value) }

// ---------------------------------------------------------------------------
// _tool_result_failed — lines 214-249
// ---------------------------------------------------------------------------

/// Mirrors `def _tool_result_failed(result: Optional[str], tool_name: str | None = None) -> bool` (214-249).
/// Conservative: only structured failures (`success==False`, `exit_code!=0`,
/// canonical `"Error executing tool '"` prefix, polished-tool `{"error":...}`) count as failed.
pub fn tool_result_failed(result: Option<&str>, tool_name: Option<&str>) -> bool {
    // Canonical wrapper prefix (221-228)
    if let Some(r) = result {
        if r.starts_with("Error executing tool '") { return true; }
    }
    let data = match json_loads_maybe(result) {
        Some(v) => v,
        None => return false,
    };
    let obj = match data.as_object() {
        Some(m) => m,
        None => return false,
    };
    // `success` / `ok` == False (234-236)
    for key in ["success", "ok"] {
        if let Some(JsonVal::Bool(false)) = obj.get(key) { return true; }
    }
    // exit_code / returncode != 0 (238-240)
    for key in ["exit_code", "returncode"] {
        if let Some(v) = obj.get(key) {
            if let Some(n) = v.as_f64() {
                if (n as i64) != 0 { return true; }
            }
            // Also handle Int-like via string? Python checks `isinstance(exit_code, int)`
            if let Some(n) = v.as_i64() { if n != 0 { return true; } }
        }
    }
    // Polished error payload (246-247)
    if let Some(tn) = tool_name {
        if is_polished(tn) {
            let has_error = obj.get("error").map(|v| match v {
                JsonVal::Str(s) => !s.trim().is_empty(),
                JsonVal::Bool(b) => *b,
                JsonVal::Null => false,
                _ => true,
            }).unwrap_or(false);
            let has_content = obj.get("content").map(|v| match v {
                JsonVal::Str(s) => !s.trim().is_empty(),
                JsonVal::Null => false,
                _ => true,
            }).unwrap_or(false);
            if has_error && !has_content { return true; }
        }
    }
    false
}

#[allow(dead_code)]
pub fn _tool_result_failed(result: Option<&str>, tool_name: Option<&str>) -> bool {
    tool_result_failed(result, tool_name)
}

// ---------------------------------------------------------------------------
// _truncate_text — lines 252-255
// ---------------------------------------------------------------------------

/// Mirrors `def _truncate_text(text: str, limit: int = 5000) -> str` (252-255).
pub fn truncate_text(text: &str, limit: usize) -> String {
    if text.len() <= limit { return text.to_string(); }
    let trunc_at = limit.saturating_sub(100);
    let end = text.floor_char_boundary(trunc_at);
    format!("{}\n... ({} chars total, truncated)", &text[..end], text.len())
}
#[allow(dead_code)]
pub fn _truncate_text(text: &str, limit: usize) -> String { truncate_text(text, limit) }

// ---------------------------------------------------------------------------
// _fenced_text — lines 258-262
// ---------------------------------------------------------------------------

/// Mirrors `def _fenced_text(text: str, language: str = "") -> str` (258-262).
/// Chooses a fence longer than any backtick run inside `text`.
pub fn fenced_text(text: &str, language: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for ch in text.chars() {
        if ch == '`' { run += 1; longest = longest.max(run); } else { run = 0; }
    }
    let fence_len = (longest + 1).max(3);
    let fence = "`".repeat(fence_len);
    format!("{fence}{language}\n{text}\n{fence}")
}
#[allow(dead_code)]
pub fn _fenced_text(text: &str, language: &str) -> String { fenced_text(text, language) }

// ---------------------------------------------------------------------------
// _extract_markdown_headings — lines 416-426
// ---------------------------------------------------------------------------

/// Mirrors `def _extract_markdown_headings(content: str, limit: int = 8) -> list[str]` (416-426).
pub fn extract_markdown_headings(content: &str, limit: usize) -> Vec<String> {
    let mut headings = Vec::new();
    for line in content.lines() {
        let stripped = line.trim();
        if stripped.starts_with('#') {
            let heading = stripped.trim_start_matches('#').trim().to_string();
            if !heading.is_empty() { headings.push(heading); }
        }
        if headings.len() >= limit { break; }
    }
    headings
}
#[allow(dead_code)]
pub fn _extract_markdown_headings(content: &str, limit: usize) -> Vec<String> {
    extract_markdown_headings(content, limit)
}

// ---------------------------------------------------------------------------
// _format_todo_result — lines 265-294
// ---------------------------------------------------------------------------

/// Mirrors `def _format_todo_result(result: Optional[str]) -> Optional[str]` (265-294).
pub fn format_todo_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    let todos = obj.get("todos")?.as_array()?;
    let summary = obj.get("summary").and_then(|v| v.as_object());
    let lines = {
        let mut l = vec!["**Todo list**".to_string(), String::new()];
        for item in todos {
            let m = match item.as_object() { Some(m) => m, None => continue };
            let status = m.get("status").and_then(|v| v.as_str()).unwrap_or("pending").to_string();
            let content = m.get("content").and_then(|v| v.as_str())
                .or_else(|| m.get("id").and_then(|v| v.as_str()))
                .unwrap_or("").trim().to_string();
            if content.is_empty() { continue; }
            let icon = match status.as_str() {
                "completed" => "✅",
                "in_progress" => "🔄",
                "pending" => "⏳",
                "cancelled" => "✗",
                _ => "•",
            };
            l.push(format!("- {icon} {content}"));
        }
        if let Some(s) = summary {
            let cancelled = s.get("cancelled").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            let completed = s.get("completed").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            let in_progress = s.get("in_progress").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            let pending = s.get("pending").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            l.push(String::new());
            let mut prog = format!("**Progress:** {completed} completed, {in_progress} in progress, {pending} pending");
            if cancelled != 0 { prog.push_str(&format!(", {cancelled} cancelled")); }
            l.push(prog);
        }
        l
    };
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_read_file_result — lines 297-321
// ---------------------------------------------------------------------------

/// Mirrors `def _format_read_file_result(result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (297-321).
pub fn format_read_file_result(result: Option<&str>, args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
        && obj.get("content").and_then(|v| v.as_str()).is_none() {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Some(format!("Read failed: {err}"));
    }
    let content = obj.get("content")?.as_str()?.to_string();
    let path = args.and_then(|a| arg_str(a, "path"))
        .or_else(|| obj.get("path").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .or_else(|| obj.get("path").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "file".to_string());
    let path = path.trim().to_string();
    let offset = args.and_then(|a| a.get("offset")).and_then(|v| match v {
        ArgVal::Int(n) => Some(*n),
        ArgVal::Str(s) => s.parse::<i64>().ok(),
        _ => None,
    });
    let limit_val = args.and_then(|a| a.get("limit")).and_then(|v| match v {
        ArgVal::Int(n) => Some(*n),
        ArgVal::Str(s) => s.parse::<i64>().ok(),
        _ => None,
    });
    let mut range_bits = Vec::new();
    if let Some(off) = offset { if off != 0 { range_bits.push(format!("from line {off}")); } }
    if let Some(lim) = limit_val { range_bits.push(format!("limit {lim}")); }
    let suffix = if !range_bits.is_empty() { format!(" ({})", range_bits.join(", ")) } else { String::new() };
    let mut header = format!("Read {path}{suffix}");
    if let Some(total) = obj.get("total_lines").and_then(|v| v.as_f64()) {
        header.push_str(&format!(" — {} total lines", total as i64));
    }
    Some(truncate_text(&format!("{header}\n\n{}", fenced_text(&content, "")), 5000))
}

// ---------------------------------------------------------------------------
// _format_search_files_result — lines 324-380
// ---------------------------------------------------------------------------

/// Mirrors `def _format_search_files_result(result: Optional[str]) -> Optional[str]` (324-380).
pub fn format_search_files_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    // Branch 1: `files` list
    if let Some(files_val) = obj.get("files") {
        if let Some(files) = files_val.as_array() {
            let total = obj.get("total_count").and_then(|v| v.as_f64()).map(|n| n as i64).unwrap_or(files.len() as i64);
            let shown = (files.len() as i64).min(20) as usize;
            let truncated = obj.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false) || (files.len() as i64) > shown as i64;
            let mut lines = vec!["File search results".to_string(), format!("Found {total} file{}; showing {shown}.", if total==1 {""} else {"s"}), String::new()];
            for path in files.iter().take(shown) {
                if let Some(s) = path.as_str() { lines.push(format!("- {s}")); }
                else { lines.push(format!("- {path:?}")); }
            }
            if truncated {
                lines.push(String::new());
                lines.push("Results truncated. Narrow the search, add path/file_glob, or use offset to page.".to_string());
            }
            return Some(truncate_text(&lines.join("\n"), 7000));
        }
    }
    // Branch 2: `matches` list
    let matches = obj.get("matches")?.as_array()?;
    let total = obj.get("total_count").and_then(|v| v.as_f64()).map(|n| n as i64).unwrap_or(matches.len() as i64);
    let shown = (matches.len() as i64).min(12) as usize;
    let truncated = obj.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false) || (matches.len() as i64) > shown as i64;
    let mut lines = vec!["Search results".to_string(), format!("Found {total} match{}; showing {shown}.", if total==1 {""} else {"es"}), String::new()];
    for m in matches.iter().take(shown) {
        if let Some(map) = m.as_object() {
            let path = map.get("path").and_then(|v| v.as_str())
                .or_else(|| map.get("file").and_then(|v| v.as_str()))
                .or_else(|| map.get("filename").and_then(|v| v.as_str()))
                .unwrap_or("?");
            let line = map.get("line").or_else(|| map.get("line_number")).and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_f64().map(|n| (n as i64).to_string())));
            let content = map.get("content").or_else(|| map.get("text")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let loc = if let Some(ln) = line { format!("{path}:{ln}") } else { path.to_string() };
            lines.push(format!("- {loc}"));
            if !content.is_empty() {
                let snippet = truncate_text(&content.split_whitespace().collect::<Vec<_>>().join(" "), 300);
                lines.push(format!("  {snippet}"));
            }
        } else if let Some(s) = m.as_str() {
            lines.push(format!("- {s}"));
        } else {
            lines.push(format!("- {m:?}"));
        }
    }
    if truncated {
        lines.push(String::new());
        lines.push("Results truncated. Narrow the search, add file_glob, or use offset to page.".to_string());
    }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_execute_code_result — lines 383-413
// ---------------------------------------------------------------------------

/// Mirrors `def _format_execute_code_result(result: Optional[str]) -> Optional[str]` (383-413).
pub fn format_execute_code_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result);
    let obj = match data.as_ref().and_then(|v| v.as_object()) {
        Some(o) => o,
        None => {
            // `if not isinstance(data, dict): return result if isinstance(result, str) and result.strip() else None`
            return result.and_then(|s| if s.trim().is_empty() { None } else { Some(s.to_string()) });
        }
    };
    let output = obj.get("output").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let error = obj.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let exit_code = obj.get("exit_code").and_then(|v| v.as_f64()).map(|n| n as i64);
    let mut parts: Vec<String> = Vec::new();
    if let Some(code) = exit_code { parts.push(format!("Exit code: {code}")); } else { parts.push("Execution complete".to_string()); }
    if obj.get("stdout_truncated").and_then(|v| v.as_bool()).unwrap_or(false) {
        let total = obj.get("stdout_bytes_total").and_then(|v| v.as_f64()).map(|n| n as i64);
        let captured = obj.get("stdout_bytes_captured").and_then(|v| v.as_f64()).map(|n| n as i64);
        let omitted = obj.get("stdout_bytes_omitted").and_then(|v| v.as_f64()).map(|n| n as i64);
        if let (Some(c), Some(t), Some(o)) = (captured, total, omitted) {
            parts.push(String::new());
            parts.push(format!("Output truncated: captured {c:,} of {t:,} bytes ({o:,} omitted)."));
        } else {
            parts.push(String::new());
            parts.push("Output truncated.".to_string());
        }
    }
    let warning = obj.get("warning").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !warning.is_empty() { parts.push(String::new()); parts.push("Warning:".to_string()); parts.push(warning); }
    if !output.is_empty() { parts.push(String::new()); parts.push("Output:".to_string()); parts.push(output); }
    if !error.is_empty() { parts.push(String::new()); parts.push("Error:".to_string()); parts.push(error); }
    Some(truncate_text(&parts.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_skill_view_result — lines 429-459
// ---------------------------------------------------------------------------

/// Mirrors `def _format_skill_view_result(result: Optional[str]) -> Optional[str]` (429-459).
pub fn format_skill_view_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Some(format!("Skill view failed: {err}"));
    }
    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("skill").to_string();
    let file_path = obj.get("file").or_else(|| obj.get("path")).and_then(|v| v.as_str()).unwrap_or("SKILL.md").to_string();
    let description = obj.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let linked = obj.get("linked_files").and_then(|v| v.as_object());
    let mut lines = vec!["**Skill loaded**".to_string(), String::new(),
        format!("- **Name:** `{name}`"), format!("- **File:** `{file_path}`")];
    if !description.is_empty() { lines.push(format!("- **Description:** {description}")); }
    if !content.is_empty() { lines.push(format!("- **Content:** {} chars loaded into agent context", content.len())); }
    if let Some(linked) = linked {
        let linked_count: usize = linked.values().filter_map(|v| v.as_array()).map(|a| a.len()).sum();
        lines.push(format!("- **Linked files:** {linked_count}"));
    }
    let headings = extract_markdown_headings(&content, 8);
    if !headings.is_empty() {
        lines.push(String::new());
        lines.push("**Sections**".to_string());
        for h in headings { lines.push(format!("- {h}")); }
    }
    lines.push(String::new());
    lines.push("_Full skill content is available to the agent but hidden here to keep ACP readable._".to_string());
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_skill_manage_result — lines 462-489
// ---------------------------------------------------------------------------

/// Mirrors `def _format_skill_manage_result(result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (462-489).
pub fn format_skill_manage_result(result: Option<&str>, args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    let action = args.and_then(|a| a.get("action")).and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "manage".to_string()).trim().to_string();
    let action = if action.is_empty() { "manage".to_string() } else { action };
    let name = args.and_then(|a| a.get("name")).and_then(|v| v.as_str().map(|s| s.to_string()))
        .or_else(|| obj.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "skill".to_string()).trim().to_string();
    let name = if name.is_empty() { "skill".to_string() } else { name };
    let file_path = args.and_then(|a| a.get("file_path")).and_then(|v| v.as_str().map(|s| s.to_string()))
        .or_else(|| obj.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "SKILL.md".to_string()).trim().to_string();
    let file_path = if file_path.is_empty() { "SKILL.md".to_string() } else { file_path };
    let success = obj.get("success").and_then(|v| v.as_bool());
    let status = if success == Some(false) { "✗ Skill update failed" } else { "✅ Skill updated" };
    let mut lines = vec![format!("**{status}**"), String::new(),
        format!("- **Action:** `{action}`"), format!("- **Skill:** `{name}`")];
    if action != "delete" { lines.push(format!("- **File:** `{file_path}`")); }
    let message = obj.get("message").or_else(|| obj.get("error")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !message.is_empty() { lines.push(format!("- **Result:** {message}")); }
    if let Some(rep) = obj.get("replacements").or_else(|| obj.get("replacement_count")) {
        let rep_str = match rep { JsonVal::Number(n) => (*n as i64).to_string(), JsonVal::Str(s) => s.clone(), _ => format!("{rep:?}") };
        lines.push(format!("- **Replacements:** {rep_str}"));
    }
    let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !path.is_empty() { lines.push(format!("- **Path:** `{path}`")); }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_web_search_result — lines 492-509
// ---------------------------------------------------------------------------

/// Mirrors `def _format_web_search_result(result: Optional[str]) -> Optional[str]` (492-509).
pub fn format_web_search_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    // `web = data.get("data", {}).get("web") if isinstance(data.get("data"), dict) else data.get("web")`
    let web_val = if let Some(d) = obj.get("data").and_then(|v| v.as_object()) {
        d.get("web")
    } else {
        obj.get("web")
    };
    let web = web_val?.as_array()?;
    let mut lines = vec![format!("Web results: {}", web.len())];
    for item in web.iter().take(10) {
        let m = match item.as_object() { Some(m) => m, None => continue };
        let title = m.get("title").or_else(|| m.get("url")).and_then(|v| v.as_str()).unwrap_or("result").trim().to_string();
        let url = m.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let desc = m.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let mut line = format!("• {title}");
        if !url.is_empty() { line.push_str(&format!(" — {url}")); }
        lines.push(line);
        if !desc.is_empty() { lines.push(format!("  {desc}")); }
    }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_web_extract_result — lines 512-540
// ---------------------------------------------------------------------------

/// Mirrors `def _format_web_extract_result(result: Optional[str]) -> Optional[str]` (512-540).
/// Only surfaces failures; success stays compact via title.
pub fn format_web_extract_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(err) = obj.get("error").and_then(|v| v.as_str()) {
            if !err.is_empty() { return Some(format!("Web extract failed: {err}")); }
        }
    }
    let results = obj.get("results")?.as_array()?;
    let mut failures: Vec<String> = Vec::new();
    for item in results.iter().take(10) {
        let m = match item.as_object() { Some(m) => m, None => continue };
        let error = m.get("error").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if error.is_empty() || error=="None" || error=="null" { continue; }
        let url = m.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let title = m.get("title").and_then(|v| v.as_str()).unwrap_or_else(|| if url.is_empty() { "Untitled" } else { url.as_str() }).trim().to_string();
        let title = if title.is_empty() { "Untitled".to_string() } else { title };
        let mut entry = format!("- {title}");
        if !url.is_empty() && url != title { entry.push_str(&format!(" — {url}")); }
        entry.push_str(&format!("\n  Error: {}", truncate_text(&error, 500)));
        failures.push(entry);
    }
    if failures.is_empty() { return None; }
    let mut lines = vec![format!("Web extract failed for {} URL{}", failures.len(), if failures.len()==1 {""} else {"s"})];
    lines.extend(failures);
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_process_result — lines 543-587
// ---------------------------------------------------------------------------

/// Mirrors `def _format_process_result(result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (543-587).
pub fn format_process_result(result: Option<&str>, args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result);
    let obj = match data.as_ref().and_then(|v| v.as_object()) {
        Some(o) => o,
        None => {
            return result.and_then(|s| if s.trim().is_empty() { None } else { Some(s.to_string()) });
        }
    };
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(err) = obj.get("error").and_then(|v| v.as_str()) {
            if !err.is_empty() { return Some(format!("Process error: {err}")); }
        }
    }
    // `if isinstance(data.get("processes"), list):`
    if let Some(procs) = obj.get("processes").and_then(|v| v.as_array()) {
        let mut lines = vec![format!("Processes: {}", procs.len())];
        for proc in procs.iter().take(20) {
            if let Some(m) = proc.as_object() {
                let sid = m.get("session_id").or_else(|| m.get("id")).and_then(|v| v.as_str()).unwrap_or("?").to_string();
                let status = m.get("status").and_then(|v| v.as_str())
                    .or_else(|| if m.get("exited").and_then(|v| v.as_bool()) == Some(true) { Some("exited") } else { Some("running") })
                    .unwrap_or("running").to_string();
                let cmd = m.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let pid = m.get("pid").and_then(|v| v.as_f64()).map(|n| n as i64);
                let code = m.get("exit_code").and_then(|v| v.as_f64()).map(|n| n as i64);
                let mut bits = vec![status];
                if let Some(p) = pid { bits.push(format!("pid {p}")); }
                if let Some(c) = code { bits.push(format!("exit {c}")); }
                let mut line = format!("- `{sid}` — {}", bits.join(", "));
                if !cmd.is_empty() { line.push_str(&format!(" — {}", &cmd[..cmd.len().min(120)])); }
                lines.push(line);
            } else if let Some(s) = proc.as_str() {
                lines.push(format!("- {s}"));
            }
        }
        if procs.len() > 20 { lines.push(format!("... {} more process(es)", procs.len()-20)); }
        return Some(lines.join("\n"));
    }
    let action = args.and_then(|a| arg_str(a, "action")).unwrap_or_else(|| "process".to_string()).trim().to_string();
    let action = if action.is_empty() { "process".to_string() } else { action };
    let status = obj.get("status").or_else(|| obj.get("state")).and_then(|v| v.as_str()).unwrap_or(action.as_str()).trim().to_string();
    let sid = obj.get("session_id").and_then(|v| v.as_str())
        .or_else(|| args.and_then(|a| a.get("session_id")).and_then(|v| v.as_str().map(|s| s.to_string())).as_deref().map(|_| "")) // placeholder
        .unwrap_or_else(|| args.and_then(|a| arg_str(a, "session_id")).unwrap_or_default().as_str().to_string().leak() as &str)
        .to_string();
    // Simpler sid extraction: try data.session_id then args.session_id
    let sid2 = obj.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| args.and_then(|a| arg_str(a, "session_id"))).unwrap_or_default().trim().to_string();
    let sid_disp = sid2;
    let _ = sid; // keep for parity
    let mut lines = vec![format!("Process {action}: {status}") + if !sid_disp.is_empty() { &format!(" (`{sid_disp}`)") } else { "" }];
    for (key, label) in [("command","Command"),("pid","PID"),("exit_code","Exit code"),("returncode","Exit code"),("lines","Lines")] {
        if let Some(v) = obj.get(key) {
            let val_str = match v { JsonVal::Str(s) => s.clone(), JsonVal::Number(n) => (*n as i64).to_string(), JsonVal::Bool(b) => b.to_string(), _ => format!("{v:?}") };
            lines.push(format!("- **{label}:** {val_str}"));
        }
    }
    let output = obj.get("output").or_else(|| obj.get("new_output")).or_else(|| obj.get("log")).or_else(|| obj.get("stdout")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let error = obj.get("error").or_else(|| obj.get("stderr")).and_then(|v| v.as_str()).map(|s| s.to_string());
    if let Some(out) = output { if !out.is_empty() { lines.push(String::new()); lines.push("Output:".to_string()); lines.push(truncate_text(&out, 5000)); } }
    if let Some(err) = error { if !err.is_empty() { lines.push(String::new()); lines.push("Error:".to_string()); lines.push(truncate_text(&err, 2000)); } }
    if let Some(msg) = obj.get("message").and_then(|v| v.as_str()) {
        let has_output = obj.get("output").or_else(|| obj.get("new_output")).or_else(|| obj.get("log")).or_else(|| obj.get("stdout")).is_some();
        let has_error = obj.get("error").or_else(|| obj.get("stderr")).is_some();
        if !has_output && !has_error && !msg.is_empty() { lines.push(msg.to_string()); }
    }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_delegate_result — lines 590-633
// ---------------------------------------------------------------------------

/// Mirrors `def _format_delegate_result(result: Optional[str]) -> Optional[str]` (590-633).
pub fn format_delegate_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
        && obj.get("results").and_then(|v| v.as_array()).is_none() {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Some(format!("Delegation failed: {err}"));
    }
    let results = obj.get("results")?.as_array()?;
    let total = obj.get("total_duration_seconds").and_then(|v| v.as_f64());
    let mut lines = vec![format!("Delegation results: {} task{}", results.len(), if results.len()==1 {""} else {"s"}) + if let Some(t)=total { &format!(" in {t}s") } else { "" }];
    for item in results {
        let m = match item.as_object() { Some(m) => m, None => { lines.push(format!("- {item:?}")); continue; } };
        let idx = m.get("task_index").and_then(|v| v.as_f64()).map(|n| n as i64);
        let status = m.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let model = m.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
        let dur = m.get("duration_seconds").and_then(|v| v.as_f64());
        let role = m.get("_child_role").and_then(|v| v.as_str()).map(|s| s.to_string());
        let icon = match status.as_str() { "completed" => "✅", "failed" => "✗", "error" => "✗", "timeout" => "⏱", "interrupted" => "⚠", _ => "•" };
        let mut header = format!("{icon} Task {}: {status}", idx.map(|i| (i+1).to_string()).unwrap_or_else(|| "?".to_string()));
        let mut bits: Vec<String> = Vec::new();
        if let Some(mo)=model { bits.push(mo); }
        if let Some(r)=role { bits.push(format!("role={r}")); }
        if let Some(d)=dur { bits.push(format!("{d}s")); }
        if !bits.is_empty() { header.push_str(&format!(" ({})", bits.join(", "))); }
        lines.push(String::new());
        lines.push(header);
        let summary = m.get("summary").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let error = m.get("error").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if !summary.is_empty() { lines.push(truncate_text(&summary, 1200)); }
        if !error.is_empty() { lines.push(format!("Error: {}", truncate_text(&error, 800))); }
        if let Some(trace) = m.get("tool_trace").and_then(|v| v.as_array()) {
            if !trace.is_empty() {
                let names: Vec<String> = trace.iter().filter_map(|t| t.as_object()?.get("tool")?.as_str().map(|s| s.to_string())).collect();
                if !names.is_empty() {
                    let list = names.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
                    let suffix = if names.len()>12 { format!(" (+{})", names.len()-12) } else { String::new() };
                    lines.push(format!("Tools: {list}{suffix}"));
                }
            }
        }
    }
    Some(truncate_text(&lines.join("\n"), 8000))
}

// ---------------------------------------------------------------------------
// _format_session_search_result — lines 636-664
// ---------------------------------------------------------------------------

/// Mirrors `def _format_session_search_result(result: Optional[str]) -> Optional[str]` (636-664).
pub fn format_session_search_result(result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Some(format!("Session search failed: {err}"));
    }
    let results = obj.get("results")?.as_array()?;
    let mode = obj.get("mode").and_then(|v| v.as_str()).unwrap_or("search").to_string();
    let query = obj.get("query").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mut lines = vec![ if mode=="recent" { "Recent sessions".to_string() } else { format!("Session search results{}", if let Some(q)=query.as_deref() { format!(" for `{q}`") } else { String::new() }) } ];
    if results.is_empty() {
        let msg = obj.get("message").and_then(|v| v.as_str()).unwrap_or("No matching sessions found.");
        lines.push(msg.to_string());
        return Some(lines.join("\n"));
    }
    for item in results {
        let m = match item.as_object() { Some(m) => m, None => continue };
        let sid = m.get("session_id").and_then(|v| v.as_str()).unwrap_or("?").to_string();
        let title = m.get("title").or_else(|| m.get("when")).and_then(|v| v.as_str()).unwrap_or("Untitled session").trim().to_string();
        let when = m.get("last_active").or_else(|| m.get("started_at")).or_else(|| m.get("when")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let count = m.get("message_count").and_then(|v| v.as_f64()).map(|n| n as i64);
        let source = m.get("source").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let mut meta_parts: Vec<String> = Vec::new();
        if !when.is_empty() { meta_parts.push(when.clone()); }
        if !source.is_empty() { meta_parts.push(source.clone()); }
        if let Some(c)=count { meta_parts.push(format!("{c} msgs")); }
        let meta = meta_parts.join(", ");
        lines.push(format!("- **{title}** (`{sid}`)") + if !meta.is_empty() { &format!(" — {meta}") } else { "" });
        let summary = m.get("summary").or_else(|| m.get("preview")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if !summary.is_empty() {
            lines.push(format!("  {}", truncate_text(&summary.split_whitespace().collect::<Vec<_>>().join(" "), 500)));
        }
    }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_memory_result — lines 667-691
// ---------------------------------------------------------------------------

/// Mirrors `def _format_memory_result(result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (667-691).
pub fn format_memory_result(result: Option<&str>, args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    let action = args.and_then(|a| arg_str(a, "action")).unwrap_or_else(|| "memory".to_string()).trim().to_string();
    let action = if action.is_empty() { "memory".to_string() } else { action };
    let target = obj.get("target").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| args.and_then(|a| arg_str(a, "target"))).unwrap_or_else(|| "memory".to_string());
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let mut lines = vec![format!("✗ Memory {action} failed ({target})"), obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error").to_string()];
        if let Some(matches) = obj.get("matches").and_then(|v| v.as_array()) {
            if !matches.is_empty() {
                lines.push("Matches:".to_string());
                for m in matches.iter().take(5) {
                    let s = match m { JsonVal::Str(st)=>st.clone(), _=>format!("{m:?}") };
                    lines.push(format!("- {}", truncate_text(&s, 160)));
                }
            }
        }
        return Some(lines.join("\n"));
    }
    let mut lines = vec![format!("✅ Memory {action} saved ({target})")];
    if let Some(msg) = obj.get("message").and_then(|v| v.as_str()) { if !msg.is_empty() { lines.push(msg.to_string()); } }
    if let Some(cnt) = obj.get("entry_count").and_then(|v| v.as_f64()) { lines.push(format!("Entries: {}", cnt as i64)); }
    if let Some(u) = obj.get("usage").and_then(|v| v.as_str()) { if !u.is_empty() { lines.push(format!("Usage: {u}")); } }
    let preview = args.and_then(|a| a.get("content").or_else(|| a.get("old_text")).and_then(|v| v.as_str().map(|s| s.to_string()))).unwrap_or_default().trim().to_string();
    if !preview.is_empty() { lines.push(format!("Preview: {}", truncate_text(&preview, 300))); }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_edit_result — lines 694-714
// ---------------------------------------------------------------------------

/// Mirrors `def _format_edit_result(tool_name: str, result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (694-714).
pub fn format_edit_result(tool_name: &str, result: Option<&str>, args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result);
    let path = args.and_then(|a| arg_str(a, "path")).unwrap_or_else(|| "file".to_string()).trim().to_string();
    if let Some(ref val) = data {
        if let Some(obj) = val.as_object() {
            if obj.get("success").and_then(|v| v.as_bool()) == Some(false) || obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
                let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                return Some(format!("{tool_name} failed for {path}: {err}"));
            }
            let message = obj.get("message").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let replacements = obj.get("replacements").or_else(|| obj.get("replacement_count")).map(|v| match v { JsonVal::Number(n) => (*n as i64).to_string(), JsonVal::Str(s)=>s.clone(), _=>format!("{v:?}") });
            let mut lines = vec![format!("✅ {tool_name} completed") + if !path.is_empty() { &format!(" for `{path}`") } else { "" }];
            if !message.is_empty() { lines.push(message); }
            if let Some(rep)=replacements { lines.push(format!("Replacements: {rep}")); }
            if let Some(files) = obj.get("files_modified").and_then(|v| v.as_array()) {
                let list: Vec<String> = files.iter().take(8).filter_map(|v| v.as_str().map(|s| format!("`{s}`"))).collect();
                if !list.is_empty() { lines.push(format!("Files: {}", list.join(", "))); }
            }
            return Some(lines.join("\n"));
        }
    }
    if let Some(r) = result { if !r.trim().is_empty() { return Some(truncate_text(r, 3000)); } }
    Some(format!("✅ {tool_name} completed") + if !path.is_empty() { &format!(" for `{path}`") } else { "" })
}

// ---------------------------------------------------------------------------
// _format_browser_result — lines 717-740
// ---------------------------------------------------------------------------

/// Mirrors `def _format_browser_result(tool_name: str, result: Optional[str], args: Optional[Dict[str, Any]]) -> Optional[str]` (717-740).
pub fn format_browser_result(tool_name: &str, _args: Option<&Args>) -> impl Fn(Option<&str>) -> Option<String> + '_ {
    move |result: Option<&str>| {
        let data = json_loads_maybe(result)?;
        let obj = data.as_object()?;
        if obj.get("success").and_then(|v| v.as_bool()) == Some(false) || obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
            let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
            return Some(format!("{tool_name} failed: {err}"));
        }
        if tool_name=="browser_get_images" {
            let images = obj.get("images").or_else(|| obj.get("data")).and_then(|v| v.as_array());
            if let Some(imgs)=images {
                let mut lines = vec![format!("Images found: {}", imgs.len())];
                for img in imgs.iter().take(12) {
                    if let Some(m)=img.as_object() {
                        let alt = m.get("alt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        let url = m.get("url").or_else(|| m.get("src")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        let label = if !alt.is_empty() { alt } else { "image".to_string() };
                        lines.push(format!("- {label}") + if !url.is_empty() { &format!(" — {url}") } else { "" });
                    }
                }
                return Some(truncate_text(&lines.join("\n"), 5000));
            }
        }
        let title = obj.get("title").or_else(|| obj.get("url")).or_else(|| obj.get("status")).and_then(|v| v.as_str()).unwrap_or(tool_name).to_string();
        let text = obj.get("text").or_else(|| obj.get("content")).or_else(|| obj.get("snapshot")).or_else(|| obj.get("analysis")).or_else(|| obj.get("message")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let mut lines = vec![title.clone()];
        if let Some(url)=obj.get("url").and_then(|v| v.as_str()) { if url!=title { lines.push(url.to_string()); } }
        if !text.is_empty() { lines.push(String::new()); lines.push(truncate_text(&text, 5000)); }
        Some(truncate_text(&lines.join("\n"), 7000))
    }
}

// Simpler direct helper matching `format_browser_result(tool, result, args)` shape used elsewhere:

pub fn format_browser_result_direct(tool_name: &str, result: Option<&str>, _args: Option<&Args>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) || obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Some(format!("{tool_name} failed: {err}"));
    }
    if tool_name=="browser_get_images" {
        if let Some(imgs)=obj.get("images").or_else(|| obj.get("data")).and_then(|v| v.as_array()) {
            let mut lines = vec![format!("Images found: {}", imgs.len())];
            for img in imgs.iter().take(12) {
                if let Some(m)=img.as_object() {
                    let alt = m.get("alt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    let url = m.get("url").or_else(|| m.get("src")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    let label = if !alt.is_empty() { alt } else { "image".to_string() };
                    lines.push(format!("- {label}") + if !url.is_empty() { &format!(" — {url}") } else { "" });
                }
            }
            return Some(truncate_text(&lines.join("\n"), 5000));
        }
    }
    let title = obj.get("title").or_else(|| obj.get("url")).or_else(|| obj.get("status")).and_then(|v| v.as_str()).unwrap_or(tool_name).to_string();
    let text = obj.get("text").or_else(|| obj.get("content")).or_else(|| obj.get("snapshot")).or_else(|| obj.get("analysis")).or_else(|| obj.get("message")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut lines = vec![title.clone()];
    if let Some(url)=obj.get("url").and_then(|v| v.as_str()) { if url!=title { lines.push(url.to_string()); } }
    if !text.is_empty() { lines.push(String::new()); lines.push(truncate_text(&text, 5000)); }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _format_media_or_cron_result — lines 743-753
// ---------------------------------------------------------------------------

/// Mirrors `def _format_media_or_cron_result(tool_name: str, result: Optional[str]) -> Optional[str]` (743-753).
pub fn format_media_or_cron_result(tool_name: &str, result: Option<&str>) -> Option<String> {
    let data = json_loads_maybe(result)?;
    let obj = data.as_object()?;
    if obj.get("success").and_then(|v| v.as_bool()) == Some(false) || obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Some(format!("{tool_name} failed: {err}"));
    }
    let mut lines = vec![format!("✅ {tool_name} completed")];
    for key in ["file_path","path","url","image_url","job_id","id","status","message","next_run"] {
        if let Some(v)=obj.get(key).and_then(|x| x.as_str()) { if !v.is_empty() { lines.push(format!("- **{key}:** {v}")); } }
        else if let Some(v)=obj.get(key).and_then(|x| x.as_f64()) { lines.push(format!("- **{key}:** {}", v as i64)); }
    }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// _format_structured_value — lines 756-843
// ---------------------------------------------------------------------------

/// Mirrors `def _format_structured_value(key: str, value: Any, *, indent, max_depth, max_items) -> List[str]` (756-843).
/// Renders nested JSON-ish values as compact Markdown bullets.
pub fn format_structured_value(key: &str, value: &JsonVal, indent: usize, max_depth: usize, max_items: usize) -> Vec<String> {
    let prefix = "  ".repeat(indent);
    let bullet = format!("{prefix}- ");
    let label = if key.is_empty() { String::new() } else { format!("**{key}:**") };
    if matches!(value, JsonVal::Null) { return vec![]; }
    if let JsonVal::Str(s)=value { if s.is_empty() { return vec![]; } }
    if let JsonVal::Array(a)=value { if a.is_empty() { return vec![]; } }
    if let JsonVal::Object(m)=value { if m.is_empty() { return vec![]; } }

    if max_depth==0 {
        let preview = match value {
            JsonVal::Str(s)=> truncate_text(s, 240),
            JsonVal::Number(n)=> truncate_text(&n.to_string(), 240),
            JsonVal::Bool(b)=> truncate_text(&b.to_string(), 240),
            _=> truncate_text(&format!("{value:?}"), 240),
        };
        return vec![ if label.is_empty() { format!("{bullet}{preview}") } else { format!("{bullet}{label} {preview}") } ];
    }
    match value {
        JsonVal::Object(map) => {
            let mut lines = vec![ if label.is_empty() { format!("{bullet}{} fields", map.len()) } else { format!("{bullet}{label}") } ];
            let mut shown=0;
            for (child_key, child_value) in map {
                if matches!(child_value, JsonVal::Null) { continue; }
                if let JsonVal::Str(s)=child_value { if s.is_empty() { continue; } }
                // quick empty check for array/object already done above but re-check
                lines.extend(format_structured_value(child_key, child_value, indent+1, max_depth-1, max_items));
                shown+=1;
                if shown>=max_items {
                    let remaining = map.len().saturating_sub(shown);
                    if remaining>0 { lines.push(format!("{}- ... {remaining} more fields", "  ".repeat(indent+1))); }
                    break;
                }
            }
            lines
        },
        JsonVal::Array(arr) => {
            let mut lines = vec![ if label.is_empty() { format!("{bullet}{} item{}", arr.len(), if arr.len()==1 {""} else {"s"}) } else { format!("{bullet}{label} {} item{}", arr.len(), if arr.len()==1 {""} else {"s"}) } ];
            for (idx, item) in arr.iter().take(max_items).enumerate() {
                if let Some(m)=item.as_object() {
                    let headline = m.get("content").or_else(|| m.get("message")).or_else(|| m.get("title")).or_else(|| m.get("name")).or_else(|| m.get("id")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if !headline.is_empty() {
                        lines.push(format!("{}{}. {}", "  ".repeat(indent+1), idx+1, truncate_text(&headline, 220)));
                        for child_key in ["id","status","type","scope","quality_score","score","path","url"] {
                            if let Some(child_value)=m.get(child_key) {
                                if matches!(child_value, JsonVal::Null) { continue; }
                                if let JsonVal::Str(s)=child_value { if s.is_empty() { continue; } }
                                if let JsonVal::Array(a)=child_value { if a.is_empty() { continue; } }
                                let preview = match child_value { JsonVal::Str(s)=>truncate_text(s,180), JsonVal::Number(n)=>truncate_text(&n.to_string(),180), JsonVal::Bool(b)=>truncate_text(&b.to_string(),180), _=>truncate_text(&format!("{child_value:?}"),180)};
                                lines.push(format!("{}- **{child_key}:** {preview}", "  ".repeat(indent+2)));
                            }
                        }
                    } else {
                        lines.push(format!("{}{}.", "  ".repeat(indent+1), idx+1));
                        let keys: Vec<_> = m.keys().take(max_items).cloned().collect();
                        for child_key in keys {
                            if let Some(child_value)=m.get(&child_key) {
                                lines.extend(format_structured_value(&child_key, child_value, indent+2, max_depth-1, max_items));
                            }
                        }
                    }
                } else if let JsonVal::Array(nested)=item {
                    lines.push(format!("{}{}. {} items", "  ".repeat(indent+1), idx+1, nested.len()));
                    for nested_item in nested.iter().take(max_items) {
                        lines.extend(format_structured_value("", nested_item, indent+2, max_depth-1, max_items));
                    }
                } else {
                    let s = match item { JsonVal::Str(st)=>truncate_text(st,240), _=>truncate_text(&format!("{item:?}"),240) };
                    lines.push(format!("{}{}. {s}", "  ".repeat(indent+1), idx+1));
                }
            }
            if arr.len()>max_items { lines.push(format!("{}... {} more items", "  ".repeat(indent+1), arr.len()-max_items)); }
            lines
        },
        _ => {
            let s = match value { JsonVal::Str(st)=>truncate_text(st,500), JsonVal::Number(n)=>truncate_text(&n.to_string(),500), JsonVal::Bool(b)=>truncate_text(&b.to_string(),500), _=>truncate_text(&format!("{value:?}"),500) };
            vec![ if label.is_empty() { format!("{bullet}{s}") } else { format!("{bullet}{label} {s}") } ]
        }
    }
}

// ---------------------------------------------------------------------------
// _format_generic_structured_result — lines 846-895
// ---------------------------------------------------------------------------

/// Mirrors `def _format_generic_structured_result(tool_name: str, result: Optional[str], *, fallback_to_text: bool = True) -> Optional[str]` (846-895).
pub fn format_generic_structured_result(tool_name: &str, result: Option<&str>, fallback_to_text: bool) -> Option<String> {
    let data = json_loads_maybe(result);
    let val = match data {
        Some(v) => v,
        None => {
            return if fallback_to_text { result.and_then(|s| if s.trim().is_empty() { None } else { Some(s.to_string()) }) } else { None };
        }
    };
    if let JsonVal::Array(arr)=&val {
        let mut lines = vec![format!("{tool_name}: {} item{}", arr.len(), if arr.len()==1 {""} else {"s"})];
        for item in arr.iter().take(12) {
            match item {
                JsonVal::Object(_) | JsonVal::Array(_) => lines.extend(format_structured_value("", item, 0, 2, 6)),
                JsonVal::Str(s)=>lines.push(format!("- {}", truncate_text(s,240))),
                _=>lines.push(format!("- {}", truncate_text(&format!("{item:?}"),240))),
            }
        }
        if arr.len()>12 { lines.push(format!("... {} more items", arr.len()-12)); }
        return Some(truncate_text(&lines.join("\n"), 5000));
    }
    let obj = match val.as_object() { Some(o)=>o, None=> return None };
    if obj.get("success").and_then(|v| v.as_bool())==Some(false) || obj.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Some(format!("{tool_name} failed: {err}"));
    }
    let mut lines: Vec<String> = Vec::new();
    let success_true = obj.get("success").and_then(|v| v.as_bool())==Some(true);
    lines.push(if success_true { format!("✅ {tool_name} completed") } else { format!("{tool_name} result") });
    let priority_keys = ["message","status","id","task_id","issue_id","title","name","entity_id","state","service","url","path","file_path","count","total","next_run"];
    let mut seen = std::collections::HashSet::new();
    for key in priority_keys {
        if let Some(v)=obj.get(key) {
            if matches!(v, JsonVal::Null) { continue; }
            if let JsonVal::Str(s)=v { if s.is_empty() { continue; } }
            if let JsonVal::Array(a)=v { if a.is_empty() { continue; } }
            seen.insert(key);
            let val_str = match v { JsonVal::Str(s)=>truncate_text(s,500), JsonVal::Number(n)=>truncate_text(&(*n as i64).to_string(),500), _=>truncate_text(&format!("{v:?}"),500) };
            lines.push(format!("- **{key}:** {val_str}"));
        }
    }
    for (key, value) in obj {
        if seen.contains(key.as_str()) || ["success","raw","content","entries"].contains(&key.as_str()) { continue; }
        if matches!(value, JsonVal::Null) { continue; }
        if let JsonVal::Str(s)=value { if s.is_empty() { continue; } }
        if let JsonVal::Array(a)=value { if a.is_empty() { continue; } }
        if let JsonVal::Object(m)=value { if m.is_empty() { continue; } }
        lines.extend(format_structured_value(key, value, 0, 3, 8));
        if lines.len()>=40 { lines.push("- ... more fields truncated".to_string()); break; }
    }
    if let Some(content)=obj.get("content").and_then(|v| v.as_str()) { if !content.trim().is_empty() { lines.push(String::new()); lines.push(truncate_text(content.trim(),1500)); } }
    Some(truncate_text(&lines.join("\n"), 7000))
}

// ---------------------------------------------------------------------------
// _build_polished_completion_content — lines 898-934
// ---------------------------------------------------------------------------

/// Mirrors `def _build_polished_completion_content(tool_name: str, result: Optional[str], function_args: Optional[Dict[str, Any]]) -> Optional[List[Any]]` (898-934).
pub fn build_polished_completion_content(tool_name: &str, result: Option<&str>, function_args: Option<&Args>) -> Option<Vec<ToolContent>> {
    let text: Option<String> = match tool_name {
        "todo" => format_todo_result(result),
        "read_file" => format_read_file_result(result, function_args),
        "write_file" | "patch" => format_edit_result(tool_name, result, function_args),
        "search_files" => format_search_files_result(result),
        "execute_code" => format_execute_code_result(result),
        "process" => format_process_result(result, function_args),
        "delegate_task" => format_delegate_result(result),
        "session_search" => format_session_search_result(result),
        "memory" => format_memory_result(result, function_args),
        "skill_view" => format_skill_view_result(result),
        "skill_manage" => format_skill_manage_result(result, function_args),
        "web_search" => format_web_search_result(result),
        "web_extract" => format_web_extract_result(result),
        "browser_navigate" | "browser_snapshot" | "browser_vision" | "browser_get_images" => format_browser_result_direct(tool_name, result, function_args),
        "vision_analyze" | "image_generate" | "cronjob" => format_media_or_cron_result(tool_name, result),
        _ => {
            if is_polished(tool_name) {
                format_generic_structured_result(tool_name, result, true)
            } else {
                format_generic_structured_result(tool_name, result, false)
            }
        }
    };
    // For non-polished fallback, `format_generic_structured_result(..., fallback=False)` returns None on non-JSON; caller distinguishes.
    let t = text?;
    if t.is_empty() { return None; }
    Some(vec![_text(t)])
}

// ---------------------------------------------------------------------------
// _strip_diff_prefix — lines 937-941
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_diff_prefix(path: str) -> str` (937-941).
pub fn strip_diff_prefix(path: &str) -> String {
    let raw = path.trim();
    if raw.starts_with("a/") || raw.starts_with("b/") { raw[2..].to_string() } else { raw.to_string() }
}
#[allow(dead_code)]
pub fn _strip_diff_prefix(path: &str) -> String { strip_diff_prefix(path) }

// ---------------------------------------------------------------------------
// _parse_unified_diff_content — lines 944-1000
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_unified_diff_content(diff_text: str) -> List[Any]` (944-1000).
/// Converts unified diff text into ACP diff content blocks.
pub fn parse_unified_diff_content(diff_text: &str) -> Vec<ToolContent> {
    if diff_text.is_empty() { return Vec::new(); }
    let mut content: Vec<ToolContent> = Vec::new();
    let mut current_old_path: Option<String> = None;
    let mut current_new_path: Option<String> = None;
    let mut old_lines: Vec<String> = Vec::new();
    let mut new_lines: Vec<String> = Vec::new();

    // Helper to flush accumulated hunk into a diff block.
    let mut flush = |old_path: &mut Option<String>, new_path: &mut Option<String>, old: &mut Vec<String>, new: &mut Vec<String>, out: &mut Vec<ToolContent>| {
        if old_path.is_none() && new_path.is_none() { return; }
        let path = if let Some(ref np)=*new_path { if np!="\/dev/null" { np.clone() } else { old_path.clone().unwrap_or_default() } } else { old_path.clone().unwrap_or_default() };
        if path.is_empty() || path=="/dev/null" {
            *old_path=None; *new_path=None; old.clear(); new.clear(); return;
        }
        let old_text = if old.is_empty() { None } else { Some(old.join("\n")) };
        let new_text = new.join("\n");
        out.push(tool_diff_content(strip_diff_prefix(&path), old_text, new_text));
        *old_path=None; *new_path=None; old.clear(); new.clear();
    };

    for line in diff_text.lines() {
        if line.starts_with("--- ") {
            flush(&mut current_old_path, &mut current_new_path, &mut old_lines, &mut new_lines, &mut content);
            current_old_path = Some(line[4..].trim().to_string());
            continue;
        }
        if line.starts_with("+++ ") {
            current_new_path = Some(line[4..].trim().to_string());
            continue;
        }
        if line.starts_with("@@") { continue; }
        if current_old_path.is_none() && current_new_path.is_none() { continue; }
        if line.starts_with('+') {
            new_lines.push(line[1..].to_string());
        } else if line.starts_with('-') {
            old_lines.push(line[1..].to_string());
        } else if line.starts_with(' ') {
            let shared = line[1..].to_string();
            old_lines.push(shared.clone());
            new_lines.push(shared);
        }
    }
    flush(&mut current_old_path, &mut current_new_path, &mut old_lines, &mut new_lines, &mut content);
    content
}
#[allow(dead_code)]
pub fn _parse_unified_diff_content(diff_text: &str) -> Vec<ToolContent> { parse_unified_diff_content(diff_text) }

// ---------------------------------------------------------------------------
// _build_tool_complete_content — lines 1003-1036
// ---------------------------------------------------------------------------

/// Mirrors `def _build_tool_complete_content(tool_name: str, result: Optional[str], *, function_args, snapshot) -> List[Any]` (1003-1036).
pub fn build_tool_complete_content(tool_name: &str, result: Option<&str>, function_args: Option<&Args>, snapshot: Option<&str>) -> Vec<ToolContent> {
    let display_result = result.unwrap_or("");
    let display_result_owned: String = if display_result.len() > 5000 {
        let r = result.unwrap_or("");
        format!("{}... ({} chars total, truncated)", &r[..4900.min(r.len())], r.len())
    } else { display_result.to_string() };

    // skill_manage diff path (1015-1030) — `extract_edit_diff` may produce unified diff.
    // In Rust we emulate: if snapshot looks like a diff, parse it; otherwise skip.
    if tool_name=="skill_manage" {
        if let Some(snap)=snapshot {
            if snap.trim().starts_with("--- ") || snap.contains("\n--- ") {
                let diff_content = parse_unified_diff_content(snap);
                if !diff_content.is_empty() { return diff_content; }
            }
            // Also check result itself as diff-like fallback.
            if let Some(r)=result {
                if r.contains("diff --git") || r.trim_start().starts_with("--- ") {
                    let dc = parse_unified_diff_content(r);
                    if !dc.is_empty() { return dc; }
                }
            }
        }
    }
    if let Some(polished)=build_polished_completion_content(tool_name, result, function_args) {
        return polished;
    }
    vec![_text(display_result_owned)]
}
#[allow(dead_code)]
pub fn _build_tool_complete_content(tool_name: &str, result: Option<&str>, function_args: Option<&Args>, snapshot: Option<&str>) -> Vec<ToolContent> {
    build_tool_complete_content(tool_name, result, function_args, snapshot)
}

// ---------------------------------------------------------------------------
// build_tool_start / _build_tool_start — lines 1044-1299
// ---------------------------------------------------------------------------

/// Mirrors `def build_tool_start(tool_call_id, tool_name, arguments, *, edit_diff=None) -> ToolCallStart` (1044-1071).
/// Never aborts the turn on malformed args — falls back to minimal valid start event.
pub fn build_tool_start(tool_call_id: &str, tool_name: &str, arguments: &Args, edit_diff: Option<&EditDiff>) -> ToolCallStart {
    // Python wraps `_build_tool_start` in try/except and falls back.
    // In Rust we replicate the handler without panicking (all branches are infallible).
    // Keep the fallback shape for traceability.
    let title = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_tool_title(tool_name, arguments)))
        .unwrap_or_else(|_| tool_name.to_string());
    let kind = get_tool_kind(tool_name);
    let locations = extract_locations(arguments);
    // Dispatch through `_build_tool_start` (which itself handles edit_diff / per-tool content).
    // We inline minimal fallback: if any helper would panic, return minimal event.
    let safe_name = if tool_name.is_empty() { "tool" } else { tool_name };
    // Call inner builder; if it somehow yields empty title, use safe_name.
    let inner = _build_tool_start_inner(tool_call_id, tool_name, arguments, edit_diff, &kind, &title, locations.clone());
    match inner {
        Some(ev) => ev,
        None => start_tool_call(tool_call_id.to_string(), safe_name.to_string(), get_tool_kind(safe_name), None, vec![], None),
    }
}

fn _build_tool_start_inner(tool_call_id: &str, tool_name: &str, arguments: &Args, edit_diff: Option<&EditDiff>, kind: &str, title: &str, locations: Vec<ToolCallLocation>) -> Option<ToolCallStart> {
    // Mirrors `def _build_tool_start(...)` (1074-1299) — per-tool content branches.
    if tool_name=="patch" {
        let content = if let Some(diff)=edit_diff {
            vec![tool_diff_content(diff.path.clone(), diff.old_text.clone(), diff.new_text.clone())]
        } else {
            let mode = arg_str(arguments, "mode").unwrap_or_else(|| "replace".to_string());
            let path = arg_str(arguments, "path").unwrap_or_else(|| "patch input".to_string());
            vec![_text(format!("Preparing {mode} edit for {path}. Approval prompt shows the diff."))]
        };
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="write_file" {
        let content = if let Some(diff)=edit_diff {
            vec![tool_diff_content(diff.path.clone(), diff.old_text.clone(), diff.new_text.clone())]
        } else {
            let path = arg_str(arguments, "path").unwrap_or_default();
            let msg = if !path.is_empty() { format!("Preparing write to {path}. Approval prompt shows the diff.") } else { "Preparing file write. Approval prompt shows the diff.".to_string() };
            vec![_text(msg)]
        };
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="terminal" {
        let command = arg_str(arguments, "command").unwrap_or_default();
        let content = vec![_text(format!("$ {command}"))];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="read_file" {
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), None, locations, None));
    }
    if tool_name=="search_files" {
        let pattern = arg_str(arguments, "pattern").unwrap_or_default();
        let target = arg_str(arguments, "target").unwrap_or_else(|| "content".to_string());
        let search_path = arg_str(arguments, "path");
        let where_suffix = if let Some(sp)=search_path { if !sp.is_empty() { format!(" in {sp}") } else { String::new() } } else { String::new() };
        let content = vec![_text(format!("Searching for '{pattern}' ({target}){where_suffix}"))];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="todo" {
        let content = if let Some(ArgVal::List(items))=arguments.get("todos") {
            let mut preview_lines = vec!["Updating todo list".to_string(), String::new()];
            for item in items.iter().take(8) {
                if let ArgVal::Dict(d)=item {
                    let status = d.get("status").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_else(|| "pending".to_string());
                    let content = d.get("content").or_else(|| d.get("id")).and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                    preview_lines.push(format!("- {status}: {content}"));
                }
            }
            if items.len()>8 { preview_lines.push(format!("... {} more", items.len()-8)); }
            vec![_text(preview_lines.join("\n"))]
        } else {
            vec![_text("Reading todo list".to_string())]
        };
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="skill_view" {
        let name = arg_str_trim(arguments, "name");
        let name = if name.is_empty() { "?" } else { name.as_str() };
        let file_path = arg_str_trim(arguments, "file_path");
        let file_path = if file_path.is_empty() { "SKILL.md" } else { file_path.as_str() };
        // Need to own copies for formatting without borrow conflict
        let name_owned = arg_str_trim(arguments, "name");
        let name_disp = if name_owned.trim().is_empty() { "?" } else { name_owned.trim() };
        let fp_owned = arg_str_trim(arguments, "file_path");
        let fp_disp = if fp_owned.trim().is_empty() { "SKILL.md" } else { fp_owned.trim() };
        let _ = (name, file_path);
        let content = vec![_text(format!("Loading skill '{name_disp}' ({fp_disp})"))];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="skill_manage" {
        let action = { let a=arg_str_trim(arguments, "action"); if a.is_empty() { "manage".to_string() } else { a } };
        let name = { let n=arg_str_trim(arguments, "name"); if n.is_empty() { "?".to_string() } else { n } };
        let file_path = { let f=arg_str_trim(arguments, "file_path"); if f.is_empty() { "SKILL.md".to_string() } else { f } };
        let path = if !file_path.is_empty() { format!("skills/{name}/{file_path}") } else { format!("skills/{name}") };
        let content: Vec<ToolContent> = match action.as_str() {
            "patch" => {
                let old = arg_str(arguments, "old_string").unwrap_or_default();
                let new = arg_str(arguments, "new_string").unwrap_or_default();
                vec![tool_diff_content(path, if old.is_empty() { None } else { Some(old) }, new)]
            },
            "edit" | "create" => {
                let new_text = arg_str(arguments, "content").unwrap_or_default();
                vec![tool_diff_content(path, None, new_text)]
            },
            "write_file" => {
                let target = arg_str(arguments, "file_path").unwrap_or_else(|| "file".to_string());
                let new_text = arg_str(arguments, "file_content").unwrap_or_default();
                vec![tool_diff_content(format!("skills/{name}/{target}"), None, new_text)]
            },
            "delete" | "remove_file" => {
                let target = arg_str(arguments, "file_path").unwrap_or_else(|| file_path.clone());
                let t = if target.is_empty() { name.clone() } else { target };
                vec![_text(format!("Removing {t} from skill '{name}'"))]
            },
            _ => vec![_text(format!("Running skill_manage action '{action}' on skill '{name}' ({file_path})"))],
        };
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="execute_code" {
        let code = arg_str(arguments, "code").unwrap_or_default().trim().to_string();
        let preview = if code.len()>1200 { format!("{}... ({} chars total, truncated)", &code[..1200], code.len()) } else { code.clone() };
        let content = vec![_text(if !preview.is_empty() { format!("Running Python helper script:\n\n```python\n{preview}\n```") } else { "Running Python helper script".to_string() })];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="web_search" {
        let query = arg_str_trim(arguments, "query");
        let content = vec![_text(if !query.is_empty() { format!("Searching the web for: {query}") } else { "Searching the web".to_string() })];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="web_extract" {
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), None, locations, None));
    }
    if tool_name=="process" {
        let action = { let a=arg_str_trim(arguments, "action"); if a.is_empty() { "manage".to_string() } else { a } };
        let sid = arg_str_trim(arguments, "session_id");
        let data_preview = arg_str_trim(arguments, "data");
        let mut text = format!("Process action: {action}");
        if !sid.is_empty() { text.push_str(&format!("\nSession: {sid}")); }
        if !data_preview.is_empty() { text.push_str(&format!("\nInput: {}", truncate_text(&data_preview, 500))); }
        let content = vec![_text(text)];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="delegate_task" {
        let tasks_val = arguments.get("tasks");
        let content = if let Some(ArgVal::List(tasks))=tasks_val {
            if !tasks.is_empty() {
                let mut lines = vec![format!("Delegating {} tasks", tasks.len()), String::new()];
                for (i, task) in tasks.iter().take(8).enumerate() {
                    if let ArgVal::Dict(d)=task {
                        let goal = d.get("goal").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default().trim().to_string();
                        let role = d.get("role").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default().trim().to_string();
                        lines.push(format!("{}. {}", i+1, truncate_text(&goal, 160)) + if !role.is_empty() { &format!(" ({role})") } else { "" });
                    }
                }
                if tasks.len()>8 { lines.push(format!("... {} more", tasks.len()-8)); }
                vec![_text(lines.join("\n"))]
            } else {
                let goal = arg_str_trim(arguments, "goal");
                vec![_text(if !goal.is_empty() { format!("Delegating task:\n{}", truncate_text(&goal, 800)) } else { "Delegating task".to_string() })]
            }
        } else {
            let goal = arg_str_trim(arguments, "goal");
            vec![_text(if !goal.is_empty() { format!("Delegating task:\n{}", truncate_text(&goal, 800)) } else { "Delegating task".to_string() })]
        };
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="session_search" {
        let query = arg_str_trim(arguments, "query");
        let content = vec![_text(if !query.is_empty() { format!("Searching past sessions for: {query}") } else { "Loading recent sessions".to_string() })];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if tool_name=="memory" {
        let action = { let a=arg_str_trim(arguments, "action"); if a.is_empty() { "manage".to_string() } else { a } };
        let target = { let t=arg_str_trim(arguments, "target"); if t.is_empty() { "memory".to_string() } else { t } };
        let preview = arg_str(arguments, "content").or_else(|| arg_str(arguments, "old_text")).unwrap_or_default().trim().to_string();
        let mut text = format!("Memory {action} ({target})");
        if !preview.is_empty() { text.push_str(&format!("\nPreview: {}", truncate_text(&preview, 500))); }
        let content = vec![_text(text)];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if is_polished(tool_name) {
        // 1275-1283
        let args_text = {
            // Try to stringify args as JSON-ish for display (indent 2); fallback to Debug.
            let mut parts: Vec<String> = Vec::new();
            for (k,v) in arguments {
                let val_str = match v { ArgVal::Str(s)=>format!("\"{s}\""), ArgVal::Int(n)=>n.to_string(), ArgVal::Float(f)=>f.to_string(), ArgVal::Bool(b)=>b.to_string(), ArgVal::Null=>"null".to_string(), ArgVal::List(l)=>format!("[{} items]", l.len()), ArgVal::Dict(d)=>format!("{{{} keys}}", d.len()) };
                parts.push(format!("  \"{k}\": {val_str}"));
            }
            format!("{{\n{}\n}}", parts.join(",\n"))
        };
        let content = vec![_text(truncate_text(&args_text, 1200))];
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, None));
    }
    if arguments.is_empty() {
        return Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), None, locations, None));
    }
    // Generic fallback (1290-1299)
    let args_text = {
        let mut parts: Vec<String> = Vec::new();
        for (k,v) in arguments {
            let val_str = match v { ArgVal::Str(s)=>format!("\"{s}\""), ArgVal::Int(n)=>n.to_string(), ArgVal::Bool(b)=>b.to_string(), _=>format!("{v:?}") };
            parts.push(format!("\"{k}\": {val_str}"));
        }
        format!("{{\n{}\n}}", parts.join(",\n"))
    };
    let content = vec![tool_content(text_block(args_text))];
    let raw_input = if is_polished(tool_name) { None } else { Some(arguments.clone()) };
    Some(start_tool_call(tool_call_id.to_string(), title.to_string(), kind.to_string(), Some(content), locations, raw_input))
}

/// Inner `_build_tool_start` alias for 1:1 traceability (lines 1074-1299).
#[allow(dead_code)]
pub fn _build_tool_start(tool_call_id: &str, tool_name: &str, arguments: &Args, edit_diff: Option<&EditDiff>) -> ToolCallStart {
    build_tool_start(tool_call_id, tool_name, arguments, edit_diff)
}

// ---------------------------------------------------------------------------
// _is_structured_json_result — lines 1302-1303
// ---------------------------------------------------------------------------

/// Mirrors `def _is_structured_json_result(result: Optional[str]) -> bool` (1302-1303).
pub fn is_structured_json_result(result: Option<&str>) -> bool {
    matches!(json_loads_maybe(result), Some(JsonVal::Object(_)) | Some(JsonVal::Array(_)))
}
#[allow(dead_code)]
pub fn _is_structured_json_result(result: Option<&str>) -> bool { is_structured_json_result(result) }

// ---------------------------------------------------------------------------
// build_tool_complete — lines 1306-1331
// ---------------------------------------------------------------------------

/// Mirrors `def build_tool_complete(tool_call_id, tool_name, result, function_args, snapshot) -> ToolCallProgress` (1306-1331).
pub fn build_tool_complete(tool_call_id: &str, tool_name: &str, result: Option<&str>, function_args: Option<&Args>, snapshot: Option<&str>) -> ToolCallProgress {
    let kind = get_tool_kind(tool_name);
    let content: Option<Vec<ToolContent>> = if tool_name=="web_extract" {
        let error_text = format_web_extract_result(result);
        error_text.map(|t| vec![_text(t)])
    } else {
        let c = build_tool_complete_content(tool_name, result, function_args, snapshot);
        if c.is_empty() { None } else { Some(c) }
    };
    let status = if tool_result_failed(result, Some(tool_name)) { "failed" } else { "completed" }.to_string();
    let raw_output = if is_polished(tool_name) || is_structured_json_result(result) { None } else { result.map(|s| s.to_string()) };
    update_tool_call(tool_call_id.to_string(), kind, status, content, raw_output)
}

// ---------------------------------------------------------------------------
// extract_locations — lines 1339-1348
// ---------------------------------------------------------------------------

/// Mirrors `def extract_locations(arguments: Dict[str, Any]) -> List[ToolCallLocation]` (1339-1348).
pub fn extract_locations(arguments: &Args) -> Vec<ToolCallLocation> {
    let mut locations: Vec<ToolCallLocation> = Vec::new();
    if let Some(path_val) = arguments.get("path").and_then(|v| v.as_str().map(|s| s.to_string())) {
        if !path_val.is_empty() {
            // `line = arguments.get("offset") or arguments.get("line")`
            let line: Option<i64> = arguments.get("offset").and_then(|v| match v {
                ArgVal::Int(n)=>Some(*n),
                ArgVal::Float(f)=>Some(*f as i64),
                ArgVal::Str(s)=>s.parse::<i64>().ok(),
                _=>None,
            }).or_else(|| arguments.get("line").and_then(|v| match v {
                ArgVal::Int(n)=>Some(*n),
                ArgVal::Float(f)=>Some(*f as i64),
                ArgVal::Str(s)=>s.parse::<i64>().ok(),
                _=>None,
            }));
            locations.push(ToolCallLocation { path: path_val, line });
        }
    }
    locations
}
#[allow(dead_code)]
pub fn _extract_locations(arguments: &Args) -> Vec<ToolCallLocation> { extract_locations(arguments) }
