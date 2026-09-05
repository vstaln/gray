//! gray-tools: builtin file/shell tools and the tool registry behind the
//! `ToolExecutor` seam. (The `Tool` trait itself lives in `gray-core::agent`.)
//!
//! Truncation policy (applied to every tool output): results are capped at
//! 2000 lines / 50 KiB, keeping head + tail with a `[truncated ...]`
//! annotation; error outputs are additionally hard-capped at 2 KiB.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod find;
pub mod grep;
pub mod ls;
pub mod read;
pub mod request_user_input;
pub mod stats;
pub mod truncate;
pub mod write;

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
pub use gray_core::agent::Tool;
use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
pub(crate) use gray_core::tool_out::{
    MAX_BYTES, MAX_LINES, fail, finish, get_opt_bool, get_opt_u64, get_str, resolve_path,
};
use serde_json::Value;

pub use bash::BashTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
pub use request_user_input::{
    REQUEST_USER_INPUT_TOOL_NAME, RequestUserInputTool, StdinQuestionAsker,
};
pub use write::WriteTool;

/// Ordered collection of tools with name lookup, wired into the agent loop
/// via [`ToolExecutor`]. Plugin assembly (manifests, builtin plugin sets)
/// lives in `gray::profile` — this crate only holds the tools.
#[derive(Default)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Registry {
    /// All builtin tools that live in this crate (no Skill/Cron: those are
    /// wired by `gray::profile` from their home crates).
    pub fn builtin() -> Self {
        Self::new(vec![
            Arc::new(ReadTool),
            Arc::new(WriteTool),
            Arc::new(EditTool),
            Arc::new(BashTool),
            Arc::new(RequestUserInputTool),
            Arc::new(GrepTool),
            Arc::new(FindTool),
            Arc::new(LsTool),
        ])
    }

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
        Self { tools: out }
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
    for (old, new) in ALIASES {
        if map.contains_key(*old)
            && !map.contains_key(*new)
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
                .or_else(|| s.trim().parse::<f64>().ok().map(|n| Value::from(n as i64)))
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
            let nullish = v.is_null()
                || matches!(v, Value::String(s) if s.trim().eq_ignore_ascii_case("null"));
            if nullish && !required.iter().any(|r| r == k) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::{ToolContext, ToolOutput};
    use gray_core::message::ToolDef;
    use serde_json::{Value, json};

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

    #[tokio::test]
    async fn new_later_tool_wins_on_conflict() {
        let a: Arc<dyn Tool> = Arc::new(StubTool {
            name: "dup",
            marker: "from-a",
        });
        let b: Arc<dyn Tool> = Arc::new(StubTool {
            name: "dup",
            marker: "from-b",
        });
        let reg = Registry::new(vec![a, b]);
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
        let out = coerce_args(
            &scalar_def(),
            json!({"limit": "10", "ratio": "2.5", "verbose": "true", "path": "/tmp/x"}),
        );
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
        let out = coerce_args(
            &def,
            json!({"edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"}),
        );
        let arr = out
            .get("edits")
            .and_then(|v| v.as_array())
            .expect("edits should coerce to array");
        assert_eq!(arr.len(), 1);
        let out2 = coerce_args(&def, json!({"edits": {"oldText": "a", "newText": "b"}}));
        let arr2 = out2
            .get("edits")
            .and_then(|v| v.as_array())
            .expect("bare object should wrap to array");
        assert_eq!(arr2.len(), 1);
    }

    #[test]
    fn coerce_null_dropped_only_when_optional() {
        let out = coerce_args(&scalar_def(), json!({"path": null, "limit": null}));
        assert!(
            out.get("path").is_some(),
            "required null must be kept so the tool errors"
        );
        assert!(
            out.get("limit").is_none(),
            "optional null must drop to None"
        );
        let out2 = coerce_args(&scalar_def(), json!({"path": "/tmp/x", "limit": "null"}));
        assert!(
            out2.get("limit").is_none(),
            "string 'null' for optional must drop, got {out2}"
        );
    }

    #[test]
    fn aliases_rename_legacy_arg_names() {
        assert!(
            ALIASES.contains(&("file_path", "path")),
            "ALIASES must map legacy names"
        );
        let out = coerce_args(&scalar_def(), json!({"file_path": "/tmp/x", "limit": "3"}));
        assert_eq!(out.get("path"), Some(&json!("/tmp/x")));
        assert!(out.get("file_path").is_none());
        assert_eq!(out.get("limit"), Some(&json!(3)));
    }

    #[test]
    fn strip_framing_unwraps_code_fences() {
        let raw = "```json\n{\"path\":\"/tmp/x\"}\n```";
        let stripped = strip_framing(raw);
        let v: Value =
            serde_json::from_str(stripped.trim()).expect("framing strip must yield JSON");
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
        let seen = Arc::new(std::sync::Mutex::new(None));
        let probe: Arc<dyn Tool> = Arc::new(Probe { seen: seen.clone() });
        let reg = Registry::new(vec![probe]);
        let out = ToolExecutor::execute(
            &reg,
            &ToolContext::default(),
            "probe",
            json!({"file_path": "/tmp/x", "limit": "7"}),
        )
        .await;
        assert!(!out.is_error, "{out:?}");
        let args = seen.lock().unwrap().clone().expect("tool should see args");
        assert_eq!(args.get("path"), Some(&json!("/tmp/x")), "{args}");
        assert_eq!(args.get("limit"), Some(&json!(7)), "{args}");
    }
}
