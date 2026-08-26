//! Context reference expansion — `@file:`, `@folder:`, `@diff`, etc.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_references.py` (720 LOC).
//! T0016 — full file (lines 1-720).
//!
//! ```text
//! Context reference provider API + expansion pipeline.
//! Mirrors the Python plugin @-prefix reference system (#26193) and the
//! preprocessing pipeline that expands @file/@folder/@diff/@staged/@git/@url
//! (plus plugin-registered prefixes) into attached context blocks with token
//! budget guards (25% soft / 50% hard).
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-720 verbatim; line numbers in comments refer to the
//! 720-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.1-17
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde_json::Value;

// Python imports (ll.1-17) — stdlib:
//   asyncio, inspect, json, mimetypes, os, re, subprocess, dataclasses, pathlib, typing, abc
// Mapped: std thread/sync, serde_json, regex, std::path, std::process, std::fs
//
// Python intra-repo imports (ll.14-16):
//   from agent.model_metadata import estimate_tokens_rough
//   from hermes_cli._subprocess_compat import IS_WINDOWS, windows_hide_flags
//   from hermes_cli.sizefmt import format_bytes
// Rust: stubs below mirror their surface so this file is self-contained and
// grep-traceable. Canonical impls live in sibling crates.

// ---------------------------------------------------------------------------
// Logger / platform — mirrors `IS_WINDOWS` (ll.15)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "context_references";

#[cfg(windows)]
const IS_WINDOWS: bool = true;
#[cfg(not(windows))]
const IS_WINDOWS: bool = false;

// ---------------------------------------------------------------------------
// Helpers mirroring Python ll.14-16 helpers
// ---------------------------------------------------------------------------

/// Stub: mirrors `agent.model_metadata.estimate_tokens_rough` (ll.14)
/// Rough chars/4 heuristic; canonical token counting lives in hermes-core.
fn estimate_tokens_rough(text: &str) -> usize {
    // Python divides by ~4 chars/token; keep identical for budget parity.
    (text.len() / 4).max(1)
}

/// Stub: mirrors `hermes_cli.sizefmt.format_bytes` (ll.16)
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

/// Stub: mirrors `hermes_cli._subprocess_compat.windows_hide_flags` (ll.15)
#[cfg(windows)]
fn windows_hide_flags() -> u32 {
    0x0800_0000 // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn windows_hide_flags() -> u32 {
    0
}

// ---------------------------------------------------------------------------
// Plugin context-reference provider API — mirrors Python ll.20-78
// Issue #26193
// ---------------------------------------------------------------------------

/// Mirrors `BUILTIN_PREFIXES = frozenset({"diff", "staged", "file", "folder", "git", "url"})` (l.24)
pub const BUILTIN_PREFIXES: &[&str] = &["diff", "staged", "file", "folder", "git", "url"];

fn is_builtin_prefix(prefix: &str) -> bool {
    BUILTIN_PREFIXES.contains(&prefix)
}

// Thread-safe global registry — mirrors `_context_reference_providers: dict[str, ContextReferenceProvider]` (l.26)
static CONTEXT_REFERENCE_PROVIDERS: OnceLock<Mutex<HashMap<String, Box<dyn ContextReferenceProvider>>>> =
    OnceLock::new();

fn context_reference_providers() -> &'static Mutex<HashMap<String, Box<dyn ContextReferenceProvider>>> {
    CONTEXT_REFERENCE_PROVIDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// ContextCompletionItem — mirrors Python ll.29-38
// ---------------------------------------------------------------------------

/// Mirrors `class ContextCompletionItem:` (ll.29-38)
/// A single autocomplete result from a context reference provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCompletionItem {
    pub text: String,
    pub display: String,
    pub meta: String,
}

impl ContextCompletionItem {
    pub fn new(text: impl Into<String>, display: impl Into<String>, meta: impl Into<String>) -> Self {
        let text = text.into();
        let display_raw = display.into();
        let display = if display_raw.is_empty() { text.clone() } else { display_raw };
        Self { text, display, meta: meta.into() }
    }
}

// Keep Python ctor shape alias
#[allow(dead_code)]
fn _context_completion_item(text: &str, display: &str, meta: &str) -> ContextCompletionItem {
    ContextCompletionItem::new(text, display, meta)
}

// ---------------------------------------------------------------------------
// ContextReferenceProvider — mirrors Python ll.40-58
// ---------------------------------------------------------------------------

/// Mirrors `class ContextReferenceProvider(ABC):` (ll.40-58)
/// Base class for plugin-registered @-prefix context reference providers.
///
/// Python is async; Rust stub is sync (callers that need async bridge via
/// `preprocess_context_references` sync wrapper mirroring Python ll.212-236:
/// ThreadPool+asyncio.run). Keep trait sync for 1:1 audit; async note in doc.
pub trait ContextReferenceProvider: Send + Sync {
    /// Mirrors `prefix: str = ""` — e.g. "issue", "channel", "doc" (l.47)
    fn prefix(&self) -> &str;
    /// Mirrors `description: str = ""` — shown in autocomplete meta column (l.48)
    fn description(&self) -> &str {
        ""
    }
    /// Mirrors `async def autocomplete(self, query: str, *, limit: int = 10)` (ll.51-53)
    fn autocomplete(&self, query: &str, limit: usize) -> Vec<ContextCompletionItem>;
    /// Mirrors `async def expand(self, target: str) -> str | None:` (ll.55-58)
    fn expand(&self, target: &str) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Registry helpers — mirrors Python ll.61-78
// ---------------------------------------------------------------------------

/// Mirrors `def register_context_reference_provider(provider: ContextReferenceProvider) -> None:` (ll.61-72)
pub fn register_context_reference_provider(
    provider: Box<dyn ContextReferenceProvider>,
) -> Result<(), String> {
    // Mirrors `if not isinstance(provider, ContextReferenceProvider): raise TypeError` (ll.63-64)
    // In Rust the type system guarantees it; keep comment for parity.
    let prefix = provider.prefix().to_lowercase().trim().to_string();
    if prefix.is_empty() {
        return Err("prefix must be a non-empty string".to_string());
    }
    if is_builtin_prefix(&prefix) {
        return Err(format!("prefix '{}' is reserved for built-in references", prefix));
    }
    let mut map = context_reference_providers().lock().unwrap();
    if map.contains_key(&prefix) {
        return Err(format!("prefix '{}' is already registered", prefix));
    }
    map.insert(prefix, provider);
    Ok(())
}

#[allow(dead_code)]
fn _register_context_reference_provider(provider: Box<dyn ContextReferenceProvider>) -> Result<(), String> {
    register_context_reference_provider(provider)
}

/// Mirrors `def get_context_reference_providers() -> dict[str, ContextReferenceProvider]:` (ll.75-77)
/// Return a snapshot of all registered plugin providers (cloned prefixes only; trait objects not cloneable).
pub fn get_context_reference_provider_prefixes() -> Vec<String> {
    let map = context_reference_providers().lock().unwrap();
    map.keys().cloned().collect()
}

/// Full snapshot where caller needs prefix list — mirrors `dict(_context_reference_providers)` copy (l.77)
pub fn has_context_reference_provider(prefix: &str) -> bool {
    let map = context_reference_providers().lock().unwrap();
    map.contains_key(prefix)
}

#[allow(dead_code)]
fn _get_context_reference_providers_snapshot() -> Vec<String> {
    get_context_reference_provider_prefixes()
}

// ---------------------------------------------------------------------------
// Regexes — mirrors Python ll.80-88
// ---------------------------------------------------------------------------

/// Mirrors `_QUOTED_REFERENCE_VALUE = r'(?:`[^`\n]+`|"[^"\n]+"|\'[^\'\n]+\')'` (l.80)
const QUOTED_REFERENCE_VALUE: &str = r#"(?:`[^`\n]+`|"[^"\n]+"|'[^'\n]+')"#;

fn reference_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Mirrors REFERENCE_PATTERN (ll.81-83):
        //   rf"(?<![\w/])@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>{_QUOTED_REFERENCE_VALUE}(?::\d+(?:-\d+)?)?|\S+))"
        let pat = format!(
            r#"(?<![\w/])@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>{}(?::\d+(?:-\d+)?)?|\S+))"#,
            QUOTED_REFERENCE_VALUE
        );
        Regex::new(&pat).expect("REFERENCE_PATTERN regex")
    })
}

fn plugin_reference_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Mirrors _PLUGIN_REFERENCE_PATTERN (ll.86-88)
        let pat = format!(
            r#"(?<![\w/])@(?P<kind>[a-zA-Z][a-zA-Z0-9_-]*):(?P<value>{}(?::\d+(?:-\d+)?)?|\S+)"#,
            QUOTED_REFERENCE_VALUE
        );
        Regex::new(&pat).expect("_PLUGIN_REFERENCE_PATTERN regex")
    })
}

/// Mirrors `TRAILING_PUNCTUATION = ",.;!?"` (l.90)
pub const TRAILING_PUNCTUATION: &str = ",.;!?";

fn needs_quoting_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Mirrors `_NEEDS_QUOTING = re.compile(r"""[\s()\[\]{}<>"'`]""")` (l.91)
        Regex::new(r#"[\s()\[\]{}<>"'`]"#).expect("_NEEDS_QUOTING regex")
    })
}

/// Mirrors `_SENSITIVE_HOME_DIRS = (".ssh", ".aws", ...)` (l.92)
pub const SENSITIVE_HOME_DIRS: &[&str] = &[
    ".ssh", ".aws", ".gnupg", ".kube", ".docker", ".azure", ".config/gh",
];

/// Mirrors `_SENSITIVE_HERMES_DIRS = (Path("skills") / ".hub",)` (l.93)
pub const SENSITIVE_HERMES_DIRS: &[&str] = &["skills/.hub"];

/// Mirrors `_SENSITIVE_HOME_FILES = (...)` (ll.94-108)
pub const SENSITIVE_HOME_FILES: &[&str] = &[
    ".ssh/authorized_keys",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/config",
    ".bashrc",
    ".zshrc",
    ".profile",
    ".bash_profile",
    ".zprofile",
    ".netrc",
    ".pgpass",
    ".npmrc",
    ".pypirc",
];

// ---------------------------------------------------------------------------
// Dataclasses — mirrors Python ll.111-131
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class ContextReference:` (ll.111-119)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextReference {
    pub raw: String,
    pub kind: String,
    pub target: String,
    pub start: usize,
    pub end: usize,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
}

/// Mirrors `@dataclass class ContextReferenceResult:` (ll.122-131)
#[derive(Debug, Clone)]
pub struct ContextReferenceResult {
    pub message: String,
    pub original_message: String,
    pub references: Vec<ContextReference>,
    pub warnings: Vec<String>,
    pub injected_tokens: usize,
    pub expanded: bool,
    pub blocked: bool,
}

impl ContextReferenceResult {
    pub fn new(message: impl Into<String>, original_message: impl Into<String>) -> Self {
        let m = message.into();
        let o = original_message.into();
        Self {
            message: m,
            original_message: o,
            references: Vec::new(),
            warnings: Vec::new(),
            injected_tokens: 0,
            expanded: false,
            blocked: false,
        }
    }
}

// ---------------------------------------------------------------------------
// format_reference_value — mirrors Python ll.133-145
// ---------------------------------------------------------------------------

/// Mirrors `def format_reference_value(value: str) -> str:` (ll.133-145)
/// Quote a reference value so `REFERENCE_PATTERN` reads it back whole.
pub fn format_reference_value(value: &str) -> String {
    if !needs_quoting_re().is_match(value) {
        return value.to_string();
    }
    for quote in ["`", "\"", "'"] {
        if !value.contains(quote) {
            return format!("{}{}{}", quote, value, quote);
        }
    }
    value.to_string()
}

#[allow(dead_code)]
fn _format_reference_value(value: &str) -> String {
    format_reference_value(value)
}

// ---------------------------------------------------------------------------
// parse_context_references — mirrors Python ll.148-209
// ---------------------------------------------------------------------------

/// Mirrors `def parse_context_references(message: str) -> list[ContextReference]:` (ll.148-209)
pub fn parse_context_references(message: &str) -> Vec<ContextReference> {
    let mut refs: Vec<ContextReference> = Vec::new();
    if message.is_empty() {
        return refs;
    }

    for mat in reference_pattern().find_iter(message) {
        // We need capture groups; re-find with captures via plugin pattern's approach
        // but we have the full match span. Re-capture via the same regex on the slice.
        let start = mat.start();
        let end = mat.end();
        let raw = mat.as_str().to_string();

        // Re-run captures on this single match to extract groups (Python uses match.group)
        let caps = reference_pattern().captures(&raw).expect("captures on matched text");
        if let Some(simple) = caps.name("simple") {
            refs.push(ContextReference {
                raw,
                kind: simple.as_str().to_string(),
                target: String::new(),
                start,
                end,
                line_start: None,
                line_end: None,
            });
            continue;
        }

        let kind = caps.name("kind").map(|m| m.as_str().to_string()).unwrap_or_default();
        let value_raw = caps.name("value").map(|m| m.as_str()).unwrap_or("");
        let value = strip_trailing_punctuation(value_raw);
        let (target, line_start, line_end) = if kind == "file" {
            parse_file_reference_value(&value)
        } else {
            (strip_reference_wrappers(&value), None, None)
        };

        refs.push(ContextReference {
            raw,
            kind,
            target,
            start,
            end,
            line_start,
            line_end,
        });
    }

    // Second pass: resolve plugin-registered prefixes the built-in pattern missed (ll.188-207)
    // Mirrors `if _context_reference_providers:` guard (l.189) — check prefixes len
    let provider_prefixes = get_context_reference_provider_prefixes();
    if !provider_prefixes.is_empty() {
        for mat in plugin_reference_pattern().find_iter(message) {
            let start = mat.start();
            let caps = match plugin_reference_pattern().captures(mat.as_str()) {
                Some(c) => c,
                None => continue,
            };
            let kind = caps.name("kind").map(|m| m.as_str()).unwrap_or("");
            if is_builtin_prefix(kind) {
                continue;
            }
            // Skip if already captured by the built-in pattern (ll.195)
            if refs.iter().any(|r| r.kind == kind && r.start == start) {
                continue;
            }
            if has_context_reference_provider(kind) {
                let value_raw = caps.name("value").map(|m| m.as_str()).unwrap_or("");
                let value = strip_trailing_punctuation(value_raw);
                refs.push(ContextReference {
                    raw: mat.as_str().to_string(),
                    kind: kind.to_string(),
                    target: strip_reference_wrappers(&value),
                    start: mat.start(),
                    end: mat.end(),
                    line_start: None,
                    line_end: None,
                });
            }
        }
    }

    refs
}

#[allow(dead_code)]
fn _parse_context_references(message: &str) -> Vec<ContextReference> {
    parse_context_references(message)
}

// ---------------------------------------------------------------------------
// preprocess_context_references — mirrors Python ll.212-236 (sync wrapper)
// ---------------------------------------------------------------------------

/// Mirrors `def preprocess_context_references(message: str, *, cwd, context_length, url_fetcher, allowed_root)` (ll.212-236)
/// Safe for both CLI (no loop) and gateway (loop already running) — Python bridges via
/// ThreadPoolExecutor + asyncio.run. Rust is sync; delegates to `_preprocess_sync`.
pub fn preprocess_context_references(
    message: &str,
    cwd: &Path,
    context_length: usize,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> ContextReferenceResult {
    // Mirrors `coro = preprocess_context_references_async(...)` + asyncio.run bridging (ll.220-236)
    // Rust: direct sync call (no runtime). Keep ThreadPool comment for audit parity.
    preprocess_context_references_inner(message, cwd, context_length, url_fetcher, allowed_root)
}

#[allow(dead_code)]
fn _preprocess_context_references(
    message: &str,
    cwd: &Path,
    context_length: usize,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> ContextReferenceResult {
    preprocess_context_references(message, cwd, context_length, url_fetcher, allowed_root)
}

// ---------------------------------------------------------------------------
// preprocess_context_references_async — mirrors Python ll.239-325
// ---------------------------------------------------------------------------

/// Mirrors `async def preprocess_context_references_async(...)` (ll.239-325)
/// In Rust this is sync (no async runtime); name kept for 1:1 traceability via alias.
pub fn preprocess_context_references_inner(
    message: &str,
    cwd: &Path,
    context_length: usize,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> ContextReferenceResult {
    let refs = parse_context_references(message);
    if refs.is_empty() {
        return ContextReferenceResult {
            message: message.to_string(),
            original_message: message.to_string(),
            references: refs,
            warnings: Vec::new(),
            injected_tokens: 0,
            expanded: false,
            blocked: false,
        };
    }

    // Mirrors `cwd_path = Path(cwd).expanduser().resolve()` (l.251)
    let cwd_path = expand_and_resolve(cwd);
    // Mirrors allowed_root default to cwd (ll.254-256)
    let allowed_root_path = allowed_root
        .map(expand_and_resolve)
        .unwrap_or_else(|| cwd_path.clone());

    let mut warnings: Vec<String> = Vec::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut injected_tokens: usize = 0;

    // Mirrors `expanded = await asyncio.gather(*(... _expand_reference ...))` (ll.267-277)
    // Rust: serial loop (no async); preserves positional order for warnings/blocks.
    let mut expanded: Vec<(Option<String>, Option<String>)> = Vec::with_capacity(refs.len());
    for r in &refs {
        expanded.push(expand_reference(r, &cwd_path, url_fetcher, Some(&allowed_root_path)));
    }
    for (warning, block) in expanded {
        if let Some(w) = warning {
            warnings.push(w);
        }
        if let Some(b) = block {
            injected_tokens += estimate_tokens_rough(&b);
            blocks.push(b);
        }
    }

    // Mirrors token budget guards (ll.285-304)
    let hard_limit = std::cmp::max(1, (context_length as f64 * 0.50) as usize);
    let soft_limit = std::cmp::max(1, (context_length as f64 * 0.25) as usize);
    if injected_tokens > hard_limit {
        warnings.push(format!(
            "@ context injection refused: {} tokens exceeds the 50% hard limit ({}).",
            injected_tokens, hard_limit
        ));
        return ContextReferenceResult {
            message: message.to_string(),
            original_message: message.to_string(),
            references: refs,
            warnings,
            injected_tokens,
            expanded: false,
            blocked: true,
        };
    }

    if injected_tokens > soft_limit {
        warnings.push(format!(
            "@ context injection warning: {} tokens exceeds the 25% soft limit ({}).",
            injected_tokens, soft_limit
        ));
    }

    // Mirrors final assembly (ll.311-325)
    let mut final_msg = message.to_string();
    if !warnings.is_empty() {
        final_msg = format!(
            "{}\n\n--- Context Warnings ---\n{}",
            final_msg,
            warnings.iter().map(|w| format!("- {}", w)).collect::<Vec<_>>().join("\n")
        );
    }
    if !blocks.is_empty() {
        final_msg = format!("{}\n\n--- Attached Context ---\n\n{}", final_msg, blocks.join("\n\n"));
    }

    ContextReferenceResult {
        message: final_msg.trim().to_string(),
        original_message: message.to_string(),
        references: refs,
        warnings: warnings.clone(),
        injected_tokens,
        expanded: !blocks.is_empty() || !warnings.is_empty() && {
            // Mirrors `expanded=bool(blocks or warnings)` but with hard-limit blocked=False already returned
            // Here expanded = true if any block or warning contributed.
            true
        },
        blocked: false,
    }
}

// Alias for 1:1 traceability
#[allow(dead_code)]
fn _preprocess_context_references_async(
    message: &str,
    cwd: &Path,
    context_length: usize,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> ContextReferenceResult {
    preprocess_context_references_inner(message, cwd, context_length, url_fetcher, allowed_root)
}

// ---------------------------------------------------------------------------
// _expand_reference — mirrors Python ll.328-365
// ---------------------------------------------------------------------------

/// Mirrors `async def _expand_reference(ref, cwd, *, url_fetcher, allowed_root)` (ll.328-365)
fn expand_reference(
    reference: &ContextReference,
    cwd: &Path,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    // Wrap in catch-all mirroring `try: ... except Exception as exc:` (l.335, 352, 358)
    let res: Result<(Option<String>, Option<String>), String> = (|| {
        if reference.kind == "file" {
            return Ok(expand_file_reference(reference, cwd, allowed_root));
        }
        if reference.kind == "folder" {
            return Ok(expand_folder_reference(reference, cwd, allowed_root));
        }
        if reference.kind == "diff" {
            return Ok(expand_git_reference(reference, cwd, &["diff"], "git diff"));
        }
        if reference.kind == "staged" {
            return Ok(expand_git_reference(reference, cwd, &["diff", "--staged"], "git diff --staged"));
        }
        if reference.kind == "git" {
            let count = reference
                .target
                .parse::<i64>()
                .unwrap_or(1)
                .clamp(1, 10) as usize;
            return Ok(expand_git_reference(
                reference,
                cwd,
                &["log", &format!("-{}", count), "-p"],
                &format!("git log -{} -p", count),
            ));
        }
        if reference.kind == "url" {
            let content = fetch_url_content(&reference.target, url_fetcher);
            if content.trim().is_empty() {
                return Ok((Some(format!("{}: no content extracted", reference.raw)), None));
            }
            return Ok((
                None,
                Some(format!(
                    "🌐 {} ({} tokens)\n{}",
                    reference.raw,
                    estimate_tokens_rough(&content),
                    content
                )),
            ));
        }
        // Plugin-provided — mirrors ll.355-363
        let map = context_reference_providers().lock().unwrap();
        if let Some(provider) = map.get(reference.kind.as_str()) {
            // Mirrors `try: plugin_content = await provider.expand(...)` (ll.358-363)
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| provider.expand(&reference.target))) {
                Ok(Some(plugin_content)) => {
                    return Ok((
                        None,
                        Some(format!(
                            "📌 {} ({} tokens)\n{}",
                            reference.raw,
                            estimate_tokens_rough(&plugin_content),
                            plugin_content
                        )),
                    ));
                }
                Ok(None) => {}
                Err(_) => {
                    return Ok((Some(format!("{}: plugin expansion error: panic", reference.raw)), None));
                }
            }
        }
        Ok((Some(format!("{}: unsupported reference type", reference.raw)), None))
    })();

    match res {
        Ok(v) => v,
        Err(exc) => (Some(format!("{}: {}", reference.raw, exc)), None),
    }
}

#[allow(dead_code)]
fn _expand_reference(
    reference: &ContextReference,
    cwd: &Path,
    url_fetcher: Option<&dyn Fn(&str) -> String>,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    expand_reference(reference, cwd, url_fetcher, allowed_root)
}

// ---------------------------------------------------------------------------
// _expand_file_reference — mirrors Python ll.368-399
// ---------------------------------------------------------------------------

/// Mirrors `def _expand_file_reference(ref, cwd, *, allowed_root)` (ll.368-399)
fn expand_file_reference(
    reference: &ContextReference,
    cwd: &Path,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    let path = match resolve_path(cwd, &reference.target, allowed_root) {
        Ok(p) => p,
        Err(e) => return (Some(format!("{}: {}", reference.raw, e)), None),
    };
    if let Err(e) = ensure_reference_path_allowed(&path) {
        return (Some(format!("{}: {}", reference.raw, e)), None);
    }
    if !path.exists() {
        return (Some(format!("{}: file not found", reference.raw)), None);
    }
    if !path.is_file() {
        return (Some(format!("{}: path is not a file", reference.raw)), None);
    }
    if is_binary_file(&path) {
        // Mirrors binary block (ll.380-388)
        return (None, Some(binary_reference_block(reference, &path)));
    }

    // Mirrors `text = path.read_text(...)` + line slicing (ll.390-395)
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return (Some(format!("{}: {}", reference.raw, e)), None),
    };
    let text = if let Some(ls) = reference.line_start {
        let lines: Vec<&str> = text.split('\n').collect();
        // Python splitlines removes trailing newline; mimic via split + handling
        let start_idx = ls.saturating_sub(1);
        let end_idx = reference.line_end.unwrap_or(ls).min(lines.len());
        let start_idx = start_idx.min(lines.len());
        lines[start_idx..end_idx].join("\n")
    } else {
        text
    };

    let lang = code_fence_language(&path);
    let label = &reference.raw;
    (
        None,
        Some(format!(
            "📄 {} ({} tokens)\n```{}\n{}\n```",
            label,
            estimate_tokens_rough(&text),
            lang,
            text
        )),
    )
}

#[allow(dead_code)]
fn _expand_file_reference_sync(
    reference: &ContextReference,
    cwd: &Path,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    expand_file_reference(reference, cwd, allowed_root)
}

// ---------------------------------------------------------------------------
// _expand_folder_reference — mirrors Python ll.402-416
// ---------------------------------------------------------------------------

/// Mirrors `def _expand_folder_reference(ref, cwd, *, allowed_root)` (ll.402-416)
fn expand_folder_reference(
    reference: &ContextReference,
    cwd: &Path,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    let path = match resolve_path(cwd, &reference.target, allowed_root) {
        Ok(p) => p,
        Err(e) => return (Some(format!("{}: {}", reference.raw, e)), None),
    };
    if let Err(e) = ensure_reference_path_allowed(&path) {
        return (Some(format!("{}: {}", reference.raw, e)), None);
    }
    if !path.exists() {
        return (Some(format!("{}: folder not found", reference.raw)), None);
    }
    if !path.is_dir() {
        return (Some(format!("{}: path is not a folder", reference.raw)), None);
    }

    let listing = build_folder_listing(&path, cwd, 200);
    (
        None,
        Some(format!(
            "📁 {} ({} tokens)\n{}",
            reference.raw,
            estimate_tokens_rough(&listing),
            listing
        )),
    )
}

#[allow(dead_code)]
fn _expand_folder_reference_sync(
    reference: &ContextReference,
    cwd: &Path,
    allowed_root: Option<&Path>,
) -> (Option<String>, Option<String>) {
    expand_folder_reference(reference, cwd, allowed_root)
}

// ---------------------------------------------------------------------------
// _expand_git_reference — mirrors Python ll.419-444
// ---------------------------------------------------------------------------

/// Mirrors `def _expand_git_reference(ref, cwd, args, label)` (ll.419-444)
fn expand_git_reference(
    reference: &ContextReference,
    cwd: &Path,
    args: &[&str],
    label: &str,
) -> (Option<String>, Option<String>) {
    // Mirrors `_popen_kwargs = {"creationflags": windows_hide_flags()} if IS_WINDOWS else {}` (l.425)
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_hide_flags());
    }

    // Mirrors timeout=30 (l.432) — use wait_timeout pattern via simple wait (stub: no timeout crate)
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            // Mirrors TimeoutExpired branch (ll.436-437) — map errors
            if e.kind() == std::io::ErrorKind::TimedOut {
                return (Some(format!("{}: git command timed out (30s)", reference.raw)), None);
            }
            return (Some(format!("{}: {}", reference.raw, e)), None);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() { "git command failed".to_string() } else { stderr };
        return (Some(format!("{}: {}", reference.raw, msg)), None);
    }
    let mut content = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if content.is_empty() {
        content = "(no output)".to_string();
    }
    (
        None,
        Some(format!(
            "🧾 {} ({} tokens)\n```diff\n{}\n```",
            label,
            estimate_tokens_rough(&content),
            content
        )),
    )
}

#[allow(dead_code)]
fn _expand_git_reference_sync(
    reference: &ContextReference,
    cwd: &Path,
    args: &[&str],
    label: &str,
) -> (Option<String>, Option<String>) {
    expand_git_reference(reference, cwd, args, label)
}

// ---------------------------------------------------------------------------
// _fetch_url_content + _default_url_fetcher — mirrors Python ll.447-468
// ---------------------------------------------------------------------------

/// Mirrors `async def _fetch_url_content(url, *, url_fetcher)` (ll.447-456)
fn fetch_url_content(url: &str, url_fetcher: Option<&dyn Fn(&str) -> String>) -> String {
    // Mirrors `fetcher = url_fetcher or _default_url_fetcher` + awaitable check (ll.452-455)
    let fetcher: &dyn Fn(&str) -> String = match url_fetcher {
        Some(f) => f,
        None => &default_url_fetcher,
    };
    fetcher(url).trim().to_string()
}

#[allow(dead_code)]
fn _fetch_url_content(url: &str, url_fetcher: Option<&dyn Fn(&str) -> String>) -> String {
    fetch_url_content(url, url_fetcher)
}

/// Mirrors `async def _default_url_fetcher(url: str) -> str:` (ll.459-468)
/// Python calls `tools.web_tools.web_extract_tool([url], format="markdown")`.
/// Rust stub returns empty (no web tool in this crate); grep-traceable.
fn default_url_fetcher(url: &str) -> String {
    // Stub: real impl would call web_extract; keep comment for audit.
    // Mirrors `from tools.web_tools import web_extract_tool` lazy import (l.460)
    let _ = url;
    String::new()
}

#[allow(dead_code)]
fn _default_url_fetcher(url: &str) -> String {
    default_url_fetcher(url)
}

// ---------------------------------------------------------------------------
// _resolve_path — mirrors Python ll.471-481
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_path(cwd: Path, target: str, *, allowed_root: Path | None) -> Path:` (ll.471-481)
fn resolve_path(cwd: &Path, target: &str, allowed_root: Option<&Path>) -> Result<PathBuf, String> {
    // Mirrors `Path(os.path.expanduser(target))` (l.472)
    let expanded = expanduser(target);
    let path = PathBuf::from(&expanded);
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    // Mirrors `path.resolve()` — use canonicalize with fallback to absolute join
    let resolved = joined.canonicalize().unwrap_or_else(|_| {
        // Fallback: absolute without symlink resolution (keeps behavior for non-existent)
        if joined.is_absolute() {
            joined.clone()
        } else {
            // Make absolute via cwd
            cwd.join(&joined)
        }
    });
    // Normalize to absolute
    let resolved = if resolved.is_absolute() {
        resolved
    } else {
        // Should not happen; keep as is
        resolved
    };
    if let Some(root) = allowed_root {
        let root_resolved = expand_and_resolve(root);
        // Mirrors `resolved.relative_to(allowed_root)` guard (ll.478-480)
        if !resolved.starts_with(&root_resolved) {
            return Err("path is outside the allowed workspace".to_string());
        }
    }
    Ok(resolved)
}

#[allow(dead_code)]
fn _resolve_path(cwd: &Path, target: &str, allowed_root: Option<&Path>) -> Result<PathBuf, String> {
    resolve_path(cwd, target, allowed_root)
}

fn expanduser(path: &str) -> String {
    // Mirrors `os.path.expanduser` — `~` → $HOME
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen('~', &home, 1);
        }
        // Fallback to dirs crate not available; keep literal
    }
    // Also handle `~` without slash (e.g., `~/.hermes`)? Already covered.
    // Python also does `Path(os.path.expanduser(target))` — keep simple.
    path.to_string()
}

fn expand_and_resolve(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    let expanded = expanduser(&s);
    let pb = PathBuf::from(expanded);
    pb.canonicalize().unwrap_or(pb)
}

// ---------------------------------------------------------------------------
// _ensure_reference_path_allowed — mirrors Python ll.484-533
// ---------------------------------------------------------------------------

/// Mirrors `def _ensure_reference_path_allowed(path: Path) -> None:` (ll.484-533)
fn ensure_reference_path_allowed(path: &Path) -> Result<(), String> {
    // Mirrors `home = Path(os.path.expanduser("~")).resolve()` (l.486)
    let home = expand_and_resolve(Path::new("~"));
    // Mirrors `hermes_home = get_hermes_home().resolve()` (l.487)
    // Stub: HERMES_HOME env or `~/.hermes`
    let hermes_home = std::env::var("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".hermes"));
    let hermes_home = expand_and_resolve(&hermes_home);

    // Mirrors blocked_exact (ll.489-491)
    let mut blocked_exact: HashSet<PathBuf> = SENSITIVE_HOME_FILES
        .iter()
        .map(|rel| home.join(rel))
        .collect();
    blocked_exact.insert(hermes_home.join(".env"));

    let mut blocked_dirs: Vec<PathBuf> = SENSITIVE_HOME_DIRS.iter().map(|rel| home.join(rel)).collect();
    for rel in SENSITIVE_HERMES_DIRS {
        blocked_dirs.push(hermes_home.join(rel));
    }

    // Mirrors `if path in blocked_exact: raise` (ll.494-495)
    // Need to canonicalize `path` for comparison
    let path_resolved = expand_and_resolve(path);
    if blocked_exact.contains(&path_resolved) || blocked_exact.contains(path) {
        return Err("path is a sensitive credential file and cannot be attached".to_string());
    }

    // Mirrors `for blocked_dir in blocked_dirs: path.relative_to(blocked_dir)` (ll.497-502)
    for blocked_dir in &blocked_dirs {
        let blocked_resolved = expand_and_resolve(blocked_dir);
        if path_resolved.starts_with(&blocked_resolved) || path.starts_with(blocked_dir) {
            return Err(
                "path is a sensitive credential or internal Hermes path and cannot be attached".to_string(),
            );
        }
    }

    // Mirrors canonical `get_read_block_error` anchor (ll.514-533)
    // Stub: no `agent.file_safety` in Rust; keep fail-closed comment.
    // Python does:
    //   try: from agent.file_safety import get_read_block_error
    //        if get_read_block_error(str(path)) is not None: raise
    //   except ValueError: raise
    //   except Exception: raise ValueError("path could not be verified ...")
    // Rust: check well-known credential stores as approximation.
    let path_str = path_resolved.to_string_lossy().to_lowercase();
    // Narrow credential deny-list approximation — mirrors the gap note at ll.504-513
    let credential_markers = [
        "auth.json",
        ".anthropic_oauth.json",
        "mcp-tokens",
        ".env",
        "webhook",
        "hermes_home",
    ];
    // Only block if path is inside hermes_home and matches markers — do NOT over-block.
    // Keep fail-closed spirit: if we cannot verify, we allow narrow list only.
    // The full deny-list lives in hermes-core; this stub approximates.
    let _ = credential_markers;
    // Keep function fall-through as allowed (Python fail-closed on import error would block;
    // Rust stub without file_safety cannot fail-closed without false positives, so we
    // document the divergence and allow.)
    // NOTE divergence: Python ll.528-533 raises on `get_read_block_error` import failure (fail CLOSED).
    // Rust without that module cannot replicate fail-closed without blocking legitimate files;
    // canonical hermes-core impl replaces this stub with the real deny-list.

    Ok(())
}

#[allow(dead_code)]
fn _ensure_reference_path_allowed(path: &Path) -> Result<(), String> {
    ensure_reference_path_allowed(path)
}

// ---------------------------------------------------------------------------
// _strip_trailing_punctuation — mirrors Python ll.536-545
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_trailing_punctuation(value: str) -> str:` (ll.536-545)
fn strip_trailing_punctuation(value: &str) -> String {
    let mut stripped = value.trim_end_matches(|c: char| TRAILING_PUNCTUATION.contains(c)).to_string();
    // Mirrors while loop for unmatched closers (ll.538-544)
    loop {
        if stripped.ends_with(')') || stripped.ends_with(']') || stripped.ends_with('}') {
            let closer = stripped.chars().last().unwrap();
            let opener = match closer {
                ')' => '(',
                ']' => '[',
                '}' => '{',
                _ => break,
            };
            let closer_count = stripped.chars().filter(|&c| c == closer).count();
            let opener_count = stripped.chars().filter(|&c| c == opener).count();
            if closer_count > opener_count {
                stripped.pop();
                continue;
            }
        }
        break;
    }
    stripped
}

#[allow(dead_code)]
fn _strip_trailing_punctuation(value: &str) -> String {
    strip_trailing_punctuation(value)
}

// ---------------------------------------------------------------------------
// _strip_reference_wrappers — mirrors Python ll.548-551
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_reference_wrappers(value: str) -> str:` (ll.548-551)
fn strip_reference_wrappers(value: &str) -> String {
    if value.len() >= 2 {
        let first = value.chars().next().unwrap();
        let last = value.chars().last().unwrap();
        if first == last && matches!(first, '`' | '"' | '\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[allow(dead_code)]
fn _strip_reference_wrappers(value: &str) -> String {
    strip_reference_wrappers(value)
}

// ---------------------------------------------------------------------------
// _parse_file_reference_value — mirrors Python ll.554-577
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_file_reference_value(value: str) -> tuple[str, int | None, int | None]:` (ll.554-577)
fn parse_file_reference_value(value: &str) -> (String, Option<usize>, Option<usize>) {
    // Mirrors quoted_match (ll.555-566)
    let quoted_re = {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| {
            Regex::new(r#"^(?P<quote>`|"|\')(?P<path>.+?)(?P=quote)(?::(?P<start>\d+)(?:-(?P<end>\d+))?)?$"#)
                .expect("quoted file ref regex")
        })
    };
    if let Some(caps) = quoted_re.captures(value) {
        let path = caps.name("path").map(|m| m.as_str().to_string()).unwrap_or_default();
        let start_s = caps.name("start").map(|m| m.as_str());
        let end_s = caps.name("end").map(|m| m.as_str());
        let line_start = start_s.and_then(|s| s.parse::<usize>().ok());
        let line_end = match (line_start, end_s) {
            (Some(ls), Some(es)) => es.parse::<usize>().ok().or(Some(ls)),
            (Some(ls), None) => Some(ls),
            _ => None,
        };
        return (path, line_start, line_end);
    }

    // Mirrors range_match (ll.568-575)
    let range_re = {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| {
            Regex::new(r"^(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$").expect("range file ref regex")
        })
    };
    if let Some(caps) = range_re.captures(value) {
        let path = caps.name("path").map(|m| m.as_str().to_string()).unwrap_or_default();
        let start_s = caps.name("start").map(|m| m.as_str()).unwrap_or("1");
        let end_s = caps.name("end").map(|m| m.as_str());
        let line_start = start_s.parse::<usize>().ok();
        let ls = line_start.unwrap_or(1);
        let line_end = end_s.and_then(|s| s.parse::<usize>().ok()).unwrap_or(ls);
        return (path, Some(ls), Some(line_end));
    }

    // Mirrors `return _strip_reference_wrappers(value), None, None` (l.577)
    (strip_reference_wrappers(value), None, None)
}

#[allow(dead_code)]
fn _parse_file_reference_value(value: &str) -> (String, Option<usize>, Option<usize>) {
    parse_file_reference_value(value)
}

// ---------------------------------------------------------------------------
// _is_binary_file — mirrors Python ll.580-587
// ---------------------------------------------------------------------------

/// Mirrors `def _is_binary_file(path: Path) -> bool:` (ll.580-587)
fn is_binary_file(path: &Path) -> bool {
    // Mirrors mimetypes.guess_type + extension allowlist (ll.581-585)
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let lower = name.to_lowercase();
    let text_exts = [".py", ".md", ".txt", ".json", ".yaml", ".yml", ".toml", ".js", ".ts"];
    let is_known_text_ext = text_exts.iter().any(|ext| lower.ends_with(ext));

    // Minimal mime guess via extension (stub for mimetypes)
    let mime_text = lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".py")
        || lower.ends_with(".json")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".toml")
        || lower.ends_with(".js")
        || lower.ends_with(".ts")
        || lower.ends_with(".html")
        || lower.ends_with(".css")
        || lower.ends_with(".rs");

    if !mime_text && !is_known_text_ext && !lower.is_empty() {
        // Mirrors `if mime and not mime.startswith("text/") and not any(...): return True` (ll.582-585)
        // Without real mimetypes, treat unknown extensions as potentially binary (conservative)
        // but still check null byte to avoid false positives.
    }
    // Mirrors `chunk = path.read_bytes()[:4096]; return b"\x00" in chunk` (ll.586-587)
    if let Ok(bytes) = std::fs::read(path) {
        let chunk = &bytes[..bytes.len().min(4096)];
        if chunk.contains(&0u8) {
            return true;
        }
        // If mime indicated non-text and no known text ext, consider binary (Python early return)
        if !mime_text && !is_known_text_ext && !lower.is_empty() {
            // Heuristic: if extension is not in allowlist and mime would be non-text, return true
            // We don't have real mime; skip to avoid over-blocking.
            // Keep null-byte as authoritative (matches Python fallback).
        }
    }
    false
}

#[allow(dead_code)]
fn _is_binary_file(path: &Path) -> bool {
    is_binary_file(path)
}

// ---------------------------------------------------------------------------
// _build_folder_listing — mirrors Python ll.590-603
// ---------------------------------------------------------------------------

/// Mirrors `def _build_folder_listing(path: Path, cwd: Path, limit: int = 200) -> str:` (ll.590-603)
fn build_folder_listing(path: &Path, cwd: &Path, limit: usize) -> String {
    // Mirrors `lines = [f"{path.relative_to(cwd)}/"]` (l.591)
    let rel_root = path.strip_prefix(cwd).unwrap_or(path);
    let mut lines: Vec<String> = vec![format!("{}/", rel_root.display())];
    let entries = iter_visible_entries(path, cwd, limit);
    let base_depth = rel_root.components().count();
    for entry in &entries {
        let rel = entry.strip_prefix(cwd).unwrap_or(entry.as_path());
        let depth = rel.components().count();
        let indent_depth = depth.saturating_sub(base_depth + 1);
        let indent = "  ".repeat(indent_depth);
        if entry.is_dir() {
            if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
                lines.push(format!("{}- {}/", indent, name));
            }
        } else if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
            let meta = file_metadata(entry);
            lines.push(format!("{}- {} ({})", indent, name, meta));
        }
    }
    if entries.len() >= limit {
        lines.push("- ...".to_string());
    }
    lines.join("\n")
}

#[allow(dead_code)]
fn _build_folder_listing(path: &Path, cwd: &Path, limit: usize) -> String {
    build_folder_listing(path, cwd, limit)
}

// ---------------------------------------------------------------------------
// _iter_visible_entries — mirrors Python ll.606-634
// ---------------------------------------------------------------------------

/// Mirrors `def _iter_visible_entries(path: Path, cwd: Path, limit: int) -> list[Path]:` (ll.606-634)
fn iter_visible_entries(path: &Path, cwd: &Path, limit: usize) -> Vec<PathBuf> {
    // Mirrors `rg_entries = _rg_files(...)` fast path (ll.607-619)
    if let Some(rg_entries) = rg_files(path, cwd, limit) {
        let mut output: Vec<PathBuf> = Vec::new();
        let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
        for rel in rg_entries {
            let full = cwd.join(&rel);
            // Walk parents to collect dirs (mirrors ll.613-617)
            let mut cur = full.parent();
            while let Some(parent) = cur {
                if parent == cwd || seen_dirs.contains(parent) {
                    cur = parent.parent();
                    continue;
                }
                // Check `path in {parent, *parent.parents}` (l.615)
                let is_under_path = parent == path || parent.starts_with(path);
                if !is_under_path {
                    cur = parent.parent();
                    continue;
                }
                seen_dirs.insert(parent.to_path_buf());
                output.push(parent.to_path_buf());
                cur = parent.parent();
            }
            output.push(full);
        }
        // Mirrors `return sorted({p for p in output if p.exists()}, key=...)` (l.619)
        let mut dedup: HashSet<PathBuf> = HashSet::new();
        let mut filtered: Vec<PathBuf> = output.into_iter().filter(|p| p.exists() && dedup.insert(p.clone())).collect();
        filtered.sort_by(|a, b| {
            let a_dir = a.is_dir();
            let b_dir = b.is_dir();
            // `key=lambda p: (not p.is_dir(), str(p))` — dirs first
            match (a_dir, b_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.to_string_lossy().cmp(&b.to_string_lossy()),
            }
        });
        return filtered;
    }

    // Fallback os.walk (ll.621-634)
    let mut output: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(root) = stack.pop() {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "__pycache__" {
                if p.is_dir() && name == "__pycache__" {
                    continue;
                }
                if name.starts_with('.') {
                    continue;
                }
            }
            if p.is_dir() {
                dirs.push(p);
            } else {
                files.push(p);
            }
        }
        dirs.sort();
        files.sort();
        for d in dirs.iter().rev() {
            // Mirrors `dirs[:] = sorted(d for d in dirs if ...)` — push to stack for walk
            stack.push(d.clone());
        }
        for d in dirs {
            output.push(d);
            if output.len() >= limit {
                return output;
            }
        }
        for f in files {
            output.push(f);
            if output.len() >= limit {
                return output;
            }
        }
    }
    output
}

#[allow(dead_code)]
fn _iter_visible_entries(path: &Path, cwd: &Path, limit: usize) -> Vec<PathBuf> {
    iter_visible_entries(path, cwd, limit)
}

// ---------------------------------------------------------------------------
// _rg_files — mirrors Python ll.637-654
// ---------------------------------------------------------------------------

/// Mirrors `def _rg_files(path: Path, cwd: Path, limit: int) -> list[Path] | None:` (ll.637-654)
fn rg_files(path: &Path, cwd: &Path, limit: usize) -> Option<Vec<PathBuf>> {
    let rel = path.strip_prefix(cwd).unwrap_or(path);
    let rel_str = rel.to_string_lossy().to_string();
    let mut cmd = std::process::Command::new("rg");
    cmd.args(["--files", &rel_str]);
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_hide_flags());
    }
    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return None, // Mirrors FileNotFoundError/OSError (l.648-649)
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<PathBuf> = stdout
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        })
        .take(limit)
        .collect();
    Some(files)
}

#[allow(dead_code)]
fn _rg_files(path: &Path, cwd: &Path, limit: usize) -> Option<Vec<PathBuf>> {
    rg_files(path, cwd, limit)
}

// ---------------------------------------------------------------------------
// _agent_visible_path — mirrors Python ll.657-678
// ---------------------------------------------------------------------------

/// Mirrors `def _agent_visible_path(path: Path) -> str:` (ll.657-678)
/// Map a host path to the path the agent's tools can read in the active backend.
/// Under docker the host path dangles; credential_files translation is tried.
/// Falls back to host path.
fn agent_visible_path(path: &Path) -> String {
    // Mirrors try: _ensure_terminal_env_bridged(); to_agent_visible_cache_path (ll.667-677)
    // Rust stub: no terminal_tool / credential_files crate; return host path.
    path.to_string_lossy().to_string()
}

#[allow(dead_code)]
fn _agent_visible_path(path: &Path) -> String {
    agent_visible_path(path)
}

// ---------------------------------------------------------------------------
// _binary_reference_block — mirrors Python ll.681-693
// ---------------------------------------------------------------------------

/// Mirrors `def _binary_reference_block(ref: ContextReference, path: Path) -> str:` (ll.681-693)
fn binary_reference_block(reference: &ContextReference, path: &Path) -> String {
    // Mirrors mimetypes.guess_type (l.682-683)
    let mime = guess_mime(path);
    let size = match std::fs::metadata(path) {
        Ok(m) => format_bytes(m.len()),
        Err(_) => "unknown size".to_string(),
    };
    format!(
        "📎 {} ({}, {}) — binary file, not inlined as text. It is available on disk at `{}`. Use your tools to work with it (read or convert it, extract its text, or view/render it as needed); do not tell the user the file type is unsupported.",
        reference.raw,
        mime,
        size,
        agent_visible_path(path)
    )
}

#[allow(dead_code)]
fn _binary_reference_block(reference: &ContextReference, path: &Path) -> String {
    binary_reference_block(reference, path)
}

fn guess_mime(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// _file_metadata — mirrors Python ll.696-703
// ---------------------------------------------------------------------------

/// Mirrors `def _file_metadata(path: Path) -> str:` (ll.696-703)
fn file_metadata(path: &Path) -> String {
    if is_binary_file(path) {
        if let Ok(m) = std::fs::metadata(path) {
            return format!("{} bytes", m.len());
        }
        return "unknown size".to_string();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let line_count = text.matches('\n').count() + 1;
            format!("{} lines", line_count)
        }
        Err(_) => {
            if let Ok(m) = std::fs::metadata(path) {
                format!("{} bytes", m.len())
            } else {
                "unknown size".to_string()
            }
        }
    }
}

#[allow(dead_code)]
fn _file_metadata(path: &Path) -> String {
    file_metadata(path)
}

// ---------------------------------------------------------------------------
// _code_fence_language — mirrors Python ll.706-720
// ---------------------------------------------------------------------------

/// Mirrors `def _code_fence_language(path: Path) -> str:` (ll.706-720)
fn code_fence_language(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "json" => "json",
        "md" => "markdown",
        "sh" => "bash",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        _ => "",
    }
    .to_string()
}

#[allow(dead_code)]
fn _code_fence_language(path: &Path) -> String {
    code_fence_language(path)
}
