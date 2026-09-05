//! Plugin profile: builtin plugin definitions + `gray.yml` profile loading.
//!
//! (Moved from `gray-tools/src/plugin.rs` + `gray/src/lib.rs` so `gray-tools`
//! depends on `gray-core` only. Single source of truth for the default
//! builtin plugins — [`profile_plugins`] and [`build_registry`] share it so
//! the registry cannot drift from the profile resolution.)

use std::sync::Arc;

use gray_core::agent::Tool;
use gray_plugin::{Manifest, Plugin};
use gray_tools::{
    BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, Registry, RequestUserInputTool,
    WriteTool,
};

use crate::skills_tool::SkillTool;

pub struct ToolsBasicPlugin;

impl Plugin for ToolsBasicPlugin {
    fn manifest(&self) -> Manifest {
        // Names derived from tools() so the two can't drift.
        let tools = self.tools();
        Manifest {
            name: "tools-basic".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def()).collect(),
            commands: vec![],
            hooks: vec![],
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(ReadTool),
            Arc::new(WriteTool),
            Arc::new(EditTool),
            Arc::new(BashTool),
            Arc::new(SkillTool),
            Arc::new(RequestUserInputTool),
        ]
    }
}

pub struct ToolsSearchPlugin;

impl Plugin for ToolsSearchPlugin {
    fn manifest(&self) -> Manifest {
        let tools = self.tools();
        Manifest {
            name: "tools-search".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def()).collect(),
            commands: vec![],
            hooks: vec![],
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(GrepTool), Arc::new(FindTool), Arc::new(LsTool)]
    }
}

/// Single source of truth for the default (builtin) plugins.
/// Used by [`profile_plugins`] and [`build_registry`] so the registry cannot
/// drift from the profile resolution.
fn default_plugins() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(ToolsBasicPlugin) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
    ]
}

/// Profile warnings queued for transcript display. Raw `eprintln!` while the
/// composer viewport is live collides with the next draw (ghost/overlapped
/// rows), so lib code never prints — it queues here and the UI drains.
/// One lock, one Vec: each distinct message is queued once per drain cycle
/// (N is tiny; Vec scan is fine). A rebuild re-queues a still-broken profile
/// warning — correct, like a compiler re-emitting warnings.
static PROFILE_WARNINGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn queue_profile_warning(msg: String) {
    PROFILE_WARNINGS
        .lock()
        .map(|mut q| {
            if !q.contains(&msg) {
                q.push(msg);
            }
        })
        .ok();
}

/// Drains queued profile warnings (transcript/non-TUI display owns rendering).
pub fn take_profile_warnings() -> Vec<String> {
    PROFILE_WARNINGS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Ordered plugins named by the `gray.yml` profile, or `None` when the
/// profile is missing/unparseable (caller falls back to builtin).
/// A missing file is silent (default state); parse errors and unknown names
/// queue warnings for the UI to drain. Sidecar spawn failure is a hard `Err`
/// naming entry index + argv (boot aborts).
/// Async on the ambient runtime: tokio process stdio handles are runtime-bound,
/// so sidecars must spawn on the same runtime that drives them (main/CLI entry).
async fn profile_plugins() -> anyhow::Result<Option<Vec<Arc<dyn Plugin>>>> {
    let defaults = default_plugins();
    match gray_plugin::profile::load_entries("gray.yml") {
        Ok(entries) => {
            let mut plugins = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                match e {
                    gray_plugin::profile::PluginEntry::Builtin(n) => {
                        match defaults.iter().find(|p| p.manifest().name == *n).cloned() {
                            Some(p) => plugins.push(p),
                            None => queue_profile_warning(format!(
                                "unknown plugin {n:?} in gray.yml — ignoring"
                            )),
                        }
                    }
                    gray_plugin::profile::PluginEntry::Sidecar(spec) => {
                        use anyhow::Context;
                        let label = spec.0.join(" ");
                        let plugin = gray_plugin::sidecar::SidecarPlugin::spawn(spec.0.clone())
                            .await
                            .with_context(|| format!("sidecar[{i}] ({label}) failed to spawn"))?;
                        plugins.push(Arc::new(plugin) as Arc<dyn Plugin>);
                    }
                }
            }
            Ok(Some(plugins))
        }
        Err(e) => {
            // Missing file is the default state — silent. Anything else
            // (parse error) warns once via the UI drain.
            let missing = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
            if !missing {
                queue_profile_warning(format!(
                    "cannot load gray.yml profile ({e}); using builtin plugins"
                ));
            }
            Ok(None)
        }
    }
}

/// Ordered active plugins: the `gray.yml` profile order, or builtins when
/// the profile is missing/unparseable/empty. Single spawn site shared by
/// [`build_registry`] and [`build_agent`] so sidecar children spawn once
/// per build. Returns `(plugins, used_fallback)`.
pub(crate) async fn active_plugins() -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    match profile_plugins().await? {
        Some(plugins) if !plugins.is_empty() => Ok((plugins, false)),
        _ => Ok((default_plugins(), true)),
    }
}

/// Collects tools from plugins in order; on name conflict the owner wins
/// (later manifests win, mirroring `merge_manifests`). Returns the registry
/// plus the manifests in registration order — manifests travel with the
/// registry so `--dump-manifest` can't drift from what's registered.
pub fn from_plugins(plugins: &[Arc<dyn Plugin>]) -> (Registry, Vec<Manifest>) {
    let manifests: Vec<Manifest> = plugins.iter().map(|p| p.manifest()).collect();
    let owners = gray_plugin::merge_manifests(manifests.clone());
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for p in plugins {
        let owner_name = p.manifest().name;
        for t in p.tools() {
            if owners
                .get(&t.def().name)
                .map(|o| o == &owner_name)
                .unwrap_or(false)
            {
                if let Some(pos) = tools.iter().position(|e| e.def().name == t.def().name) {
                    tools[pos] = t.clone();
                } else {
                    tools.push(t.clone());
                }
            }
        }
    }
    (Registry::new(tools), manifests)
}

/// The default builtin registry (no profile file).
pub fn builtin_registry() -> Registry {
    from_plugins(&default_plugins()).0
}

/// Builds the tool registry from the `gray.yml` profile plugin order,
/// falling back to builtins when no profile file is present.
/// Returns `(registry, manifests, used_fallback)` — the flag feeds
/// `--dump-manifest`'s note.
/// A sidecar spawn failure is a hard `Err` naming the entry (caller aborts boot).
pub async fn build_registry() -> anyhow::Result<(Registry, Vec<Manifest>, bool)> {
    let (plugins, fallback) = active_plugins().await?;
    let (registry, manifests) = from_plugins(&plugins);
    Ok((registry, manifests, fallback))
}
