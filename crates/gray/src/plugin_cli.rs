use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

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

fn migrate_commands_json(home: &Path) {
    let commands = home.join("plugins/commands.json");
    if !commands.exists() {
        return;
    }
    if let Err(e) = migrate_commands(home, &commands) {
        log::warn!("commands.json migration skipped: {e:#}");
    }
}

/// Reconciles the live `plugins/commands.json` CLI registry with the plugin
/// lock. A row folds into the lock only once an entry of the same name
/// exists — plugins write that entry themselves when their `setup` command
/// registers a sidecar argv (gray-discord's register() refuses to overwrite
/// a foreign argv, and the builder would spawn any non-empty `argv` as a
/// sidecar). Rows with no lock counterpart stay in commands.json, which is
/// still the registry `forward` reads for `gray <name>` dispatch.
fn migrate_commands(home: &Path, commands: &Path) -> anyhow::Result<()> {
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let old = LockFile::load(commands)?;
    let path = gray_plugin::lock::lock_path(home);
    let mut lock = LockFile::load(&path)?;
    let remaining = merge_command_entries(&mut lock, old);
    lock.save(&path)?;
    if remaining.plugins.is_empty() {
        std::fs::rename(commands, commands.with_file_name("commands.json.migrated"))?;
    } else {
        remaining.save(commands)?;
    }
    Ok(())
}

fn merge_command_entries(lock: &mut LockFile, old: LockFile) -> LockFile {
    let mut remaining = LockFile {
        schema: 1,
        plugins: std::collections::BTreeMap::new(),
    };
    for (name, entry) in old.plugins {
        match lock.plugins.get_mut(&name) {
            Some(cur) => {
                if cur.cli_argv.is_none() {
                    cur.cli_argv = Some(entry.argv);
                }
            }
            None => {
                remaining.plugins.insert(name, entry);
            }
        }
    }
    remaining
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

pub(crate) fn declared_subcommands(home: &Path, name: &str) -> Vec<String> {
    metadata(home, name)
        .ok()
        .and_then(|m| m["completion"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustGate {
    Ask,

    Quiet,

    Lines,
}

async fn index_entry(name: &str) -> Option<gray_pkg::index::Entry> {
    let client = gray_pkg::fetch::client().ok()?;
    if let Ok(Ok(index)) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        gray_pkg::index::fetch_hub_index(&client),
    )
    .await
        && let Ok(entry) = gray_pkg::index::lookup(&index, name)
    {
        return Some(entry.clone());
    }
    let index = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        gray_pkg::index::fetch_index(&client),
    )
    .await
    .ok()?
    .ok()?;
    gray_pkg::index::lookup(&index, name).ok().cloned()
}

pub async fn install_spec_lines(
    home: &Path,
    spec: &str,
    force: bool,
) -> anyhow::Result<Vec<String>> {
    install_spec_gated(home, spec, force, TrustGate::Lines).await
}

async fn install_spec_gated(
    home: &Path,
    spec: &str,
    force: bool,
    gate: TrustGate,
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
        return register_native_gated(home, Some(spec), Path::new(&path), force, gate).await;
    }
    let path = Path::new(spec);

    let path_like = path.is_absolute()
        || spec.starts_with('.')
        || spec.contains(std::path::MAIN_SEPARATOR)
        || spec.contains('/');
    if path_like && path.exists() {
        return register_native_gated(home, None, path, force, gate).await;
    }
    if validate_name(spec).is_ok()
        && let Some(paths) = std::env::var_os("PATH")
    {
        for dir in std::env::split_paths(&paths) {
            for stem in [format!("gray-{spec}"), spec.to_string()] {
                let candidate = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
                if candidate.is_file() {
                    return register_native_gated(home, Some(spec), &candidate, force, gate).await;
                }
            }
        }
    }
    let parsed = gray_pkg::ops::parse_spec(spec);
    let mut lines = Vec::new();

    if let gray_pkg::ops::NameOrUrl::Name(name) = &parsed
        && let Some(entry) = index_entry(name).await
        && !entry.trusted()
    {
        let warning = format!(
            "warning: '{name}' is an unverified community plugin — installing runs third-party code with your permissions"
        );
        match gate {
            TrustGate::Lines => {
                lines.push(warning);
                lines.push(
                    "  (unverified — use `gray plugin install --yes <name>` in the CLI to confirm)"
                        .to_string(),
                );
            }
            TrustGate::Quiet => eprintln!("{warning}"),
            TrustGate::Ask => {
                eprintln!("{warning}");

                if interactive() {
                    let granted = confirm("Install anyway? [y/N] ")?;
                    anyhow::ensure!(
                        granted,
                        "install cancelled: '{name}' is an unverified community plugin"
                    );
                }
            }
        }
    }
    let r = gray_pkg::ops::install(parsed, gray_pkg::ops::InstallOpts::default()).await?;
    if !r.deps.is_empty() {
        lines.push(format!(
            "auto-installed plugin dependencies: {}",
            r.deps.join(", ")
        ));
    }

    match adopt_cli(home, &r.name, &r.path).await {
        Ok(true) => lines.push(format!(
            "installed {} {} at {}. Run: gray {} --help",
            r.name,
            r.version,
            r.path.display(),
            r.name
        )),
        Ok(false) => lines.push(format!(
            "installed {} {} at {}",
            r.name,
            r.version,
            r.path.display()
        )),
        Err(e) => {
            lines.push(format!(
                "installed {} {} at {}",
                r.name,
                r.version,
                r.path.display()
            ));
            lines.push(format!(
                "warning: could not set up `gray {}` forwarding: {e:#}",
                r.name
            ));
        }
    }
    Ok(lines)
}

pub async fn install_spec(home: &Path, spec: &str, force: bool, yes: bool) -> anyhow::Result<()> {
    let gate = if yes {
        TrustGate::Quiet
    } else {
        TrustGate::Ask
    };
    for line in install_spec_gated(home, spec, force, gate).await? {
        println!("{line}");
    }
    Ok(())
}

const CATALOG: &[(&str, &str, &str)] = &[(
    "discord",
    "git+https://github.com/vstaln/gray-discord-plugin.git@b149022a1ca7c9e099007ffd8411c14489d8db93",
    "gray_discord",
)];

fn catalog_names() -> String {
    std::iter::once("background")
        .chain(CATALOG.iter().map(|(name, _, _)| *name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn commands_path(home: &Path) -> PathBuf {
    home.join("plugins/commands.json")
}

fn commands_or_empty(home: &Path) -> LockFile {
    load_commands(home).unwrap_or_else(|_| LockFile {
        schema: 1,
        plugins: std::collections::BTreeMap::new(),
    })
}

fn load_commands(home: &Path) -> anyhow::Result<LockFile> {
    let registry = LockFile::load(&commands_path(home))?;
    anyhow::ensure!(
        registry.schema == 1,
        "unsupported plugin command registry schema"
    );
    Ok(registry)
}

fn cli_registered(home: &Path, name: &str) -> bool {
    load_commands(home)
        .map(|registry| registry.plugins.contains_key(name))
        .unwrap_or(false)
        || load_lock(home)
            .map(|lock| {
                lock.plugins
                    .get(name)
                    .is_some_and(|entry| entry.cli_argv.is_some())
            })
            .unwrap_or(false)
}

/// A commands.json row resolves `gray <name>` forwarding: its `argv` is the
/// plugin's CLI argv. These rows are the live registry — unlike lock
/// entries, their `argv` is never spawned as a sidecar, so no lock entry is
/// created until the plugin writes one itself (e.g. `gray discord setup`).
fn commands_cli_entry(registry: &LockFile, name: &str, global_only: bool) -> Option<LockEntry> {
    registry
        .plugins
        .get(name)
        .filter(|e| !global_only || e.global)
        .map(|e| {
            let mut e = e.clone();
            e.cli_argv = Some(e.argv.clone());
            e
        })
}

fn run_installer(program: impl AsRef<OsStr>, args: &[&OsStr]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .env("PIP_NO_INPUT", "1")
        .status()
        .context("could not start plugin installer; check Python and python3-venv")?;
    anyhow::ensure!(
        status.success(),
        "plugin installation step failed ({status}); see output above"
    );
    Ok(())
}

async fn install_catalog(home: &Path, name: &str) -> anyhow::Result<()> {
    let (_, source, module) = CATALOG
        .iter()
        .find(|(n, _, _)| *n == name)
        .with_context(|| format!("Unknown plugin '{name}'. Available: {}", catalog_names()))?;
    if cli_registered(home, name) {
        println!("Plugin '{name}' is already registered. Run: gray {name} --help");
        return Ok(());
    }
    let root = home.join("plugins/cli");
    std::fs::create_dir_all(&root)?;
    let env = tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempdir_in(&root)?;
    let python = match std::env::var_os("GRAY_PLUGIN_PYTHON") {
        Some(python) => python,
        None => ["python3", "python", "/usr/bin/python3"]
            .into_iter()
            .find(|python| {
                Command::new(python)
                    .args(["-c", "import sys, venv, ensurepip, ctypes; assert sys.version_info >= (3, 11)"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            })
            .map(std::ffi::OsString::from)
            .context("Python 3.11+ with venv/pip support is required; install python3-venv or set GRAY_PLUGIN_PYTHON")?,
    };
    println!("Installing '{name}' from {source} (plugin code runs with your user permissions).");
    run_installer(
        &python,
        &[OsStr::new("-m"), OsStr::new("venv"), env.path().as_os_str()],
    )?;
    let executable = env.path().join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    run_installer(
        executable.as_os_str(),
        &[
            OsStr::new("-m"),
            OsStr::new("pip"),
            OsStr::new("install"),
            OsStr::new(source),
        ],
    )?;
    run_installer(
        executable.as_os_str(),
        &[OsStr::new("-m"), OsStr::new(module), OsStr::new("--help")],
    )?;
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins)?;
    {
        let _guard = gray_pkg::ops::hold_registry_lock_in(&plugins)?;
        if cli_registered(home, name) {
            println!("Plugin '{name}' is already registered. Run: gray {name} --help");
            return Ok(());
        }
        let mut registry = load_commands(home)?;
        registry.plugins.insert(
            name.into(),
            LockEntry {
                ecosystem: "gray-cli".into(),
                version: "catalog".into(),
                hash: String::new(),
                source: source.to_string(),
                argv: vec![
                    executable.to_string_lossy().into_owned(),
                    "-m".into(),
                    module.to_string(),
                ],
                adapter_version: "1".into(),
                installed_at: chrono::Utc::now().to_rfc3339(),
                scope: "user".into(),
                enabled: true,
                granted_capabilities: Vec::new(),
                capabilities_hash: None,
                runtime_role: None,
                cli_argv: None,
                global: false,
            },
        );
        registry.save(&commands_path(home))?;
    }
    let _ = env.keep();
    println!("Installed '{name}'. Next: gray {name} setup");
    Ok(())
}

pub async fn install(home: &Path, name: &str) -> anyhow::Result<()> {
    validate_name(name)?;
    if let Some(path) = std::env::var_os("GRAY_PLUGIN_PATH") {
        let line = register_native(home, Some(name), Path::new(&path), false).await?;
        if !line.is_empty() {
            println!("{line}");
        }
        return Ok(());
    }
    if name == "background" {
        install_spec(home, name, false, false).await?;
        return Ok(());
    }
    if CATALOG.iter().any(|(n, _, _)| *n == name) {
        return install_catalog(home, name).await;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for stem in [format!("gray-{name}"), name.to_string()] {
                let candidate = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
                if candidate.is_file() {
                    let line = register_native(home, Some(name), &candidate, false).await?;
                    if !line.is_empty() {
                        println!("{line}");
                    }
                    return Ok(());
                }
            }
        }
    }
    if index_entry(name).await.is_some() {
        install_spec(home, name, false, false).await?;
        return Ok(());
    }
    anyhow::bail!(
        "Unknown plugin '{name}'. Catalog: {}. For a local native plugin, put gray-{name} on PATH or set GRAY_PLUGIN_PATH to its executable",
        catalog_names()
    )
}

/// Install several specs in one call — `gray plugin install memory search mcp`.
/// One bad spec does not abort the rest; failures print inline and the command
/// exits non-zero with the list.
pub async fn install_specs(
    home: &Path,
    specs: &[String],
    force: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let mut failed = Vec::new();
    for spec in specs {
        if let Err(e) = install_spec(home, spec, force, yes).await {
            eprintln!("{spec}: {e:#}");
            failed.push(spec.as_str());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{} install(s) failed: {}", failed.len(), failed.join(", "))
    }
}

fn scan_or_block(root: &Path, force: bool, gate: TrustGate) -> anyhow::Result<Vec<String>> {
    use gray_plugin::scan::{Verdict, scan_tree};
    let report = scan_tree(root)?;
    let to_lines = matches!(gate, TrustGate::Lines);
    let mut out = Vec::new();
    let mut note = |s: String| {
        if to_lines {
            out.push(s);
        } else {
            crate::repl::say(None, &s);
        }
    };
    match report.verdict() {
        Verdict::Safe => {
            note(format!(
                "  plugin scan: {} — {}",
                report.summary(),
                Verdict::Safe.label()
            ));
        }
        Verdict::Caution => {
            if to_lines {
                anyhow::ensure!(
                    force,
                    "plugin scan reported caution ({}) and this surface cannot ask; \
                     re-run `gray plugin install` on the command line to review the findings",
                    report.summary()
                );
                note(format!("  plugin scan: caution — {}", report.summary()));
                for line in report.render().lines() {
                    note(line.to_string());
                }
                note("  continuing (--force)".to_string());
                return Ok(out);
            }
            note(format!("  plugin scan: caution — {}", report.summary()));
            crate::repl::say(None, report.render().trim_end());
            if force {
                note("  continuing (--force)".to_string());
                return Ok(out);
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
        }
        Verdict::Dangerous => {
            let critical: Vec<String> = report
                .criticals()
                .iter()
                .map(|f| format!("{} ({}:{})", f.rule, f.path, f.line))
                .collect();
            if !to_lines {
                crate::repl::say(None, report.render().trim_end());
            }
            anyhow::bail!(
                "plugin scan blocked the install: {} critical of {} findings ({}); \
                 --force does not override a dangerous verdict",
                critical.len(),
                report.findings.len(),
                critical.join(", ")
            );
        }
    }
    Ok(out)
}

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

fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

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

fn consent_capabilities(
    name: &str,
    declared: &[String],
    gate: TrustGate,
) -> (Vec<String>, Option<String>, Vec<String>) {
    if declared.is_empty() {
        return (Vec::new(), None, Vec::new());
    }
    let hash = gray_plugin::capabilities::consent_hash(declared);
    let mut lines = vec![format!(
        "'{name}' declares {} capability(ies):",
        declared.len()
    )];
    for id in declared {
        let what = gray_plugin::capabilities::spec(id)
            .map(|s| s.description)
            .unwrap_or("(unknown to this build)");
        lines.push(format!("  - {id}: {what}"));
    }
    if matches!(gate, TrustGate::Lines) {
        lines.push(format!(
            "  not granted (deferred). Grant later: gray plugin capabilities {name} --all"
        ));
        return (Vec::new(), Some(hash), lines);
    }
    for line in &lines {
        println!("{line}");
    }
    if !interactive() {
        println!(
            "  not granted (non-interactive session). Grant later: gray plugin capabilities {name} --all"
        );
        return (Vec::new(), Some(hash), Vec::new());
    }
    let granted = confirm("Grant these to this plugin? [y/N] ").unwrap_or(false);
    if granted {
        println!("  granted");
        (declared.to_vec(), Some(hash), Vec::new())
    } else {
        println!("  not granted; the plugin will run without them");
        (Vec::new(), Some(hash), Vec::new())
    }
}

pub fn print_capabilities(only: Option<&str>) -> anyhow::Result<()> {
    let home = home()?;

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

pub fn grant_all_capabilities(name: &str) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    let declared = gray_plugin::capabilities::parse_declared(
        &metadata(&home, name)
            .context("no cached manifest — is the plugin installed?")?
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    anyhow::ensure!(
        !declared.is_empty(),
        "'{name}' declares no capabilities — nothing to grant"
    );
    let mut lock = load_lock(&home)?;
    let entry = lock
        .plugins
        .get_mut(name)
        .with_context(|| format!("no plugin named '{name}'"))?;
    entry.granted_capabilities = declared.clone();
    entry.capabilities_hash = Some(gray_plugin::capabilities::consent_hash(&declared));
    lock.save(&gray_plugin::lock::lock_path(&home))?;
    crate::providers::ProviderRegistry::refresh(&home)?;
    println!("{name}: granted {}", declared.join(", "));
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPlugin {
    pub name: String,
    pub entry: LockEntry,
    pub cli: bool,
}

pub fn list_managed(home: &Path) -> anyhow::Result<Vec<ManagedPlugin>> {
    migrate_commands_json(home);
    let lock = load_lock(home)?;
    let mut rows: Vec<ManagedPlugin> = lock
        .plugins
        .iter()
        .map(|(name, entry)| ManagedPlugin {
            name: name.clone(),
            entry: entry.clone(),
            cli: entry.cli_argv.is_some(),
        })
        .collect();
    if let Ok(commands) = load_commands(home) {
        for (name, entry) in commands.plugins {
            if !lock.plugins.contains_key(&name) {
                rows.push(ManagedPlugin {
                    name,
                    entry,
                    cli: true,
                });
            }
        }
    }
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRow {
    pub name: String,
    pub version: String,
    pub scope: String,
    pub ecosystem: String,
    pub on: bool,
    pub cli: bool,
    pub global: bool,
}

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
                global: e.global,
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
            global: p.entry.global,
        })
        .collect())
}

fn update_commands_entry(
    home: &Path,
    name: &str,
    f: impl FnOnce(&mut LockEntry),
) -> anyhow::Result<()> {
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let path = commands_path(home);
    let mut registry = LockFile::load(&path)?;
    let Some(entry) = registry.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    f(entry);
    registry.save(&path)?;
    Ok(())
}

fn remove_commands_entry(home: &Path, name: &str) -> anyhow::Result<bool> {
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let path = commands_path(home);
    let mut registry = LockFile::load(&path)?;
    let removed = registry.plugins.remove(name).is_some();
    if removed {
        registry.save(&path)?;
    }
    Ok(removed)
}

fn commands_only(home: &Path, name: &str) -> bool {
    load_commands(home).is_ok_and(|r| r.plugins.contains_key(name))
        && load_lock(home).is_ok_and(|l| !l.plugins.contains_key(name))
}

pub fn remove_managed(name: &str) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    validate_name(name)?;
    let mut removed = remove_commands_entry(&home, name)?;
    if load_lock(&home)?.plugins.contains_key(name) {
        gray_pkg::ops::remove(name)?;
        removed = true;
    }
    let cli_root = home.join("plugins/cli");
    if cli_root.is_dir() {
        let prefix = format!("{name}-");
        for dir in std::fs::read_dir(&cli_root)?.flatten() {
            let dir = dir.path();
            if dir.is_dir()
                && dir
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
    anyhow::ensure!(removed, "not installed: {name}");
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

pub fn set_managed_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    if commands_only(&home, name) {
        return update_commands_entry(&home, name, |e| {
            e.enabled = on;
            if !on {
                e.global = false;
            }
        });
    }
    gray_pkg::ops::set_enabled(name, on)
}

pub fn set_managed_global(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    if commands_only(&home, name) {
        return update_commands_entry(&home, name, |e| e.global = on);
    }
    gray_pkg::ops::set_global(name, on)
}

pub async fn update_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::UpdateRow>> {
    let reports = gray_pkg::ops::update(target).await?;
    let home = home()?;
    for r in &reports {
        if let (Some(path), gray_pkg::ops::UpdateAction::Installed(_)) = (&r.path, &r.action)
            && let Err(e) = adopt_cli(&home, &r.name, path).await
        {
            eprintln!(
                "warning: could not set up `gray {}` forwarding: {e:#}",
                r.name
            );
        }
    }
    Ok(reports)
}

pub async fn check_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::UpdateRow>> {
    gray_pkg::ops::check_update(target).await
}

#[allow(dead_code)]
fn is_local_command(home: &Path, name: &str) -> bool {
    migrate_commands_json(home);
    load_lock(home)
        .ok()
        .and_then(|lock| {
            lock.plugins
                .get(name)
                .map(|e| e.cli_argv.is_some() && e.hash.is_empty())
        })
        .unwrap_or_else(|| {
            commands_cli_entry(&commands_or_empty(home), name, false)
                .is_some_and(|e| e.hash.is_empty())
        })
}

fn removed_builtin(name: &str) -> Option<&'static str> {
    match name {
        "view" => Some(
            "`gray view` was removed — `cat <path>` as the whole bash command \
             attaches the media to the turn",
        ),
        "login" | "whoami" | "logout" | "cloud" | "maker" => Some(
            "this moved to the account plugin — install or update it with: gray plugin install account",
        ),
        "cron" => Some(
            "this moved to the gateway plugin — install or update it with: gray plugin install gateway",
        ),
        _ => None,
    }
}

async fn index_owner_hint(name: &str) -> Option<String> {
    if crate::config::offline() {
        return None;
    }
    let client = gray_pkg::fetch::client().ok()?;
    let index = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        gray_pkg::index::fetch_index(&client),
    )
    .await
    .ok()?
    .ok()?;
    index
        .plugins
        .iter()
        .find(|(_, e)| {
            e.trusted()
                && e.commands
                    .iter()
                    .any(|c| c.trim_start_matches('/') == name)
        })
        .map(|(owner, _)| {
            format!(
                "'{name}' is provided by the {owner} plugin — install it with: gray install plugin {owner}"
            )
        })
}

fn owners_for_command(
    home: &Path,
    name: &str,
    lock: &LockFile,
    global_only: bool,
) -> Vec<(PathBuf, String, LockEntry)> {
    lock.plugins
        .iter()
        .filter(|(_, entry)| !global_only || entry.global)
        .filter(|(owner, entry)| {
            entry.cli_argv.is_some()
                && metadata(home, owner)
                    .ok()
                    .and_then(|m| m["commands"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .any(|v| {
                        v.as_str()
                            .is_some_and(|cmd| cmd.trim_start_matches('/') == name)
                    })
        })
        .map(|(owner, entry)| (home.to_path_buf(), owner.clone(), entry.clone()))
        .collect()
}

fn command_conflict_note(name: &str, claimers: &[(PathBuf, String, LockEntry)]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let owners = claimers
        .iter()
        .map(|(_, o, _)| o.as_str())
        .filter(|o| seen.insert(*o))
        .collect::<Vec<_>>();
    if owners.len() < 2 {
        return None;
    }
    Some(format!(
        "'{name}' is claimed by multiple plugins ({}) — using {}",
        owners.join(", "),
        owners[0]
    ))
}

fn disabled_hint(home: &Path, entry_home: &Path, project: Option<&LockFile>, name: &str) -> String {
    if project
        .and_then(|p| p.plugins.get(name))
        .is_some_and(|e| !e.enabled)
    {
        return "remove the disable from this project's .gray/plugins.json".to_string();
    }
    if crate::providers::registry::differs(home, entry_home) {
        return format!(
            "the flag lives in {} — re-enable with: GRAY_HOME={} gray plugin enable {name}",
            entry_home.display(),
            entry_home.display()
        );
    }
    format!("re-enable with: gray plugin enable {name}")
}

fn user_home_lock(home: &Path) -> Option<(PathBuf, LockFile, LockFile)> {
    let user = gray_core::paths::gray_user_home()
        .filter(|u| crate::providers::registry::differs(home, u))?;
    migrate_commands_json(&user);
    let lock = load_lock(&user).ok()?;
    let commands = commands_or_empty(&user);
    Some((user, lock, commands))
}

pub async fn forward(home: &Path, name: &str, rest: &[String]) -> anyhow::Result<()> {
    validate_name(name)?;
    migrate_commands_json(home);
    let lock = load_lock(home)?;
    let commands = load_commands(home)?;
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    let user_home = user_home_lock(home);

    let mut sidecar_named = false;
    let mut direct = None;
    for (h, l, c, global_only) in std::iter::once((home.to_path_buf(), &lock, &commands, false))
        .chain(user_home.iter().map(|(u, l, c)| (u.clone(), l, c, true)))
    {
        match l.plugins.get(name) {
            Some(e) if e.cli_argv.is_some() && (!global_only || e.global) => {
                direct = Some((e.clone(), h));
                break;
            }
            Some(e) if !e.cli_argv.is_some() && (!global_only || e.global) => {
                sidecar_named = true;
            }
            _ => {}
        }
        if let Some(e) = commands_cli_entry(c, name, global_only) {
            direct = Some((e, h));
            break;
        }
    }
    let (entry, entry_home, entry_name, prepend) = match direct {
        Some((e, u)) => (e, u, name.to_string(), false),
        None => {
            let mut claimers = owners_for_command(home, name, &lock, false);
            if let Some((u, l, _)) = &user_home {
                claimers.extend(owners_for_command(u, name, l, true));
            }
            let (ready, dead): (Vec<_>, Vec<_>) =
                claimers.into_iter().partition(|(h, o, e)| enabled(h, o, e));
            if let Some(note) = command_conflict_note(name, &ready) {
                eprintln!("{note}");
            }
            match ready.into_iter().next() {
                Some((u, owner, e)) => (e, u, owner, true),
                None if !dead.is_empty() => {
                    let (dead_home, owner, _) = &dead[0];
                    return Err(anyhow::anyhow!(
                        "plugin command '{owner}' is disabled — {}",
                        disabled_hint(home, dead_home, project.as_ref(), owner)
                    ));
                }
                None => {
                    return Err(anyhow::anyhow!(if sidecar_named {
                        format!("'{name}' is a sidecar plugin, not a CLI command")
                    } else {
                        match index_owner_hint(name).await {
                            Some(hint) => hint,
                            None => removed_builtin(name)
                                .map(str::to_string)
                                .unwrap_or_else(|| {
                                    format!(
                                        "no plugin command '{name}' — install it with: gray install plugin {name}"
                                    )
                                }),
                        }
                    }));
                }
            }
        }
    };
    anyhow::ensure!(
        enabled(&entry_home, &entry_name, &entry),
        "plugin command '{entry_name}' is disabled — {}",
        disabled_hint(home, &entry_home, project.as_ref(), &entry_name)
    );
    let argv = entry
        .cli_argv
        .with_context(|| format!("'{name}' is a sidecar plugin, not a CLI command"))?;
    let (program, args) = argv
        .split_first()
        .context("plugin command has no entry point")?;
    anyhow::ensure!(
        !program.is_empty(),
        "plugin command has an empty executable"
    );
    let mut command = Command::new(program);
    command.args(args);
    if prepend {
        command.arg(name);
    }
    command
        .args(rest)
        .env("GRAY_BIN", crate::exe_watch::on_disk_exe()?);
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

async fn cli_manifest(binary: &Path) -> Option<Value> {
    let mut command = tokio::process::Command::new(binary);
    command.arg("manifest");
    capture(command, std::time::Duration::from_secs(10))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|m| m["name"].is_string())
}

pub async fn register_native(
    home: &Path,
    expected: Option<&str>,
    binary: &Path,
    force: bool,
) -> anyhow::Result<String> {
    Ok(
        register_native_gated(home, expected, binary, force, TrustGate::Ask)
            .await?
            .pop()
            .unwrap_or_default(),
    )
}

async fn register_native_gated(
    home: &Path,
    expected: Option<&str>,
    binary: &Path,
    force: bool,
    gate: TrustGate,
) -> anyhow::Result<Vec<String>> {
    let binary = std::fs::canonicalize(binary)?;
    let cli_probe = cli_manifest(&binary).await;
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

    let mut lines = if binary.is_dir() {
        scan_or_block(binary.as_ref(), force, gate)?
    } else {
        Vec::new()
    };
    let declared = manifest["capabilities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (granted, capabilities_hash, consent_lines) = consent_capabilities(&name, &declared, gate);
    let runtime_role = provider_only_role_json(&manifest);
    std::fs::create_dir_all(home.join("plugins"))?;
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    if manifest["widget"].as_bool() == Some(true) && !wire_only {
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

            argv: std::iter::once(binary.to_string_lossy().into_owned())
                .chain(
                    manifest["sidecar_args"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_string)),
                )
                .collect(),
            cli_argv: (!wire_only).then(|| vec![binary.to_string_lossy().into_owned()]),
            adapter_version: manifest["protocol"].as_str().unwrap_or("1").to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
            global: lock.plugins.get(&name).is_some_and(|e| e.global),
            granted_capabilities: granted.clone(),
            capabilities_hash,
        },
    );

    let mut metadata = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
    use std::io::Write;
    writeln!(metadata, "{}", manifest)?;
    metadata.persist(home.join("plugins").join(format!("{name}-manifest.json")))?;
    lock.save(&gray_plugin::lock::lock_path(home))?;
    if !(manifest["widget"].as_bool() == Some(true) && !wire_only) {
        let widgets = home.join("plugins/widgets.json");
        if let Ok(raw) = std::fs::read(&widgets)
            && serde_json::from_slice::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v["name"].as_str().map(str::to_string))
                == Some(name.clone())
        {
            let _ = std::fs::remove_file(&widgets);
        }
    }

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
    lines.push(if wire_only {
        format!("Registered '{name}' from {}", binary.display())
    } else {
        format!(
            "Registered '{name}' from {}. Run: gray {name} --help",
            binary.display()
        )
    });
    lines.extend(consent_lines);
    Ok(lines)
}

pub(crate) async fn adopt_cli(home: &Path, name: &str, dir: &Path) -> anyhow::Result<bool> {
    validate_name(name)?;
    let exe = gray_plugin::builder::resolve_argv(dir)
        .ok()
        .and_then(|argv| argv.into_iter().next());
    let manifest = match exe.as_deref() {
        Some(exe) => cli_manifest(Path::new(exe)).await,
        None => None,
    };
    let plugins = home.join("plugins");
    let _guard = gray_pkg::ops::hold_registry_lock_in(&plugins)?;
    let lock_path = gray_plugin::lock::lock_path(home);
    let mut lock = load_lock(home)?;
    let Some(entry) = lock.plugins.get_mut(name) else {
        return Ok(false);
    };
    match manifest {
        Some(manifest) => {
            let exe = exe.expect("a manifest implies an executable");

            entry.argv = std::iter::once(exe.clone())
                .chain(
                    manifest["sidecar_args"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_string)),
                )
                .collect();
            entry.cli_argv = Some(vec![exe]);

            let mut metadata = tempfile::NamedTempFile::new_in(&plugins)?;
            use std::io::Write;
            writeln!(metadata, "{manifest}")?;
            metadata.persist(plugins.join(format!("{name}-manifest.json")))?;
            lock.save(&lock_path)?;

            crate::providers::ProviderRegistry::refresh(home)?;
            Ok(true)
        }
        None => {
            if !entry.hash.is_empty() {
                let mut changed = entry.cli_argv.take().is_some();
                let manifest_path = plugins.join(format!("{name}-manifest.json"));
                match std::fs::remove_file(&manifest_path) {
                    Ok(()) => changed = true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                if changed {
                    lock.save(&lock_path)?;
                    crate::providers::ProviderRegistry::refresh(home)?;
                }
            }
            Ok(false)
        }
    }
}

pub async fn capture_slash(name: &str, args: &[String]) -> anyhow::Result<Option<String>> {
    let home = home()?;
    migrate_commands_json(&home);
    let lock = load_lock(&home)?;
    let commands = commands_or_empty(&home);
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    let user_home = user_home_lock(&home);
    let mut conflict_note = None;
    let selected = if let Some(entry) = lock.plugins.get(name).filter(|e| e.cli_argv.is_some()) {
        Some((home.clone(), name.to_string(), entry.clone(), false))
    } else if let Some(entry) = commands_cli_entry(&commands, name, false) {
        Some((home.clone(), name.to_string(), entry, false))
    } else {
        let direct = user_home.as_ref().and_then(|(u, l, c)| {
            l.plugins
                .get(name)
                .filter(|e| e.cli_argv.is_some() && e.global)
                .map(|e| (u.clone(), name.to_string(), e.clone(), false))
                .or_else(|| {
                    commands_cli_entry(c, name, true)
                        .map(|e| (u.clone(), name.to_string(), e, false))
                })
        });
        direct.or_else(|| {
            let mut claimers = owners_for_command(&home, name, &lock, false);
            if let Some((u, l, _)) = &user_home {
                claimers.extend(owners_for_command(u, name, l, true));
            }
            let (ready, dead): (Vec<_>, Vec<_>) =
                claimers.into_iter().partition(|(h, o, e)| enabled(h, o, e));
            conflict_note = command_conflict_note(name, &ready);
            if ready.is_empty() {
                if let Some((u, owner, e)) = dead.into_iter().next() {
                    return Some((u, owner, e, true));
                }
                return None;
            }
            ready
                .into_iter()
                .next()
                .map(|(u, owner, e)| (u, owner, e, true))
        })
    };
    let Some((entry_home, entry_name, entry, prepend)) = selected else {
        return Ok(match index_owner_hint(name).await {
            Some(hint) => Some(hint),
            None => removed_builtin(name).map(str::to_string),
        });
    };
    if !enabled(&entry_home, &entry_name, &entry) {
        anyhow::bail!(
            "plugin command '{entry_name}' is disabled — {}",
            disabled_hint(&home, &entry_home, project.as_ref(), &entry_name)
        );
    }
    let argv = entry.cli_argv.as_ref().context("empty plugin command")?;
    let (program, base) = argv.split_first().context("empty plugin command")?;
    let mut command = tokio::process::Command::new(program);
    command.args(base);
    if prepend {
        command.arg(name);
    }
    command
        .args(args)
        .env("GRAY_BIN", crate::exe_watch::on_disk_exe()?);
    let bytes = match capture(command, std::time::Duration::from_secs(10)).await {
        Ok(bytes) => bytes,
        Err(e) => match &conflict_note {
            Some(note) => anyhow::bail!("{note}\n{e:#}"),
            None => return Err(e),
        },
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Some(match (conflict_note, text.trim().is_empty()) {
        (Some(note), false) => format!("{note}\n{text}"),
        (Some(note), true) => note,
        (None, _) => text,
    }))
}

struct CompletionRow {
    command: String,

    plugin: String,

    args: Vec<String>,
}

struct CompletionTable {
    home: PathBuf,
    user_home: Option<PathBuf>,
    stamps: Vec<(PathBuf, Option<(std::time::SystemTime, u64)>)>,
    rows: Vec<CompletionRow>,
}

#[derive(Default)]
struct CompletionCache {
    table: Option<CompletionTable>,
    builds: usize,
}

static COMPLETIONS: Mutex<CompletionCache> = Mutex::new(CompletionCache {
    table: None,
    builds: 0,
});

fn file_mtime(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn enabled_in(user: &LockFile, project: Option<&LockFile>, name: &str, entry: &LockEntry) -> bool {
    entry.enabled
        && project
            .and_then(|lock| lock.plugins.get(name))
            .or_else(|| user.plugins.get(name))
            .is_none_or(|entry| entry.enabled)
}

fn scan_lock_completions(
    home: &Path,
    lock: &LockFile,
    project: Option<&LockFile>,
    global_only: bool,
    stamps: &mut Vec<(PathBuf, Option<(std::time::SystemTime, u64)>)>,
    rows: &mut Vec<CompletionRow>,
    seen: &mut std::collections::HashSet<String>,
) {
    for (name, entry) in &lock.plugins {
        if entry.cli_argv.is_none()
            || (global_only && !entry.global)
            || !enabled_in(lock, project, name, entry)
        {
            continue;
        }
        let manifest = home.join("plugins").join(format!("{name}-manifest.json"));

        stamps.push((manifest.clone(), file_mtime(&manifest)));
        let Ok(raw) = std::fs::read(&manifest) else {
            continue;
        };
        let Ok(m) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(commands) = m["commands"].as_array() else {
            continue;
        };
        let own_commands: std::collections::HashSet<&str> = commands
            .iter()
            .filter_map(|v| v.as_str())
            .map(|c| c.trim_start_matches('/'))
            .collect();
        let args: Vec<String> = m["completion"]
            .as_array()
            .map(|args| {
                args.iter()
                    .filter_map(|v| v.as_str())
                    .filter(|a| !own_commands.contains(a))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for command in commands.iter().filter_map(|v| v.as_str()) {
            let command = command.trim_start_matches('/');
            if crate::repl::is_builtin_command(command) || !seen.insert(command.to_string()) {
                continue;
            }
            rows.push(CompletionRow {
                command: command.to_string(),
                plugin: name.clone(),
                args: args.clone(),
            });
        }
    }
}

fn build_completion_table(home: &Path, user_home: Option<&Path>) -> Option<CompletionTable> {
    let lock = load_lock(home).ok()?;
    let project_path = std::env::current_dir()
        .ok()
        .map(|cwd| gray_plugin::lock::project_lock_path(&cwd));
    let project = project_path
        .as_ref()
        .and_then(|path| LockFile::load(path).ok());
    let mut stamps = vec![
        (
            gray_plugin::lock::lock_path(home),
            file_mtime(&gray_plugin::lock::lock_path(home)),
        ),
        (
            project_path.clone().unwrap_or_default(),
            project_path.as_ref().and_then(|path| file_mtime(path)),
        ),
    ];
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scan_lock_completions(
        home,
        &lock,
        project.as_ref(),
        false,
        &mut stamps,
        &mut rows,
        &mut seen,
    );
    if let Some(user) = user_home.filter(|u| crate::providers::registry::differs(home, u)) {
        let user_lock_path = gray_plugin::lock::lock_path(user);
        stamps.push((user_lock_path.clone(), file_mtime(&user_lock_path)));
        migrate_commands_json(user);
        if let Ok(user_lock) = load_lock(user) {
            scan_lock_completions(
                user,
                &user_lock,
                project.as_ref(),
                true,
                &mut stamps,
                &mut rows,
                &mut seen,
            );
        }
    }
    Some(CompletionTable {
        home: home.to_path_buf(),
        user_home: user_home.map(Path::to_path_buf),
        stamps,
        rows,
    })
}

fn completion_table<'a>(
    home: &Path,
    user_home: Option<&Path>,
    cache: &'a mut CompletionCache,
) -> Option<&'a CompletionTable> {
    let fresh = cache.table.as_ref().is_some_and(|table| {
        table.home == home
            && table.user_home.as_deref() == user_home
            && table
                .stamps
                .iter()
                .all(|(path, stamp)| file_mtime(path) == *stamp)
    });
    if !fresh {
        cache.table = build_completion_table(home, user_home);
        cache.builds += 1;
    }
    cache.table.as_ref()
}

pub(crate) fn completions(query: &str) -> Vec<(String, String)> {
    let Ok(home) = home() else { return Vec::new() };
    migrate_commands_json(&home);
    let user_home = gray_core::paths::gray_user_home();
    let mut out = Vec::new();
    let Ok(mut guard) = COMPLETIONS.lock() else {
        return out;
    };
    let Some(table) = completion_table(&home, user_home.as_deref(), &mut guard) else {
        return Vec::new();
    };
    for row in &table.rows {
        if !query.contains(' ') && row.command.starts_with(query) {
            out.push((row.command.clone(), format!("{} plugin", row.plugin)));
        }
        for arg in &row.args {
            let full = format!("{} {arg}", row.command);
            if query.contains(' ') && full.starts_with(query) {
                out.push((full, format!("{} plugin command", row.plugin)));
            }
        }
    }
    out
}

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
        if !status.success() {
            let tail = String::from_utf8_lossy(&bytes);
            let tail = tail.trim_end();
            let tail = tail.get(tail.len().saturating_sub(500)..).unwrap_or(tail);
            if tail.is_empty() {
                anyhow::bail!("plugin exited {status}");
            }
            anyhow::bail!("plugin exited {status} — last output: {tail}");
        }
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
    fn migrate_keeps_orphan_command_rows_in_the_live_registry() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/demo"])}),
        );
        migrate_commands_json(home);
        // A CLI-only row must never create a lock entry: the plugin writes
        // one itself when `gray <name> setup` registers the sidecar argv,
        // and a foreign argv there makes register() refuse to overwrite.
        assert!(!load_lock(home).unwrap().plugins.contains_key("demo"));
        let registry = load_commands(home).unwrap();
        let entry = commands_cli_entry(&registry, "demo", false).unwrap();
        assert_eq!(
            entry.cli_argv.as_deref(),
            Some(&["/usr/bin/demo".to_string()][..])
        );
        assert!(home.join("plugins/commands.json").exists());
        assert!(!home.join("plugins/commands.json.migrated").exists());
    }

    #[test]
    fn migrate_rewrites_unconsumed_rows_back_to_commands_json() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let bare = LockEntry {
            ecosystem: "gray-native".into(),
            argv: vec!["/sidecar".into()],
            ..Default::default()
        };
        write_lock(home, &serde_json::json!({"demo": bare}));
        write_commands(
            home,
            &serde_json::json!({
                "demo": cli_entry(&["/usr/bin/demo"]),
                "solo": cli_entry(&["/usr/bin/solo"]),
            }),
        );
        migrate_commands_json(home);
        let lock = load_lock(home).unwrap();
        assert_eq!(
            lock.plugins["demo"].cli_argv.as_deref(),
            Some(&["/usr/bin/demo".to_string()][..])
        );
        let registry = load_commands(home).unwrap();
        assert!(!registry.plugins.contains_key("demo"));
        assert!(registry.plugins.contains_key("solo"));
        assert!(home.join("plugins/commands.json").exists());
        assert!(!home.join("plugins/commands.json.migrated").exists());
    }

    #[test]
    fn commands_cli_entry_respects_the_global_flag() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut local = cli_entry(&["/usr/bin/local"]);
        local.global = false;
        let mut shared = cli_entry(&["/usr/bin/shared"]);
        shared.global = true;
        write_commands(home, &serde_json::json!({"local": local, "shared": shared}));
        let registry = load_commands(home).unwrap();
        assert!(commands_cli_entry(&registry, "local", true).is_none());
        assert!(commands_cli_entry(&registry, "shared", true).is_some());
        assert!(commands_cli_entry(&registry, "local", false).is_some());
    }

    #[test]
    fn commands_only_rows_update_and_remove_without_touching_the_lock() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/demo"])}),
        );
        assert!(commands_only(home, "demo"));
        update_commands_entry(home, "demo", |e| {
            e.enabled = false;
            e.global = false;
        })
        .unwrap();
        assert!(!load_commands(home).unwrap().plugins["demo"].enabled);
        assert!(!load_lock(home).unwrap().plugins.contains_key("demo"));
        assert!(remove_commands_entry(home, "demo").unwrap());
        assert!(!commands_only(home, "demo"));
        assert!(!remove_commands_entry(home, "demo").unwrap());
    }

    #[test]
    fn commands_only_is_false_once_a_lock_entry_exists() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"demo": cli_entry(&["/usr/bin/demo"])}),
        );
        assert!(commands_only(home, "demo"));
        let bare = LockEntry {
            ecosystem: "gray-native".into(),
            argv: vec!["/sidecar".into()],
            ..Default::default()
        };
        write_lock(home, &serde_json::json!({"demo": bare}));
        assert!(!commands_only(home, "demo"));
        assert!(!commands_only(home, "missing"));
    }

    #[test]
    fn list_managed_includes_live_commands_rows() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"cli-only": cli_entry(&["/usr/bin/x"])}),
        );
        let rows = list_managed(home).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "cli-only");
        assert!(rows[0].cli);
    }

    #[test]
    fn migrate_fills_missing_cli_argv_but_keeps_existing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();

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
        assert!(!home.join("plugins/commands.json").exists());
        assert!(home.join("plugins/commands.json.migrated").exists());
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
        assert!(!home.join("plugins/commands.json").exists());
        assert!(home.join("plugins/commands.json.migrated").exists());
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
        std::fs::create_dir_all(home.path().join("plugins")).unwrap();
        std::fs::write(
            home.path().join("plugins/widgets.json"),
            r#"{"name":"wiretest","argv":["old","widget"]}"#,
        )
        .unwrap();
        register_native(home.path(), None, &exe, false)
            .await
            .unwrap();
        assert!(!home.path().join("plugins/widgets.json").exists());
        let lock = load_lock(home.path()).unwrap();
        let entry = &lock.plugins["wiretest"];

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

        let cached = crate::providers::ProviderRegistry::load_cached(home.path());
        let entry = &cached.cache().plugins["provtest"];

        assert!(entry.providers.is_empty());
        assert!(
            entry
                .errors
                .iter()
                .any(|e| e.contains("provider capability not granted")),
            "{:?}",
            entry.errors
        );

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

        let mut index_cli = cli_entry(&[]);
        index_cli.cli_argv = Some(vec!["/usr/bin/index-cli".into()]);
        index_cli.hash = "sha256:abc".into();
        write_lock(
            home,
            &serde_json::json!({
                "cli-one": cli,
                "index-cli": index_cli,
                "sidecar-one": LockEntry::default()
            }),
        );
        assert!(is_local_command(home, "cli-one"));
        assert!(!is_local_command(home, "index-cli"));
        assert!(!is_local_command(home, "sidecar-one"));
        assert!(!is_local_command(home, "absent"));

        write_commands(
            home,
            &serde_json::json!({"migrated": cli_entry(&["/usr/bin/legacy"])}),
        );
        assert!(is_local_command(home, "migrated"));
    }

    fn index_row(cli_argv: Option<Vec<String>>) -> LockEntry {
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            hash: "sha256:abc".into(),
            source: "https://example.invalid/demo.tar.gz".into(),
            argv: vec![],
            cli_argv,
            ..Default::default()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_cli_gives_an_index_install_gray_forwarding() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let dir = home.join("plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();

        let exe = dir.join("gray-demo");
        std::fs::write(
            &exe,
            "#!/bin/sh\nprintf '%s\\n' '{\"name\":\"demo-sub\",\"version\":\"1.0.0\",\"completion\":[\"run\"],\"sidecar_args\":[\"sidecar\"]}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_lock(home, &serde_json::json!({"demo": index_row(None)}));

        assert!(adopt_cli(home, "demo", &dir).await.unwrap());

        let entry = &load_lock(home).unwrap().plugins["demo"];
        assert_eq!(
            entry.cli_argv.as_deref(),
            Some([exe.to_string_lossy().into_owned()].as_slice())
        );

        assert_eq!(
            entry.argv,
            vec![exe.to_string_lossy().into_owned(), "sidecar".to_string()]
        );

        assert_eq!(entry.hash, "sha256:abc");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.ecosystem, "gray-native");
        assert_eq!(entry.source, "https://example.invalid/demo.tar.gz");
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(home.join("plugins/demo-manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "demo-sub");
        assert_eq!(manifest["completion"], serde_json::json!(["run"]));

        let cache =
            crate::providers::ProviderCache::load(&crate::providers::registry::cache_path(home));
        assert!(cache.plugins.contains_key("demo"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_cli_leaves_wire_only_sidecars_alone() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let dir = home.join("plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("gray-demo");

        std::fs::write(&exe, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_lock(home, &serde_json::json!({"demo": index_row(None)}));

        assert!(!adopt_cli(home, "demo", &dir).await.unwrap());
        assert_eq!(load_lock(home).unwrap().plugins["demo"].cli_argv, None);
        assert!(!home.join("plugins/demo-manifest.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_cli_clears_a_stale_cli_argv_when_an_update_drops_cli() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let dir = home.join("plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("gray-demo");
        std::fs::write(&exe, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        write_lock(
            home,
            &serde_json::json!({"demo": index_row(Some(vec!["/old/exe".into()]))}),
        );
        std::fs::write(
            home.join("plugins/demo-manifest.json"),
            "{\"name\":\"demo-sub\"}",
        )
        .unwrap();

        assert!(!adopt_cli(home, "demo", &dir).await.unwrap());
        let entry = &load_lock(home).unwrap().plugins["demo"];
        assert_eq!(entry.cli_argv, None);
        assert_eq!(entry.hash, "sha256:abc");
        assert!(!home.join("plugins/demo-manifest.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_spec_lines_defers_capability_consent() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let exe = home.join("gray-capsdemo");
        std::fs::write(
            &exe,
            "#!/bin/sh\nprintf '%s\\n' '{\"name\":\"capsdemo\",\"version\":\"1.0.0\",\"capabilities\":[\"provider.credentials\"]}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = exe.to_string_lossy().into_owned();
        let lines = install_spec_lines(home, &spec, false).await.unwrap();
        let text = lines.join("\n");
        assert!(
            text.contains("Registered 'capsdemo'"),
            "missing confirmation: {text}"
        );
        assert!(
            text.contains("declares 1 capability(ies):"),
            "missing declaration block: {text}"
        );
        assert!(
            text.contains("provider.credentials"),
            "missing capability id: {text}"
        );
        assert!(
            text.contains("not granted (deferred)"),
            "missing deferral line: {text}"
        );
        assert!(
            text.contains("gray plugin capabilities capsdemo --all"),
            "missing grant hint: {text}"
        );
        let entry = &load_lock(home).unwrap().plugins["capsdemo"];
        assert!(entry.granted_capabilities.is_empty());
        assert!(entry.capabilities_hash.is_some());
    }

    #[test]
    fn completion_table_rebuilds_only_when_inputs_move() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut cli = cli_entry(&["/usr/bin/demo"]);
        cli.cli_argv = Some(cli.argv.clone());
        write_lock(home, &serde_json::json!({"demo": cli}));
        let manifest = home.join("plugins/demo-manifest.json");
        std::fs::write(
            &manifest,
            r#"{"name":"demo","commands":["/demo"],"completion":["run"]}"#,
        )
        .unwrap();
        let mut cache = CompletionCache::default();
        {
            let table = completion_table(home, None, &mut cache).unwrap();
            assert_eq!(table.rows.len(), 1);
            assert_eq!(table.rows[0].command, "demo");
            assert_eq!(table.rows[0].args, vec!["run".to_string()]);

            assert_eq!(table.stamps.len(), 3);
        }
        assert_eq!(cache.builds, 1);

        for _ in 0..5 {
            assert_eq!(
                completion_table(home, None, &mut cache).unwrap().rows.len(),
                1
            );
        }
        assert_eq!(cache.builds, 1);

        std::fs::remove_file(&manifest).unwrap();
        assert!(
            completion_table(home, None, &mut cache)
                .unwrap()
                .rows
                .is_empty()
        );
        assert_eq!(cache.builds, 2);

        let lock_path = gray_plugin::lock::lock_path(home);
        let mut lock = gray_plugin::lock::LockFile::load(&lock_path).unwrap();
        lock.plugins.get_mut("demo").unwrap().enabled = false;
        lock.save(&lock_path).unwrap();
        assert!(
            completion_table(home, None, &mut cache)
                .unwrap()
                .rows
                .is_empty()
        );
        assert_eq!(cache.builds, 3);
    }

    #[test]
    fn completion_args_shadowed_by_own_commands_are_dropped() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut cli = cli_entry(&["/usr/bin/acct"]);
        cli.cli_argv = Some(cli.argv.clone());
        write_lock(home, &serde_json::json!({"acct": cli}));
        std::fs::write(
            home.join("plugins/acct-manifest.json"),
            r#"{"name":"acct","commands":["/login","/marketplace"],"completion":["login","new"]}"#,
        )
        .unwrap();
        let mut cache = CompletionCache::default();
        let table = completion_table(home, None, &mut cache).unwrap();
        assert_eq!(table.rows.len(), 2);
        for row in &table.rows {
            assert_eq!(row.args, vec!["new".to_string()]);
        }
    }
}
