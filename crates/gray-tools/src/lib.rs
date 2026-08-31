//! gray-tools: the `Tool` trait, four builtin file/shell tools, and the
//! tool registry behind the `ToolExecutor` seam.
//!
//! Truncation policy (applied to every tool output): results are capped at
//! 2000 lines / 50 KiB, keeping head + tail with a `[truncated ...]`
//! annotation; error outputs are additionally hard-capped at 2 KiB.

pub mod bash;
pub mod cron_tool;
pub mod delegate;
pub mod edit;
pub mod edit_diff;
pub mod find;
pub mod grep;
pub mod ls;
pub mod file_mutation_queue;
pub mod read;
pub mod truncate;
pub mod write;

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;

pub use bash::BashTool;
pub use cron_tool::CronTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
pub use write::WriteTool;

/// Maximum number of lines kept in a successful tool output.
pub const MAX_LINES: usize = 2000;
/// Maximum size in bytes of a successful tool output.
pub const MAX_BYTES: usize = 50 * 1024;
/// Hard cap for error outputs (applied after the general truncation).
pub const MAX_ERROR_BYTES: usize = 2048;

/// A single agent-callable tool.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Static definition surfaced to the model (name, description, schema).
    fn def(&self) -> ToolDef;

    /// One-line snippet rendered in the system prompt's "Available tools" list.
    /// `None` hides the tool from that list (mirrors pi's `toolSnippets[name]` filter).
    fn prompt_snippet(&self) -> Option<&'static str> {
        None
    }

    /// Guideline bullets contributed to the system prompt when this tool is active.
    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// Whether this tool may run in parallel with others in the same turn.
    /// Args allow context-sensitive decisions (e.g. read-only operations).
    fn is_concurrency_safe(&self, args: &Value) -> bool {
        let _ = args;
        true
    }

    /// Executes the tool. Failures are data ([`ToolOutput::error`]), never panics.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput;
}

/// Ordered collection of tools with name lookup, wired into the agent loop
/// via [`ToolExecutor`].
#[derive(Default)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Coding bundle: read + bash + edit + write + cron (mirrors pi's `createCodingTools`).
    pub fn coding() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(BashTool));
        reg.register(Box::new(ReadTool));
        reg.register(Box::new(WriteTool));
        reg.register(Box::new(EditTool));
        reg.register(Box::new(CronTool));
        // ponytail: delegate_task global semaphore, per-account caps if throughput matters
        {
            use gray_core::delegation::{DelegateConfig, DelegationState};
            use crate::delegate::DelegateTool;
            let cfg = DelegateConfig::default();
            let state = DelegationState::new(cfg.max_concurrent_children);
            reg.register(Box::new(DelegateTool::new(cfg, state)));
        }
        reg
    }

    /// Alias for [`Self::coding`] — kept for backwards compatibility.
    pub fn builtin() -> Self {
        Self::coding()
    }

    /// Read-only bundle: read + grep + find + ls (mirrors pi's `createReadOnlyTools`).
    pub fn readonly() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(ReadTool));
        reg.register(Box::new(GrepTool));
        reg.register(Box::new(FindTool));
        reg.register(Box::new(LsTool));
        reg
    }

    /// All tools bundle: coding + read-only (mirrors pi's `createAllTools` sans powershell).
    pub fn all() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(BashTool));
        reg.register(Box::new(ReadTool));
        reg.register(Box::new(WriteTool));
        reg.register(Box::new(EditTool));
        reg.register(Box::new(GrepTool));
        reg.register(Box::new(FindTool));
        reg.register(Box::new(LsTool));
        reg
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(Arc::from(tool));
    }

    /// Tool definitions in registration order (for the chat request).
    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| t.def()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.def().name == name)
            .map(|t| t.as_ref())
    }

    /// Clones an owned handle so execution futures can be `'static`.
    fn lookup(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.def().name == name).cloned()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Names of registered tools in registration order.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.def().name.clone()).collect()
    }

    /// One-line snippets keyed by tool name — only tools with `Some` snippet are included
    /// (mirrors pi's `visibleTools = tools.filter(name => !!toolSnippets[name])`).
    pub fn prompt_snippets(&self) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        for tool in &self.tools {
            if let Some(snippet) = tool.prompt_snippet() {
                m.insert(tool.def().name.clone(), snippet.to_string());
            }
        }
        m
    }

    /// Collected guideline bullets from all registered tools (in registration order, deduped by caller).
    pub fn prompt_guidelines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for tool in &self.tools {
            if let Some(guidelines) = tool.prompt_guidelines() {
                out.extend(guidelines.iter().map(|g| g.to_string()));
            }
        }
        out
    }
}

#[async_trait]
impl ToolExecutor for Registry {
    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let tool = self.lookup(name);
        let ctx = ctx.clone();
        let name = name.to_string();
        Box::pin(async move {
            log::info!(target: "gray_tools", "tool start: {name}");
            let out = match tool {
                Some(tool) => tool.execute(&ctx, args).await,
                None => ToolOutput::error(format!("unknown tool: {name}")),
            };
            if out.is_error {
                log::warn!(target: "gray_tools", "tool {name} failed: {}", out.content);
            } else {
                log::info!(target: "gray_tools", "tool {name} done");
            }
            out
        })
    }
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

/// Truncates a successful output: 2000-line / 50 KiB cap, head + tail kept,
/// with a `[truncated N lines / M bytes]` annotation in the middle.
pub(crate) fn truncate_output(text: &str) -> String {
    let mut notes: Vec<String> = Vec::new();

    // Line cap: keep first half + last half of the allowed budget.
    let total_lines = text.lines().count();
    let body = if total_lines > MAX_LINES {
        let dropped = total_lines - MAX_LINES;
        notes.push(format!("{dropped} lines"));
        let keep = MAX_LINES / 2;
        let all: Vec<&str> = text.lines().collect();
        let mut parts = all[..keep].to_vec();
        parts.extend_from_slice(&all[all.len() - keep..]);
        parts.join("\n")
    } else if text.ends_with('\n') {
        // `lines()` drops the trailing newline; preserve it verbatim.
        text.to_string()
    } else {
        text.to_string()
    };

    // Byte cap on what remains (head + tail around the annotation).
    if body.len() > MAX_BYTES {
        let dropped_bytes = body.len() - MAX_BYTES;
        notes.push(format!("{dropped_bytes} bytes"));
        let half = MAX_BYTES / 2;
        let head_end = floor_char_boundary(&body, half);
        let tail_start = ceil_char_boundary(&body, body.len() - half);
        format!(
            "{}\n{}\n{}",
            &body[..head_end],
            annotation(&notes),
            &body[tail_start..]
        )
    } else if notes.is_empty() {
        body
    } else {
        // Line-truncated but within byte budget: insert the annotation in
        // the middle without touching the rest of the content.
        let lines: Vec<&str> = body.lines().collect();
        let mid = lines.len() / 2;
        let mut out = lines[..mid].join("\n");
        out.push('\n');
        out.push_str(&annotation(&notes));
        out.push('\n');
        out.push_str(&lines[mid..].join("\n"));
        if body.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// Error outputs: general truncation, then a hard 2 KiB head cap.
pub(crate) fn truncate_error(text: &str) -> String {
    let truncated = truncate_output(text);
    if truncated.len() > MAX_ERROR_BYTES {
        let cut = floor_char_boundary(&truncated, MAX_ERROR_BYTES);
        format!("{}\n[error truncated to 2KiB]", &truncated[..cut])
    } else {
        truncated
    }
}

/// Wraps raw stdout-like text into a successful [`ToolOutput`].
pub(crate) fn finish(raw: String) -> ToolOutput {
    ToolOutput::ok(truncate_output(&raw))
}

/// Wraps raw failure text into an error [`ToolOutput`] (capped at 2 KiB).
pub(crate) fn fail(raw: String) -> ToolOutput {
    ToolOutput::error(truncate_error(&raw))
}

fn annotation(notes: &[String]) -> String {
    format!("[truncated {}]", notes.join(" / "))
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Arg validation helpers (schemas are declared in each tool's `def`)
// ---------------------------------------------------------------------------

/// Required string argument.
pub(crate) fn get_str(args: &Value, key: &str) -> Result<String, ToolOutput> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected string"))),
        None => Err(fail(format!("missing required argument '{key}'"))),
    }
}

/// Optional unsigned integer argument (`null`/absent -> `None`).
pub(crate) fn get_opt_u64(
    args: &Value,
    key: &str,
) -> Result<Option<u64>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| {
                fail(format!("invalid argument '{key}': expected non-negative integer"))
            }),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected integer"))),
    }
}

/// Optional boolean argument (`null`/absent -> `None`).
pub(crate) fn get_opt_bool(
    args: &Value,
    key: &str,
) -> Result<Option<bool>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected boolean"))),
    }
}

/// Resolves a user-supplied path against the execution cwd; absolute paths
/// are used verbatim.
pub(crate) fn resolve_path(cwd: &std::path::Path, p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

