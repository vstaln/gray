//! gray-tools: builtin file/shell tools and the tool registry behind the
//! `ToolExecutor` seam. (The `Tool` trait itself lives in `gray-core::agent`.)
//!
//! Truncation policy (applied to every tool output): results are capped at
//! 2000 lines / 50 KiB, keeping head + tail with a `[truncated ...]`
//! annotation; error outputs are additionally hard-capped at 2 KiB.

pub mod bash;
pub mod cron_tool;
pub mod edit;
pub mod edit_diff;
pub mod find;
pub mod grep;
pub mod ls;
pub mod plugin;
pub mod read;
pub mod request_user_input;
pub mod skill;
pub mod truncate;
pub mod write;

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
pub use gray_core::agent::Tool;
use gray_core::message::ToolDef;
use serde_json::Value;

pub use bash::BashTool;
pub use cron_tool::CronTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
pub use request_user_input::{
    RequestUserInputTool, StdinQuestionAsker, REQUEST_USER_INPUT_TOOL_NAME,
};
pub use skill::SkillTool;
pub use write::WriteTool;

/// Maximum number of lines kept in a successful tool output.
pub const MAX_LINES: usize = 2000;
/// Maximum size in bytes of a successful tool output.
pub const MAX_BYTES: usize = 50 * 1024;
/// Hard cap for error outputs (applied after the general truncation).
pub const MAX_ERROR_BYTES: usize = 2048;

/// Ordered collection of tools with name lookup, wired into the agent loop
/// via [`ToolExecutor`].
#[derive(Default)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
    manifests: Vec<gray_plugin::Manifest>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self::from_plugins(&[
            Arc::new(plugin::ToolsBasicPlugin),
            Arc::new(plugin::ToolsSearchPlugin),
            Arc::new(plugin::CronPlugin),
        ])
    }

    /// Collects tools from plugins in order; on name conflict later entries win.
    /// Manifests travel with the registry so `--dump-manifest` can't drift
    /// from what's actually registered. (ponytail-audit #13)
    pub fn from_plugins(plugins: &[Arc<dyn gray_plugin::Plugin>]) -> Self {
        let manifests: Vec<gray_plugin::Manifest> =
            plugins.iter().map(|p| p.manifest()).collect();
        let owners = gray_plugin::merge_manifests(manifests.clone());
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for p in plugins {
            let owner_name = p.manifest().name;
            for t in p.tools() {
                if owners.get(&t.def().name).map(|o| o == &owner_name).unwrap_or(false) {
                    if let Some(pos) = tools.iter().position(|e| e.def().name == t.def().name) {
                        tools[pos] = t.clone();
                    } else {
                        tools.push(t.clone());
                    }
                }
            }
        }
        Self { tools, manifests }
    }

    /// Plugin manifests in registration order (for `--dump-manifest`).
    pub fn manifests(&self) -> &[gray_plugin::Manifest] {
        &self.manifests
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
];

/// Strips trivial framing: markdown fences, then outer prose around `{…}`.
fn strip_framing(s: &str) -> &str {
    let mut t = s.trim();
    if t.starts_with("```") {
        if let Some(nl) = t.find('\n') {
            t = t[nl + 1..].trim_start();
        } else {
            t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t).trim_start();
        }
        if let Some(stripped) = t.strip_suffix("```") {
            t = stripped.trim_end();
        }
        t = t.trim();
    }
    if let (Some(start), Some(end)) = (t.find('{'), t.rfind('}')) {
        if start <= end {
            t = t[start..=end].trim();
        }
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
    let Value::Object(ref mut map) = args else { return args };
    for (old, new) in ALIASES {
        if map.contains_key(*old) && !map.contains_key(*new) {
            if let Some(v) = map.remove(*old) {
                map.insert(new.to_string(), v);
            }
        }
    }
    let props: Vec<(String, String)> = def
        .parameters
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|o| o.iter().filter_map(|(k, v)| v.get("type").and_then(|t| t.as_str()).map(|t| (k.clone(), t.to_string()))).collect())
        .unwrap_or_default();
    let required: Vec<String> = def
        .parameters
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    for (key, typ) in &props {
        let Some(val) = map.get(key).cloned() else { continue };
        let coerced = match (typ.as_str(), val) {
            ("integer", Value::String(s)) => s.trim().parse::<i64>().ok().map(Value::from).or_else(|| s.trim().parse::<f64>().ok().map(|n| Value::from(n as i64))).unwrap_or(Value::String(s)),
            ("number", Value::String(s)) => s.trim().parse::<f64>().ok().map(Value::from).unwrap_or(Value::String(s)),
            ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Value::from(true),
                "false" | "0" | "no" => Value::from(false),
                _ => Value::String(s),
            },
            ("boolean", Value::Number(n)) => {
                if n.as_u64() == Some(1) { Value::from(true) } else if n.as_u64() == Some(0) { Value::from(false) } else { Value::Number(n) }
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
            ("object", Value::String(s)) => match serde_json::from_str::<Value>(strip_framing(&s)) {
                Ok(Value::Object(m)) => Value::Object(m),
                _ => Value::String(s),
            },
            (_, v) => v,
        };
        map.insert(key.clone(), coerced);
    }
    let drop: Vec<String> = map
        .iter()
        .filter_map(|(k, v)| {
            let nullish = v.is_null() || matches!(v, Value::String(s) if s.trim().eq_ignore_ascii_case("null"));
            if nullish && !required.iter().any(|r| r == k) { Some(k.clone()) } else { None }
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
    ) -> BoxFuture<'static, ToolOutput> {
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
                None => ToolOutput::error(format!("Tool '{name}' does not exist. Available: {}", if available.is_empty() { "(none)".to_string() } else { available.join(", ") })),
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

pub(crate) fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub(crate) fn ceil_char_boundary(s: &str, index: usize) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::{ToolContext, ToolOutput};
    use gray_core::message::ToolDef;
    use serde_json::{json, Value};

    struct StubTool {
        name: &'static str,
        marker: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for StubTool {
        fn def(&self) -> ToolDef {
            ToolDef::new(self.name, "stub", json!({"type": "object"}))
        }
        async fn execute(&self, _ctx: &ToolContext, _args: Value) -> ToolOutput {
            ToolOutput::ok(self.marker)
        }
    }

    struct StubPlugin {
        name: &'static str,
        tool_name: &'static str,
        marker: &'static str,
    }

    impl gray_plugin::Plugin for StubPlugin {
        fn manifest(&self) -> gray_plugin::Manifest {
            gray_plugin::Manifest {
                name: self.name.to_string(),
                version: "0.0.0".to_string(),
                tools: vec![gray_core::message::ToolDef::new(
                    self.tool_name,
                    "stub",
                    serde_json::json!({"type": "object"}),
                )],
                commands: vec![],
                hooks: vec![],
                provider: None,
            }
        }
        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![Arc::new(StubTool { name: self.tool_name, marker: self.marker })]
        }
    }

    #[tokio::test]
    async fn from_plugins_later_plugin_wins_on_conflict() {
        let a: Arc<dyn gray_plugin::Plugin> = Arc::new(StubPlugin {
            name: "a",
            tool_name: "dup",
            marker: "from-a",
        });
        let b: Arc<dyn gray_plugin::Plugin> = Arc::new(StubPlugin {
            name: "b",
            tool_name: "dup",
            marker: "from-b",
        });
        let reg = Registry::from_plugins(&[a, b]);
        assert_eq!(reg.defs().len(), 1);
        let out = reg
            .lookup("dup")
            .unwrap()
            .execute(&ToolContext::default(), json!({}))
            .await;
        assert!(format!("{out:?}").contains("from-b"), "{out:?}");
    }

    fn scalar_def() -> ToolDef {
        ToolDef::new(
            "probe",
            "probe",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer" },
                    "ratio": { "type": "number" },
                    "verbose": { "type": "boolean" },
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        )
    }

    #[test]
    fn coerce_string_scalars_to_typed_values() {
        let out = coerce_args(&scalar_def(), json!({"limit": "10", "ratio": "2.5", "verbose": "true", "path": "/tmp/x"}));
        assert_eq!(out.get("limit"), Some(&json!(10)));
        assert_eq!(out.get("ratio"), Some(&json!(2.5)));
        assert_eq!(out.get("verbose"), Some(&json!(true)));
    }

    #[test]
    fn coerce_json_string_and_bare_scalar_to_array() {
        let def = ToolDef::new(
            "probe",
            "probe",
            json!({"type": "object", "properties": {"edits": {"type": "array"}}, "required": []}),
        );
        let out = coerce_args(&def, json!({"edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"}));
        let arr = out.get("edits").and_then(|v| v.as_array()).expect("edits should coerce to array");
        assert_eq!(arr.len(), 1);
        let out2 = coerce_args(&def, json!({"edits": {"oldText": "a", "newText": "b"}}));
        let arr2 = out2.get("edits").and_then(|v| v.as_array()).expect("bare object should wrap to array");
        assert_eq!(arr2.len(), 1);
    }

    #[test]
    fn coerce_null_dropped_only_when_optional() {
        let out = coerce_args(&scalar_def(), json!({"path": null, "limit": null}));
        assert!(out.get("path").is_some(), "required null must be kept so the tool errors");
        assert!(out.get("limit").is_none(), "optional null must drop to None");
        let out2 = coerce_args(&scalar_def(), json!({"path": "/tmp/x", "limit": "null"}));
        assert!(out2.get("limit").is_none(), "string 'null' for optional must drop, got {out2}");
    }

    #[test]
    fn aliases_rename_legacy_arg_names() {
        assert!(ALIASES.contains(&("file_path", "path")), "ALIASES must map legacy names");
        let out = coerce_args(&scalar_def(), json!({"file_path": "/tmp/x", "limit": "3"}));
        assert_eq!(out.get("path"), Some(&json!("/tmp/x")));
        assert!(out.get("file_path").is_none());
        assert_eq!(out.get("limit"), Some(&json!(3)));
    }

    #[test]
    fn strip_framing_unwraps_code_fences() {
        let raw = "```json\n{\"path\":\"/tmp/x\"}\n```";
        let stripped = strip_framing(raw);
        let v: Value = serde_json::from_str(stripped.trim()).expect("framing strip must yield JSON");
        assert_eq!(v.get("path"), Some(&json!("/tmp/x")));
    }

    #[tokio::test]
    async fn registry_execute_applies_aliases_and_coercion() {
        struct Probe {
            seen: Arc<std::sync::Mutex<Option<Value>>>,
        }
        #[async_trait::async_trait]
        impl Tool for Probe {
            fn def(&self) -> ToolDef {
                ToolDef::new(
                    "probe",
                    "probe",
                    json!({
                        "type": "object",
                        "properties": {"limit": {"type": "integer"}, "path": {"type": "string"}},
                        "required": ["path"]
                    }),
                )
            }
            async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
                *self.seen.lock().unwrap() = Some(args);
                ToolOutput::ok("ok")
            }
        }
        struct ProbePlugin {
            probe: Arc<Probe>,
        }
        impl gray_plugin::Plugin for ProbePlugin {
            fn manifest(&self) -> gray_plugin::Manifest {
                gray_plugin::Manifest { name: "p".to_string(), version: "0.0.0".to_string(), tools: vec![scalar_def()], commands: vec![], hooks: vec![], provider: None }
            }
            fn tools(&self) -> Vec<Arc<dyn Tool>> {
                vec![self.probe.clone()]
            }
        }
        let seen = Arc::new(std::sync::Mutex::new(None));
        let plugin: Arc<dyn gray_plugin::Plugin> =
            Arc::new(ProbePlugin { probe: Arc::new(Probe { seen: seen.clone() }) });
        let reg = Registry::from_plugins(&[plugin]);
        let out = ToolExecutor::execute(&reg, &ToolContext::default(), "probe", json!({"file_path": "/tmp/x", "limit": "7"})).await;
        assert!(!out.is_error, "{out:?}");
        let args = seen.lock().unwrap().clone().expect("tool should see args");
        assert_eq!(args.get("path"), Some(&json!("/tmp/x")), "{args}");
        assert_eq!(args.get("limit"), Some(&json!(7)), "{args}");
    }
}

