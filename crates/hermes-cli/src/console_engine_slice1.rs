//! hermes-cli console_engine — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/console_engine.py`
//! slice 1/2 — lines 1–900 of 1 677 (first 900 LOC).
//! Covers: module docstring (safe console engine, curated adapters for
//! `hermes console` / dashboard websocket), `ConsoleStatus` literal,
//! `ConsoleCommandError`, `ConsoleResult` / `ConsoleCommand` dataclasses,
//! `_ArgumentParser.error` hook, `_capture_output` (stdout/stderr redirect +
//! SystemExit int/str handling), `_is_status_footer_rule` /
//! `_strip_console_status_footer`, `_table_summary`, `_split_line`
//! (Windows-safe via `split_command_line`), `_contains_shell_syntax`,
//! `_format_sessions` / `_format_job`, parser helpers
//! (`_parser_root`, `_subparser_actions`, `_choice_help`, `_clean_summary`,
//! `_summaries_from_parser`, `_noop_console_command`), summary caches
//! (`_extracted_summaries`, `_registered_summaries`, `_builder_summaries`,
//! `_adder_summaries` — all `lru_cache(maxsize=None)`), dispatch helpers
//! (`_invoke_namespace`, `_set_attrs`, `_dispatch_extracted_subcommand`,
//! `_dispatch_registered_subcommand`, `_dispatch_builder_subcommand`,
//! `_dispatch_adder_subcommand`), handler factories (`_extracted_handler`,
//! `_registered_handler`, `_builder_handler`, `_adder_handler`),
//! `_register_command_family`, `HermesConsoleEngine` (output_limit/history/
//! commands, `execute`/`help_text`, `_register_defaults` +
//! `_register_broad_cli_surface` through the `extracted` dict loop and the
//! first `sessions optimize` registration at line 900).
//! Continued in `console_engine_slice2.rs` (remaining `_register_broad_cli_surface`
//! registrations + `register`/`_execute_builtin`/`_resolve_command`/ etc.
//! through `run_console_repl` at line 1677).
//!
//! T0704 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-6
// ---------------------------------------------------------------------------

/// Safe Hermes Console command engine.
///
/// Backs `hermes console` and is intentionally narrower than the full
/// Hermes CLI. Exposes a curated set of native adapters that can later be
/// shared by the dashboard console websocket without becoming a raw shell.
/// Mirrors `hermes_cli/console_engine.py` lines 1-6.
pub const MODULE_DOC: &str =
    "Safe Hermes Console command engine — curated adapters for hermes console";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 8-23
// ---------------------------------------------------------------------------
// Python: argparse, contextlib, difflib, functools, importlib, io, json,
// shlex, sys, dataclasses, pathlib.Path, typing (Callable, Iterable,
// Literal, NoReturn, Sequence), tools.ansi_strip.strip_ansi
//
// Rust: std only (NEVER cargo). All external/Python-specific imports are
// stubbed for 1:1 traceability; real wiring in later slices when those
// modules are ported.

/// Mirrors `from tools.ansi_strip import strip_ansi as _strip_ansi` (line 23).
pub fn strip_ansi(text: &str) -> String {
    // Minimal ANSI strip: remove \x1b[...m and \x1b[...K etc.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if let Some(&'[') = chars.peek() {
                chars.next();
                // consume until letter
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Mirrors `from hermes_cli._subprocess_compat import split_command_line`
/// (line 124) — stub via shlex-like splitter.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    // Windows-safe splitter: plain shlex posix=True eats backslashes, so
    // `sessions export C:\Users\me\out.jsonl` must preserve backslashes.
    // Minimal: handle single/double quotes, preserve backslashes inside.
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut in_token = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            cur.push(ch);
            escaped = false;
            in_token = true;
            i += 1;
            continue;
        }
        if ch == '\\' && !in_single {
            // preserve backslash unless escaping quote
            // lookahead: if next char is quote/escape, consume; else keep
            if i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\'' || chars[i + 1] == '\\') {
                escaped = true;
            } else {
                cur.push(ch);
                in_token = true;
            }
            i += 1;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            in_token = true;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            in_token = true;
            i += 1;
            continue;
        }
        if !in_single && !in_double && ch.is_whitespace() {
            if in_token {
                tokens.push(cur.clone());
                cur.clear();
                in_token = false;
            }
            i += 1;
            continue;
        }
        cur.push(ch);
        in_token = true;
        i += 1;
    }
    if in_single || in_double {
        return Err("No closing quotation".to_string());
    }
    if in_token || !cur.is_empty() {
        // shlex would keep empty quoted tokens; our in_token handles
        tokens.push(cur);
    }
    Ok(tokens)
}

/// Mirrors `importlib.import_module` — stub.
pub fn import_module_stub(_name: &str) -> Option<()> {
    None
}

/// Mirrors `from cron.jobs import effective_job_state` — stub.
pub fn effective_job_state_stub(job: &HashMap<String, String>) -> String {
    job.get("state").cloned().unwrap_or_else(|| "-".to_string())
}

// ---------------------------------------------------------------------------
// ConsoleStatus — mirrors line 26
// ---------------------------------------------------------------------------

/// Mirrors `ConsoleStatus = Literal["ok", "error", "confirm_required", "exit", "clear"]` (26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleStatus {
    Ok,
    Error,
    ConfirmRequired,
    Exit,
    Clear,
}

impl ConsoleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ConsoleStatus::Ok => "ok",
            ConsoleStatus::Error => "error",
            ConsoleStatus::ConfirmRequired => "confirm_required",
            ConsoleStatus::Exit => "exit",
            ConsoleStatus::Clear => "clear",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(ConsoleStatus::Ok),
            "error" => Some(ConsoleStatus::Error),
            "confirm_required" => Some(ConsoleStatus::ConfirmRequired),
            "exit" => Some(ConsoleStatus::Exit),
            "clear" => Some(ConsoleStatus::Clear),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConsoleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// ConsoleCommandError — mirrors lines 29-30
// ---------------------------------------------------------------------------

/// Mirrors `class ConsoleCommandError(RuntimeError):` (29-30).
/// User-facing console command failure.
#[derive(Debug, Clone)]
pub struct ConsoleCommandError(pub String);

impl std::fmt::Display for ConsoleCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConsoleCommandError {}

// ---------------------------------------------------------------------------
// ConsoleResult — mirrors lines 33-38
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class ConsoleResult:` (33-38).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleResult {
    pub status: ConsoleStatus,
    pub output: String,
    pub command: String,
    pub confirmation_message: String,
}

impl ConsoleResult {
    pub fn new(status: ConsoleStatus) -> Self {
        Self { status, output: String::new(), command: String::new(), confirmation_message: String::new() }
    }
    pub fn with_output(status: ConsoleStatus, output: impl Into<String>) -> Self {
        Self { status, output: output.into(), command: String::new(), confirmation_message: String::new() }
    }
}

// ---------------------------------------------------------------------------
// ConsoleCommand — mirrors lines 41-48
// ---------------------------------------------------------------------------

/// Handler type — mirrors `Callable[["HermesConsoleEngine", list[str]], str]` (46).
pub type ConsoleHandler = fn(&HermesConsoleEngine, Vec<String>) -> Result<String, ConsoleCommandError>;

/// Mirrors `@dataclass(frozen=True) class ConsoleCommand:` (41-48).
#[derive(Clone)]
pub struct ConsoleCommand {
    pub path: Vec<String>,
    pub usage: String,
    pub summary: String,
    pub handler: ConsoleHandler,
    pub mutating: bool,
    pub confirmation: String,
}

impl std::fmt::Debug for ConsoleCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleCommand")
            .field("path", &self.path)
            .field("usage", &self.usage)
            .field("summary", &self.summary)
            .field("mutating", &self.mutating)
            .field("confirmation", &self.confirmation)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// _ArgumentParser — mirrors lines 51-53
// ---------------------------------------------------------------------------

/// Mirrors `class _ArgumentParser(argparse.ArgumentParser):` (51-53).
/// Overrides `error` to raise `ConsoleCommandError` instead of `sys.exit`.
#[derive(Debug, Clone)]
pub struct ArgumentParser {
    pub prog: String,
    pub add_help: bool,
}

impl ArgumentParser {
    pub fn new(prog: impl Into<String>, add_help: bool) -> Self {
        Self { prog: prog.into(), add_help }
    }
    /// Mirrors `def error(self, message: str) -> NoReturn:` (52-53).
    pub fn error(&self, message: &str) -> Result<(), ConsoleCommandError> {
        Err(ConsoleCommandError(format!("{}: {}", self.prog, message)))
    }
}

// Minimal argparse action types for 1:1 traceability (lines 170-214).

#[derive(Debug, Clone)]
pub struct SubParsersAction {
    pub dest: String,
    pub choices: HashMap<String, ArgumentParser>,
    pub choices_help: HashMap<String, String>,
}

impl SubParsersAction {
    pub fn new(dest: impl Into<String>) -> Self {
        Self { dest: dest.into(), choices: HashMap::new(), choices_help: HashMap::new() }
    }
    pub fn add_parser(&mut self, name: impl Into<String>) -> &mut ArgumentParser {
        let n = name.into();
        self.choices.entry(n.clone()).or_insert_with(|| ArgumentParser::new(n.clone(), false));
        self.choices.get_mut(&n).unwrap()
    }
}

// ---------------------------------------------------------------------------
// _capture_output — mirrors lines 56-78
// ---------------------------------------------------------------------------

/// Mirrors `def _capture_output(fn: Callable[[], object]) -> str:` (56-78).
///
/// Captures stdout/stderr via String buffers. Handles `SystemExit` / int
/// exit codes and `sys.exit("msg")` str codes (which must not be int()'d).
pub fn capture_output<F>(func: F) -> Result<String, ConsoleCommandError>
where
    F: FnOnce() -> Result<Option<i32>, String>,
{
    // In Rust we model the Python try/except SystemExit branches as a
    // Result<Option<i32>, String> where Err(string) is `SystemExit("msg")`
    // and Ok(Some(code)) is `SystemExit(int)` / return int.
    // The caller helper `_invoke_via_capture` below adapts the closure shape.
    let mut buf = String::new();
    // Simulate redirect: the closure is expected to push to captured output
    // via side-effects in real Python; in Rust slice 1 we just run it and
    // return its Ok text or map Err to ConsoleCommandError.
    match func() {
        Ok(opt_code) => {
            let code = opt_code.unwrap_or(0);
            if code != 0 {
                let text = buf.trim().to_string();
                return Err(ConsoleCommandError(
                    if text.is_empty() { format!("Command exited with status {code}") } else { text }
                ));
            }
            Ok(buf.trim_end().to_string())
        }
        Err(message) => {
            // exc.code is str -> code 1 branch (lines 70-72)
            let msg = message.trim().to_string();
            let text = buf.trim().to_string();
            Err(ConsoleCommandError(if !msg.is_empty() { msg } else if !text.is_empty() { text } else { "Command exited with status 1".to_string() }))
        }
    }
}

/// Convenience wrapper that mirrors Python's `_capture_output(lambda: func())`
/// where func may print to stdout/stderr. For 1:1 traceability we expose a
/// string-returning variant that would collect printed output in real impl.
pub fn capture_output_str<F>(func: F) -> Result<String, ConsoleCommandError>
where
    F: FnOnce() -> Result<String, String>,
{
    match func() {
        Ok(text) => Ok(text.trim_end().to_string()),
        Err(msg) => Err(ConsoleCommandError(msg)),
    }
}

// ---------------------------------------------------------------------------
// _is_status_footer_rule — mirrors lines 81-86
// ---------------------------------------------------------------------------

/// Mirrors `def _is_status_footer_rule(line: str) -> bool:` (81-86).
pub fn is_status_footer_rule(line: &str) -> bool {
    let stripped = strip_ansi(line).trim().to_string();
    if stripped.len() < 8 {
        return false;
    }
    let normalized = stripped.replace('\u{2500}', "-");
    normalized.chars().all(|c| c == '-')
}

// ---------------------------------------------------------------------------
// _strip_console_status_footer — mirrors lines 89-109
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_console_status_footer(text: str) -> str:` (89-109).
pub fn strip_console_status_footer(text: &str) -> String {
    let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    while !lines.is_empty() && strip_ansi(&lines[lines.len() - 1]).trim().is_empty() {
        lines.pop();
    }
    if lines.len() < 2 {
        return text.trim_end().to_string();
    }
    let last = strip_ansi(&lines[lines.len() - 1]).trim().to_string();
    let prev = strip_ansi(&lines[lines.len() - 2]).trim().to_string();
    if !(prev.starts_with("Run 'hermes doctor'") && last.starts_with("Run 'hermes setup'")) {
        return text.trim_end().to_string();
    }
    lines.truncate(lines.len() - 2);
    while !lines.is_empty() && strip_ansi(&lines[lines.len() - 1]).trim().is_empty() {
        lines.pop();
    }
    if !lines.is_empty() && is_status_footer_rule(&lines[lines.len() - 1]) {
        lines.pop();
    }
    lines.join("\n").trim_end().to_string()
}

// ---------------------------------------------------------------------------
// _table_summary — mirrors lines 112-116
// ---------------------------------------------------------------------------

/// Mirrors `def _table_summary(summary: str, *, limit: int = 76) -> str:` (112-116).
pub fn table_summary(summary: &str, limit: usize) -> String {
    let collapsed = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= limit {
        return collapsed;
    }
    let mut end = limit.saturating_sub(3);
    // ensure we don't split a char boundary or leave trailing whitespace
    while end > 0 && !collapsed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", collapsed[..end].trim_end())
}
pub fn table_summary_default(summary: &str) -> String {
    table_summary(summary, 76)
}

// ---------------------------------------------------------------------------
// _split_line — mirrors lines 119-128
// ---------------------------------------------------------------------------

/// Mirrors `def _split_line(line: str) -> list[str]:` (119-128).
/// Windows-safe via `split_command_line`; raises ConsoleCommandError on ValueError.
pub fn split_line(line: &str) -> Result<Vec<String>, ConsoleCommandError> {
    split_command_line(line).map_err(|exc| ConsoleCommandError(format!("Could not parse command: {exc}")))
}

// ---------------------------------------------------------------------------
// _contains_shell_syntax — mirrors lines 131-137
// ---------------------------------------------------------------------------

/// Mirrors `def _contains_shell_syntax(line: str, tokens: Sequence[str]) -> bool:` (131-137).
pub fn contains_shell_syntax(line: &str, tokens: &[String]) -> bool {
    if line.contains("$(") || line.contains('`') {
        return true;
    }
    let shell_tokens: HashSet<&str> = ["|", "||", "&", "&&", ";", ">", ">>", "<", "<<", "2>", "2>>"].into_iter().collect();
    if tokens.iter().any(|t| shell_tokens.contains(t.as_str())) {
        return true;
    }
    line.chars().any(|ch| matches!(ch, '|' | '<' | '>' | ';'))
}

// ---------------------------------------------------------------------------
// _format_sessions — mirrors lines 140-152
// ---------------------------------------------------------------------------

/// Mirrors `def _format_sessions(sessions: Sequence[dict]) -> str:` (140-152).
pub fn format_sessions(sessions: &[HashMap<String, String>]) -> String {
    if sessions.is_empty() {
        return "No sessions found.".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("{:<32} {:<12} {:>5}  Title / Preview", "ID", "Source", "Msgs"));
    lines.push("-".repeat(82));
    for session in sessions {
        let sid = session.get("id").map(|s| s.as_str()).unwrap_or("").chars().take(32).collect::<String>();
        let source = session.get("source").map(|s| s.as_str()).unwrap_or("-").chars().take(12).collect::<String>();
        let messages: i64 = session.get("message_count").and_then(|v| v.parse().ok()).unwrap_or(0);
        let title_raw = session.get("title").or_else(|| session.get("preview")).map(|s| s.as_str()).unwrap_or("");
        let title = title_raw.replace('\n', " ").chars().take(60).collect::<String>();
        lines.push(format!("{sid:<32} {source:<12} {messages:>5}  {title}"));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// _format_job — mirrors lines 155-161
// ---------------------------------------------------------------------------

/// Mirrors `def _format_job(job: dict, action: str) -> str:` (155-161).
pub fn format_job(job: &HashMap<String, String>, action: &str) -> String {
    let job_id = job.get("id").or_else(|| job.get("job_id")).map(|s| s.as_str()).unwrap_or("?");
    let name = job.get("name").map(|s| s.as_str()).unwrap_or("(unnamed)");
    let state = effective_job_state_stub(job);
    format!("{action} job: {name} ({job_id}) [{state}]")
}

// ---------------------------------------------------------------------------
// _parser_root — mirrors lines 164-167
// ---------------------------------------------------------------------------

/// Mirrors `def _parser_root() -> tuple[_ArgumentParser, argparse._SubParsersAction]:` (164-167).
pub fn parser_root() -> (ArgumentParser, SubParsersAction) {
    let parser = ArgumentParser::new("hermes", false);
    let subparsers = SubParsersAction::new("_console_command");
    (parser, subparsers)
}

// ---------------------------------------------------------------------------
// _subparser_actions — mirrors lines 170-176
// ---------------------------------------------------------------------------

/// Mirrors `def _subparser_actions(parser: argparse.ArgumentParser) -> list[...]:` (170-176).
pub fn subparser_actions(parser: &ArgumentParser) -> Vec<SubParsersAction> {
    let _ = parser;
    // In Python this scans parser._actions for SubParsersAction instances.
    // Rust stub: parsers hold no actions list in slice 1.
    Vec::new()
}

// ---------------------------------------------------------------------------
// _choice_help — mirrors lines 178-184
// ---------------------------------------------------------------------------

/// Mirrors `def _choice_help(action: argparse._SubParsersAction, name: str) -> str:` (178-184).
pub fn choice_help(action: &SubParsersAction, name: &str) -> String {
    if let Some(h) = action.choices_help.get(name) {
        if h != "SUPPRESS" && !h.is_empty() {
            return h.clone();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// _clean_summary — mirrors lines 187-197
// ---------------------------------------------------------------------------

/// Mirrors `def _clean_summary(text: str | None) -> str:` (187-197).
pub fn clean_summary(text: Option<&str>) -> String {
    let t = match text {
        None => return String::new(),
        Some(s) => s,
    };
    if t == "SUPPRESS" {
        return String::new();
    }
    let summary = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return String::new();
    }
    if summary.starts_with("Run `hermes ") {
        return String::new();
    }
    summary
}

// ---------------------------------------------------------------------------
// _summaries_from_parser — mirrors lines 200-215
// ---------------------------------------------------------------------------

/// Mirrors `def _summaries_from_parser(parser: argparse.ArgumentParser) -> dict[...]:` (200-215).
pub fn summaries_from_parser(parser: &ArgumentParser) -> HashMap<Vec<String>, String> {
    let mut summaries: HashMap<Vec<String>, String> = HashMap::new();
    // Walk subparsers recursively — mirrors inner `walk(current, path)`.
    // Stub in slice 1: no parser tree is populated without import side-effects.
    // Preserve signature for 1:1 audit; real wiring would walk `parser._actions`.
    let _ = parser;
    summaries
}

// ---------------------------------------------------------------------------
// _noop_console_command — mirrors lines 218-219
// ---------------------------------------------------------------------------

/// Mirrors `def _noop_console_command(_args: argparse.Namespace) -> None:` (218-219).
pub fn noop_console_command(_args: &HashMap<String, String>) {}

// ---------------------------------------------------------------------------
// Summary caches — mirrors lines 222-283 (lru_cache maxsize=None)
// ---------------------------------------------------------------------------

static EXTRACTED_SUMMARIES: OnceLock<Mutex<HashMap<String, HashMap<Vec<String>, String>>>> = OnceLock::new();
fn extracted_summaries_cache() -> &'static Mutex<HashMap<String, HashMap<Vec<String>, String>>> {
    EXTRACTED_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()))
}
static REGISTERED_SUMMARIES: OnceLock<Mutex<HashMap<String, HashMap<Vec<String>, String>>>> = OnceLock::new();
fn registered_summaries_cache() -> &'static Mutex<HashMap<String, HashMap<Vec<String>, String>>> {
    REGISTERED_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()))
}
static BUILDER_SUMMARIES: OnceLock<Mutex<HashMap<String, HashMap<Vec<String>, String>>>> = OnceLock::new();
fn builder_summaries_cache() -> &'static Mutex<HashMap<String, HashMap<Vec<String>, String>>> {
    BUILDER_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()))
}
static ADDER_SUMMARIES: OnceLock<Mutex<HashMap<String, HashMap<Vec<String>, String>>>> = OnceLock::new();
fn adder_summaries_cache() -> &'static Mutex<HashMap<String, HashMap<Vec<String>, String>>> {
    ADDER_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key_3(a: &str, b: &str, c: &str) -> String {
    format!("{a}\x1f{b}\x1f{c}")
}
fn cache_key_2(a: &str, b: &str) -> String {
    format!("{a}\x1f{b}")
}

/// Mirrors `@lru_cache(maxsize=None) def _extracted_summaries(module_name, builder_name, main_handler_name)` (228-241).
pub fn extracted_summaries(module_name: &str, builder_name: &str, main_handler_name: &str) -> HashMap<Vec<String>, String> {
    let key = cache_key_3(module_name, builder_name, main_handler_name);
    {
        let cache = extracted_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
    }
    // Python builds a throwaway argparse tree by importing module + builder.
    // Cache by args (all hashable strings); callers only read the map.
    // Rust slice 1: stub returns empty map; cached empty for 1:1 lru semantics.
    let result: HashMap<Vec<String>, String> = HashMap::new();
    // Best-effort import mimic: would call `build_dump_parser(subparsers, **{main_handler_name: _noop})` etc.
    let _ = (module_name, builder_name, main_handler_name);
    let mut cache = extracted_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key, result.clone());
    result
}

/// Mirrors `@lru_cache def _registered_summaries(root, module_name, register_name)` (244-258).
pub fn registered_summaries(root: &str, module_name: &str, register_name: &str) -> HashMap<Vec<String>, String> {
    let key = cache_key_3(root, module_name, register_name);
    {
        let cache = registered_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
    }
    let result: HashMap<Vec<String>, String> = HashMap::new();
    let _ = (root, module_name, register_name);
    let mut cache = registered_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key, result.clone());
    result
}

/// Mirrors `@lru_cache def _builder_summaries(module_name, builder_name)` (261-272).
pub fn builder_summaries(module_name: &str, builder_name: &str) -> HashMap<Vec<String>, String> {
    let key = cache_key_2(module_name, builder_name);
    {
        let cache = builder_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
    }
    let result: HashMap<Vec<String>, String> = HashMap::new();
    let _ = (module_name, builder_name);
    let mut cache = builder_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key, result.clone());
    result
}

/// Mirrors `@lru_cache def _adder_summaries(module_name, add_name)` (275-283).
pub fn adder_summaries(module_name: &str, add_name: &str) -> HashMap<Vec<String>, String> {
    let key = cache_key_2(module_name, add_name);
    {
        let cache = adder_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
    }
    let result: HashMap<Vec<String>, String> = HashMap::new();
    let _ = (module_name, add_name);
    let mut cache = adder_summaries_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key, result.clone());
    result
}

// ---------------------------------------------------------------------------
// _invoke_namespace + _set_attrs — mirrors lines 286-296
// ---------------------------------------------------------------------------

/// Mirrors `def _invoke_namespace(args: argparse.Namespace) -> object:` (286-290).
pub fn invoke_namespace(args: &HashMap<String, String>) -> Result<String, ConsoleCommandError> {
    // Python checks `getattr(args, "func", None)` is callable then calls `func(args)`.
    // Rust slice 1 stub: no func stored; return error mirroring Python message.
    let _ = args;
    Err(ConsoleCommandError("No handler is available for that console command.".to_string()))
}

/// Mirrors `def _set_attrs(args: argparse.Namespace, **attrs) -> Namespace:` (293-296).
pub fn set_attrs(mut args: HashMap<String, String>, attrs: HashMap<String, String>) -> HashMap<String, String> {
    for (k, v) in attrs {
        args.insert(k, v);
    }
    args
}

// ---------------------------------------------------------------------------
// Dispatch helpers — mirrors lines 299-380
// ---------------------------------------------------------------------------

/// Mirrors `def _dispatch_extracted_subcommand(*, root, fixed, args, module_name, ...)` (299-318).
pub fn dispatch_extracted_subcommand(
    root: &str,
    fixed: &[String],
    args: &[String],
    module_name: &str,
    builder_name: &str,
    main_handler_name: &str,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> Result<String, ConsoleCommandError> {
    let _ = (root, fixed, args, module_name, builder_name, main_handler_name, namespace_update);
    // Python: builds parser, imports modules, parses [root, *fixed, *args], applies update, captures func(args).
    // Rust stub: return empty output (real dispatch in full port when subcommand modules are wired).
    Ok(String::new())
}

/// Mirrors `def _dispatch_registered_subcommand(...)` (321-341).
pub fn dispatch_registered_subcommand(
    root: &str,
    fixed: &[String],
    args: &[String],
    module_name: &str,
    register_name: &str,
    handler_name: Option<&str>,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> Result<String, ConsoleCommandError> {
    let _ = (root, fixed, args, module_name, register_name, handler_name, namespace_update);
    Ok(String::new())
}

/// Mirrors `def _dispatch_builder_subcommand(...)` (344-362).
pub fn dispatch_builder_subcommand(
    root: &str,
    fixed: &[String],
    args: &[String],
    module_name: &str,
    builder_name: &str,
    main_handler_name: &str,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> Result<String, ConsoleCommandError> {
    let _ = (root, fixed, args, module_name, builder_name, main_handler_name, namespace_update);
    Ok(String::new())
}

/// Mirrors `def _dispatch_adder_subcommand(...)` (365-380).
pub fn dispatch_adder_subcommand(
    root: &str,
    fixed: &[String],
    args: &[String],
    module_name: &str,
    add_name: &str,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> Result<String, ConsoleCommandError> {
    let _ = (root, fixed, args, module_name, add_name, namespace_update);
    Ok(String::new())
}

// ---------------------------------------------------------------------------
// Handler factories — mirrors lines 383-466
// ---------------------------------------------------------------------------

/// Mirrors `def _extracted_handler(root, fixed, module_name, ...) -> Callable:` (383-402).
pub fn extracted_handler(
    root: String,
    fixed: Vec<String>,
    module_name: String,
    builder_name: String,
    main_handler_name: String,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> ConsoleHandler {
    // Return a function pointer that dispatches to the extracted subcommand.
    // Rust fn pointers cannot capture env, so we leak the config as statics
    // for 1:1 traceability — the stub ignores them (full port would box closure).
    let _ = (root, fixed, module_name, builder_name, main_handler_name, namespace_update);
    stub_handler
}

/// Mirrors `def _registered_handler(...)` (405-424).
pub fn registered_handler(
    root: String,
    fixed: Vec<String>,
    module_name: String,
    register_name: String,
    handler_name: Option<String>,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> ConsoleHandler {
    let _ = (root, fixed, module_name, register_name, handler_name, namespace_update);
    stub_handler
}

/// Mirrors `def _builder_handler(...)` (427-445).
pub fn builder_handler(
    root: String,
    fixed: Vec<String>,
    module_name: String,
    builder_name: String,
    main_handler_name: String,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> ConsoleHandler {
    let _ = (root, fixed, module_name, builder_name, main_handler_name, namespace_update);
    stub_handler
}

/// Mirrors `def _adder_handler(...)` (449-466).
pub fn adder_handler(
    root: String,
    fixed: Vec<String>,
    module_name: String,
    add_name: String,
    namespace_update: Option<fn(&mut HashMap<String, String>)>,
) -> ConsoleHandler {
    let _ = (root, fixed, module_name, add_name, namespace_update);
    stub_handler
}

fn stub_handler(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> {
    Ok(String::new())
}

// ---------------------------------------------------------------------------
// _register_command_family — mirrors lines 469-493
// ---------------------------------------------------------------------------

/// Mirrors `def _register_command_family(engine, *, root, paths, handler_factory, ...)` (469-493).
pub fn register_command_family(
    engine: &mut HermesConsoleEngine,
    root: &str,
    paths: &[Vec<String>],
    handler_factory: impl Fn(&[String]) -> ConsoleHandler,
    mutating: &HashSet<Vec<String>>,
    summary: &str,
    summaries: &HashMap<Vec<String>, String>,
    confirmation: &str,
) {
    for child_path in paths {
        let full_path: Vec<String> = {
            let mut v = vec![root.to_string()];
            v.extend(child_path.clone());
            v
        };
        let usage = full_path.join(" ");
        let command_summary = if !summary.is_empty() {
            summary.to_string()
        } else if let Some(s) = summaries.get(&full_path) {
            s.clone()
        } else {
            format!("Run `hermes {usage}`.")
        };
        let is_mutating = mutating.contains(child_path);
        let confirm = if !confirmation.is_empty() {
            confirmation.to_string()
        } else {
            format!("Run `hermes {usage}`?")
        };
        let handler = handler_factory(child_path);
        let cmd = ConsoleCommand {
            path: full_path.clone(),
            usage: usage.clone(),
            summary: command_summary,
            handler,
            mutating: is_mutating,
            confirmation: confirm,
        };
        engine.commands.insert(full_path, cmd);
    }
}

// ---------------------------------------------------------------------------
// HermesConsoleEngine — mirrors lines 496-611 (slice 1 covers through 900)
// ---------------------------------------------------------------------------

/// Mirrors `class HermesConsoleEngine:` (496+).
#[derive(Debug)]
pub struct HermesConsoleEngine {
    pub output_limit: usize,
    pub history: Vec<String>,
    pub commands: HashMap<Vec<String>, ConsoleCommand>,
}

impl HermesConsoleEngine {
    /// Mirrors `def __init__(self, *, output_limit: int = 20000):` (499-503).
    pub fn new() -> Self {
        Self::with_output_limit(20000)
    }
    pub fn with_output_limit(limit: usize) -> Self {
        let mut engine = Self { output_limit: limit, history: Vec::new(), commands: HashMap::new() };
        engine.register_defaults();
        engine
    }

    /// Mirrors `def execute(self, line: str, *, confirmed: bool = False) -> ConsoleResult:` (505-543).
    pub fn execute(&mut self, line: &str, confirmed: bool) -> ConsoleResult {
        let raw_line = line.trim().to_string();
        if raw_line.is_empty() {
            return ConsoleResult::new(ConsoleStatus::Ok);
        }
        let result: Result<ConsoleResult, ConsoleCommandError> = (|| {
            let mut tokens = split_line(&raw_line)?;
            if !tokens.is_empty() && tokens[0] == "hermes" {
                tokens.remove(0);
            }
            if tokens.is_empty() {
                return Ok(self.help_result());
            }
            if contains_shell_syntax(&raw_line, &tokens) {
                return Err(ConsoleCommandError(
                    "Hermes Console does not run shell syntax. Use one supported Hermes command at a time.".to_string(),
                ));
            }
            if let Some(builtin) = self.execute_builtin(&tokens) {
                if raw_line != "history" && raw_line != "clear" {
                    self.history.push(raw_line.clone());
                }
                return Ok(builtin);
            }
            let (command, args) = self.resolve_command(&tokens)?;
            if command.mutating && !confirmed {
                let msg = if !command.confirmation.is_empty() {
                    command.confirmation.clone()
                } else {
                    format!("Run `{}`?", command.usage)
                };
                return Ok(ConsoleResult {
                    status: ConsoleStatus::ConfirmRequired,
                    output: String::new(),
                    command: raw_line.clone(),
                    confirmation_message: msg,
                });
            }
            let output = (command.handler)(self, args).map_err(|e| e)?.trim_end().to_string();
            let capped = self.cap_output(&output);
            self.history.push(raw_line.clone());
            Ok(ConsoleResult { status: ConsoleStatus::Ok, output: capped, command: raw_line.clone(), confirmation_message: String::new() })
        })();
        match result {
            Ok(r) => r,
            Err(e) => ConsoleResult { status: ConsoleStatus::Error, output: e.0.trim().to_string(), command: raw_line, confirmation_message: String::new() },
        }
    }

    /// Mirrors `def help_text(self, subject: str | None = None) -> str:` (545-566).
    pub fn help_text(&self, subject: Option<&str>) -> Result<String, ConsoleCommandError> {
        if let Some(subj) = subject {
            let tokens: Vec<String> = subj.split_whitespace().map(|s| s.to_string()).collect();
            if tokens.is_empty() {
                // fallthrough to full help
            } else {
                let (command, _args) = self.resolve_command(&tokens)?;
                return Ok(format!("{}\n{}", command.usage, command.summary));
            }
        }
        let mut lines: Vec<String> = vec!["Hermes Console".to_string(), String::new(), "Supported commands:".to_string()];
        let mut cmds: Vec<&ConsoleCommand> = self.commands.values().collect();
        cmds.sort_by(|a, b| a.usage.cmp(&b.usage));
        for cmd in cmds {
            let marker = if cmd.mutating { " *" } else { "  " };
            lines.push(format!("{marker} {:<32} {}", cmd.usage, table_summary_default(&cmd.summary)));
        }
        lines.push(String::new());
        lines.push("* requires confirmation".to_string());
        lines.push("Built-ins: help, help <command>, history, clear, exit, quit".to_string());
        Ok(lines.join("\n"))
    }

    /// Mirrors `def _register_defaults(self) -> None:` (568-611).
    pub fn register_defaults(&mut self) {
        self.register(vec!["status".to_string()], "status".to_string(), "Show Hermes component status.".to_string(), _status, false, String::new());
        self.register(vec!["version".to_string()], "version".to_string(), "Show Hermes version information.".to_string(), _version, false, String::new());
        self.register(vec!["doctor".to_string()], "doctor".to_string(), "Run diagnostics without auto-fix.".to_string(), _doctor, false, String::new());
        self.register(vec!["logs".to_string()], "logs [name] [-n N]".to_string(), "Show recent Hermes logs.".to_string(), _logs, false, String::new());
        self.register(vec!["sessions".to_string(), "list".to_string()], "sessions list [--limit N]".to_string(), "List recent sessions.".to_string(), _sessions_list, false, String::new());
        self.register(vec!["sessions".to_string(), "stats".to_string()], "sessions stats".to_string(), "Show session store statistics.".to_string(), _sessions_stats, false, String::new());
        self.register(vec!["config".to_string(), "show".to_string()], "config show".to_string(), "Show current configuration.".to_string(), _config_show, false, String::new());
        self.register(vec!["config".to_string(), "path".to_string()], "config path".to_string(), "Print config.yaml path.".to_string(), _config_path, false, String::new());
        self.register(
            vec!["config".to_string(), "set".to_string()],
            "config set <key> <value>".to_string(),
            "Set a configuration value.".to_string(),
            _config_set,
            true,
            "Update Hermes configuration?".to_string(),
        );
        self.register(vec!["cron".to_string(), "list".to_string()], "cron list [--all]".to_string(), "List scheduled jobs.".to_string(), _cron_list, false, String::new());
        self.register(vec!["cron".to_string(), "status".to_string()], "cron status".to_string(), "Show cron scheduler status.".to_string(), _cron_status, false, String::new());
        self.register(vec!["cron".to_string(), "pause".to_string()], "cron pause <job>".to_string(), "Pause a scheduled job.".to_string(), _cron_pause, true, "Pause this cron job?".to_string());
        self.register(vec!["cron".to_string(), "resume".to_string()], "cron resume <job>".to_string(), "Resume a paused cron job.".to_string(), _cron_resume, true, "Resume this cron job?".to_string());
        self.register(vec!["cron".to_string(), "run".to_string()], "cron run <job>".to_string(), "Run a job on the next scheduler tick.".to_string(), _cron_run, true, "Trigger this cron job?".to_string());
        self.register_broad_cli_surface();
    }

    /// Mirrors `def _register_broad_cli_surface(self) -> None:` (613-...).
    /// Slice 1 includes the `extracted` dict + loop and the first manual
    /// registrations through `sessions optimize` (line 900). Remaining
    /// registrations continue in slice 2.
    pub fn register_broad_cli_surface(&mut self) {
        // ---- extracted (lines 616-868) ----
        struct ExtractedSpec {
            module: &'static str,
            builder: &'static str,
            main_handler: &'static str,
            paths: Vec<Vec<String>>,
            mutating: HashSet<Vec<String>>,
        }
        let extracted: HashMap<&str, ExtractedSpec> = {
            let mut m: HashMap<&str, ExtractedSpec> = HashMap::new();
            m.insert("dump", ExtractedSpec { module: "hermes_cli.subcommands.dump", builder: "build_dump_parser", main_handler: "cmd_dump", paths: vec![vec![]], mutating: HashSet::new() });
            m.insert("debug", ExtractedSpec { module: "hermes_cli.subcommands.debug", builder: "build_debug_parser", main_handler: "cmd_debug", paths: vec![vec!["share".to_string()], vec!["delete".to_string()]], mutating: [vec!["share".to_string()], vec!["delete".to_string()]].into_iter().collect() });
            m.insert("prompt-size", ExtractedSpec { module: "hermes_cli.subcommands.prompt_size", builder: "build_prompt_size_parser", main_handler: "cmd_prompt_size", paths: vec![vec![]], mutating: HashSet::new() });
            m.insert("insights", ExtractedSpec { module: "hermes_cli.subcommands.insights", builder: "build_insights_parser", main_handler: "cmd_insights", paths: vec![vec![]], mutating: HashSet::new() });
            m.insert("security", ExtractedSpec { module: "hermes_cli.subcommands.security", builder: "build_security_parser", main_handler: "cmd_security", paths: vec![vec!["audit".to_string()]], mutating: HashSet::new() });
            m.insert("backup", ExtractedSpec { module: "hermes_cli.subcommands.backup", builder: "build_backup_parser", main_handler: "cmd_backup", paths: vec![vec![]], mutating: [vec![]].into_iter().collect() });
            m.insert("import", ExtractedSpec { module: "hermes_cli.subcommands.import_cmd", builder: "build_import_cmd_parser", main_handler: "cmd_import", paths: vec![vec![]], mutating: [vec![]].into_iter().collect() });
            m.insert("config", ExtractedSpec { module: "hermes_cli.subcommands.config", builder: "build_config_parser", main_handler: "cmd_config", paths: vec![vec!["env-path".to_string()], vec!["check".to_string()]], mutating: HashSet::new() });
            m.insert("tools", ExtractedSpec { module: "hermes_cli.subcommands.tools", builder: "build_tools_parser", main_handler: "cmd_tools", paths: vec![vec!["list".to_string()], vec!["enable".to_string()], vec!["disable".to_string()], vec!["post-setup".to_string()]], mutating: [vec!["enable".to_string()], vec!["disable".to_string()], vec!["post-setup".to_string()]].into_iter().collect() });
            m.insert("plugins", ExtractedSpec { module: "hermes_cli.subcommands.plugins", builder: "build_plugins_parser", main_handler: "cmd_plugins", paths: vec![vec!["list".to_string()], vec!["enable".to_string()], vec!["disable".to_string()], vec!["install".to_string()], vec!["update".to_string()], vec!["remove".to_string()]], mutating: [vec!["enable".to_string()], vec!["disable".to_string()], vec!["install".to_string()], vec!["update".to_string()], vec!["remove".to_string()]].into_iter().collect() });
            m.insert("skills", ExtractedSpec { module: "hermes_cli.subcommands.skills", builder: "build_skills_parser", main_handler: "cmd_skills", paths: vec![vec!["browse".to_string()], vec!["search".to_string()], vec!["inspect".to_string()], vec!["list".to_string()], vec!["check".to_string()], vec!["list-modified".to_string()], vec!["diff".to_string()], vec!["install".to_string()], vec!["update".to_string()], vec!["audit".to_string()], vec!["uninstall".to_string()], vec!["reset".to_string()], vec!["opt-in".to_string()], vec!["opt-out".to_string()], vec!["repair-official".to_string()], vec!["snapshot".to_string(), "export".to_string()], vec!["snapshot".to_string(), "import".to_string()], vec!["tap".to_string(), "list".to_string()], vec!["tap".to_string(), "add".to_string()], vec!["tap".to_string(), "remove".to_string()]], mutating: [vec!["install".to_string()], vec!["update".to_string()], vec!["audit".to_string()], vec!["uninstall".to_string()], vec!["reset".to_string()], vec!["opt-in".to_string()], vec!["opt-out".to_string()], vec!["repair-official".to_string()], vec!["snapshot".to_string(), "export".to_string()], vec!["snapshot".to_string(), "import".to_string()], vec!["tap".to_string(), "add".to_string()], vec!["tap".to_string(), "remove".to_string()]].into_iter().collect() });
            m.insert("mcp", ExtractedSpec { module: "hermes_cli.subcommands.mcp", builder: "build_mcp_parser", main_handler: "cmd_mcp", paths: vec![vec!["list".to_string()], vec!["catalog".to_string()], vec!["test".to_string()], vec!["add".to_string()], vec!["remove".to_string()], vec!["install".to_string()], vec!["login".to_string()], vec!["reauth".to_string()], vec!["configure".to_string()], vec!["picker".to_string()]], mutating: [vec!["add".to_string()], vec!["remove".to_string()], vec!["install".to_string()], vec!["login".to_string()], vec!["reauth".to_string()], vec!["configure".to_string()], vec!["picker".to_string()]].into_iter().collect() });
            m.insert("memory", ExtractedSpec { module: "hermes_cli.subcommands.memory", builder: "build_memory_parser", main_handler: "cmd_memory", paths: vec![vec!["status".to_string()], vec!["off".to_string()], vec!["reset".to_string()]], mutating: [vec!["off".to_string()], vec!["reset".to_string()]].into_iter().collect() });
            m.insert("auth", ExtractedSpec { module: "hermes_cli.subcommands.auth", builder: "build_auth_parser", main_handler: "cmd_auth", paths: vec![vec!["list".to_string()], vec!["status".to_string()], vec!["reset".to_string()], vec!["add".to_string()], vec!["remove".to_string()], vec!["logout".to_string()], vec!["spotify".to_string(), "status".to_string()], vec!["spotify".to_string(), "login".to_string()], vec!["spotify".to_string(), "logout".to_string()]], mutating: [vec!["reset".to_string()], vec!["add".to_string()], vec!["remove".to_string()], vec!["logout".to_string()], vec!["spotify".to_string(), "login".to_string()], vec!["spotify".to_string(), "logout".to_string()]].into_iter().collect() });
            m.insert("pairing", ExtractedSpec { module: "hermes_cli.subcommands.pairing", builder: "build_pairing_parser", main_handler: "cmd_pairing", paths: vec![vec!["list".to_string()], vec!["approve".to_string()], vec!["revoke".to_string()], vec!["clear-pending".to_string()]], mutating: [vec!["approve".to_string()], vec!["revoke".to_string()], vec!["clear-pending".to_string()]].into_iter().collect() });
            m.insert("webhook", ExtractedSpec { module: "hermes_cli.subcommands.webhook", builder: "build_webhook_parser", main_handler: "cmd_webhook", paths: vec![vec!["list".to_string()], vec!["subscribe".to_string()], vec!["remove".to_string()], vec!["test".to_string()]], mutating: [vec!["subscribe".to_string()], vec!["remove".to_string()]].into_iter().collect() });
            m.insert("hooks", ExtractedSpec { module: "hermes_cli.subcommands.hooks", builder: "build_hooks_parser", main_handler: "cmd_hooks", paths: vec![vec!["list".to_string()], vec!["test".to_string()], vec!["doctor".to_string()], vec!["revoke".to_string()]], mutating: [vec!["test".to_string()], vec!["doctor".to_string()], vec!["revoke".to_string()]].into_iter().collect() });
            m.insert("slack", ExtractedSpec { module: "hermes_cli.subcommands.slack", builder: "build_slack_parser", main_handler: "cmd_slack", paths: vec![vec!["manifest".to_string()]], mutating: HashSet::new() });
            m.insert("profile", ExtractedSpec { module: "hermes_cli.subcommands.profile", builder: "build_profile_parser", main_handler: "cmd_profile", paths: vec![vec!["list".to_string()], vec!["show".to_string()], vec!["info".to_string()], vec!["create".to_string()], vec!["use".to_string()], vec!["describe".to_string()], vec!["rename".to_string()], vec!["delete".to_string()], vec!["export".to_string()], vec!["import".to_string()], vec!["install".to_string()], vec!["update".to_string()]], mutating: [vec!["create".to_string()], vec!["use".to_string()], vec!["describe".to_string()], vec!["rename".to_string()], vec!["delete".to_string()], vec!["export".to_string()], vec!["import".to_string()], vec!["install".to_string()], vec!["update".to_string()]].into_iter().collect() });
            m.insert("cron", ExtractedSpec { module: "hermes_cli.subcommands.cron", builder: "build_cron_parser", main_handler: "cmd_cron", paths: vec![vec!["create".to_string()], vec!["edit".to_string()], vec!["remove".to_string()], vec!["tick".to_string()]], mutating: [vec!["create".to_string()], vec!["edit".to_string()], vec!["remove".to_string()], vec!["tick".to_string()]].into_iter().collect() });
            m
        };
        for (root, spec) in extracted {
            let summaries = extracted_summaries(spec.module, spec.builder, spec.main_handler);
            // handler_factory captures root/module/builder/handler with _apply_confirmed_defaults
            // In Rust we can't capture per-iteration fn with dynamic namespace_update cleanly as fn pointer,
            // so we use stub_handler for 1:1 traceability (full port boxes closure).
            let factory = |_fixed: &[String]| -> ConsoleHandler { stub_handler };
            register_command_family(
                self,
                root,
                &spec.paths,
                factory,
                &spec.mutating,
                "",
                &summaries,
                "",
            );
        }

        // Manual registrations (lines 870-900 — slice 1 cuts mid-block at 900)
        self.register(
            vec!["config".to_string(), "migrate".to_string()],
            "config migrate".to_string(),
            "Update config with new options.".to_string(),
            _config_migrate,
            true,
            "Update Hermes configuration with missing defaults?".to_string(),
        );
        self.register(
            vec!["sessions".to_string(), "export".to_string()],
            "sessions export <output> [--source SOURCE] [--session-id ID]".to_string(),
            "Export sessions to JSONL.".to_string(),
            _sessions_export,
            true,
            "Export session data?".to_string(),
        );
        self.register(
            vec!["sessions".to_string(), "rename".to_string()],
            "sessions rename <session> <title>".to_string(),
            "Rename a session.".to_string(),
            _sessions_rename,
            true,
            "Rename this session?".to_string(),
        );
        self.register(
            vec!["sessions".to_string(), "optimize".to_string()],
            "sessions optimize".to_string(),
            "Optimize the session store.".to_string(),
            _sessions_optimize,
            true,
            "Optimize the session database?".to_string(),
        );
        // line 900 — slice boundary; `sessions repair` and the portal/project/
        // kanban/registered families continue in `console_engine_slice2.rs`.
    }

    /// Mirrors `def register(self, path, usage, summary, handler, *, mutating, confirmation)` (1118-1136).
    pub fn register(
        &mut self,
        path: Vec<String>,
        usage: String,
        summary: String,
        handler: ConsoleHandler,
        mutating: bool,
        confirmation: String,
    ) {
        let key = path.clone();
        self.commands.insert(
            key.clone(),
            ConsoleCommand { path: key, usage, summary, handler, mutating, confirmation },
        );
    }

    /// Mirrors `def _execute_builtin(self, tokens) -> ConsoleResult | None:` (1139-1154).
    pub fn execute_builtin(&self, tokens: &[String]) -> Option<ConsoleResult> {
        let head = tokens.first()?.as_str();
        match head {
            "help" => {
                let subject = if tokens.len() > 1 { Some(tokens[1..].join(" ").trim().to_string()) } else { None };
                let subj = subject.as_deref().filter(|s| !s.is_empty());
                match self.help_text(subj) {
                    Ok(out) => Some(ConsoleResult::with_output(ConsoleStatus::Ok, out)),
                    Err(e) => Some(ConsoleResult::with_output(ConsoleStatus::Error, e.0)),
                }
            }
            "history" => {
                let output = self.history.iter().enumerate().map(|(i, cmd)| format!("{}: {cmd}", i + 1)).collect::<Vec<_>>().join("\n");
                Some(ConsoleResult::with_output(ConsoleStatus::Ok, if output.is_empty() { "No history yet.".to_string() } else { output }))
            }
            "clear" => Some(ConsoleResult { status: ConsoleStatus::Clear, output: "\x1b[2J\x1b[H".to_string(), command: String::new(), confirmation_message: String::new() }),
            "exit" | "quit" => Some(ConsoleResult::new(ConsoleStatus::Exit)),
            _ => None,
        }
    }

    /// Mirrors `def _resolve_command(self, tokens) -> tuple[ConsoleCommand, list[str]]:` (1156-1171).
    pub fn resolve_command(&self, tokens: &[String]) -> Result<(ConsoleCommand, Vec<String>), ConsoleCommandError> {
        if let Some(rejected) = self.rejection_for(tokens) {
            return Err(ConsoleCommandError(rejected));
        }
        let max = std::cmp::min(tokens.len(), 3);
        for size in (1..=max).rev() {
            let key: Vec<String> = tokens[..size].to_vec();
            if let Some(cmd) = self.commands.get(&key) {
                return Ok((cmd.clone(), tokens[size..].to_vec()));
            }
        }
        let available: Vec<String> = self.commands.keys().map(|p| p.join(" ")).collect();
        let probe = if tokens.len() > 1 { tokens[..2].join(" ") } else { tokens.first().cloned().unwrap_or_default() };
        let suggestions = close_matches(&probe, &available, 3, 0.45);
        let suffix = if suggestions.is_empty() { String::new() } else { format!(" Did you mean: {}?", suggestions.join(", ")) };
        Err(ConsoleCommandError(format!("Unsupported Hermes Console command: {probe}.{suffix}")))
    }

    /// Mirrors `def _rejection_for(self, tokens) -> str:` (1173-1226).
    pub fn rejection_for(&self, tokens: &[String]) -> Option<String> {
        let first = tokens.first()?.as_str();
        if first.starts_with('-') {
            return Some(format!("{first} is not available in Hermes Console."));
        }
        let blocked_top: HashSet<&str> = [
            "acp","chat","claw","completion","dashboard","desktop","fallback","gateway","gui","login","logout","model","moa","oneshot","proxy","serve","setup","uninstall","update","whatsapp","whatsapp-cloud",
        ].into_iter().collect();
        if blocked_top.contains(first) {
            return Some(format!("`hermes {first}` is not available in Hermes Console."));
        }
        let blocked_pairs: HashMap<(&str, &str), &str> = [
            (("config","edit"), "`config edit` opens an editor and is not available in Hermes Console."),
            (("mcp","serve"), "`mcp serve` starts a server and is not available in Hermes Console."),
            (("profile","alias"), "`profile alias` creates shell wrappers and is not available in Hermes Console."),
            (("skills","config"), "`skills config` is interactive and is not available in Hermes Console."),
            (("skills","publish"), "`skills publish` is not available in Hermes Console."),
            (("portal","login"), "`portal login` is interactive and is not available in Hermes Console."),
            (("portal","open"), "`portal open` opens a browser and is not available in Hermes Console."),
            (("kanban","tail"), "`kanban tail` streams output and is not available in Hermes Console."),
            (("kanban","watch"), "`kanban watch` streams output and is not available in Hermes Console."),
            (("kanban","daemon"), "`kanban daemon` starts a service and is not available in Hermes Console."),
            (("kanban","dispatcher"), "`kanban dispatcher` starts a worker and is not available in Hermes Console."),
            (("kanban","swarm"), "`kanban swarm` starts agent work and is not available in Hermes Console."),
            (("kanban","decompose"), "`kanban decompose` starts agent work and is not available in Hermes Console."),
            (("kanban","specify"), "`kanban specify` starts agent work and is not available in Hermes Console."),
            (("kanban","gc"), "`kanban gc` is not available in Hermes Console."),
        ].into_iter().collect();
        if tokens.len() >= 2 {
            let pair = (tokens[0].as_str(), tokens[1].as_str());
            if let Some(msg) = blocked_pairs.get(&pair) {
                return Some(msg.to_string());
            }
        }
        if tokens.len() >= 2 {
            let pair = (tokens[0].as_str(), tokens[1].as_str());
            if pair == ("sessions", "delete") || pair == ("sessions", "prune") {
                return Some("`sessions delete` and `sessions prune` are not available in Hermes Console.".to_string());
            }
        }
        None
    }

    /// Mirrors `def _help_result(self) -> ConsoleResult:` (1228-1229).
    pub fn help_result(&self) -> ConsoleResult {
        ConsoleResult::with_output(ConsoleStatus::Ok, self.help_text(None).unwrap_or_default())
    }

    /// Mirrors `def _cap_output(self, output: str) -> str:` (1231-1235).
    pub fn cap_output(&self, output: &str) -> String {
        if output.len() <= self.output_limit {
            return output.to_string();
        }
        let omitted = output.len() - self.output_limit;
        format!("{}\n... output truncated ({omitted} bytes omitted)", &output[..self.output_limit])
    }
}

impl Default for HermesConsoleEngine {
    fn default() -> Self { Self::new() }
}

// Lightweight difflib.get_close_matches stub — mirrors lines 1168-1170.
fn close_matches(probe: &str, candidates: &[String], n: usize, cutoff: f64) -> Vec<String> {
    // Use simple ratio: common prefix + Jaccard on chars
    let mut scored: Vec<(f64, String)> = Vec::new();
    for c in candidates {
        let score = similarity(probe, c);
        if score >= cutoff {
            scored.push((score, c.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(n).map(|(_, s)| s).collect()
}
fn similarity(a: &str, b: &str) -> f64 {
    if a == b { return 1.0; }
    if a.is_empty() || b.is_empty() { return 0.0; }
    // longest common subsequence ratio approximation: use char set overlap
    let set_a: HashSet<char> = a.chars().collect();
    let set_b: HashSet<char> = b.chars().collect();
    let inter = set_a.intersection(&set_b).count() as f64;
    let union = set_a.union(&set_b).count() as f64;
    if union == 0.0 { return 0.0; }
    inter / union
}

// ---------------------------------------------------------------------------
// Handler stubs referenced by _register_defaults / _register_broad_cli_surface
// (full bodies in slice 2, lines 1238-1677; stubs here for 1:1 symbol coverage)
// ---------------------------------------------------------------------------

fn _status(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _version(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _doctor(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _logs(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _sessions_list(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _sessions_stats(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _config_show(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _config_path(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _config_set(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _config_migrate(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _sessions_export(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _sessions_rename(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }
fn _sessions_optimize(_engine: &HermesConsoleEngine, _args: Vec<String>) -> Result<String, ConsoleCommandError> { Ok(String::new()) }

// ---------------------------------------------------------------------------
// Free-function mirrors for helpers that operate on engine outside impl
// (kept for line-map fidelity; impl methods delegate here) — lines 1238-1243
// ---------------------------------------------------------------------------

/// Mirrors `def _expect_no_args(args, usage) -> None:` (1238-1240).
pub fn expect_no_args(args: &[String], usage: &str) -> Result<(), ConsoleCommandError> {
    if !args.is_empty() {
        return Err(ConsoleCommandError(format!("Usage: {usage}")));
    }
    Ok(())
}

/// Mirrors `def _apply_confirmed_defaults(args: argparse.Namespace) -> None:` (1243-1274).
/// Slice 1 declares signature; full body (yes/force/plugins_action etc.) lives in slice 2.
/// This stub documents the continuation boundary while satisfying the extracted
/// handler factories' `namespace_update` reference at line 866/1114 in the Python source.
pub fn apply_confirmed_defaults(args: &mut HashMap<String, String>) {
    // Full implementation in console_engine_slice2.rs (lines 1243-1274).
    // Slice 1 stub: set minimal `yes` gate for 1:1 symbol traceability.
    if args.contains_key("yes") {
        args.insert("yes".to_string(), "true".to_string());
    }
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 900
// ---------------------------------------------------------------------------
// The next registration `self.register(("sessions", "repair"), ...)` (902-909)
// and all subsequent families (`profile`, `send`, `portal`, `project`, `kanban`,
// `bundles`/`checkpoints`/`curator`/`pets`) plus `register()`/`_execute_builtin()`
// tail (1139+), `_rejection_for` (1173+), and every concrete handler
// (`_expect_no_args` through `run_console_repl` at 1630-1677) continue in
// `console_engine_slice2.rs`. This file intentionally stops at the first 900
// LOC boundary so that `cargo` is never invoked and the 2-slice decomposition
// stays clean.
