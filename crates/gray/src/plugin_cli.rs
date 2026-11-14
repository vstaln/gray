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

/// Reconciles a legacy `plugins/commands.json` CLI registry with the plugin
/// lock. A row folds into the lock only once an entry of the same name
/// exists — plugins write that entry themselves when their `setup` command
/// registers a sidecar argv (gray-discord's register() refuses to overwrite
/// a foreign argv, and the builder would spawn any non-empty `argv` as a
/// sidecar). Rows with no lock counterpart stay in commands.json.
fn migrate_commands(home: &Path, commands: &Path) -> anyhow::Result<()> {
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let old = LockFile::load(commands)?;
    let path = gray_plugin::lock::lock_path(home);
    let mut lock = LockFile::load(&path)?;
    let mut remaining = LockFile::default();
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
    lock.save(&path)?;
    if remaining.plugins.is_empty() {
        std::fs::rename(commands, commands.with_file_name("commands.json.migrated"))?;
    } else {
        remaining.save(commands)?;
    }
    Ok(())
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
    let Ok(user) = LockFile::load(&gray_plugin::lock::lock_path(home)) else {
        return false;
    };
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    enabled_in(&user, project.as_ref(), name, entry)
}

pub(crate) fn declared_subcommands(home: &Path, name: &str) -> Vec<String> {
    metadata(home, name)
        .ok()
        .and_then(|m| m["completion"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| completion_entry(&v).map(|(arg, _)| arg))
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

pub(crate) fn command_description(home: &Path, name: &str, command: &str) -> Option<String> {
    description_in(&metadata(home, name).ok()?, command)
}

pub(crate) fn plugin_description(home: &Path, name: &str) -> Option<String> {
    blurb(&metadata(home, name).ok()?)
}

fn blurb(manifest: &serde_json::Value) -> Option<String> {
    manifest["description"]
        .as_str()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

fn description_in(manifest: &serde_json::Value, command: &str) -> Option<String> {
    let map = manifest["command_descriptions"].as_object()?;
    let bare = command.trim_start_matches('/');
    map.get(&format!("/{bare}"))
        .or_else(|| map.get(bare))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
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

/// A local `gray-<name>` on PATH, for plugins the catalog does not carry.
/// Never the bare `<name>`: that is whatever program owns the word (`discord`
/// is the Discord desktop client), and catalog names install from the index.
fn native_on_path(name: &str) -> Option<PathBuf> {
    native_in(&std::env::var_os("PATH")?, name)
}

fn native_in(paths: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    let file = format!("gray-{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(paths)
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file())
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
        && let Some(candidate) = native_on_path(spec)
        && index_entry(spec).await.is_none()
    {
        return register_native_gated(home, Some(spec), &candidate, force, gate).await;
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
    // Dependencies are installed without a cached manifest; adopt them now so
    // they start lazily like the plugin that pulled them in.
    for dep in &r.deps {
        if let Err(e) = adopt_cli(home, dep, &home.join("plugins").join(dep)).await {
            lines.push(format!(
                "warning: could not set up `gray {dep}` forwarding: {e:#}"
            ));
        }
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
    if index_entry(name).await.is_some() {
        install_spec(home, name, false, false).await?;
        return Ok(());
    }
    if let Some(candidate) = native_on_path(name) {
        let line = register_native(home, Some(name), &candidate, false).await?;
        if !line.is_empty() {
            println!("{line}");
        }
        return Ok(());
    }
    anyhow::bail!(
        "Unknown plugin '{name}'. For a local native plugin, put gray-{name} on PATH or set GRAY_PLUGIN_PATH to its executable"
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
    if matches!(gate, TrustGate::Quiet) {
        println!("  granted (--yes)");
        return (declared.to_vec(), Some(hash), Vec::new());
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
            declared
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
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

/// Grant a single declared capability to an installed plugin (no output);
/// returns true when the grant set changed. Used when an in-TUI flow installs
/// a provider plugin specifically to sign in — the interactive install prompt
/// cannot run there, so `provider.credentials` would otherwise stay deferred
/// and the provider row hidden.
pub(crate) fn grant_capability(home: &Path, name: &str, capability: &str) -> anyhow::Result<bool> {
    migrate_commands_json(home);
    let declared = gray_plugin::capabilities::parse_declared(
        &metadata(home, name)
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
    if !declared.iter().any(|d| d == capability) {
        return Ok(false);
    }
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
    let mut lock = load_lock(home)?;
    let entry = lock
        .plugins
        .get_mut(name)
        .with_context(|| format!("no plugin named '{name}'"))?;
    if entry.granted_capabilities.iter().any(|g| g == capability) {
        return Ok(false);
    }
    entry.granted_capabilities.push(capability.to_string());
    entry.capabilities_hash = Some(gray_plugin::capabilities::consent_hash(&declared));
    lock.save(&gray_plugin::lock::lock_path(home))?;
    crate::providers::ProviderRegistry::refresh(home)?;
    Ok(true)
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
    let _guard = gray_pkg::ops::hold_registry_lock_in(&home.join("plugins"))?;
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

/// Names of every managed plugin.
pub fn installed_names(home: &Path) -> std::collections::BTreeSet<String> {
    list_managed(home)
        .map(|rows| rows.into_iter().map(|p| p.name).collect())
        .unwrap_or_default()
}

pub fn list_managed(home: &Path) -> anyhow::Result<Vec<ManagedPlugin>> {
    migrate_commands_json(home);
    let lock = load_lock(home)?;
    Ok(lock
        .plugins
        .iter()
        .map(|(name, entry)| ManagedPlugin {
            name: name.clone(),
            entry: entry.clone(),
            cli: entry.cli_argv.is_some(),
        })
        .collect())
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

pub fn remove_managed(name: &str) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    validate_name(name)?;
    let removed = load_lock(&home)?.plugins.contains_key(name);
    if removed {
        gray_pkg::ops::remove(name)?;
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
    gray_pkg::ops::set_enabled(name, on)
}

pub fn set_managed_global(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
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
    if let Err(e) = crate::providers::ProviderRegistry::refresh(&home) {
        log::warn!("could not refresh providers after update: {e:#}");
    }
    Ok(reports)
}

pub async fn check_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::UpdateRow>> {
    gray_pkg::ops::check_update(target).await
}

/// The singular/plural counterpart of `name`: drops a trailing 's', or
/// adds one. `forward` tries the typed spelling first — this is only a
/// fallback alias.
fn number_variant(name: &str) -> Option<String> {
    match name.strip_suffix('s') {
        Some(stripped) if !stripped.is_empty() => Some(stripped.to_string()),
        Some(_) => None,
        None => Some(format!("{name}s")),
    }
}

/// Close-match hints for an unknown `gray <name>`: builtin subcommands and
/// their aliases, installed plugin names, and claimed /commands from
/// manifests. Same jaro-winkler cutoff as gray-core's unknown-tool hint.
fn command_suggestion(home: &Path, lock: &LockFile, typed: &str) -> Option<String> {
    use clap::CommandFactory;
    let mut names: std::collections::BTreeSet<String> = lock.plugins.keys().cloned().collect();
    for sub in crate::Cli::command().get_subcommands() {
        names.insert(sub.get_name().to_string());
        names.extend(sub.get_all_aliases().map(str::to_string));
    }
    for owner in lock.plugins.keys() {
        if let Ok(m) = metadata(home, owner) {
            names.extend(
                m["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .map(|c| c.trim_start_matches('/').to_string()),
            );
        }
    }
    let mut scored: Vec<(String, f64)> = names
        .into_iter()
        .map(|n| {
            let s = strsim::jaro_winkler(typed, &n);
            (n, s)
        })
        .filter(|(n, s)| *s >= 0.87 && n != typed)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut it = scored.into_iter().take(3).map(|(n, _)| format!("'{n}'"));
    let first = it.next()?;
    let rest: Vec<String> = it.collect();
    Some(if rest.is_empty() {
        first
    } else {
        format!("{}, {}", first, rest.join(", "))
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
        .find(|(owner, e)| {
            e.trusted()
                && (owner.as_str() == name
                    || e.commands
                        .iter()
                        .any(|c| c.trim_start_matches('/') == name))
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

fn command_conflict_note<'a>(
    name: &str,
    claimers: impl Iterator<Item = &'a str>,
) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let owners = claimers.filter(|o| seen.insert(*o)).collect::<Vec<_>>();
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

fn user_home_lock(home: &Path) -> Option<(PathBuf, LockFile)> {
    let user = gray_core::paths::gray_user_home()
        .filter(|u| crate::providers::registry::differs(home, u))?;
    migrate_commands_json(&user);
    let lock = load_lock(&user).ok()?;
    Some((user, lock))
}

fn sidecar_usage(home: &Path, name: &str) -> String {
    if let Err(e) = gray_plugin::builder::resolve_argv(&home.join("plugins").join(name)) {
        return format!("plugin '{name}' cannot start: {e:#}");
    }
    format!(
        "'{name}' runs inside a Gray session, not as `gray {name}`: start `gray` and type /{name}, or run it headless with `gray cmd run {name} [args]`"
    )
}

/// Probe a lock-registered plugin whose `cli_argv` was never recorded —
/// bulk syncs and older installers wrote entries straight to the lock —
/// so `gray <name>` forwarding self-heals on first dispatch instead of
/// dying on "sidecar plugin, not a CLI command". Reloads `lock` when
/// adoption wrote the entry; silent no-op when nothing changed.
async fn lazy_adopt(home: &Path, name: &str, lock: &mut LockFile) {
    let needs_probe = lock
        .plugins
        .get(name)
        .is_some_and(|e| e.cli_argv.is_none() && enabled(home, name, e));
    if needs_probe
        && adopt_cli(home, name, &home.join("plugins").join(name))
            .await
            .unwrap_or(false)
        && let Ok(fresh) = load_lock(home)
    {
        *lock = fresh;
    }
}

/// Plugin names `gray <name>` forwarding can reach: every enabled lock
/// entry qualifies, since dispatch lazily adopts entries whose `manifest`
/// verb was never probed at install time.
pub(crate) fn forwardable_names() -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let Ok(home) = home() else {
        return names;
    };
    if let Ok(lock) = load_lock(&home) {
        names.extend(lock.plugins.into_keys());
    }
    if let Some((_, lock)) = user_home_lock(&home) {
        names.extend(lock.plugins.into_keys());
    }
    names
}

pub async fn forward(home: &Path, name: &str, rest: &[String]) -> anyhow::Result<()> {
    validate_name(name)?;
    migrate_commands_json(home);
    let mut lock = load_lock(home)?;
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    let mut user_home = user_home_lock(home);

    // The typed name plus its singular/plural counterpart as an implicit
    // alias — the exact spelling always wins; the variant is only a
    // fallback (subagent → subagents, plugins → plugin).
    let alt = number_variant(name);
    let names: Vec<&str> = std::iter::once(name).chain(alt.as_deref()).collect();
    for n in &names {
        lazy_adopt(home, n, &mut lock).await;
        if let Some((u, l)) = user_home.as_mut() {
            lazy_adopt(u, n, l).await;
        }
    }

    let mut sidecar_named = false;
    let mut direct = None;
    'scan: for (h, l, global_only) in std::iter::once((home.to_path_buf(), &lock, false))
        .chain(user_home.iter().map(|(u, l)| (u.clone(), l, true)))
    {
        for n in &names {
            match l.plugins.get(*n) {
                Some(e) if e.cli_argv.is_some() && (!global_only || e.global) => {
                    direct = Some((e.clone(), h, (*n).to_string()));
                    break 'scan;
                }
                Some(e) if !e.cli_argv.is_some() && (!global_only || e.global) => {
                    sidecar_named = true;
                }
                _ => {}
            }
        }
    }
    let (entry, entry_home, entry_name, prepend) = match direct {
        Some((e, u, resolved)) => (e, u, resolved, None),
        None => {
            let mut claimers = Vec::new();
            for n in &names {
                claimers.extend(
                    owners_for_command(home, n, &lock, false)
                        .into_iter()
                        .map(|(h, o, e)| (h, o, e, (*n).to_string())),
                );
            }
            if let Some((u, l)) = &user_home {
                for n in &names {
                    claimers.extend(
                        owners_for_command(u, n, l, true)
                            .into_iter()
                            .map(|(h, o, e)| (h, o, e, (*n).to_string())),
                    );
                }
            }
            let (ready, dead): (Vec<_>, Vec<_>) = claimers
                .into_iter()
                .partition(|(h, o, e, _)| enabled(h, o, e));
            if let Some(note) =
                command_conflict_note(name, ready.iter().map(|(_, o, _, _)| o.as_str()))
            {
                eprintln!("{note}");
            }
            match ready.into_iter().next() {
                Some((u, owner, e, matched)) => (e, u, owner, Some(matched)),
                None if !dead.is_empty() => {
                    let (dead_home, owner, _, _) = &dead[0];
                    return Err(anyhow::anyhow!(
                        "plugin command '{owner}' is disabled — {}",
                        disabled_hint(home, dead_home, project.as_ref(), owner)
                    ));
                }
                None => {
                    return Err(anyhow::anyhow!(if sidecar_named {
                        sidecar_usage(home, name)
                    } else {
                        match index_owner_hint(name).await {
                            Some(hint) => hint,
                            None => removed_builtin(name).map(str::to_string).unwrap_or_else(
                                || match command_suggestion(home, &lock, name) {
                                    Some(s) => {
                                        format!("no plugin command '{name}' — did you mean {s}?")
                                    }
                                    None => format!(
                                        "no plugin command '{name}' — install it with: gray install plugin {name}"
                                    ),
                                },
                            ),
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
        .with_context(|| sidecar_usage(&entry_home, name))?;
    let (program, args) = argv
        .split_first()
        .context("plugin command has no entry point")?;
    anyhow::ensure!(
        !program.is_empty(),
        "plugin command has an empty executable"
    );
    let mut command = Command::new(program);
    command.args(args);
    if let Some(arg0) = prepend {
        command.arg(arg0);
    }
    command
        .args(rest)
        .env("GRAY_BIN", crate::update::exe_watch::on_disk_exe()?);
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
            installed_at: gray_core::time::now_secs().to_string(),
            scope: "user".into(),
            enabled: true,
            global: lock.plugins.get(&name).is_some_and(|e| e.global),
            granted_capabilities: granted.clone(),
            capabilities_hash,
            extra: lock
                .plugins
                .get(&name)
                .map(|e| e.extra.clone())
                .unwrap_or_default(),
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

/// Set when the session runs `--bare`: no plugin slash commands are offered
/// or run, matching bare's promise of no plugins.
static SLASH_COMMANDS_OFF: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn disable_slash_commands() {
    SLASH_COMMANDS_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn slash_commands_off() -> bool {
    SLASH_COMMANDS_OFF.load(std::sync::atomic::Ordering::Relaxed)
}

pub async fn capture_slash(name: &str, args: &[String]) -> anyhow::Result<Option<String>> {
    if slash_commands_off() {
        return Ok(None);
    }
    let home = home()?;
    migrate_commands_json(&home);
    let mut lock = load_lock(&home)?;
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| LockFile::load(&gray_plugin::lock::project_lock_path(&cwd)).ok());
    let mut user_home = user_home_lock(&home);

    // Same implicit singular/plural alias as `forward`: exact first.
    let alt = number_variant(name);
    let names: Vec<&str> = std::iter::once(name).chain(alt.as_deref()).collect();
    for n in &names {
        lazy_adopt(&home, n, &mut lock).await;
        if let Some((u, l)) = user_home.as_mut() {
            lazy_adopt(u, n, l).await;
        }
    }
    let mut conflict_note = None;
    let mut selected = None;
    for n in &names {
        if let Some(entry) = lock.plugins.get(*n).filter(|e| e.cli_argv.is_some()) {
            selected = Some((home.clone(), (*n).to_string(), entry.clone(), None));
            break;
        }
        let direct = user_home.as_ref().and_then(|(u, l)| {
            l.plugins
                .get(*n)
                .filter(|e| e.cli_argv.is_some() && e.global)
                .map(|e| (u.clone(), (*n).to_string(), e.clone(), None))
        });
        if direct.is_some() {
            selected = direct;
            break;
        }
    }
    let selected = selected.or_else(|| {
        let mut claimers = Vec::new();
        for n in &names {
            claimers.extend(
                owners_for_command(&home, n, &lock, false)
                    .into_iter()
                    .map(|(h, o, e)| (h, o, e, (*n).to_string())),
            );
        }
        if let Some((u, l)) = &user_home {
            for n in &names {
                claimers.extend(
                    owners_for_command(u, n, l, true)
                        .into_iter()
                        .map(|(h, o, e)| (h, o, e, (*n).to_string())),
                );
            }
        }
        let (ready, dead): (Vec<_>, Vec<_>) = claimers
            .into_iter()
            .partition(|(h, o, e, _)| enabled(h, o, e));
        conflict_note = command_conflict_note(name, ready.iter().map(|(_, o, _, _)| o.as_str()));
        if ready.is_empty() {
            if let Some((u, owner, e, matched)) = dead.into_iter().next() {
                return Some((u, owner, e, Some(matched)));
            }
            return None;
        }
        ready
            .into_iter()
            .next()
            .map(|(u, owner, e, matched)| (u, owner, e, Some(matched)))
    });
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
    if let Some(arg0) = prepend {
        command.arg(arg0);
    }
    command
        .args(args)
        .env("GRAY_BIN", crate::update::exe_watch::on_disk_exe()?);
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
    description: Option<String>,

    plugin_description: Option<String>,
    /// Argument completions after the command, with an optional description.
    args: Vec<(String, Option<String>)>,
}

/// One `completion` entry: a bare argument string, or
/// `{"arg": "...", "description": "..."}`.
fn completion_entry(v: &serde_json::Value) -> Option<(String, Option<String>)> {
    if let Some(arg) = v.as_str() {
        return Some((arg.to_string(), None));
    }
    let arg = v["arg"].as_str().filter(|a| !a.trim().is_empty())?;
    let desc = v["description"]
        .as_str()
        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|d| !d.is_empty());
    Some((arg.to_string(), desc))
}

/// Completion arguments a plugin publishes at runtime, beside its manifest:
/// `plugins/<name>-completion.json` holding `{"completion": [...]}` in the
/// manifest's entry format. Lets a plugin offer arguments that change after
/// install (installed items, names from the current project).
fn dynamic_completion_path(home: &Path, name: &str) -> PathBuf {
    home.join("plugins").join(format!("{name}-completion.json"))
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
        let dynamic_path = dynamic_completion_path(home, name);
        stamps.push((dynamic_path.clone(), file_mtime(&dynamic_path)));
        let dynamic = std::fs::read(&dynamic_path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
        let mut args: Vec<(String, Option<String>)> = Vec::new();
        for entry in [
            m["completion"].as_array(),
            dynamic.as_ref().and_then(|d| d["completion"].as_array()),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(completion_entry)
        {
            if own_commands.contains(entry.0.as_str()) || args.iter().any(|(a, _)| *a == entry.0) {
                continue;
            }
            args.push(entry);
        }
        for command in commands.iter().filter_map(|v| v.as_str()) {
            let command = command.trim_start_matches('/');
            if crate::repl::is_builtin_command(command) || !seen.insert(command.to_string()) {
                continue;
            }
            rows.push(CompletionRow {
                command: command.to_string(),
                description: description_in(&m, command),
                plugin_description: blurb(&m),
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
    if slash_commands_off() {
        return Vec::new();
    }
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
            out.push((
                row.command.clone(),
                row.description
                    .clone()
                    .or_else(|| row.plugin_description.clone())
                    .unwrap_or_default(),
            ));
        }
        for (arg, desc) in &row.args {
            let full = format!("{} {arg}", row.command);
            if query.contains(' ') && full.starts_with(query) {
                let desc = desc
                    .clone()
                    .or_else(|| row.plugin_description.clone())
                    .unwrap_or_default();
                out.push((full, desc));
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
    #[cfg(windows)]
    command.creation_flags(0x0000_0200);
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
mod completion_entry_tests {
    use super::completion_entry;
    use serde_json::json;

    #[test]
    fn strings_and_described_objects_parse() {
        assert_eq!(
            completion_entry(&json!("list")),
            Some(("list".into(), None))
        );
        assert_eq!(
            completion_entry(&json!({"arg": "be-ambitious", "description": "Go  big\nnow"})),
            Some(("be-ambitious".into(), Some("Go big now".into())))
        );
        assert_eq!(
            completion_entry(&json!({"arg": "x"})),
            Some(("x".into(), None))
        );
        assert_eq!(completion_entry(&json!({"description": "no arg"})), None);
        assert_eq!(completion_entry(&json!(3)), None);
    }
}
