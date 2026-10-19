use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use gray_core::agent::{Agent, Tool, ToolExecutor};
use gray_core::credential::CredentialSource;
use gray_provider::{OpenAiProvider, OpenAiProviderProfile};
use gray_tools::Registry;

use crate::profile::{PluginEntry, load_entries};
use crate::{HostHandler, Manifest, Plugin, PluginHookAdapter, SidecarPlugin, merge_manifests};

fn is_live_plugin(p: &dyn Plugin) -> bool {
    p.manifest().protocol.as_deref() == Some("1.3")
}

pub fn dynamic_plugins(plugins: &[Arc<dyn Plugin>]) -> Vec<Arc<dyn Plugin>> {
    plugins
        .iter()
        .filter(|p| is_live_plugin(p.as_ref()))
        .cloned()
        .collect()
}

pub fn from_plugins(plugins: &[Arc<dyn Plugin>]) -> (Registry, Vec<Manifest>) {
    let manifests: Vec<Manifest> = plugins.iter().map(|p| p.manifest()).collect();
    let owners = merge_manifests(manifests.clone());

    let ledger = Arc::new(gray_tools::FileLedger::new());
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(
        gray_tools::BashTool::default()
            .with_ledger(ledger.clone())
            .with_plugin_env(merged_shell_env(plugins)),
    )];
    for p in plugins {
        let owner_name = p.manifest().name;

        if is_live_plugin(p.as_ref()) {
            continue;
        }
        let may_override = p
            .capabilities()
            .iter()
            .any(|c| c == crate::capabilities::TOOL_OVERRIDE);
        for t in p.tools() {
            if t.def().name == "bash" && !may_override {
                push_builder_warning(format!(
                    "plugin `{}` claims built-in tool `bash`; grant `{}` to let it (gray plugin capabilities {})",
                    owner_name,
                    crate::capabilities::TOOL_OVERRIDE,
                    owner_name
                ));
                continue;
            }
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
    let registry = Registry::new(tools);
    track_current_ledger(&ledger);
    (registry, manifests)
}

/// Fold each enabled plugin's `shell_env` manifest field into one
/// contribution, in registry order. A plugin without the `shell.env` grant
/// contributes nothing; per-entry validation (reserved names, non-absolute
/// paths) happens inside [`gray_tools::PluginEnv::add_plugin`].
fn merged_shell_env(plugins: &[Arc<dyn Plugin>]) -> gray_tools::PluginEnv {
    let mut env = gray_tools::PluginEnv::default();
    for p in plugins {
        let manifest = p.manifest();
        let Some(decl) = &manifest.shell_env else {
            continue;
        };
        if !p
            .capabilities()
            .iter()
            .any(|c| c == crate::capabilities::SHELL_ENV)
        {
            push_builder_warning(format!(
                "plugin {:?} declares shell_env without the `{}` grant — ignoring it (gray plugin capabilities {} --all)",
                manifest.name,
                crate::capabilities::SHELL_ENV,
                manifest.name
            ));
            continue;
        }
        env.add_plugin(
            &manifest.name,
            decl.vars.iter().map(|(k, v)| (k.clone(), v.clone())),
            decl.path_prepend.iter().map(PathBuf::from),
        );
    }
    env
}

static CURRENT_LEDGER: std::sync::Mutex<Option<std::sync::Weak<gray_tools::FileLedger>>> =
    std::sync::Mutex::new(None);

pub fn current_file_ledger() -> Option<Arc<gray_tools::FileLedger>> {
    CURRENT_LEDGER.lock().ok()?.as_ref()?.upgrade()
}

fn track_current_ledger(ledger: &Arc<gray_tools::FileLedger>) {
    if let Ok(mut g) = CURRENT_LEDGER.lock() {
        *g = Some(Arc::downgrade(ledger));
    }
}

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

pub fn take_builder_warnings() -> Vec<String> {
    BUILD_WARNINGS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub name: String,
    pub error: Option<String>,
}

static SPAWN_OUTCOMES: std::sync::Mutex<Option<Vec<SpawnOutcome>>> = std::sync::Mutex::new(None);

pub fn spawn_outcomes() -> Option<Vec<SpawnOutcome>> {
    SPAWN_OUTCOMES.lock().ok().and_then(|s| s.clone())
}

pub fn resolve_argv(dir: &Path) -> anyhow::Result<Vec<String>> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        let Ok(m) = std::fs::metadata(p) else {
            return false;
        };
        if !m.is_file() {
            return false;
        }

        #[cfg(unix)]
        {
            m.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    };
    if is_exec(dir) {
        return Ok(vec![dir.to_string_lossy().into_owned()]);
    }
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory (or executable)", dir.display());
    }
    let script = dir.join("plugin.sh");
    if is_exec(&script) {
        return Ok(vec![script.to_string_lossy().into_owned()]);
    }
    let triple = if cfg!(windows) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-musl"
    } else {
        "x86_64-unknown-linux-musl"
    };
    let leg_dir = dir.join(triple);
    if leg_dir.is_dir()
        && let Ok(argv) = resolve_argv(&leg_dir)
    {
        return Ok(argv);
    }
    let mut execs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if is_exec(&path) {
            execs.push(path);
        }
    }
    match execs.len() {
        1 => Ok(vec![execs[0].to_string_lossy().into_owned()]),
        0 => anyhow::bail!(
            "no executable in {} (expected plugin.sh or one executable)",
            dir.display()
        ),
        _ => anyhow::bail!(
            "ambiguous plugin dir {}: several executables, add plugin.sh",
            dir.display()
        ),
    }
}

fn load_lock_files(cwd: &Path) -> (crate::lock::LockFile, crate::lock::LockFile, Vec<String>) {
    use std::collections::BTreeMap;
    let mut warnings = Vec::new();
    let mut load = |path: PathBuf| -> crate::lock::LockFile {
        match crate::lock::LockFile::load(&path) {
            Ok(lf) => lf,
            Err(e) => {
                warnings.push(format!("cannot load {} ({e:#}); ignoring", path.display()));
                crate::lock::LockFile {
                    schema: 1,
                    plugins: BTreeMap::new(),
                }
            }
        }
    };

    let user = gray_core::paths::gray_home()
        .map(|h| load(crate::lock::lock_path(&h)))
        .unwrap_or(crate::lock::LockFile {
            schema: 1,
            plugins: BTreeMap::new(),
        });
    let project = load(crate::lock::project_lock_path(cwd));
    (user, project, warnings)
}

pub async fn active_plugins(
    defaults: Vec<Arc<dyn Plugin>>,
    default_names: &[&str],
    profile_path: &str,
    handler: Option<HostHandler>,
    abort_on_spawn_failure: bool,
) -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    let entries: Vec<PluginEntry> = match load_entries(profile_path) {
        Ok(entries) => entries,
        Err(e) => {
            let missing = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
            if !missing {
                push_builder_warning(format!(
                    "cannot load {profile_path} profile ({e}); using builtin plugins"
                ));
            }
            Vec::new()
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (user_lock, project_lock, lock_warnings) = load_lock_files(&cwd);
    for w in lock_warnings {
        push_builder_warning(w);
    }

    let disabled = crate::lock::disabled_sidecar_argvs(&user_lock, &project_lock);

    let pdir = gray_core::paths::gray_home().map(|h| h.join("plugins"));
    let mut disabled_paths: Vec<String> = Vec::new();
    let mut disabled_dirs: Vec<String> = Vec::new();
    for name in user_lock.plugins.keys() {
        if crate::lock::effective_enabled(&user_lock, &project_lock, name) {
            continue;
        }
        if let Some(pd) = &pdir {
            let dir = pd.join(name);
            disabled_dirs.push(dir.to_string_lossy().into_owned());
            if let Ok(argv) = resolve_argv(&dir) {
                disabled_paths.extend(argv);
            }
        }
    }
    let profile_disabled = |spec: &[String]| -> bool {
        if !spec.is_empty() && disabled.contains(&spec.to_vec()) {
            return true;
        }

        if spec.len() == 1 {
            if disabled_paths.contains(&spec[0]) || disabled_dirs.contains(&spec[0]) {
                return true;
            }
            if let Ok(argv) = resolve_argv(Path::new(&spec[0]))
                && argv.iter().any(|a| disabled_paths.contains(a))
            {
                return true;
            }
        }
        false
    };

    let mut plugins = if entries.is_empty() {
        defaults
            .iter()
            .filter(|p| default_names.contains(&p.manifest().name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
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
                if spec.0.iter().any(|a| a == "-y" || a == "--yes")
                    || spec
                        .0
                        .first()
                        .is_some_and(|p| p.ends_with("npx") || p.ends_with("uvx"))
                {
                    push_builder_warning(format!(
                        "sidecar[{i}] auto-downloads its package ({}) — only install sidecars you trust",
                        spec.0.join(" ")
                    ));
                }
                if profile_disabled(&spec.0) {
                    push_builder_warning(format!(
                        "sidecar[{i}] disabled in plugin lock — skipping"
                    ));
                    continue;
                }
                let label = spec.0.join(" ");
                match SidecarPlugin::spawn(spec.0.clone()).await {
                    Ok(p) => {
                        if let Some(h) = &handler {
                            p.set_host_handler(h.clone()).await;
                        }

                        p.set_capabilities(p.manifest().capabilities.clone());
                        plugins.push(Arc::new(p) as Arc<dyn Plugin>);
                    }
                    Err(e) if abort_on_spawn_failure => {
                        return Err(e)
                            .with_context(|| format!("sidecar[{i}] ({label}) failed to spawn"));
                    }
                    Err(e) => push_builder_warning(format!(
                        "sidecar[{i}] ({label}) failed to spawn, skipping: {e:#}"
                    )),
                }
            }
        }
    }

    let mut outcomes: Vec<SpawnOutcome> = Vec::new();
    for (name, entry) in user_lock.plugins.iter() {
        if !crate::lock::effective_enabled(&user_lock, &project_lock, name) {
            continue;
        }

        if entry.runtime_role.as_deref() == Some("provider_only") {
            continue;
        }
        if let Some(p) =
            spawn_lock_plugin(name, entry, pdir.as_deref(), &handler, &mut outcomes).await
        {
            plugins.push(p);
        }
    }

    let global_home = gray_core::paths::gray_user_home();
    let same_home = match (&gray_core::paths::gray_home(), &global_home) {
        (Some(a), Some(b)) => {
            a == b || std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
        }
        (None, None) => true,
        _ => false,
    };
    if !same_home && let Some(gh) = global_home {
        let gpath = crate::lock::lock_path(&gh);
        let global_lock = match crate::lock::LockFile::load(&gpath) {
            Ok(lf) => lf,
            Err(e) => {
                push_builder_warning(format!("cannot load {} ({e:#}); ignoring", gpath.display()));
                crate::lock::LockFile {
                    schema: 1,
                    plugins: std::collections::BTreeMap::new(),
                }
            }
        };
        let gpdir = gh.join("plugins");
        let booted: std::collections::HashSet<String> =
            plugins.iter().map(|p| p.manifest().name.clone()).collect();
        for (name, entry) in global_lock.plugins.iter() {
            if !entry.global
                || !crate::lock::effective_enabled(&global_lock, &project_lock, name)
                || entry.runtime_role.as_deref() == Some("provider_only")
                || booted.contains(name)
            {
                continue;
            }
            if let Some(p) =
                spawn_lock_plugin(name, entry, Some(&gpdir), &handler, &mut outcomes).await
            {
                p.set_global(true);
                plugins.push(p);
            }
        }
    }
    if let Ok(mut s) = SPAWN_OUTCOMES.lock() {
        *s = Some(outcomes);
    }

    if plugins.is_empty() {
        let fallback: Vec<Arc<dyn Plugin>> = defaults
            .into_iter()
            .filter(|p| default_names.contains(&p.manifest().name.as_str()))
            .collect();
        return Ok((fallback, true));
    }

    let mut deduped: Vec<Arc<dyn Plugin>> = Vec::new();
    for p in plugins {
        let name = p.manifest().name.clone();
        if let Some(pos) = deduped.iter().position(|e| e.manifest().name == name) {
            deduped.remove(pos);
        }
        deduped.push(p);
    }
    warn_missing_requires(&deduped);
    let fallback = entries.is_empty() && user_lock.plugins.values().all(|entry| !entry.enabled);
    Ok((deduped, fallback))
}

fn warn_missing_requires(plugins: &[Arc<dyn Plugin>]) {
    let loaded: std::collections::BTreeSet<String> =
        plugins.iter().map(|p| p.manifest().name.clone()).collect();
    for p in plugins {
        let m = p.manifest();
        let missing: Vec<String> = m
            .requires_plugins
            .iter()
            .filter(|r| !loaded.contains(r.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            push_builder_warning(format!(
                "plugin {:?} requires plugin(s) {} — not loaded (gray plugin install {})",
                m.name,
                missing.join(", "),
                missing.join(" ")
            ));
        }
    }
}

static AUTOSTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Let manifests marked `autostart` spawn at boot. Only long-lived
/// interactive hosts (REPL, IDE) turn this on; one-shot runs, subagents and
/// gateway child turns keep every plugin lazy so they never start background
/// work they would kill on exit.
pub fn set_autostart(allowed: bool) {
    AUTOSTART.store(allowed, std::sync::atomic::Ordering::Relaxed);
}

async fn spawn_lock_plugin(
    name: &str,
    entry: &crate::lock::LockEntry,
    pdir: Option<&Path>,
    handler: &Option<HostHandler>,
    outcomes: &mut Vec<SpawnOutcome>,
) -> Option<Arc<dyn Plugin>> {
    let argv: Vec<String> = if !entry.argv.is_empty() {
        entry.argv.clone()
    } else {
        let pd = pdir?;
        match resolve_argv(&pd.join(name)) {
            Ok(a) => a,
            Err(e) => {
                outcomes.push(SpawnOutcome {
                    name: name.to_string(),
                    error: Some(format!("{e:#}")),
                });
                return None;
            }
        }
    };

    let lazy_manifest = if crate::lazy::lazy_enabled() {
        pdir.and_then(|pd| {
            crate::lazy::cached_manifest(&pd.join(format!("{name}-manifest.json")), name)
        })
        // autostart plugins own background work the session needs (cron
        // ticks); they spawn eagerly like profile sidecars
        .filter(|m| !(m.autostart && AUTOSTART.load(std::sync::atomic::Ordering::Relaxed)))
    } else {
        None
    };
    if let Some(m) = lazy_manifest {
        let p = crate::lazy::LazySidecarPlugin::new(argv.clone(), m);
        if let Some(h) = handler {
            p.set_host_handler(h.clone()).await;
        }

        let declared = p.manifest().capabilities.clone();
        p.set_capabilities(
            crate::capabilities::granted_for(entry, &declared)
                .into_iter()
                .collect(),
        );
        let pending = crate::capabilities::pending_consent(entry, &declared);
        if !pending.is_empty() {
            push_builder_warning(format!(
                "plugin {name:?} declares ungranted {} ({}); grant with gray plugin capabilities {name} --all",
                if pending.len() == 1 {
                    "capability".to_string()
                } else {
                    "capabilities".to_string()
                },
                pending.join(", ")
            ));
        }
        outcomes.push(SpawnOutcome {
            name: name.to_string(),
            error: None,
        });
        return Some(p.arc() as Arc<dyn Plugin>);
    }
    match SidecarPlugin::spawn(argv).await {
        Ok(p) => {
            if let Some(h) = handler {
                p.set_host_handler(h.clone()).await;
            }

            let declared = p.manifest().capabilities.clone();
            p.set_capabilities(
                crate::capabilities::granted_for(entry, &declared)
                    .into_iter()
                    .collect(),
            );
            let pending = crate::capabilities::pending_consent(entry, &declared);
            if !pending.is_empty() {
                push_builder_warning(format!(
                    "plugin {name:?} declares ungranted {} ({}); grant with gray plugin capabilities {name} --all",
                    if pending.len() == 1 {
                        "capability".to_string()
                    } else {
                        "capabilities".to_string()
                    },
                    pending.join(", ")
                ));
            }
            outcomes.push(SpawnOutcome {
                name: name.to_string(),
                error: None,
            });
            Some(Arc::new(p) as Arc<dyn Plugin>)
        }
        Err(e) => {
            log::warn!(target: "gray_plugin", "lock plugin {name:?} failed to spawn: {e:#}");
            push_builder_warning(format!(
                "plugin {name:?} failed to start — /plugins to see why"
            ));
            outcomes.push(SpawnOutcome {
                name: name.to_string(),
                error: Some(format!("{e:#}")),
            });
            None
        }
    }
}

pub const PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

pub fn clamp_prompt_cache_key(key: &str) -> &str {
    if key.len() <= PROMPT_CACHE_KEY_MAX_LENGTH {
        return key;
    }

    let mut end = PROMPT_CACHE_KEY_MAX_LENGTH;
    while !key.is_char_boundary(end) {
        end -= 1;
    }
    &key[..end]
}

pub fn provider_cache_key(session_id: Option<&str>) -> String {
    if let Some(s) = session_id.filter(|s| !s.is_empty()) {
        return clamp_prompt_cache_key(s).to_string();
    }
    static FALLBACK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FALLBACK
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone()
}

pub enum SystemPrompt {
    Literal(String),
    Build(PromptBuilder),
}

pub type PromptBuilder = Box<dyn FnOnce(&Registry) -> String + Send>;

pub type ExecutorWrap = Box<dyn FnOnce(Arc<dyn ToolExecutor>) -> Arc<dyn ToolExecutor> + Send>;

pub struct BuilderOptions {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub reasoning_effort: Option<String>,

    pub temperature: Option<f32>,
    pub top_p: Option<f32>,

    pub context_window: Option<usize>,

    pub session_id: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: SystemPrompt,

    pub extra_plugins: Vec<Arc<dyn Plugin>>,
    pub host_handler: Option<HostHandler>,
    pub profile_path: String,

    pub bare: bool,
    pub abort_on_spawn_failure: bool,
    pub wrap_executor: Option<ExecutorWrap>,

    pub dynamic_provider_profile: Option<OpenAiProviderProfile>,

    pub dynamic_credential_source: Option<Arc<dyn CredentialSource>>,
    pub prebooted_plugins: Option<Vec<Arc<dyn Plugin>>>,
}

pub async fn build_agent(opts: BuilderOptions) -> anyhow::Result<Agent> {
    let BuilderOptions {
        model,
        api_key,
        base_url,
        reasoning_effort,
        temperature,
        top_p,
        context_window,
        session_id,
        cwd,
        system_prompt,
        extra_plugins,
        host_handler,
        profile_path,
        bare,
        abort_on_spawn_failure,
        wrap_executor,
        dynamic_provider_profile,
        dynamic_credential_source,
        prebooted_plugins,
    } = opts;

    let mut plugins = if let Some(prebooted) = prebooted_plugins {
        prebooted
    } else if bare {
        Vec::new()
    } else {
        active_plugins(
            Vec::new(),
            &[],
            &profile_path,
            host_handler,
            abort_on_spawn_failure,
        )
        .await?
        .0
    };

    for p in extra_plugins {
        let name = p.manifest().name.clone();
        if let Some(pos) = plugins.iter().position(|e| e.manifest().name == name) {
            plugins.remove(pos);
        }
        plugins.push(p);
    }

    for p in &plugins {
        p.set_session_id(session_id.as_deref().unwrap_or(""));
    }
    let (registry, manifests) = from_plugins(&plugins);
    let dynamic = dynamic_plugins(&plugins);
    let ledger = current_file_ledger();
    let system = match system_prompt {
        SystemPrompt::Literal(s) => s,
        SystemPrompt::Build(f) => f(&registry),
    };
    let provider: Box<dyn gray_core::agent::Provider> =
        match (dynamic_provider_profile, dynamic_credential_source) {
            (Some(profile), Some(source)) => Box::new(
                OpenAiProvider::new_with_profile(
                    model,
                    reasoning_effort,
                    Some(provider_cache_key(session_id.as_deref())),
                    profile,
                    source,
                )
                .map_err(|e| anyhow::anyhow!("failed to initialize plugin provider: {e}"))?
                .with_sampling(temperature, top_p),
            ),

            (None, None) if gray_provider::anthropic::is_anthropic_base_url(&base_url) => Box::new(
                gray_provider::AnthropicProvider::new(api_key, model, base_url, reasoning_effort)
                    .map_err(|e| anyhow::anyhow!("failed to initialize Anthropic provider: {e}"))?
                    .with_sampling(temperature, top_p),
            ),
            (None, None) => Box::new(
                OpenAiProvider::new(
                    api_key,
                    model,
                    base_url,
                    reasoning_effort,
                    Some(provider_cache_key(session_id.as_deref())),
                )
                .map_err(|e| anyhow::anyhow!("failed to initialize OpenAI provider: {e}"))?
                .with_sampling(temperature, top_p),
            ),
            _ => anyhow::bail!("a dynamic provider needs both a profile and a credential source"),
        };

    let tool_defs = registry.defs();

    let tool_labels: Vec<(String, String)> = manifests
        .iter()
        .flat_map(|m| m.tools.iter())
        .filter_map(|t| {
            t.label
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| (t.name.clone(), s.to_string()))
        })
        .collect();
    let tool_previews: Vec<(String, String)> = manifests
        .iter()
        .flat_map(|m| m.tools.iter())
        .filter_map(|t| {
            t.preview
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| (t.name.clone(), s.to_string()))
        })
        .collect();

    let registry = Arc::new(registry);
    let base: Arc<dyn ToolExecutor> = if dynamic.is_empty() {
        registry
    } else {
        Arc::new(crate::LiveRegistry::new(registry, dynamic))
    };
    let executor: Arc<dyn ToolExecutor> = match wrap_executor {
        Some(wrap) => wrap(base),
        None => base,
    };
    let hooks = PluginHookAdapter::for_plugins(&plugins, &cwd.to_string_lossy());
    Ok(Agent::new(provider, executor)
        .with_system(system)
        .with_tools(tool_defs)
        .with_tool_labels(tool_labels)
        .with_tool_previews(tool_previews)
        .with_context_window(context_window)
        .with_history_rewrite_hook(Arc::new(move || {
            if let Some(ledger) = &ledger {
                ledger.disarm_all_dedup();
            }
        }))
        .with_hooks(hooks))
}

#[path = "builder_tests.rs"]
#[cfg(test)]
mod tests;
