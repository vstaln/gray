//! Native plugin command registration, help metadata and bounded UI requests. No model calls.
//!
//! One registry: `lock.json`. A `cli_argv` on a lock entry makes the plugin
//! a `gray <name> …` command and enables slash-command capture; `argv` is
//! always the sidecar invocation vector.
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use gray_plugin::lock::{LockEntry, LockFile};
use serde_json::Value;

pub fn home() -> anyhow::Result<PathBuf> {
    Ok(crate::sys_prompt_path()?
        .parent()
        .context("cannot resolve gray home")?
        .to_path_buf())
}

fn load_lock(home: &Path) -> anyhow::Result<LockFile> {
    let lock = LockFile::load(&gray_plugin::lock::lock_path(home))?;
    anyhow::ensure!(lock.schema == 1, "unsupported plugin lock schema");
    Ok(lock)
}

/// One-time migration: fold a legacy `plugins/commands.json` registry into
/// `lock.json`. Each entry becomes (or fills in) a lock entry whose
/// `cli_argv` is the old command argv; the file is renamed to
/// `commands.json.migrated` so a second call is a no-op. Failures warn and
/// leave both files untouched.
fn migrate_commands_json(home: &Path) {
    let commands = home.join("plugins/commands.json");
    if !commands.exists() {
        return;
    }
    if let Err(e) = migrate_commands(home, &commands) {
        log::warn!("commands.json migration skipped: {e:#}");
    }
}

fn migrate_commands(home: &Path, commands: &Path) -> anyhow::Result<()> {
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let old = LockFile::load(commands)?;
    let path = gray_plugin::lock::lock_path(home);
    let mut lock = LockFile::load(&path)?;
    merge_command_entries(&mut lock, old);
    lock.save(&path)?;
    std::fs::rename(commands, commands.with_file_name("commands.json.migrated"))?;
    Ok(())
}

fn merge_command_entries(lock: &mut LockFile, old: LockFile) {
    for (name, mut entry) in old.plugins {
        match lock.plugins.get_mut(&name) {
            Some(cur) => {
                if cur.cli_argv.is_none() {
                    cur.cli_argv = Some(entry.argv);
                }
            }
            None => {
                entry.cli_argv = Some(entry.argv.clone());
                lock.plugins.insert(name, entry);
            }
        }
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
        "invalid plugin command name"
    );
    Ok(())
}

pub(crate) fn enabled(home: &Path, name: &str, entry: &LockEntry) -> bool {
    if !entry.enabled {
        return false;
    }
    let Ok(user) = LockFile::load(&gray_plugin::lock::lock_path(home)) else {
        return false;
    };
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    project
        .as_ref()
        .and_then(|lock| lock.plugins.get(name))
        .or_else(|| user.plugins.get(name))
        .is_none_or(|entry| entry.enabled)
}

/// Subcommands a registered plugin declares for itself: its manifest's
/// `completion` array. That is the subcommand vocabulary (`settings`, `run`,
/// …); the slash aliases live in `commands`, and a caller showing the app's
/// own name has no use for them. Empty when the plugin registered no manifest
/// (or the file is unreadable) — nothing is invented on its behalf.
pub(crate) fn declared_subcommands(home: &Path, name: &str) -> Vec<String> {
    metadata(home, name)
        .ok()
        .and_then(|m| m["completion"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// Slash commands a plugin declares in its manifest (`"/x"` entries, kept
/// verbatim — the `/` prefix is the caller's concern). Provider plugins use
/// these to name their login shortcut; the plugin owns the name.
pub(crate) fn declared_commands(home: &Path, name: &str) -> Vec<String> {
    metadata(home, name)
        .ok()
        .and_then(|m| m["commands"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn metadata(home: &Path, name: &str) -> anyhow::Result<serde_json::Value> {
    validate_name(name)?;
    Ok(serde_json::from_slice(&std::fs::read(
        home.join("plugins").join(format!("{name}-manifest.json")),
    )?)?)
}

/// `gray plugin install <name|url|path>` — one entry point for every
/// plugin source, tried in order:
///   0. a skill spec (`clawhub:`/`github:`/`url:`) → `skills_ops` (skill
///      installs write `~/.gray/skills`, never a plugin lock row);
///   a. `GRAY_PLUGIN_PATH` set → register that executable under `spec`.
///   b. `spec` is an existing path → register it (the manifest names it).
///   c. `spec` is a valid name and `gray-<spec>` is on PATH → register it.
///   d. otherwise → a gray-pkg index name or an https tarball URL.
///
/// Returns the user-facing lines; callers choose how to display them (the
/// CLI prints, the TUI routes them through `say` — never stdout mid-frame).
pub async fn install_spec_lines(
    home: &Path,
    spec: &str,
    force: bool,
) -> anyhow::Result<Vec<String>> {
    migrate_commands_json(home);
    if gray_pkg::skills_ops::is_skill_spec(spec) {
        let r = gray_pkg::skills_ops::install(spec).await?;
        return Ok(vec![format!(
            "installed {} {} at {}",
            r.name,
            r.version,
            r.path.display()
        )]);
    }
    if let Some(path) = std::env::var_os("GRAY_PLUGIN_PATH") {
        anyhow::ensure!(!path.is_empty(), "GRAY_PLUGIN_PATH is set but empty");
        return register_native(home, Some(spec), Path::new(&path), force)
            .await
            .map(|line| vec![line]);
    }
    let path = Path::new(spec);
    // A bare name only resolves as a path when it looks like one — absolute,
    // dot-prefixed, or containing a separator. Otherwise `gray plugin install
    // demo` in a directory that happens to contain `demo` would register and
    // execute it instead of installing `demo` from the index.
    let path_like = path.is_absolute()
        || spec.starts_with('.')
        || spec.contains(std::path::MAIN_SEPARATOR)
        || spec.contains('/');
    if path_like && path.exists() {
        return register_native(home, None, path, force)
            .await
            .map(|line| vec![line]);
    }
    if validate_name(spec).is_ok()
        && let Some(paths) = std::env::var_os("PATH")
    {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(format!("gray-{spec}{}", std::env::consts::EXE_SUFFIX));
            if candidate.is_file() {
                return register_native(home, Some(spec), &candidate, force)
                    .await
                    .map(|line| vec![line]);
            }
        }
    }
    let r = gray_pkg::ops::install(
        gray_pkg::ops::parse_spec(spec),
        gray_pkg::ops::InstallOpts::default(),
    )
    .await?;
    Ok(vec![format!(
        "installed {} {} at {}",
        r.name,
        r.version,
        r.path.display()
    )])
}

/// `gray plugin install` — CLI entry: same resolution as the REPL's, printed
/// to stdout.
pub async fn install_spec(home: &Path, spec: &str, force: bool) -> anyhow::Result<()> {
    for line in install_spec_lines(home, spec, force).await? {
        println!("{line}");
    }
    Ok(())
}

/// Run the install-time security scan and act on its verdict.
///
/// `dangerous` blocks with no override: the operator may be impatient,
/// but a reverse shell is not a preference. `caution` needs either an
/// interactive yes or `--force`; a session that cannot ask refuses
/// instead of installing code nobody reviewed.
fn scan_or_block(root: &Path, force: bool) -> anyhow::Result<()> {
    use gray_plugin::scan::{Verdict, scan_tree};
    let report = scan_tree(root)?;
    match report.verdict() {
        Verdict::Safe => {
            println!(
                "  plugin scan: {} — {}",
                report.summary(),
                Verdict::Safe.label()
            );
            Ok(())
        }
        Verdict::Caution => {
            println!("  plugin scan: caution — {}", report.summary());
            print!("{}", report.render());
            if force {
                println!("  continuing (--force)");
                return Ok(());
            }
            if !interactive() {
                anyhow::bail!(
                    "plugin scan reported caution ({}) and this session cannot ask; \
                     re-run interactively, or pass --force to accept the findings",
                    report.summary()
                );
            }
            let granted = confirm("Review the findings above. Install anyway? [y/N] ")?;
            anyhow::ensure!(granted, "install cancelled after a caution scan");
            Ok(())
        }
        Verdict::Dangerous => {
            let critical: Vec<String> = report
                .criticals()
                .iter()
                .map(|f| format!("{} ({}:{})", f.rule, f.path, f.line))
                .collect();
            print!("{}", report.render());
            anyhow::bail!(
                "plugin scan blocked the install: {} critical of {} findings ({}); \
                 --force does not override a dangerous verdict",
                critical.len(),
                report.findings.len(),
                critical.join(", ")
            );
        }
    }
}

/// Same rule as the typed-manifest check: a protocol-1.2 sidecar that owns
/// no other surface stays in `lock.json` for provider discovery while
/// `active_plugins` skips spawning it.
fn provider_only_role_json(manifest: &serde_json::Value) -> Option<String> {
    let empty = |key: &str| {
        manifest
            .get(key)
            .and_then(serde_json::Value::as_array)
            .is_none_or(|items| items.is_empty())
    };
    let provider_only = manifest.get("protocol").and_then(Value::as_str) == Some("1.2")
        && manifest
            .get("providers")
            .and_then(Value::as_array)
            .is_some_and(|providers| !providers.is_empty())
        && empty("tools")
        && empty("commands")
        && empty("hooks");
    provider_only.then(|| "provider_only".to_string())
}

/// Whether this process can ask the operator a question.
fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// One yes/no question on the terminal. `false` when it cannot be answered.
fn confirm(prompt: &str) -> anyhow::Result<bool> {
    use std::io::Write as _;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Show what a plugin declares, ask for a grant, and return
/// `(granted, consent hash)`.
///
/// Nothing declared → no hash and no grant (an undeclared capability is
/// not a capability). A session that cannot prompt grants nothing but
/// still records the hash, so the plugin runs ungranted rather than
/// grandfathered, and `gray plugin capabilities <name>` can grant later.
fn consent_capabilities(name: &str, declared: &[String]) -> (Vec<String>, Option<String>) {
    if declared.is_empty() {
        return (Vec::new(), None);
    }
    let hash = gray_plugin::capabilities::consent_hash(declared);
    println!("'{name}' declares {} capability(ies):", declared.len());
    for id in declared {
        let what = gray_plugin::capabilities::spec(id)
            .map(|s| s.description)
            .unwrap_or("(unknown to this build)");
        println!("  - {id}: {what}");
    }
    if !interactive() {
        println!(
            "  not granted (non-interactive session). Grant later: gray plugin capabilities {name}"
        );
        return (Vec::new(), Some(hash));
    }
    let granted = confirm("Grant these to this plugin? [y/N] ").unwrap_or(false);
    if granted {
        println!("  granted");
        (declared.to_vec(), Some(hash))
    } else {
        println!("  not granted; the plugin will run without them");
        (Vec::new(), Some(hash))
    }
}

/// `gray plugin capabilities [name]` — declared vs granted, per plugin,
/// with the exact command that grants. Reads the cached manifest every
/// install writes, so a plugin that never booted still lists correctly.
pub fn print_capabilities(only: Option<&str>) -> anyhow::Result<()> {
    let home = home()?;
    // list_managed runs the commands.json migration itself, so row.entry is
    // the post-migration lock entry — a lock loaded here would predate it.
    let rows = list_managed(&home)?;
    let mut shown = 0usize;
    for row in &rows {
        if let Some(want) = only
            && row.name != want
        {
            continue;
        }
        shown += 1;
        let entry = &row.entry;
        let declared = metadata(&home, &row.name)
            .ok()
            .and_then(|m| {
                m["capabilities"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let (granted, hash) = (
            gray_plugin::capabilities::granted_for(entry, &declared),
            entry.capabilities_hash.clone(),
        );
        println!(
            "{} ({}, {})",
            row.name, row.entry.ecosystem, row.entry.version
        );
        if declared.is_empty() {
            println!("  declares: none");
        } else {
            for id in &declared {
                let mark = if granted.contains(id) {
                    "granted"
                } else {
                    "not granted"
                };
                println!("  - {id}: {mark}");
            }
        }
        match hash {
            None if !declared.is_empty() => println!(
                "  consent: never asked (grandfathered). Grant: gray plugin capabilities {} --all",
                row.name
            ),
            Some(h) => {
                let drift = gray_plugin::capabilities::needs_reconsent(entry, &declared);
                if drift {
                    println!(
                        "  consent: hash {h} no longer matches the manifest — re-run `gray plugin update {}`",
                        row.name
                    );
                }
            }
            _ => {}
        }
    }
    if shown == 0 {
        if let Some(want) = only {
            anyhow::bail!("no plugin named '{want}'");
        }
        println!("no plugins installed");
    }
    Ok(())
}

/// One installed plugin in `lock.json`. `cli` is true for entries carrying
/// `cli_argv` — they forward `gray <name> …` to an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPlugin {
    pub name: String,
    pub entry: LockEntry,
    pub cli: bool,
}

/// The unified registry listing, sorted by name. Missing lockfile reads as
/// empty; a corrupt file is an error (callers warn the same way boot does).
pub fn list_managed(home: &Path) -> anyhow::Result<Vec<ManagedPlugin>> {
    migrate_commands_json(home);
    let lock = load_lock(home)?;
    Ok(lock
        .plugins
        .into_iter()
        .map(|(name, entry)| {
            let cli = entry.cli_argv.is_some();
            ManagedPlugin { name, entry, cli }
        })
        .collect())
}

/// One display row of the registry, pre-resolved: `on` already folds the
/// project enable overlay (see [`enabled`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRow {
    pub name: String,
    pub version: String,
    pub scope: String,
    pub ecosystem: String,
    pub on: bool,
    pub cli: bool,
}

/// Display rows (sorted by name) for `plugin list`, `/plugin list`, and the
/// manager picker. When the home dir is unresolvable, falls back to the
/// gray-pkg lockfile view instead of failing.
pub fn list_rows() -> anyhow::Result<Vec<ManagedRow>> {
    let Ok(home) = home() else {
        return Ok(gray_pkg::ops::list()?
            .into_iter()
            .map(|(name, e)| ManagedRow {
                cli: e.cli_argv.is_some(),
                name,
                version: e.version,
                scope: e.scope,
                ecosystem: e.ecosystem,
                on: e.enabled,
            })
            .collect());
    };
    Ok(list_managed(&home)?
        .into_iter()
        .map(|p| ManagedRow {
            on: enabled(&home, &p.name, &p.entry),
            name: p.name.clone(),
            version: p.entry.version.clone(),
            scope: p.entry.scope.clone(),
            ecosystem: p.entry.ecosystem.clone(),
            cli: p.cli,
        })
        .collect())
}

/// Remove a plugin: drop its `lock.json` entry and plugin dir via gray-pkg,
/// then drop its `<name>-manifest.json` so completion and help stop
/// advertising it, and clear a widget slot it owned.
pub fn remove_managed(name: &str) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    validate_name(name)?;
    gray_pkg::ops::remove(name)?;
    let _ = std::fs::remove_file(home.join("plugins").join(format!("{name}-manifest.json")));
    let widgets = home.join("plugins/widgets.json");
    if let Ok(raw) = std::fs::read(&widgets)
        && serde_json::from_slice::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))
            == Some(name.to_string())
    {
        let _ = std::fs::remove_file(&widgets);
    }
    Ok(())
}

/// `gray plugin enable/disable <name>`.
pub fn set_managed_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    gray_pkg::ops::set_enabled(name, on)
}

/// `gray plugin update [name|all]` — index-installed entries only; local
/// command registrations carry no index source and are skipped with a
/// warning before any index fetch.
pub async fn update_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::Report>> {
    if target != "all" && is_local_command(&home()?, target) {
        eprintln!("warning: skipping update of {target} (non-index source)");
        return Ok(Vec::new());
    }
    gray_pkg::ops::update(target).await
}

/// A lock row with `cli_argv` is a local command registration, not an
/// index-installed package. Lock load errors answer false (the index path
/// then reports whatever went wrong itself).
fn is_local_command(home: &Path, name: &str) -> bool {
    migrate_commands_json(home);
    load_lock(home)
        .ok()
        .and_then(|lock| lock.plugins.get(name).map(|e| e.cli_argv.is_some()))
        .unwrap_or(false)
}

/// Resolve only explicitly registered commands (never arbitrary PATH executables).
/// exec preserves terminal, signals, argument boundaries, and the child's exit code.
pub fn forward(home: &Path, name: &str, rest: &[String]) -> anyhow::Result<()> {
    validate_name(name)?;
    migrate_commands_json(home);
    let lock = load_lock(home)?;
    let entry = lock.plugins.get(name).with_context(|| {
        format!("no plugin command '{name}' — install it with: gray plugin install {name}")
    })?;
    let argv = entry
        .cli_argv
        .as_ref()
        .with_context(|| format!("'{name}' is a sidecar plugin, not a CLI command"))?;
    anyhow::ensure!(
        enabled(home, name, entry),
        "plugin command '{name}' is disabled"
    );
    let (program, args) = argv
        .split_first()
        .context("plugin command has no entry point")?;
    anyhow::ensure!(
        !program.is_empty(),
        "plugin command has an empty executable"
    );
    let mut command = Command::new(program);
    command
        .args(args)
        .args(rest)
        .env("GRAY_BIN", std::env::current_exe()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("could not start installed plugin command")
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .context("could not start installed plugin command")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Register a user-selected native executable; registration runs its manifest.
/// The executable and widgets remain owned by the separate plugin repository.
/// When `expected` is `Some`, the manifest name must match it; when `None`,
/// the manifest names the plugin (validated as a command name).
///
/// Writes one `lock.json` entry: `argv` is the sidecar invocation vector,
/// `cli_argv` enables `gray <name> …` forwarding and slash capture.
///
/// The CLI probe runs `<bin> manifest`; a wire-only sidecar (no CLI argv,
/// NDJSON on stdin) fails it and is probed over `plugin/manifest` instead —
/// those register sidecar-only (`cli_argv: None`, no widget slot).
/// Returns the user-facing confirmation line; callers choose how to display
/// it (CLI prints, the TUI routes it through `say`).
pub async fn register_native(
    home: &Path,
    expected: Option<&str>,
    binary: &Path,
    force: bool,
) -> anyhow::Result<String> {
    let binary = std::fs::canonicalize(binary)?;
    let mut command = tokio::process::Command::new(&binary);
    command.arg("manifest");
    let cli_probe = capture(command, std::time::Duration::from_secs(10))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|m| m["name"].is_string());
    let (manifest, wire_only) = match cli_probe {
        Some(m) => (m, false),
        None => {
            use gray_plugin::Plugin as _;
            let bin = binary.to_string_lossy().into_owned();
            let plugin = gray_plugin::SidecarPlugin::spawn(vec![bin]).await?;
            let manifest = serde_json::to_value(plugin.manifest())?;
            plugin.shutdown(std::time::Duration::from_secs(2)).await;
            (manifest, true)
        }
    };
    let name = match expected {
        Some(name) => {
            validate_name(name)?;
            anyhow::ensure!(
                manifest["name"].as_str() == Some(name),
                "manifest name does not match requested plugin"
            );
            name.to_string()
        }
        None => {
            let name = manifest["name"]
                .as_str()
                .context("plugin manifest has no name")?;
            validate_name(name)?;
            name.to_string()
        }
    };
    // A directory plugin is source we can read; a bare binary is not, so
    // scanning (and every verdict about its code) is skipped there.
    if binary.is_dir() {
        scan_or_block(binary.as_ref(), force)?;
    }
    let declared = manifest["capabilities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (granted, capabilities_hash) = consent_capabilities(&name, &declared);
    let runtime_role = provider_only_role_json(&manifest);
    std::fs::create_dir_all(home.join("plugins"))?;
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    if manifest["widget"].as_bool() == Some(true) && !wire_only {
        // v1 supports one above-editor plugin surface. Refuse to displace another.
        let path = home.join("plugins/widgets.json");
        if path.exists() {
            let old: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            anyhow::ensure!(
                old["name"].as_str() == Some(name.as_str()),
                "another plugin owns the widget slot"
            );
        }
    }
    let mut lock = load_lock(home)?;
    lock.plugins.insert(
        name.clone(),
        LockEntry {
            runtime_role,
            ecosystem: "gray-native".into(),
            version: manifest["version"].as_str().unwrap_or("unknown").into(),
            hash: String::new(),
            source: binary.to_string_lossy().into_owned(),
            argv: vec![binary.to_string_lossy().into_owned()],
            cli_argv: (!wire_only).then(|| vec![binary.to_string_lossy().into_owned()]),
            adapter_version: manifest["protocol"].as_str().unwrap_or("1").to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
            granted_capabilities: granted.clone(),
            capabilities_hash,
        },
    );
    lock.save(&gray_plugin::lock::lock_path(home))?;
    let mut metadata = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
    use std::io::Write;
    writeln!(metadata, "{}", manifest)?;
    metadata.persist(home.join("plugins").join(format!("{name}-manifest.json")))?;
    // Refresh the provider cache from the plugin lock so provider rows are
    // discoverable by `/connect` immediately after this registration.
    crate::providers::ProviderRegistry::refresh(home)?;
    if manifest["widget"].as_bool() == Some(true) && !wire_only {
        let mut tmp = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
        use std::io::Write;
        writeln!(
            tmp,
            "{}",
            serde_json::json!({"name":name,"argv":[binary,"widget"]})
        )?;
        tmp.persist(home.join("plugins/widgets.json"))?;
    }
    Ok(if wire_only {
        format!("Registered '{name}' from {}", binary.display())
    } else {
        format!(
            "Registered '{name}' from {}. Run: gray {name} --help",
            binary.display()
        )
    })
}

/// Slash commands use the registered executable without replacing the TUI.
pub async fn capture_slash(name: &str, args: &[String]) -> anyhow::Result<Option<String>> {
    let home = home()?;
    migrate_commands_json(&home);
    let lock = load_lock(&home)?;
    let selected = if let Some(entry) = lock.plugins.get(name).filter(|e| e.cli_argv.is_some()) {
        Some((name.to_string(), entry))
    } else {
        lock.plugins.iter().find_map(|(owner, entry)| {
            entry.cli_argv.as_ref()?;
            let m = metadata(&home, owner).ok()?;
            m["commands"]
                .as_array()?
                .iter()
                .any(|v| {
                    v.as_str()
                        .is_some_and(|cmd| cmd.trim_start_matches('/') == name)
                })
                .then_some((owner.clone(), entry))
        })
    };
    let Some((owner, entry)) = selected else {
        return Ok(None);
    };
    anyhow::ensure!(enabled(&home, &owner, entry), "plugin command is disabled");
    let argv = entry.cli_argv.as_ref().context("empty plugin command")?;
    let (program, base) = argv.split_first().context("empty plugin command")?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(base)
        .args(args)
        .env("GRAY_BIN", std::env::current_exe()?);
    let bytes = capture(command, std::time::Duration::from_secs(10)).await?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Only explicitly installed command metadata participates in completion.
pub(crate) fn completions(query: &str) -> Vec<(String, String)> {
    let Ok(home) = home() else { return Vec::new() };
    migrate_commands_json(&home);
    let Ok(lock) = load_lock(&home) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in lock.plugins {
        if entry.cli_argv.is_none() || !enabled(&home, &name, &entry) {
            continue;
        }
        let Ok(raw) = std::fs::read(home.join("plugins").join(format!("{name}-manifest.json")))
        else {
            continue;
        };
        let Ok(m) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(commands) = m["commands"].as_array() else {
            continue;
        };
        for command in commands.iter().filter_map(|v| v.as_str()) {
            let command = command.trim_start_matches('/');
            if !query.contains(' ') && command.starts_with(query) {
                out.push((command.to_string(), format!("{name} plugin")));
            }
            if let Some(args) = m["completion"].as_array() {
                for arg in args.iter().filter_map(|v| v.as_str()) {
                    let full = format!("{command} {arg}");
                    if query.contains(' ') && full.starts_with(query) {
                        out.push((full, format!("{name} plugin command")));
                    }
                }
            }
        }
    }
    out
}

/// Child stdout is bounded and every failure kills/reaps the child. No terminal
/// descriptors reach plugins. This is process isolation, not an OS sandbox.
pub(crate) async fn capture(
    mut command: tokio::process::Command,
    ttl: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().context("missing stdout")?.take(65537);
    let result = tokio::time::timeout(ttl, async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        anyhow::ensure!(bytes.len() <= 65536, "plugin output exceeds 64 KiB");
        let status = child.wait().await?;
        anyhow::ensure!(status.success(), "plugin exited {status}");
        Ok::<_, anyhow::Error>(bytes)
    })
    .await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            match other {
                Ok(Err(e)) => Err(e),
                Err(_) => anyhow::bail!("plugin request timed out"),
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_entry(argv: &[&str]) -> LockEntry {
        LockEntry {
            runtime_role: None,
            ecosystem: "gray-cli".into(),
            version: "catalog".into(),
            hash: String::new(),
            source: "git+https://example.invalid/x.git".into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            adapter_version: "1".into(),
            installed_at: "2026-09-18T00:00:00Z".into(),
            scope: "user".into(),
            enabled: true,
            ..Default::default()
        }
    }

    fn write_commands(home: &Path, plugins: &serde_json::Value) {
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(
            home.join("plugins/commands.json"),
            serde_json::json!({"schema": 1, "plugins": plugins}).to_string(),
        )
        .unwrap();
    }

    fn write_lock(home: &Path, plugins: &serde_json::Value) {
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(
            gray_plugin::lock::lock_path(home),
            serde_json::json!({"schema": 1, "plugins": plugins}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn list_managed_reads_lock_json_and_marks_cli_entries() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut cli = cli_entry(&["/usr/bin/false"]);
        cli.cli_argv = Some(cli.argv.clone());
        let sidecar = LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            source: "sidecar".into(),
            enabled: false,
            ..Default::default()
        };
        write_lock(
            home,
            &serde_json::json!({"cli-one": cli, "sidecar-one": sidecar}),
        );
        let rows = list_managed(home).unwrap();
        let names: Vec<_> = rows.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["cli-one", "sidecar-one"]);
        assert!(rows[0].cli);
        assert!(!rows[1].cli);
    }

    #[test]
    fn list_managed_is_empty_when_lockfile_missing() {
        let home = tempfile::tempdir().unwrap();
        assert!(list_managed(home.path()).unwrap().is_empty());
    }

    #[test]
    fn migrate_inserts_commands_entries_with_cli_argv() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/demo"])}),
        );
        migrate_commands_json(home);
        let lock = load_lock(home).unwrap();
        let entry = &lock.plugins["demo"];
        assert_eq!(
            entry.cli_argv.as_deref(),
            Some(&["/usr/bin/demo".to_string()][..])
        );
        assert_eq!(entry.argv, vec!["/usr/bin/demo".to_string()]);
        // The old registry is renamed aside; a second call is a no-op.
        assert!(!home.join("plugins/commands.json").exists());
        assert!(home.join("plugins/commands.json.migrated").exists());
        write_lock(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/kept"])}),
        );
        migrate_commands_json(home);
        assert_eq!(
            load_lock(home).unwrap().plugins["demo"].argv,
            vec!["/usr/bin/kept".to_string()]
        );
    }

    #[test]
    fn migrate_fills_missing_cli_argv_but_keeps_existing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // A same-named lock entry without cli_argv inherits the command argv.
        let bare = LockEntry {
            ecosystem: "gray-native".into(),
            argv: vec!["/sidecar".into()],
            ..Default::default()
        };
        write_lock(home, &serde_json::json!({"demo": bare}));
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/demo"])}),
        );
        migrate_commands_json(home);
        let lock = load_lock(home).unwrap();
        let entry = &lock.plugins["demo"];
        assert_eq!(entry.argv, vec!["/sidecar".to_string()]);
        assert_eq!(
            entry.cli_argv.as_deref(),
            Some(&["/usr/bin/demo".to_string()][..])
        );
    }

    #[test]
    fn migrate_keeps_an_existing_cli_argv() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut already = cli_entry(&["/current/bin"]);
        already.cli_argv = Some(vec!["/current/bin".into()]);
        write_lock(home, &serde_json::json!({"demo": already}));
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/stale/bin"])}),
        );
        migrate_commands_json(home);
        assert_eq!(
            load_lock(home).unwrap().plugins["demo"].cli_argv.as_deref(),
            Some(&["/current/bin".to_string()][..])
        );
    }

    #[test]
    fn migrate_is_a_no_op_when_commands_json_is_absent() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        migrate_commands_json(home);
        assert!(load_lock(home).unwrap().plugins.is_empty());
        assert!(!home.join("plugins/commands.json.migrated").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_bounds_output_and_reaps_timeout() {
        let mut flood = tokio::process::Command::new("sh");
        flood.args([
            "-c",
            "while :; do printf '01234567890123456789012345678901'; done",
        ]);
        let error = capture(flood, std::time::Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("64 KiB"), "{error}");
        let mut slow = tokio::process::Command::new("sh");
        slow.args(["-c", "exec sleep 30"]);
        let start = std::time::Instant::now();
        let error = capture(slow, std::time::Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error}");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn register_native_falls_back_to_the_wire_probe_for_cli_less_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let exe = home.path().join("wire-sidecar.sh");
        // Any argv fails the CLI probe instantly (exit 1); the sidecar wire
        // is the only language it speaks.
        std::fs::write(
            &exe,
            r#"#!/bin/sh
if [ $# -gt 0 ]; then exit 1; fi
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*) exit 0 ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"wiretest","version":"1.0.0","protocol":"1.1","tools":[],"commands":[]}}\n' "$id"
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        register_native(home.path(), None, &exe, false)
            .await
            .unwrap();
        let lock = load_lock(home.path()).unwrap();
        let entry = &lock.plugins["wiretest"];
        // register_native canonicalizes (macOS: /var → /private/var).
        let exe = std::fs::canonicalize(&exe).unwrap();
        assert_eq!(entry.argv, vec![exe.to_string_lossy().into_owned()]);
        assert_eq!(entry.cli_argv, None);
        assert_eq!(entry.adapter_version, "1.1");
        assert!(home.path().join("plugins/wiretest-manifest.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn register_native_caches_a_new_provider_plugin_on_first_install() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let exe = home.path().join("provider-sidecar.sh");
        // Wire-only sidecar whose manifest declares one provider.
        std::fs::write(
            &exe,
            r#"#!/bin/sh
if [ $# -gt 0 ]; then exit 1; fi
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*) exit 0 ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"provtest","version":"1.0.0","protocol":"1.2","tools":[],"commands":[],"providers":[{"id":"prov","name":"Prov","transport":{"kind":"openai-responses","base_url":"https://127.0.0.1:1/","authorization":{"kind":"bearer","secret_name":"k"}},"auth_methods":[{"id":"a","kind":"api_key","name":"A","operations":["models"]}]}]}}\n' "$id"
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        register_native(home.path(), None, &exe, false)
            .await
            .unwrap();
        // One install must land the plugin in the provider cache — the
        // refresh reads plugins/<name>-manifest.json, so it has to run
        // after that file exists (a second install used to be required).
        let cached = crate::providers::ProviderRegistry::load_cached(home.path());
        let entry = &cached.cache().plugins["provtest"];
        // Non-interactive consent grants nothing: the declared provider is
        // cached but hidden until granted.
        assert!(entry.providers.is_empty());
        assert!(
            entry
                .errors
                .iter()
                .any(|e| e.contains("provider capability not granted")),
            "{:?}",
            entry.errors
        );
        // After granting, a refresh exposes the declared provider.
        let mut lock = load_lock(home.path()).unwrap();
        lock.plugins
            .get_mut("provtest")
            .unwrap()
            .granted_capabilities = vec![gray_plugin::PROVIDER_CREDENTIALS.into()];
        lock.save(&gray_plugin::lock::lock_path(home.path()))
            .unwrap();
        crate::providers::ProviderRegistry::refresh(home.path()).unwrap();
        let cached = crate::providers::ProviderRegistry::load_cached(home.path());
        let entry = &cached.cache().plugins["provtest"];
        assert_eq!(entry.providers.len(), 1);
        assert_eq!(entry.providers[0].id, "prov");
    }

    #[test]
    fn is_local_command_recognizes_cli_lock_rows() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut cli = cli_entry(&["/usr/bin/false"]);
        cli.cli_argv = Some(cli.argv.clone());
        write_lock(
            home,
            &serde_json::json!({"cli-one": cli, "sidecar-one": LockEntry::default()}),
        );
        assert!(is_local_command(home, "cli-one"));
        assert!(!is_local_command(home, "sidecar-one"));
        assert!(!is_local_command(home, "absent"));
        // A not-yet-migrated commands.json row counts too.
        write_commands(
            home,
            &serde_json::json!({"migrated": cli_entry(&["/usr/bin/legacy"])}),
        );
        assert!(is_local_command(home, "migrated"));
    }
}
