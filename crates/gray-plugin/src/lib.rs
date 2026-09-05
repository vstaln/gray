use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use gray_core::agent::ToolOutput;
use gray_core::agent::{PluginCommand, PluginHooks};
use gray_core::event::Usage;
use gray_core::message::{Message, ToolDef};

pub mod profile;
pub mod sidecar;

pub use sidecar::{HOST_RUN, HOST_SAY, HostHandler, SidecarPlugin};

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
    /// Protocol version claimed by the sidecar (`None` = v1 pre-lifecycle).
    /// v1.1 sidecars send `"1.1"` and handle `plugin/shutdown` + `session`.
    #[serde(default)]
    pub protocol: Option<String>,
    /// Sandbox capabilities the sidecar declares (`exec`, `http`, `session`,
    /// `ui`). Advisory today: parsed, surfaced, and schemad — enforcement
    /// is a later pass (see `docs/plugins.md`).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Host-owned namespaces this plugin extends (`cron`, `gateway`, …).
    /// Entries route argv through the same `command/run` wire as `commands`
    /// (the adapter merges both into [`PluginHooks::commands`]).
    #[serde(default)]
    pub subcommands: Vec<String>,
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
            protocol: v.get("protocol").and_then(|s| s.as_str()).map(|s| s.into()),
            capabilities: str_list("capabilities"),
            subcommands: str_list("subcommands"),
        }
    }
}

/// Canonical `tool/before` verdict + `command/run` outcome: single source of
/// truth lives in [`gray_core::agent`] (core owns it so the agent loop never
/// depends on this crate); re-exported here so sidecar call sites keep one import.
pub use gray_core::agent::{CommandOutcome, ToolBefore};

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }
    // NOTE: an earlier `provider()` hook was deleted — every
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
    /// `command/run` hook (`params: {"name":"/x","argv"}` → `result: {"text"}` or `{"prompt"}`).
    /// Default `None` = command unclaimed (pre-v1 behavior).
    async fn run_command(&self, _name: &str, _argv: Vec<String>) -> Option<CommandOutcome> {
        None
    }
    /// Lifecycle teardown: host is going away (`session_end`/`host_exit`/`reload`
    /// sent on the wire by the sidecar transport). Default no-op so existing
    /// impls keep compiling.
    async fn shutdown(&self) {}
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

/// Thin bridge from any [`Plugin`] (notably the sidecar transport) to the
/// host-side [`PluginHooks`] view the agent loop and REPL consume.
/// Exists because the two signatures differ: `Plugin::prompt_context`
/// takes the cwd per call while `PluginHooks::prompt_context` takes none,
/// so the adapter pins the boot cwd once. `tool_before` needs no mapping
/// (one canonical [`ToolBefore`]); `commands` projects the manifest's
/// names (the wire carries no per-command descriptions).
pub struct PluginHookAdapter {
    plugin: Arc<dyn Plugin>,
    cwd: String,
}

impl PluginHookAdapter {
    pub fn new(plugin: Arc<dyn Plugin>, cwd: impl Into<String>) -> Self {
        Self { plugin, cwd: cwd.into() }
    }

    /// One adapter per plugin, in boot order (hook order = plugin order).
    pub fn for_plugins(plugins: &[Arc<dyn Plugin>], cwd: &str) -> Vec<Arc<dyn PluginHooks>> {
        plugins.iter().map(|p| Arc::new(Self::new(p.clone(), cwd)) as Arc<dyn PluginHooks>).collect()
    }
}

#[async_trait]
impl PluginHooks for PluginHookAdapter {
    async fn prompt_context(&self) -> Option<String> {
        self.plugin.prompt_context(&self.cwd).await
    }
    async fn tool_before(&self, name: &str, args: &Value) -> ToolBefore {
        self.plugin.tool_before(name, args).await
    }
    fn commands(&self) -> Vec<PluginCommand> {
        // `subcommands` ride the same `command/run` wire: the REPL routes
        // them to the owning plugin with argv untouched.
        let m = self.plugin.manifest();
        m.commands
            .into_iter()
            .chain(m.subcommands)
            .map(|name| PluginCommand { name, description: String::new() })
            .collect()
    }
    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
        match self.plugin.run_command(name, argv).await? {
            CommandOutcome::Say(s) if s.is_empty() => None,
            CommandOutcome::Prompt(s) if s.is_empty() => None,
            o => Some(o),
        }
    }
    async fn pre_tool(&self, name: &str, args: &Value) {
        let _ = self
            .plugin
            .on_event(CoreEvent::PreTool { name: name.to_string(), args: args.clone() })
            .await;
    }
    async fn post_tool(&self, name: &str, output: &ToolOutput) {
        let _ = self
            .plugin
            .on_event(CoreEvent::PostTool { name: name.to_string(), output: output.clone() })
            .await;
    }
    async fn turn_end(&self, usage: &Usage) {
        let _ = self.plugin.on_event(CoreEvent::TurnEnd { usage: *usage }).await;
    }
    async fn shutdown(&self) {
        self.plugin.shutdown().await;
    }
}
