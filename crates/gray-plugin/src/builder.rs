//! One profile-aware agent builder for every surface (REPL, `-p`, gateway, cron).
//!
//! Lives here (not in `gray`) because the `gray → gray-gateway` edge forbids
//! the gateway from calling `gray::build_agent` — this crate is the lowest
//! common crate all hosts already depend on. `gray-tools` stays core-only
//! (no tools→cron/gateway edges); the direction here is plugin→tools/provider.
//!
//! Surface policy stays with the callers: the system prompt (skills/context
//! vs gateway suffix), the executor wrapper (plain vs `GatedExecutor`), the
//! host handler, and abort-vs-warn on sidecar spawn failure all arrive via
//! [`BuilderOptions`]. Cron needs no direct call — the sidecar fires through
//! `host/run` (`gray -p`) and gateway delivery runs through `run_agent`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use gray_core::agent::{Agent, Tool, ToolExecutor};
use gray_provider::OpenAiProvider;
use gray_tools::Registry;

use crate::profile::{PluginEntry, load_entries};
use crate::{HostHandler, Manifest, Plugin, PluginHookAdapter, SidecarPlugin, merge_manifests};

// ---------------------------------------------------------------------------
// Builtin plugins (single definition; callers add surface extras via options)
// ---------------------------------------------------------------------------

/// `tools-basic` file/shell set. `extra` holds surface tools owned elsewhere
/// (`SkillTool` lives in `gray` so `gray-tools` stays core-only).
#[derive(Default)]
pub struct ToolsBasicPlugin {
    pub extra: Vec<Arc<dyn Tool>>,
}

impl Plugin for ToolsBasicPlugin {
    fn manifest(&self) -> Manifest {
        let tools = self.tools();
        Manifest {
            name: "tools-basic".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def()).collect(),
            commands: vec![],
            hooks: vec![],
            provider: None,
            protocol: None,
            capabilities: vec![],
            subcommands: vec![],
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        // T3.2/T3.3 wiring: read/write/edit share one ledger per tools() call
        // (from_plugins calls once per build, so the session tools agree).
        // No struct field: ToolsBasicPlugin literals also live in gray::profile,
        // which must keep compiling untouched — T3.4 adopts this ledger into
        // Registry::file_ledger for /new + compaction.
        let ledger = Arc::new(gray_tools::FileLedger::new());
        let mut out: Vec<Arc<dyn Tool>> = vec![
            Arc::new(gray_tools::ReadTool::new(ledger.clone())),
            Arc::new(gray_tools::WriteTool::new(ledger.clone())),
            Arc::new(gray_tools::EditTool::new(ledger.clone())),
            Arc::new(gray_tools::BashTool),
            Arc::new(gray_tools::RequestUserInputTool),
        ];
        out.extend(self.extra.iter().cloned());
        out
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
            protocol: None,
            capabilities: vec![],
            subcommands: vec![],
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(gray_tools::GrepTool),
            Arc::new(gray_tools::FindTool),
            Arc::new(gray_tools::LsTool),
        ]
    }
}

/// Default builtin plugins (no surface extras; pass those via
/// [`BuilderOptions::extra_tools`]).
pub fn default_plugins() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(ToolsBasicPlugin::default()) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
    ]
}

// ---------------------------------------------------------------------------
// Registry assembly (moved from `gray::profile`; same later-wins semantics)
// ---------------------------------------------------------------------------

/// Collects tools from plugins in order; on name conflict the owner wins
/// (later manifests win, mirroring `merge_manifests`). Returns the registry
/// plus the manifests in registration order — manifests travel with the
/// registry so `--dump-manifest` can't drift from what's registered.
pub fn from_plugins(plugins: &[Arc<dyn Plugin>]) -> (Registry, Vec<Manifest>) {
    let manifests: Vec<Manifest> = plugins.iter().map(|p| p.manifest()).collect();
    let owners = merge_manifests(manifests.clone());
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
    // T3.4 adoption: one ledger shared by the session tools AND
    // Registry::file_ledger, so the binary's /new + compaction lifecycle acts
    // on the same state the tools use. ToolsBasicPlugin::tools() already
    // shares one ledger per build, but Registry::new makes its own — rebuild
    // the tools-basic read/write/edit on the adopted ledger instead (a
    // sidecar-owned name is left alone). Fresh per build on purpose: a reused
    // ledger would leak reads across sessions in multi-session hosts.
    let ledger = Arc::new(gray_tools::FileLedger::new());
    for t in tools.iter_mut() {
        let name = t.def().name.clone();
        if owners.get(&name).map(|o| o.as_str()) != Some("tools-basic") {
            continue;
        }
        let fresh: Option<Arc<dyn Tool>> = match name.as_str() {
            "read" => Some(Arc::new(gray_tools::ReadTool::new(ledger.clone()))),
            "write" => Some(Arc::new(gray_tools::WriteTool::new(ledger.clone()))),
            "edit" => Some(Arc::new(gray_tools::EditTool::new(ledger.clone()))),
            _ => None,
        };
        if let Some(f) = fresh {
            *t = f;
        }
    }
    let mut registry = Registry::new(tools);
    registry.set_file_ledger(ledger.clone());
    track_current_ledger(&ledger);
    (registry, manifests)
}

// ---------------------------------------------------------------------------
// Session ledger lifecycle (T3.4)
// ---------------------------------------------------------------------------

// Latest build's FileLedger (Weak: builds own their ledger; the binary
// clears/disarms via this handle for /new, resume, and compaction while
// gray-core stays ignorant of tools).
static CURRENT_LEDGER: std::sync::Mutex<Option<std::sync::Weak<gray_tools::FileLedger>>> =
    std::sync::Mutex::new(None);

/// The ledger adopted by the latest [`from_plugins`] build, if still alive.
pub fn current_file_ledger() -> Option<Arc<gray_tools::FileLedger>> {
    CURRENT_LEDGER.lock().ok()?.as_ref()?.upgrade()
}

fn track_current_ledger(ledger: &Arc<gray_tools::FileLedger>) {
    if let Ok(mut g) = CURRENT_LEDGER.lock() {
        *g = Some(Arc::downgrade(ledger));
    }
}

// ---------------------------------------------------------------------------
// Builder warnings (same queue-then-drain pattern as the REPL's profile queue;
// each surface drains in its own idiom: transcript vs log)
// ---------------------------------------------------------------------------

static BUILD_WARNINGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn push_builder_warning(msg: String) {
    BUILD_WARNINGS
        .lock()
        .map(|mut q| {
            if !q.contains(&msg) {
                q.push(msg);
            }
        })
        .ok();
}

/// Drains warnings queued during [`active_plugins`] (unknown names, skipped
/// sidecars, unparseable profile).
pub fn take_builder_warnings() -> Vec<String> {
    BUILD_WARNINGS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Profile resolution
// ---------------------------------------------------------------------------

/// Ordered active plugins: the profile file order, or `defaults` when the
/// profile is missing/unparseable/empty. Sidecars spawn once per build with
/// `handler` installed (`host/run`/`host/say`). A spawn failure aborts when
/// `abort_on_spawn_failure` (interactive boot) else warns + skips (the daemon
/// must stay up). Unknown builtin names always warn + skip.
pub async fn active_plugins(
    defaults: Vec<Arc<dyn Plugin>>,
    profile_path: &str,
    handler: Option<HostHandler>,
    abort_on_spawn_failure: bool,
) -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    match load_entries(profile_path) {
        Ok(entries) => {
            let mut plugins = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                match e {
                    PluginEntry::Builtin(n) => {
                        match defaults.iter().find(|p| p.manifest().name == *n).cloned() {
                            Some(p) => plugins.push(p),
                            None => push_builder_warning(format!(
                                "unknown plugin {n:?} in {profile_path} — ignoring"
                            )),
                        }
                    }
                    PluginEntry::Sidecar(spec) => {
                        let label = spec.0.join(" ");
                        match SidecarPlugin::spawn(spec.0.clone()).await {
                            Ok(p) => {
                                if let Some(h) = &handler {
                                    p.set_host_handler(h.clone()).await;
                                }
                                plugins.push(Arc::new(p) as Arc<dyn Plugin>);
                            }
                            Err(e) if abort_on_spawn_failure => {
                                return Err(e).with_context(|| {
                                    format!("sidecar[{i}] ({label}) failed to spawn")
                                });
                            }
                            Err(e) => push_builder_warning(format!(
                                "sidecar[{i}] ({label}) failed to spawn, skipping: {e:#}"
                            )),
                        }
                    }
                }
            }
            if plugins.is_empty() {
                Ok((defaults, true))
            } else {
                Ok((plugins, false))
            }
        }
        Err(e) => {
            // Missing file is the default state — silent. Anything else
            // (parse error) warns once via the caller's drain.
            let missing = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
            if !missing {
                push_builder_warning(format!(
                    "cannot load {profile_path} profile ({e}); using builtin plugins"
                ));
            }
            Ok((defaults, true))
        }
    }
}

// ---------------------------------------------------------------------------
// Provider cache key (moved from `gray`; same truncate-don't-hash rule)
// ---------------------------------------------------------------------------

/// Max `prompt_cache_key` length (matches the Responses API cache-key limit).
pub const PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

/// Clamp a cache key to the max length (truncate, don't hash —
/// the prefix stays human-grepable in logs).
pub fn clamp_prompt_cache_key(key: &str) -> &str {
    if key.len() <= PROMPT_CACHE_KEY_MAX_LENGTH {
        return key;
    }
    // UUIDs are ASCII so byte cut == char cut; walk back over a boundary just in case.
    let mut end = PROMPT_CACHE_KEY_MAX_LENGTH;
    while !key.is_char_boundary(end) {
        end -= 1;
    }
    &key[..end]
}

/// Resolves the Responses `prompt_cache_key`: the session id when known
/// (stable across resumes, so a resumed session keeps its cache shard), else
/// a per-process stable id (rebuilds mid-session must not bust the shard).
/// A fresh random key per build guaranteed 0% cache; never do that.
pub fn provider_cache_key(session_id: Option<&str>) -> String {
    if let Some(s) = session_id.filter(|s| !s.is_empty()) {
        return clamp_prompt_cache_key(s).to_string();
    }
    static FALLBACK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FALLBACK
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone()
}

// ---------------------------------------------------------------------------
// The single builder
// ---------------------------------------------------------------------------

/// Builds the `system` prompt: either a ready string (gateway) or a
/// registry-aware closure (REPL/`-p` tool snippets + guidelines need the
/// resolved registry, which only exists after profile resolution).
pub enum SystemPrompt {
    Literal(String),
    Build(PromptBuilder),
}

/// `FnOnce(&Registry) -> String`: caller builds its prompt from the resolved
/// registry (snippets, names, guidelines).
pub type PromptBuilder = Box<dyn FnOnce(&Registry) -> String + Send>;

/// Wraps the profile-built registry executor (gateway: `GatedExecutor`;
/// `None` = plain registry).
pub type ExecutorWrap = Box<dyn FnOnce(Box<dyn ToolExecutor>) -> Box<dyn ToolExecutor> + Send>;

pub struct BuilderOptions {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub reasoning_effort: Option<String>,
    /// Pins the Responses cache shard; gateway threads its session id so
    /// daemon sessions don't all collide on the per-process fallback key.
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: SystemPrompt,
    /// Surface tools baked into `tools-basic` (gray: `SkillTool`).
    pub extra_tools: Vec<Arc<dyn Tool>>,
    pub host_handler: Option<HostHandler>,
    pub profile_path: String,
    pub abort_on_spawn_failure: bool,
    pub wrap_executor: Option<ExecutorWrap>,
}

/// Profile-aware [`Agent`] builder used by all surfaces: resolves
/// `profile_plugins → Registry::from_plugins else builtin`, wires sidecar
/// hooks, and constructs the provider. Surface warnings (unknown plugins,
/// skipped sidecars) queue in [`take_builder_warnings`] for the caller to
/// drain; spawn aborts only when `abort_on_spawn_failure`.
pub async fn build_agent(opts: BuilderOptions) -> anyhow::Result<Agent> {
    let BuilderOptions {
        model,
        api_key,
        base_url,
        reasoning_effort,
        session_id,
        cwd,
        system_prompt,
        extra_tools,
        host_handler,
        profile_path,
        abort_on_spawn_failure,
        wrap_executor,
    } = opts;
    let defaults: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(ToolsBasicPlugin { extra: extra_tools }) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
    ];
    let (plugins, _) = active_plugins(
        defaults,
        &profile_path,
        host_handler,
        abort_on_spawn_failure,
    )
    .await?;
    let (registry, _) = from_plugins(&plugins);
    let system = match system_prompt {
        SystemPrompt::Literal(s) => s,
        SystemPrompt::Build(f) => f(&registry),
    };
    let provider = OpenAiProvider::builder(api_key, model)
        .base_url(base_url)
        .reasoning_effort(reasoning_effort)
        .session_id(provider_cache_key(session_id.as_deref()))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to initialize OpenAI provider: {e}"))?;

    let tool_defs = registry.defs();
    let executor: Box<dyn ToolExecutor> = match wrap_executor {
        Some(wrap) => wrap(Box::new(registry)),
        None => Box::new(registry),
    };
    let hooks = PluginHookAdapter::for_plugins(&plugins, &cwd.to_string_lossy());
    Ok(Agent::new(Box::new(provider), executor)
        .with_system(system)
        .with_tools(tool_defs)
        .with_hooks(hooks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::{ToolContext, ToolExecutor};
    use serde_json::json;

    #[tokio::test]
    async fn from_plugins_adopts_one_ledger_into_registry() {
        // Deferred T3.2 item: the registry's file_ledger must be the same
        // state the session read/write/edit tools use.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("note.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let (reg, _) = from_plugins(&default_plugins());
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let out = ToolExecutor::execute(&reg, &ctx, "read", json!({"path": "note.txt"})).await;
        assert!(!out.is_error, "{out:?}");
        assert!(
            reg.file_ledger().get(&p).is_some(),
            "read must record into Registry::file_ledger"
        );
        // ... and the write tool honors it (no force needed after a full read).
        let out = ToolExecutor::execute(
            &reg,
            &ctx,
            "write",
            json!({"path": "note.txt", "content": "hello\nworld\n"}),
        )
        .await;
        assert!(!out.is_error, "{out:?}");
        // Lifecycle handle tracks this build's ledger.
        assert!(current_file_ledger().is_some());
    }
}
