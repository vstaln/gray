//! gray-tools: builtin file/shell tools and the tool registry behind the
//! `ToolExecutor` seam. (The `Tool` trait itself lives in `gray-core::agent`.)
//!
//! Truncation policy (applied to every tool output): results are capped at
//! 2000 lines / 50 KiB, keeping head + tail with a `[truncated ...]`
//! annotation; error outputs are additionally hard-capped at 2 KiB.

pub mod edit;
pub mod edit_diff;
pub mod find;
pub mod grep;
pub mod images;
pub mod ledger;
pub mod ls;
pub mod read;
pub mod shell;
pub mod stats;
pub mod truncate;
pub mod write;

use std::sync::Arc;

use async_trait::async_trait;
pub use gray_core::agent::Tool;
use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
pub(crate) use gray_core::tool_out::{
    MAX_BYTES, fail, finish, get_opt_bool, get_opt_u64, get_str, resolve_path,
};
use serde_json::Value;

pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ledger::{FileLedger, LedgerEntry};
pub use ls::LsTool;
pub use read::ReadTool;
pub use shell::tools::bash::BashTool;
pub use write::WriteTool;

/// Ordered collection of tools with name lookup, wired into the agent loop
/// via [`ToolExecutor`]. Plugin assembly (manifests, builtin plugin sets)
/// lives in `gray::profile` — this crate only holds the tools.
#[derive(Default)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
    file_ledger: Arc<FileLedger>,
}

impl Registry {
    /// Collects tools in order; on name conflict later entries win.
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for t in tools {
            if let Some(pos) = out.iter().position(|e| e.def().name == t.def().name) {
                out[pos] = t;
            } else {
                out.push(t);
            }
        }
        Self {
            tools: out,
            file_ledger: Arc::new(FileLedger::new()),
        }
    }

    /// Shared read-before-write/dedup state (T3.1 seam). Tools take a clone
    /// of this `Arc` in the T3.2/T3.3 wiring; `ToolsBasicPlugin` (T3.4) too.
    pub fn file_ledger(&self) -> &Arc<FileLedger> {
        &self.file_ledger
    }

    /// T3.4 adoption: point the registry at the ledger the session tools
    /// share (`from_plugins` rebuilds tools-basic read/write/edit on it).
    pub fn set_file_ledger(&mut self, ledger: Arc<FileLedger>) {
        self.file_ledger = ledger;
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

    /// Names of registered tools in registration order.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.def().name.clone()).collect()
    }
}

/// Legacy arg-name aliases applied before schema lookup (old -> canonical).
/// Single table on purpose: add a row, don't add a system.
static ALIASES: &[(&str, &str)] = &[
    ("file_path", "path"),
    ("filePath", "path"),
    ("filename", "path"),
    ("file", "path"),
    ("target", "path"),
    ("destination", "path"),
    ("target_file", "path"),
    ("targetFile", "path"),
    ("TargetFile", "path"),
    ("contents", "content"),
    ("text", "content"),
    ("code", "content"),
    ("body", "content"),
    ("data", "content"),
    ("old_text", "oldText"),
    ("new_text", "newText"),
    ("TargetContent", "oldText"),
    ("target_content", "oldText"),
    ("targetContent", "oldText"),
    ("search", "oldText"),
    ("find", "oldText"),
    ("ReplacementContent", "newText"),
    ("replacement_content", "newText"),
    ("replacementContent", "newText"),
    ("replace", "newText"),
    ("cmd", "command"),
    ("script", "command"),
    ("shell_command", "command"),
    ("command_line", "command"),
    ("run_in_background", "background"),
    ("is_background", "background"),
    ("detach", "background"),
    ("bg", "background"),
    ("timeout_secs", "timeout"),
    ("timeout_seconds", "timeout"),
    ("timeoutSec", "timeout"),
    ("task", "task_id"),
    ("taskId", "task_id"),
    ("shell_id", "task_id"),
    ("duration", "seconds"),
    ("secs", "seconds"),
];

/// Strips trivial framing: markdown fences, then outer prose around `{…}`.
fn strip_framing(s: &str) -> &str {
    let mut t = s.trim();
    if t.starts_with("```") {
        if let Some(nl) = t.find('\n') {
            t = t[nl + 1..].trim_start();
        } else {
            t = t
                .strip_prefix("```json")
                .or_else(|| t.strip_prefix("```"))
                .unwrap_or(t)
                .trim_start();
        }
        if let Some(stripped) = t.strip_suffix("```") {
            t = stripped.trim_end();
        }
        t = t.trim();
    }
    if let (Some(start), Some(end)) = (t.find('{'), t.rfind('}'))
        && start <= end
    {
        t = t[start..=end].trim();
    }
    t
}

/// Coerces loose model args into schema-typed values: string->int/float/bool,
/// JSON-encoded-string->array/object, bare scalar->single-elem array, and
/// null/"null"->None (dropped) only where the schema leaves it optional.
fn coerce_args(def: &ToolDef, args: Value) -> Value {
    let mut args = match args {
        Value::String(s) => {
            serde_json::from_str::<Value>(strip_framing(&s)).unwrap_or(Value::String(s))
        }
        v => v,
    };
    let Value::Object(ref mut map) = args else {
        return args;
    };
    // Schema property names (for schema-gated aliases below).
    let has_prop = |name: &str| {
        def.parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .is_some_and(|o| o.contains_key(name))
    };
    for (old, new) in ALIASES {
        // Rename only when the schema defines the new name and not the old
        // one: a plugin whose schema legitimately declares `text` must keep
        // it, not have it rewritten to `content`.
        if map.contains_key(*old)
            && !map.contains_key(*new)
            && has_prop(new)
            && !has_prop(old)
            && let Some(v) = map.remove(*old)
        {
            map.insert(new.to_string(), v);
        }
    }
    let props: Vec<(String, String)> = def
        .parameters
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    v.get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| (k.clone(), t.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let required: Vec<String> = def
        .parameters
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    for (key, typ) in &props {
        let Some(val) = map.get(key).cloned() else {
            continue;
        };
        let coerced = match (typ.as_str(), val) {
            ("integer", Value::String(s)) => s
                .trim()
                .parse::<i64>()
                .ok()
                .map(Value::from)
                .or_else(|| {
                    s.trim().parse::<f64>().ok().and_then(|n| {
                        // No lossy truncation: only integral, in-range
                        // floats convert ("2.0" ok; "2.5", overflow, NaN stay strings).
                        (n.fract() == 0.0 && n.abs() < 9.223372036854776e18)
                            .then_some(Value::from(n as i64))
                    })
                })
                .unwrap_or(Value::String(s)),
            ("number", Value::String(s)) => s
                .trim()
                .parse::<f64>()
                .ok()
                .map(Value::from)
                .unwrap_or(Value::String(s)),
            ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Value::from(true),
                "false" | "0" | "no" => Value::from(false),
                _ => Value::String(s),
            },
            ("boolean", Value::Number(n)) => {
                if n.as_u64() == Some(1) {
                    Value::from(true)
                } else if n.as_u64() == Some(0) {
                    Value::from(false)
                } else {
                    Value::Number(n)
                }
            }
            ("string", Value::Number(n)) => Value::String(n.to_string()),
            ("string", Value::Bool(b)) => Value::String(b.to_string()),
            ("array", Value::String(s)) => match serde_json::from_str::<Value>(strip_framing(&s)) {
                Ok(Value::Array(a)) => Value::Array(a),
                Ok(Value::Null) => Value::Null,
                Ok(other) => Value::Array(vec![other]),
                Err(_) => Value::Array(vec![Value::String(s)]),
            },
            ("array", v) if !v.is_array() && !v.is_null() => Value::Array(vec![v]),
            ("object", Value::String(s)) => {
                match serde_json::from_str::<Value>(strip_framing(&s)) {
                    Ok(Value::Object(m)) => Value::Object(m),
                    _ => Value::String(s),
                }
            }
            (_, v) => v,
        };
        map.insert(key.clone(), coerced);
    }
    let drop: Vec<String> = map
        .iter()
        .filter_map(|(k, v)| {
            // Only real JSON nulls drop (and only when optional): the literal
            // string "null" is a valid value and must survive.
            if v.is_null() && !required.iter().any(|r| r == k) {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();
    for k in drop {
        map.remove(&k);
    }
    args
}

#[async_trait]
impl ToolExecutor for Registry {
    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutput> + Send + 'static>> {
        let tool = self.lookup(name);
        let coerced = match tool.as_ref().map(|t| t.def()) {
            Some(def) => coerce_args(&def, args),
            None => args,
        };
        let available = self.tool_names();
        let ctx = ctx.clone();
        let name = name.to_string();
        Box::pin(async move {
            log::info!(target: "gray_tools", "tool start: {name}");
            let out = match tool {
                Some(tool) => tool.execute(&ctx, coerced).await,
                None => ToolOutput::error(format!(
                    "Tool '{name}' does not exist. Available: {}",
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                )),
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

#[path = "lib_tests.rs"]
#[cfg(test)]
mod tests;
