use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use gray_core::agent::ToolOutput;
use gray_core::event::Usage;
use gray_core::message::{Message, ToolDef};

pub mod profile;
pub mod sidecar;

#[derive(Debug, Clone)]
pub enum CoreEvent {
    PreStep { messages: Vec<Message> },
    PreTool { name: String, args: Value },
    PostTool { name: String, output: ToolOutput },
    TurnEnd { usage: Usage },
}

/// Protocol v1 manifest: `plugin/manifest` result
/// `{"name","version","tools":[{"name","description","parameters","snippet"}],
/// "commands":["/x"],"hooks":["prompt/context","tool/before","turn/end"]}`.
/// `commands`/`hooks` are empty for pre-v1 sidecars (field absent → default).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
    pub provider: Option<String>,
}

/// One manifest `tools` entry: the model-facing definition plus the optional
/// `snippet` shown in the Available-tools block.
#[derive(Debug, Clone)]
pub struct ManifestTool {
    pub def: ToolDef,
    pub snippet: Option<String>,
}

/// Parse one manifest `tools` entry. Pre-v1 sidecars send bare strings
/// (`"tools": ["echo"]`) — those still parse, with an empty schema.
pub fn parse_tool_entry(v: &Value) -> Option<ManifestTool> {
    if let Some(name) = v.as_str() {
        return Some(ManifestTool {
            def: ToolDef::new(name, format!("sidecar tool {name}"), serde_json::json!({})),
            snippet: None,
        });
    }
    let obj = v.as_object()?;
    let name = obj.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    let description =
        obj.get("description").and_then(|d| d.as_str()).unwrap_or_default().to_string();
    let parameters = obj.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({}));
    let snippet =
        obj.get("snippet").and_then(|s| s.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
    Some(ManifestTool { def: ToolDef::new(name, description, parameters), snippet })
}

/// Parse the `tools` array of a `plugin/manifest` result into
/// definitions + snippets (single walk; [`Manifest::from_result`] keeps
/// only the defs).
pub fn manifest_tools(v: &Value) -> Vec<ManifestTool> {
    v.get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(parse_tool_entry).collect())
        .unwrap_or_default()
}

impl Manifest {
    /// Parse a `plugin/manifest` result value. Lenient on purpose: missing
    /// sections default to empty so pre-v1 sidecars keep working.
    pub fn from_result(v: &Value) -> Self {
        let str_list = |key: &str| {
            v.get(key)
                .and_then(|t| t.as_array())
                .map(|a| a.iter().filter_map(|e| e.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default()
        };
        Self {
            name: v.get("name").and_then(|s| s.as_str()).unwrap_or_default().into(),
            version: v.get("version").and_then(|s| s.as_str()).unwrap_or_default().into(),
            tools: manifest_tools(v).into_iter().map(|t| t.def).collect(),
            commands: str_list("commands"),
            hooks: str_list("hooks"),
            provider: v.get("provider").and_then(|s| s.as_str()).map(|s| s.into()),
        }
    }
}

/// Verdict of a `tool/before` hook: allow the call, deny it with a reason
/// (surfaced as an `is_error` tool result), or rewrite its args.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolBefore {
    Allow,
    Deny { reason: String },
    Modify { args: Value },
}

impl ToolBefore {
    /// Parse a `tool/before` result. Lenient: unknown shapes fail open
    /// (pre-v1 behavior) so a confused plugin can't wedge the agent loop.
    pub fn from_result(v: &Value, args: &Value) -> Self {
        match v.get("decision").and_then(|d| d.as_str()) {
            Some("deny") => Self::Deny {
                reason: v
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .filter(|r| !r.is_empty())
                    .unwrap_or("denied by plugin")
                    .to_string(),
            },
            Some("modify") => Self::Modify {
                args: v.get("args").cloned().unwrap_or_else(|| args.clone()),
            },
            _ => Self::Allow,
        }
    }
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }
    // NOTE (ponytail-audit #6): an earlier `provider()` hook was deleted — every
    // impl returned None and nothing called it. `on_event`/`CoreEvent` stay:
    // SidecarPlugin dispatches them to the subprocess over stdio.
    async fn on_event(&self, _e: CoreEvent) -> Option<CoreEvent> {
        None
    }
    /// `prompt/context` hook (`params: {"cwd"}` → `result: {"text"}`).
    /// Default `None` = no extra context (pre-v1 behavior).
    async fn prompt_context(&self, _cwd: &str) -> Option<String> {
        None
    }
    /// `tool/before` hook (`params: {"name","args"}` → allow/deny/modify).
    /// Default allow = no veto (pre-v1 behavior).
    async fn tool_before(&self, _name: &str, _args: &Value) -> ToolBefore {
        ToolBefore::Allow
    }
    /// `command/run` hook (`params: {"name":"/x","argv"}` → `result: {"text"}`).
    /// Default `None` = command unclaimed (pre-v1 behavior).
    async fn run_command(&self, _name: &str, _argv: Vec<String>) -> Option<String> {
        None
    }
}

/// Later manifests win on tool-name conflict. Returns owner per tool name.
pub fn merge_manifests(manifests: Vec<Manifest>) -> HashMap<String, String> {
    let mut owner = HashMap::new();
    for m in manifests {
        for t in &m.tools {
            owner.insert(t.name.clone(), m.name.clone());
        }
    }
    owner
}
