use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use gray_core::agent::ToolOutput;
use gray_core::agent::{PluginCommand, PluginHooks};
use gray_core::event::Usage;
use gray_core::message::{Message, ToolDef};

pub mod builder;
pub mod capabilities;
pub mod host;
pub mod lazy;
mod live;
pub mod lock;
pub mod profile;
mod provider;
pub mod scan;
pub mod sidecar;

pub use lazy::{LazySidecarPlugin, cached_manifest, lazy_enabled};
pub use live::LiveRegistry;
pub use sidecar::{refresh_dynamic_tools, set_active_model};

pub use sidecar::{
    ASK_HANDLER_TTL, ASK_TTL, HOST_ASK, HOST_RUN, HOST_SAY, HOST_TTL, HostHandler, SidecarPlugin,
};

pub use capabilities::{
    CapabilitySpec, HOST_ASK as CAP_HOST_ASK, HOST_SAY as CAP_HOST_SAY, HOST_TURN, SHELL_ENV,
    TOOL_OVERRIDE, WIDGET_OVERRIDE,
};

pub const DYNAMIC_PROTOCOL: &str = "1.3";

pub use provider::{
    AuthMethodDecl, ModelSlot, ModelVariant, PROVIDER_CREDENTIALS, PROVIDER_PROTOCOL,
    ProviderAuthPoll, ProviderAuthStart, ProviderAuthorizationDecl, ProviderChatRequest,
    ProviderChatResult, ProviderDecl, ProviderHeaderDecl, ProviderHeaderSourceDecl, ProviderModel,
    ProviderModelCatalog, ProviderModelsRequest, ProviderRefreshRequest, ProviderRequestPolicyDecl,
    ProviderRevokeRequest, ProviderRevokeResult, ProviderRpcError, ProviderRpcFailure,
    ProviderTransportDecl, ProviderUsageLimits, ProviderUsageRequest, ProviderUsageWindow,
    ProviderValidationError, SlotOption,
};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    PreTool { name: String, args: Value },
    PostTool { name: String, output: ToolOutput },
    TurnEnd { usage: Usage },
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,

    /// First-argv verbs each declared command may run mid-turn; `""` is
    /// the bare command and `"*"` allows any argv. Keys that are not
    /// among `commands` are dropped at parse time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub turn_local: HashMap<String, Vec<String>>,

    /// Spawn the plugin at session start instead of on first use — for
    /// plugins that run background work the session depends on (e.g. the
    /// gateway's cron ticker).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub autostart: bool,

    #[serde(default)]
    pub protocol: Option<String>,

    #[serde(default)]
    pub subcommands: Vec<String>,

    /// CLI verbs the plugin advertises — `gray <name> <verb>` — also used
    /// as the slash command's completion set in the REPL.
    #[serde(default)]
    pub completion: Vec<String>,

    #[serde(default)]
    pub capabilities: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<ShellEnvDecl>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_plugins: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderDecl>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composite_rows: Vec<CompositeRowDecl>,
    #[serde(skip)]
    pub provider_errors: Vec<ProviderValidationError>,
}

/// A plugin's `shell_env` manifest field: static environment contributions
/// for the bash tool's shell, gated by the `shell.env` capability.
/// `path_prepend` accepts absolute directories only.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShellEnvDecl {
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub path_prepend: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompositeRowDecl {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub slots: Vec<CompositeSlotDecl>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompositeSlotDecl {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub source: CompositeSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompositeSource {
    #[default]
    Own,
    All,
}

pub fn parse_tool_entry(v: &Value) -> Option<ToolDef> {
    if let Some(name) = v.as_str() {
        return Some(ToolDef::new(
            name,
            format!("sidecar tool {name}"),
            serde_json::json!({}),
        ));
    }
    let obj = v.as_object()?;
    let name = obj.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    let description = obj
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    let parameters = obj
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut def = ToolDef::new(name, description, parameters);
    if let Some(label) = obj.get("label").and_then(|l| l.as_str()) {
        let label = label.trim();
        if !label.is_empty() {
            def = def.with_label(label.to_string());
        }
    }
    if let Some(preview) = obj.get("preview").and_then(|x| x.as_str()) {
        let preview = preview.trim();
        if !preview.is_empty() {
            def = def.with_preview(preview.to_string());
        }
    }
    Some(def)
}

pub fn manifest_tools(v: &Value) -> Vec<ToolDef> {
    v.get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(parse_tool_entry).collect())
        .unwrap_or_default()
}

impl Manifest {
    pub fn from_result(v: &Value) -> Self {
        let str_list = |key: &str| {
            v.get(key)
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut providers = Vec::new();
        let mut provider_errors = Vec::new();
        let protocol = v.get("protocol").and_then(|s| s.as_str()).unwrap_or("1.0");
        if let Some(raw_providers) = v.get("providers") {
            if protocol == PROVIDER_PROTOCOL || protocol == DYNAMIC_PROTOCOL {
                match raw_providers.as_array() {
                    Some(raw_providers) => {
                        let mut ids = std::collections::BTreeSet::new();
                        for raw in raw_providers {
                            match ProviderDecl::from_value(raw) {
                                Ok(provider) if !ids.insert(provider.id.clone()) => {
                                    provider_errors.push(ProviderValidationError {
                                        message: "duplicate provider id".into(),
                                    });
                                }
                                Ok(provider) => providers.push(provider),
                                Err(error) => provider_errors.push(error),
                            }
                        }
                    }
                    None => provider_errors.push(ProviderValidationError {
                        message: "providers must be an array".into(),
                    }),
                }
            } else if raw_providers
                .as_array()
                .is_some_and(|providers| !providers.is_empty())
            {
                provider_errors.push(ProviderValidationError {
                    message: "providers require protocol 1.2 or later".into(),
                });
            }
        }
        let commands: Vec<String> = str_list("commands");
        let turn_local = v
            .get("turn_local")
            .and_then(|t| t.as_object())
            .map(|obj| {
                obj.iter()
                    .filter(|(k, _)| commands.iter().any(|c| c == *k))
                    .map(|(k, val)| {
                        let verbs = val
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|e| e.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (k.clone(), verbs)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            name: v
                .get("name")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .into(),
            version: v
                .get("version")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .into(),
            tools: manifest_tools(v),
            commands,
            turn_local,
            hooks: str_list("hooks"),
            autostart: v
                .get("autostart")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            protocol: v.get("protocol").and_then(|s| s.as_str()).map(|s| s.into()),
            subcommands: str_list("subcommands"),
            completion: str_list("completion"),
            capabilities: crate::capabilities::parse_declared(&str_list("capabilities")),
            shell_env: v.get("shell_env").and_then(|raw| {
                serde_json::from_value(raw.clone())
                    .map_err(|e| log::warn!("ignoring invalid shell_env in plugin manifest: {e}"))
                    .ok()
            }),
            requires_plugins: str_list("requires_plugins"),
            providers,
            composite_rows: serde_json::from_value(
                v.get("composite_rows").cloned().unwrap_or_default(),
            )
            .unwrap_or_default(),
            provider_errors,
        }
    }
}

pub use gray_core::agent::{CommandOutcome, InputSubmit, ToolBefore};

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }

    fn capabilities(&self) -> Vec<String> {
        Vec::new()
    }
    async fn on_event(&self, _e: CoreEvent) {}

    async fn prompt_context(&self, _cwd: &str) -> Option<String> {
        None
    }

    fn set_session_id(&self, _id: &str) {}

    async fn tool_before(&self, _name: &str, _args: &Value) -> ToolBefore {
        ToolBefore::Allow
    }

    async fn run_command(&self, _name: &str, _argv: Vec<String>) -> Option<CommandOutcome> {
        None
    }

    async fn input_submit(&self, _text: &str) -> InputSubmit {
        InputSubmit::Pass
    }

    async fn agent_before_start(&self) -> Option<String> {
        None
    }

    async fn context_build(&self, _messages: &[Message]) -> Option<Vec<Message>> {
        None
    }

    async fn tool_after(&self, _name: &str, _output: &ToolOutput) -> Option<ToolOutput> {
        None
    }

    async fn shutdown(&self) {}

    fn is_global(&self) -> bool {
        false
    }

    fn set_global(&self, _global: bool) {}
}

pub fn merge_manifests(manifests: Vec<Manifest>) -> HashMap<String, String> {
    let mut owner = HashMap::new();
    for m in manifests {
        for t in &m.tools {
            owner.insert(t.name.clone(), m.name.clone());
        }
    }
    owner
}

pub struct PluginHookAdapter {
    plugin: Arc<dyn Plugin>,
    cwd: String,
}

impl PluginHookAdapter {
    pub fn new(plugin: Arc<dyn Plugin>, cwd: impl Into<String>) -> Self {
        Self {
            plugin,
            cwd: cwd.into(),
        }
    }

    pub fn for_plugins(plugins: &[Arc<dyn Plugin>], cwd: &str) -> Vec<Arc<dyn PluginHooks>> {
        plugins
            .iter()
            .map(|p| Arc::new(Self::new(p.clone(), cwd)) as Arc<dyn PluginHooks>)
            .collect()
    }
}

#[async_trait]
impl PluginHooks for PluginHookAdapter {
    fn is_global(&self) -> bool {
        self.plugin.is_global()
    }
    async fn prompt_context(&self) -> Option<String> {
        self.plugin.prompt_context(&self.cwd).await
    }
    async fn tool_before(&self, name: &str, args: &Value) -> ToolBefore {
        self.plugin.tool_before(name, args).await
    }
    fn commands(&self) -> Vec<PluginCommand> {
        let m = self.plugin.manifest();
        m.commands
            .iter()
            .chain(m.subcommands.iter())
            .map(|name| PluginCommand {
                turn_local_verbs: m.turn_local.get(name).cloned().unwrap_or_default(),
                name: name.clone(),
                description: String::new(),
            })
            .collect()
    }
    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
        match self.plugin.run_command(name, argv).await? {
            CommandOutcome::Say(s) if s.is_empty() => None,
            CommandOutcome::Prompt(s) if s.is_empty() => None,
            CommandOutcome::ModelPicker(s) if s.is_empty() => None,
            CommandOutcome::AgentPicker(s) if s.is_empty() => None,
            CommandOutcome::SubagentSettings(s) if s.is_empty() => None,
            o => Some(o),
        }
    }
    async fn pre_tool(&self, name: &str, args: &Value) {
        self.plugin
            .on_event(CoreEvent::PreTool {
                name: name.to_string(),
                args: args.clone(),
            })
            .await;
    }
    async fn post_tool(&self, name: &str, output: &ToolOutput) {
        self.plugin
            .on_event(CoreEvent::PostTool {
                name: name.to_string(),
                output: output.clone(),
            })
            .await;
    }
    async fn turn_end(&self, usage: &Usage) {
        self.plugin
            .on_event(CoreEvent::TurnEnd { usage: *usage })
            .await;
    }
    async fn input_submit(&self, text: &str) -> InputSubmit {
        self.plugin.input_submit(text).await
    }
    async fn agent_before_start(&self) -> Option<String> {
        self.plugin.agent_before_start().await
    }
    async fn context_build(&self, messages: &[Message]) -> Option<Vec<Message>> {
        self.plugin.context_build(messages).await
    }
    async fn tool_after(&self, name: &str, output: &ToolOutput) -> Option<ToolOutput> {
        self.plugin.tool_after(name, output).await
    }
    async fn shutdown(&self) {
        self.plugin.shutdown().await;
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod provider_tests;
