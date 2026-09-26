//! One profile-aware agent builder for every surface (REPL, `-p`, cron).
//!
//! Lives here (not in `gray`) so every host shares one builder without depending on the binary.
//!
//! Surface policy stays with the callers: the system prompt (skills/context),
//! the executor wrapper (plain vs `DenyExecutor`), the
//! host handler, and abort-vs-warn on sidecar spawn failure all arrive via
//! [`BuilderOptions`]. Cron needs no direct call — the sidecar fires through
//! `host/run` (`gray -p`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use gray_core::agent::{Agent, Tool, ToolExecutor};
use gray_core::credential::CredentialSource;
use gray_provider::{OpenAiProvider, OpenAiProviderProfile};
use gray_tools::Registry;

use crate::profile::{PluginEntry, load_entries};
use crate::{HostHandler, Manifest, Plugin, PluginHookAdapter, SidecarPlugin, merge_manifests};

// ---------------------------------------------------------------------------
// Builtin plugins (single definition; callers add surface extras via options)
// ---------------------------------------------------------------------------

/// `tools-basic` file/shell set.
pub struct ToolsBasicPlugin;

/// One manifest body for the builtin tool plugins (name + live tool defs).
fn builtin_manifest(name: &str, tools: &[Arc<dyn Tool>]) -> Manifest {
    Manifest {
        name: name.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tools: tools.iter().map(|t| t.def()).collect(),
        ..Manifest::default()
    }
}

impl Plugin for ToolsBasicPlugin {
    fn manifest(&self) -> Manifest {
        builtin_manifest("tools-basic", &self.tools())
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        // T3.2/T3.3 wiring: read/write/edit share one ledger per tools() call
        // (from_plugins calls once per build, so the session tools agree).
        let ledger = Arc::new(gray_tools::FileLedger::new());
        vec![
            Arc::new(gray_tools::ReadTool::new(ledger.clone())),
            Arc::new(gray_tools::WriteTool::new(ledger.clone())),
            Arc::new(gray_tools::EditTool::new(ledger.clone())),
            Arc::new(gray_tools::BashTool::default()),
        ]
    }
}

/// `tools-minimal`: the default surface — the single `bash` tool (including
/// managed jobs). Everything — read, search, edit, run, and seeing an image
/// (`cat img.png` returns it as a vision block) — goes through `bash`.
pub struct ToolsMinimalPlugin;

impl Plugin for ToolsMinimalPlugin {
    fn manifest(&self) -> Manifest {
        builtin_manifest("tools-minimal", &self.tools())
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(gray_tools::BashTool::default())]
    }
}

pub struct ToolsSearchPlugin;

impl Plugin for ToolsSearchPlugin {
    fn manifest(&self) -> Manifest {
        builtin_manifest("tools-search", &self.tools())
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(gray_tools::GrepTool::default()),
            Arc::new(gray_tools::FindTool::default()),
            Arc::new(gray_tools::LsTool),
        ]
    }
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
    let mut owners = merge_manifests(manifests.clone());
    // Reserved builtin names: tools owned by tools-basic/tools-search carry
    // name-based trust (approval Allow, parallel lane). A sidecar claiming
    // one must not inherit it — drop the claim with a warning, and always
    // backfill the real builtin so a hostile manifest can't remove it.
    let is_builtin_owner = |name: &str| name == "tools-basic" || name == "tools-search";
    let mut builtin_tools: std::collections::HashMap<String, Arc<dyn Tool>> =
        std::collections::HashMap::new();
    for p in plugins {
        if is_builtin_owner(&p.manifest().name) {
            for t in p.tools() {
                builtin_tools.insert(t.def().name.clone(), t.clone());
            }
        }
    }
    let builtin_names: std::collections::HashSet<String> = builtin_tools.keys().cloned().collect();
    // Every builtin surface a sidecar could try to sit on top of. The
    // name-based-trust set above (`tools-basic`/`tools-search`) stays
    // reserve-only; these add the default `tools-minimal` surface, so a
    // sidecar cannot quietly become `bash`.
    let is_builtin_plugin =
        |name: &str| matches!(name, "tools-minimal" | "tools-basic" | "tools-search");
    // Builtins win manifests: a hostile claim must not displace the owner
    // either (the ledger rebuild below keys off ownership).
    for plugin in plugins {
        let owner = plugin.manifest().name;
        if is_builtin_owner(&owner) {
            for tool in plugin.tools() {
                owners.insert(tool.def().name.clone(), owner.clone());
            }
        }
    }
    // Tool names the builtin surface owns, for the override gate below.
    let builtin_owned: std::collections::HashSet<String> = plugins
        .iter()
        .filter(|p| is_builtin_plugin(&p.manifest().name))
        .flat_map(|p| p.tools().into_iter().map(|t| t.def().name))
        .collect();
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for p in plugins {
        let owner_name = p.manifest().name;
        let may_override = p
            .capabilities()
            .iter()
            .any(|c| c == crate::capabilities::TOOL_OVERRIDE);
        for t in p.tools() {
            if builtin_names.contains(&t.def().name) && !is_builtin_owner(&owner_name) {
                push_builder_warning(format!(
                    "plugin `{}` claims reserved builtin tool `{}`; ignoring",
                    owner_name,
                    t.def().name
                ));
                continue;
            }
            // Replacing a built-in surface tool needs the capability, and
            // the capability needs consent: a silently winning sidecar
            // would intercept everything routed through that tool.
            if builtin_owned.contains(&t.def().name)
                && !is_builtin_plugin(&owner_name)
                && !may_override
            {
                push_builder_warning(format!(
                    "plugin `{}` claims built-in tool `{}`; grant `{}` to let it (gray plugin capabilities {})",
                    owner_name,
                    t.def().name,
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
    // Backfill any reserved name a hostile manifest displaced: builtins win.
    for (name, tool) in &builtin_tools {
        if !tools.iter().any(|e| &e.def().name == name) {
            tools.push(tool.clone());
        }
    }
    // Lifecycle adoption: one ledger shared by the session tools; the
    // binary's /new + compaction lifecycle acts on the same state via
    // `current_file_ledger`. ToolsBasicPlugin::tools() already shares one
    // ledger per build, but that Arc dies with the plugin — rebuild the
    // tools-basic read/write/edit on the tracked one instead (a
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
    let registry = Registry::new(tools);
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

/// Resolve the gray home dir (`$GRAY_HOME` else `$HOME/.gray`), mirroring
/// `gray-pkg` (which owns the lockfile writes; this crate must not depend
/// on it — networking lives there, never here). `None` when neither
/// resolves: user-scope filtering is skipped, the project overlay still
/// applies.
fn gray_home() -> Option<PathBuf> {
    gray_core::paths::gray_home()
}

/// Install dir for lock entries (`<home>/plugins`), mirroring `gray-pkg`
/// (which owns the lockfile writes; this crate must not depend on it).
/// `None` when no home resolves.
fn plugins_dir() -> Option<PathBuf> {
    gray_home().map(|h| h.join("plugins"))
}

/// Resolve the spawn argv for a plugin dir: the dir itself when
/// executable, else `plugin.sh`, else the single executable inside.
///
/// Single definition shared by the host (`active_plugins` below) and
/// `gray::plugin_check` — do not invent a second resolution rule.
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
        // Executable bit is unix-only; on Windows any file qualifies.
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

/// User lock + project overlay (`<cwd>/.gray/plugins.json`, same shape)
/// for lock-entry spawning and the disabled filter. Missing files are empty
/// (silent); corrupt files yield one warning each (paths only, never
/// argv/URLs). `None` home skips the user scope; the project overlay still
/// applies.
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
    let user = gray_home()
        .map(|h| load(crate::lock::lock_path(&h)))
        .unwrap_or(crate::lock::LockFile {
            schema: 1,
            plugins: BTreeMap::new(),
        });
    let project = load(crate::lock::project_lock_path(cwd));
    (user, project, warnings)
}

// ---------------------------------------------------------------------------
// Profile resolution
// ---------------------------------------------------------------------------

/// Ordered active plugins: the profile file order followed by enabled
/// lock-file installs, or `defaults` when both are empty. Sidecars spawn
/// once per build with `handler` installed (`host/run`/`host/say`).
/// A profile-sidecar spawn failure aborts when `abort_on_spawn_failure`
/// (interactive boot) else warns + skips (the daemon must stay up).
/// Lock-entry spawn failures always warn + skip (names only, never argv).
/// Unknown builtin names always warn + skip. Sidecars disabled in the
/// plugin lock (`enabled: false` in the user lock, `<cwd>/.gray/plugins.json`
/// overlay winning per name) warn + skip before spawning — the flag never
/// aborts, even when `abort_on_spawn_failure`.
///
/// This is the single production host: profile entries first, then enabled
/// lock entries (real installs resolve their executable from
/// `<home>/plugins/<name>` via [`resolve_argv`]; legacy entries
/// with an explicit `argv` spawn it directly). See [`crate::boot`] (kept
/// as a test-only harness) for the legacy split.
/// `defaults` is the catalog of builtin plugins a `gray.yml` may name;
/// `default_names` selects which of them is the no-profile fallback (the
/// default surface). Decoupled so the catalog can offer opt-in plugins
/// (`tools-basic`, `tools-search`) while booting `tools-minimal`.
pub async fn active_plugins(
    defaults: Vec<Arc<dyn Plugin>>,
    default_names: &[&str],
    profile_path: &str,
    handler: Option<HostHandler>,
    abort_on_spawn_failure: bool,
) -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    // Profile entries: missing file is the default state (silent, empty);
    // anything else (parse error) warns once via the caller's drain.
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

    // Lock state once per build (file reads, not per sidecar).
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (user_lock, project_lock, lock_warnings) = load_lock_files(&cwd);
    for w in lock_warnings {
        push_builder_warning(w);
    }
    // Legacy exact-argv disabled set (non-empty-argv entries only).
    let disabled = crate::lock::disabled_sidecar_argvs(&user_lock, &project_lock);
    // Path correlation for real installs (empty-argv locks): resolved
    // executable paths + install dirs of disabled entries, so a profile
    // sidecar pointing at the same install matches even though the lock
    // argv is empty.
    let pdir = plugins_dir();
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
        // Single-path specs (dir or executable) match a disabled install
        // by resolved executable or install dir.
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

    // User-installed sidecars extend the default surface when no profile exists;
    // their presence must not suppress the default bash tool.
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
                // Auto-fetch installers (`npx -y`, `uvx`, `pip install`) pull
                // mutable code at spawn: only install sidecars you trust.
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
                        // A profile sidecar is argv the operator wrote into
                        // gray.yml: placement is the consent, so it keeps
                        // everything it declares.
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

    // Enabled lock entries (the project overlay wins per name on the flag;
    // the overlay toggles, it never discovers — project-only names have no
    // known argv/dir, so only user names spawn). Real installs (empty argv)
    // resolve their executable from the install dir; legacy entries spawn
    // their recorded argv. Failures warn (names only) and never abort.
    for (name, entry) in user_lock.plugins.iter() {
        if !crate::lock::effective_enabled(&user_lock, &project_lock, name) {
            continue;
        }
        // Provider-only sidecars are lifecycle-owned by the provider runtime.
        // They stay in the lock for provider discovery, but never boot as a
        // second sidecar process.
        if entry.runtime_role.as_deref() == Some("provider_only") {
            continue;
        }
        let argv: Vec<String> = if !entry.argv.is_empty() {
            entry.argv.clone()
        } else {
            let Some(pd) = &pdir else { continue };
            match resolve_argv(&pd.join(name)) {
                Ok(a) => a,
                Err(_) => continue,
            }
        };
        match SidecarPlugin::spawn(argv).await {
            Ok(p) => {
                if let Some(h) = &handler {
                    p.set_host_handler(h.clone()).await;
                }
                // Consent decides what this sidecar may ask of the host.
                // A pre-consent entry keeps what it declares (it ran with
                // those powers before consent existed); a consented one is
                // cut down to the intersection.
                let declared = p.manifest().capabilities.clone();
                p.set_capabilities(
                    crate::capabilities::granted_for(entry, &declared)
                        .into_iter()
                        .collect(),
                );
                let pending = crate::capabilities::pending_consent(entry, &declared);
                if !pending.is_empty() {
                    push_builder_warning(format!(
                        "plugin {name:?} declares ungranted {} ({}); grant with gray plugin capabilities {name}",
                        if pending.len() == 1 {
                            "capability".to_string()
                        } else {
                            "capabilities".to_string()
                        },
                        pending.join(", ")
                    ));
                }
                plugins.push(Arc::new(p) as Arc<dyn Plugin>);
            }
            Err(e) => push_builder_warning(format!(
                "lock plugin {name:?} failed to spawn ({e:#}); ignoring"
            )),
        }
    }

    if plugins.is_empty() {
        let fallback: Vec<Arc<dyn Plugin>> = defaults
            .into_iter()
            .filter(|p| default_names.contains(&p.manifest().name.as_str()))
            .collect();
        return Ok((fallback, true));
    }
    // Later manifests win on name conflict (same rule as `merge_manifests`
    // and the legacy boot harness).
    let mut deduped: Vec<Arc<dyn Plugin>> = Vec::new();
    for p in plugins {
        let name = p.manifest().name.clone();
        if let Some(pos) = deduped.iter().position(|e| e.manifest().name == name) {
            deduped.remove(pos);
        }
        deduped.push(p);
    }
    let fallback = entries.is_empty() && user_lock.plugins.values().all(|entry| !entry.enabled);
    Ok((deduped, fallback))
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

/// Builds the `system` prompt: either a ready string or a
/// registry-aware closure (REPL/`-p` tool snippets + guidelines need the
/// resolved registry, which only exists after profile resolution).
pub enum SystemPrompt {
    Literal(String),
    Build(PromptBuilder),
}

/// `FnOnce(&Registry) -> String`: caller builds its prompt from the resolved
/// registry (snippets, names, guidelines).
pub type PromptBuilder = Box<dyn FnOnce(&Registry) -> String + Send>;

/// Wraps the profile-built registry executor (a `DenyExecutor` wrap;
/// `None` = plain registry).
pub type ExecutorWrap = Box<dyn FnOnce(Arc<dyn ToolExecutor>) -> Arc<dyn ToolExecutor> + Send>;

pub struct BuilderOptions {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub reasoning_effort: Option<String>,
    /// Sampling passthroughs for providers that accept them (None = server default).
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    /// Known model context window in tokens (`None` = unknown: only
    /// overflow-recovery compaction runs).
    pub context_window: Option<usize>,
    /// Pins the Responses cache shard; callers thread their session id so
    /// sessions don't all collide on the per-process fallback key.
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: SystemPrompt,
    /// Surface plugins that are always active regardless of the profile
    /// (gray: none — bash-only, empty by default).
    /// Appended after profile + lock plugins; on tool-name conflict the
    /// later manifest wins, so these always win their own tool names.
    pub extra_plugins: Vec<Arc<dyn Plugin>>,
    pub host_handler: Option<HostHandler>,
    pub profile_path: String,
    pub abort_on_spawn_failure: bool,
    pub wrap_executor: Option<ExecutorWrap>,
    /// Declared plugin-backed provider profile; `None` preserves the
    /// built-in static API-key provider.
    pub dynamic_provider_profile: Option<OpenAiProviderProfile>,
    /// Agent-lifetime credential lease source for `dynamic_provider_profile`.
    pub dynamic_credential_source: Option<Arc<dyn CredentialSource>>,
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
        temperature,
        top_p,
        context_window,
        session_id,
        cwd,
        system_prompt,
        extra_plugins,
        host_handler,
        profile_path,
        abort_on_spawn_failure,
        wrap_executor,
        dynamic_provider_profile,
        dynamic_credential_source,
    } = opts;
    // Catalog: any of these may be named in `gray.yml`. Fallback (no profile)
    // is `tools-minimal` — one persistent shell, gray's default surface.
    let defaults: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(ToolsMinimalPlugin) as Arc<dyn Plugin>,
        Arc::new(ToolsBasicPlugin) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
    ];
    let (mut plugins, _) = active_plugins(
        defaults,
        &["tools-minimal"],
        &profile_path,
        host_handler,
        abort_on_spawn_failure,
    )
    .await?;
    // Always-on surface plugins (see `extra_plugins`): appended after the
    // profile + lock set so they survive `tools-minimal`-only profiles.
    // Same-name dedupe as `active_plugins` (later wins) keeps a profile
    // entry with the same name from doubling up.
    for p in extra_plugins {
        let name = p.manifest().name.clone();
        if let Some(pos) = plugins.iter().position(|e| e.manifest().name == name) {
            plugins.remove(pos);
        }
        plugins.push(p);
    }
    let (registry, _) = from_plugins(&plugins);
    let ledger = current_file_ledger();
    let system = match system_prompt {
        SystemPrompt::Literal(s) => s,
        SystemPrompt::Build(f) => f(&registry),
    };
    let provider = match (dynamic_provider_profile, dynamic_credential_source) {
        (Some(profile), Some(source)) => OpenAiProvider::new_with_profile(
            model,
            reasoning_effort,
            Some(provider_cache_key(session_id.as_deref())),
            profile,
            source,
        )
        .map_err(|e| anyhow::anyhow!("failed to initialize plugin provider: {e}"))?
        .with_sampling(temperature, top_p),
        (None, None) => OpenAiProvider::new(
            api_key,
            model,
            base_url,
            reasoning_effort,
            Some(provider_cache_key(session_id.as_deref())),
        )
        .map_err(|e| anyhow::anyhow!("failed to initialize OpenAI provider: {e}"))?
        .with_sampling(temperature, top_p),
        _ => anyhow::bail!("a dynamic provider needs both a profile and a credential source"),
    };

    let tool_defs = registry.defs();
    let executor: Arc<dyn ToolExecutor> = match wrap_executor {
        Some(wrap) => wrap(Arc::new(registry)),
        None => Arc::new(registry),
    };
    let hooks = PluginHookAdapter::for_plugins(&plugins, &cwd.to_string_lossy());
    Ok(Agent::new(Box::new(provider), executor)
        .with_system(system)
        .with_tools(tool_defs)
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
