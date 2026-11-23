use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartInfo {
    pub name: String,
    pub default: bool,
    pub enabled: bool,
    pub description: String,
}

pub fn parts_of(home: &Path, bundle: &str) -> Vec<PartInfo> {
    let Ok(manifest) = metadata(home, bundle) else {
        return Vec::new();
    };
    let overrides = gray_pkg::parts::bundle(bundle);
    manifest["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let default = p["default"].as_bool().unwrap_or(true);
            let enabled = overrides
                .get(&name)
                .copied()
                .or_else(|| p["enabled"].as_bool())
                .unwrap_or(default);
            Some(PartInfo {
                name,
                default,
                enabled,
                description: p["description"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

pub fn part_count(home: &Path, bundle: &str) -> Option<(usize, usize)> {
    let parts = parts_of(home, bundle);
    if parts.is_empty() {
        return None;
    }
    let on = parts.iter().filter(|p| p.enabled).count();
    Some((on, parts.len()))
}

fn unknown_part(bundle: &str, part: &str, parts: &[PartInfo]) -> anyhow::Error {
    if parts.is_empty() {
        return anyhow::anyhow!(
            "{bundle} has no parts in this version — run: gray plugin update {bundle}"
        );
    }
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    let mut scored: Vec<(String, f64)> = names
        .iter()
        .map(|n| (n.to_string(), strsim::jaro_winkler(part, n)))
        .filter(|(n, score)| *score >= 0.85 && n != part)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let hint = scored
        .first()
        .map(|(n, _)| format!(" — did you mean '{n}'?"))
        .unwrap_or_default();
    anyhow::anyhow!(
        "unknown part '{part}' for {bundle}{hint} (parts: {})",
        names.join(", ")
    )
}

pub fn print_parts(home: &Path, target: &str) -> anyhow::Result<()> {
    migrate_commands_json(home);
    let bundle = gray_pkg::ops::canonical_name(target);
    ensure_installed(home, bundle)?;
    let parts = parts_of(home, bundle);
    if parts.is_empty() {
        return Err(unknown_part(bundle, "", &parts));
    }
    let name_w = parts.iter().map(|p| p.name.len()).max().unwrap_or(0).max(4);
    let desc_w = parts
        .iter()
        .map(|p| p.description.chars().count())
        .max()
        .unwrap_or(0)
        .max(6);
    println!(
        "{:<name_w$}  {:<8}  {:<8}  {:<desc_w$}",
        "part", "state", "default", "description"
    );
    for p in &parts {
        println!(
            "{:<name_w$}  {:<8}  {:<8}  {:<desc_w$}",
            p.name,
            if p.enabled { "on" } else { "off" },
            if p.default { "on" } else { "off" },
            p.description
        );
    }
    Ok(())
}

fn pick_parts(bundle: &str, parts: &[PartInfo]) -> anyhow::Result<Option<BTreeMap<String, bool>>> {
    use std::io::{BufRead, Write};
    let mut on: Vec<bool> = parts.iter().map(|p| p.enabled).collect();
    println!(
        "{bundle} is made of {} parts — pick the ones to turn on:",
        parts.len()
    );
    let stdin = std::io::stdin();
    loop {
        for (i, p) in parts.iter().enumerate() {
            println!(
                "  [{}] {}  {}",
                if on[i] { "x" } else { " " },
                p.name,
                p.description
            );
        }
        print!("  numbers to toggle, e.g. 1,3 (enter = keep as shown, q = cancel): ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let text = line.trim();
        if text == "q" || text == "quit" {
            return Ok(None);
        }
        if text.is_empty() {
            break;
        }
        for token in text.split(|c: char| c == ',' || c.is_whitespace()) {
            let Ok(n) = token.parse::<usize>() else {
                continue;
            };
            if n >= 1 && n <= parts.len() {
                on[n - 1] = !on[n - 1];
            }
        }
    }
    Ok(Some(
        parts
            .iter()
            .zip(&on)
            .map(|(p, on)| (p.name.clone(), *on))
            .collect(),
    ))
}

fn write_parts(
    home: &Path,
    bundle: &str,
    parts: &[PartInfo],
    chosen: &BTreeMap<String, bool>,
) -> anyhow::Result<()> {
    gray_pkg::parts::replace(bundle, chosen)?;
    let on: Vec<&str> = parts
        .iter()
        .filter(|p| chosen.get(&p.name).copied().unwrap_or(false))
        .map(|p| p.name.as_str())
        .collect();
    println!(
        "{bundle}: {} of {} parts on ({}) — applies to new sessions",
        on.len(),
        parts.len(),
        if on.is_empty() {
            "none".to_string()
        } else {
            on.join(", ")
        }
    );
    let _ = home;
    Ok(())
}

fn resolve_part(bundle: &str, raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('/');
    let tail = match trimmed.split_once('/') {
        Some((head, rest)) if head == bundle => rest,
        _ => trimmed,
    };
    gray_pkg::ops::part_of(tail)
        .filter(|(b, _)| *b == bundle)
        .map(|(_, part)| part.to_string())
        .unwrap_or_else(|| tail.to_string())
}

fn apply_parts_after_install(
    home: &Path,
    bundle: &str,
    explicit: Option<&[String]>,
    yes: bool,
) -> anyhow::Result<()> {
    let parts = parts_of(home, bundle);
    if parts.is_empty() {
        return match explicit {
            Some(_) => Err(unknown_part(bundle, "", &parts)),
            None => Ok(()),
        };
    }
    if let Some(list) = explicit {
        let mut chosen = BTreeMap::new();
        for p in &parts {
            chosen.insert(p.name.clone(), false);
        }
        for name in list {
            let part = resolve_part(bundle, name);
            if part.is_empty() {
                continue;
            }
            match parts.iter().find(|p| p.name == part) {
                Some(p) => {
                    chosen.insert(p.name.clone(), true);
                }
                None => return Err(unknown_part(bundle, &part, &parts)),
            }
        }
        return write_parts(home, bundle, &parts, &chosen);
    }
    if yes || !interactive() || parts.len() < 2 {
        return Ok(());
    }
    match pick_parts(bundle, &parts)? {
        Some(chosen) => write_parts(home, bundle, &parts, &chosen),
        None => {
            println!("{bundle} installed with its default parts");
            Ok(())
        }
    }
}

fn toggle_part(home: &Path, bundle: &str, part: &str, on: bool) -> anyhow::Result<String> {
    ensure_installed(home, bundle)?;
    let parts = parts_of(home, bundle);
    if !parts.is_empty() && !parts.iter().any(|p| p.name == part) {
        return Err(unknown_part(bundle, part, &parts));
    }
    gray_pkg::parts::set(bundle, part, on)?;
    Ok(format!(
        "{part} is part of {bundle}: turned it {} (applies to new sessions)",
        if on { "on" } else { "off" }
    ))
}

fn multi_part_note(alias: &str, bundle: &str, installed: bool) -> String {
    let parts = gray_pkg::ops::alias_parts(alias)
        .map(|(_, p)| p.to_vec())
        .unwrap_or_default();
    let list = parts.join(", ");
    if !installed {
        return format!(
            "'{alias}' is now part of the {bundle} bundle — install it with: gray plugin install {bundle}"
        );
    }
    let hints: Vec<String> = parts
        .iter()
        .map(|p| format!("gray plugin enable {bundle}/{p}"))
        .collect();
    format!(
        "'{alias}' is now part of the {bundle} bundle, split across its parts ({list}) — toggle them with: {}",
        hints.join(", ")
    )
}

fn disabled_bundle_note(lock: &LockFile, bundle: &str, line: String) -> String {
    if lock.plugins.get(bundle).is_some_and(|e| !e.enabled) {
        return format!(
            "{line}\nnote: {bundle} itself is disabled — turn it back on with: gray plugin enable {bundle}"
        );
    }
    line
}

pub fn toggle_managed(target: &str, on: bool) -> anyhow::Result<String> {
    let home = home()?;
    migrate_commands_json(&home);
    let lock = load_lock(&home)?;
    if let Some((bundle, raw)) = target.split_once('/')
        && lock.plugins.contains_key(bundle)
    {
        let line = toggle_part(&home, bundle, &resolve_part(bundle, raw), on)?;
        return Ok(disabled_bundle_note(&lock, bundle, line));
    }
    if lock.plugins.contains_key(target) {
        gray_pkg::ops::set_enabled(target, on)?;
        return Ok(format!(
            "{} {target}",
            if on { "enabled" } else { "disabled" }
        ));
    }
    if let Some((bundle, _)) = gray_pkg::ops::alias_parts(target) {
        let installed = lock.plugins.contains_key(bundle);
        anyhow::ensure!(
            installed,
            "{}",
            not_installed_message(&home, gray_pkg::ops::canonical_name(target))
        );
        return Err(anyhow::anyhow!("{}", multi_part_note(target, bundle, true)));
    }
    if let Some((bundle, part)) = gray_pkg::ops::part_of(target) {
        let line = toggle_part(&home, bundle, part, on)?;
        return Ok(disabled_bundle_note(&lock, bundle, line));
    }
    Err(anyhow::anyhow!("{}", not_installed_message(&home, target)))
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

    Consent,
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
    install_spec_gated(home, spec, force, TrustGate::Lines, None).await
}

async fn install_spec_gated(
    home: &Path,
    spec: &str,
    force: bool,
    gate: TrustGate,
    parts: Option<Vec<String>>,
) -> anyhow::Result<Vec<String>> {
    let before: BTreeSet<String> = load_lock(home)
        .map(|l| l.plugins.into_keys().collect())
        .unwrap_or_default();
    let lines = install_spec_body(home, spec, force, gate).await?;
    let Ok(after) = load_lock(home) else {
        return Ok(lines);
    };
    let mut wanted: Vec<String> = after
        .plugins
        .keys()
        .filter(|k| !before.contains(*k))
        .cloned()
        .collect();
    if wanted.is_empty()
        && let gray_pkg::ops::NameOrUrl::Name(name) = gray_pkg::ops::parse_spec(spec)
    {
        let canonical = gray_pkg::ops::canonical_name(&name).to_string();
        if after.plugins.contains_key(&canonical) {
            wanted.push(canonical);
        }
    }
    let mut parts_failed: Option<String> = None;
    for name in wanted {
        if !after.plugins.contains_key(&name) {
            continue;
        }
        if let Err(e) = apply_parts_after_install(
            home,
            &name,
            parts.as_deref(),
            matches!(gate, TrustGate::Quiet),
        ) {
            parts_failed.get_or_insert(format!("{e:#}"));
        }
    }
    if let Some(first) = parts_failed.into_iter().next() {
        for line in &lines {
            println!("{line}");
        }
        anyhow::bail!("{first}");
    }
    Ok(lines)
}

async fn install_spec_body(
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
    let mut parsed = gray_pkg::ops::parse_spec(spec);
    let mut lines = Vec::new();
    if let gray_pkg::ops::NameOrUrl::Name(ref mut name) = parsed {
        let canonical = gray_pkg::ops::canonical_name(name);
        if canonical != name.as_str() {
            let installed = load_lock(home)
                .map(|l| l.plugins.contains_key(canonical))
                .unwrap_or(false);
            if installed && let Some((bundle, part)) = gray_pkg::ops::part_of(name) {
                lines.push(toggle_part(home, bundle, part, true)?);
                lines.push(format!("{bundle} is already installed — no reinstall"));
                return Ok(lines);
            }
            if installed && let Some((bundle, _)) = gray_pkg::ops::alias_parts(name) {
                lines.push(multi_part_note(name, bundle, true));
                lines.push(format!("{bundle} is already installed — no reinstall"));
                return Ok(lines);
            }
            lines.push(format!(
                "note: '{name}' is now part of '{canonical}' — installing {canonical}"
            ));
            *name = canonical.to_string();
        }
    }

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
            TrustGate::Consent => {
                let reply = crate::ask::handle_ask(serde_json::json!({
                    "questions": [{
                        "id": "install",
                        "header": "Plugin",
                        "question": format!("A plugin wants to install '{name}', an unverified community plugin. Install it?"),
                        "options": [
                            {"label": "Install", "description": "runs third-party code with your permissions"},
                            {"label": "Cancel", "description": "leave it uninstalled"}
                        ],
                        "no_other": true
                    }]
                }))
                .await;
                let granted = reply["answers"]["install"]["answers"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("Install")));
                anyhow::ensure!(
                    granted,
                    "install cancelled: '{name}' is an unverified community plugin"
                );
            }
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
        if let Err(e) = adopt_cli(home, dep, &home.join("plugins").join(dep), None).await {
            lines.push(format!(
                "warning: could not set up `gray {dep}` forwarding: {e:#}"
            ));
        }
    }

    match adopt_cli(home, &r.name, &r.path, r.description.as_deref()).await {
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

pub(crate) async fn install_spec_default(home: &Path, spec: &str) -> anyhow::Result<()> {
    install_spec_gated(home, spec, false, TrustGate::Lines, None)
        .await
        .map(|_| ())
}

pub(crate) async fn install_spec_consent(home: &Path, spec: &str) -> anyhow::Result<Vec<String>> {
    install_spec_gated(home, spec, false, TrustGate::Consent, None).await
}

pub async fn install_spec(
    home: &Path,
    spec: &str,
    force: bool,
    yes: bool,
    parts: Option<Vec<String>>,
) -> anyhow::Result<()> {
    let gate = if yes {
        TrustGate::Quiet
    } else {
        TrustGate::Ask
    };
    for line in install_spec_gated(home, spec, force, gate, parts).await? {
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
        install_spec(home, name, false, false, None).await?;
        return Ok(());
    }
    if index_entry(name).await.is_some() {
        install_spec(home, name, false, false, None).await?;
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
    parts: Option<Vec<String>>,
) -> anyhow::Result<()> {
    let mut failed = Vec::new();
    for spec in specs {
        if let Err(e) = install_spec(home, spec, force, yes, parts.clone()).await {
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
    ensure_installed(&home, name)?;
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
    if crate::default_plugins::read_profile(&home) != "bare"
        || crate::default_plugins::FALLBACK_DEFAULT_PROFILE
            .iter()
            .any(|d| *d == name || gray_pkg::ops::canonical_name(d) == name)
    {
        let lock_file = gray_plugin::lock::lock_path(&home);
        let mut lock = load_lock(&home)?;
        lock.tombstone(name);
        lock.save(&lock_file)?;
    }
    let _ = std::fs::remove_file(home.join("plugins").join(format!("{name}-manifest.json")));
    let _ = std::fs::remove_file(home.join("plugins").join(format!("{name}-completion.json")));
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
    ensure_installed(&home, name)?;
    gray_pkg::ops::set_enabled(name, on)
}

pub fn global_target(name: &str) -> String {
    match home() {
        Ok(home) => match load_lock(&home) {
            Ok(lock) if lock.plugins.contains_key(name) => name.to_string(),
            _ => gray_pkg::ops::canonical_name(name).to_string(),
        },
        Err(_) => name.to_string(),
    }
}

pub fn set_managed_global(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    migrate_commands_json(&home);
    ensure_installed(&home, name)?;
    gray_pkg::ops::set_global(name, on)
}

fn resolve_update_target<'a>(home: &Path, target: &'a str) -> &'a str {
    if target == "all" {
        return target;
    }
    let Ok(lock) = load_lock(home) else {
        return target;
    };
    if lock.plugins.contains_key(target) {
        return target;
    }
    gray_pkg::ops::RENAMED
        .iter()
        .find(|(old, new)| *new == target && lock.plugins.contains_key(*old))
        .map_or(target, |(old, _)| *old)
}

pub async fn update_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::UpdateRow>> {
    let home = home()?;
    let target = resolve_update_target(&home, target);
    ensure_installed(&home, target)?;
    let mut reports = gray_pkg::ops::update(target).await?;
    for r in &mut reports {
        if !matches!(&r.action, gray_pkg::ops::UpdateAction::Installed(_)) {
            continue;
        }
        let Some(path) = r.path.clone() else {
            continue;
        };
        if let Err(e) = adopt_cli(&home, &r.name, &path, r.description.as_deref()).await {
            eprintln!(
                "warning: could not set up `gray {}` forwarding: {e:#}",
                r.name
            );
            r.action = gray_pkg::ops::UpdateAction::Failed(format!("not adopted: {e:#}"));
        }
    }
    if let Err(e) = crate::providers::ProviderRegistry::refresh(&home) {
        log::warn!("could not refresh providers after update: {e:#}");
    }
    Ok(reports)
}

pub async fn check_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::UpdateRow>> {
    let home = home()?;
    let target = resolve_update_target(&home, target);
    ensure_installed(&home, target)?;
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

fn not_installed_message(home: &Path, name: &str) -> String {
    let names: Vec<String> = load_lock(home)
        .map(|l| l.plugins.keys().cloned().collect())
        .unwrap_or_default();
    if names.is_empty() {
        return format!("not installed: {name} (no plugins installed)");
    }
    let mut scored: Vec<(String, f64)> = names
        .iter()
        .map(|n| (n.clone(), strsim::jaro_winkler(name, n)))
        .filter(|(n, s)| *s >= 0.87 && n != name)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let near: Vec<String> = scored
        .into_iter()
        .take(3)
        .map(|(n, _)| format!("'{n}'"))
        .collect();
    if near.is_empty() {
        format!("not installed: {name} (installed: {})", names.join(", "))
    } else {
        format!("not installed: {name} — did you mean {}?", near.join(", "))
    }
}

fn ensure_installed(home: &Path, name: &str) -> anyhow::Result<()> {
    if name == "all" {
        return Ok(());
    }
    let lock = load_lock(home)?;
    anyhow::ensure!(
        lock.plugins.contains_key(name),
        "{}",
        not_installed_message(home, name)
    );
    Ok(())
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
        && adopt_cli(home, name, &home.join("plugins").join(name), None)
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

fn alias_cli_target(
    home: &Path,
    lock: &LockFile,
    user_home: Option<&(PathBuf, LockFile)>,
    name: &str,
) -> Option<(PathBuf, String, LockEntry, String)> {
    if lock.plugins.contains_key(name)
        || user_home.is_some_and(|(_, l)| l.plugins.contains_key(name))
    {
        return None;
    }
    let (bundle, part) = gray_pkg::ops::part_of(name)?;
    let (bundle_home, entry) = match lock.plugins.get(bundle) {
        Some(entry) => (home.to_path_buf(), entry.clone()),
        None => {
            let (u, l) = user_home?;
            (u.clone(), l.plugins.get(bundle)?.clone())
        }
    };
    entry.cli_argv.as_ref()?;
    let parts = parts_of(&bundle_home, bundle);
    if !parts.is_empty() && !parts.iter().any(|p| p.name == part) {
        return None;
    }
    Some((bundle_home, bundle.to_string(), entry, part.to_string()))
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
                None if let Some((h, owner, e, part)) =
                    alias_cli_target(home, &lock, user_home.as_ref(), name) =>
                {
                    (e, h, owner, Some(part))
                }
                None => {
                    return Err(anyhow::anyhow!(if sidecar_named {
                        sidecar_usage(home, name)
                    } else {
                        if let Some((bundle, _)) = gray_pkg::ops::alias_parts(name) {
                            let installed = lock.plugins.contains_key(bundle)
                                || user_home
                                    .as_ref()
                                    .is_some_and(|(_, l)| l.plugins.contains_key(bundle));
                            return Err(anyhow::anyhow!(
                                "{}",
                                multi_part_note(name, bundle, installed)
                            ));
                        }
                        if let Some((bundle, _)) = gray_pkg::ops::part_of(name) {
                            return Err(anyhow::anyhow!(
                                "'{name}' is now a part of the {bundle} bundle — install it with: gray plugin install {bundle}"
                            ));
                        }
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
        .env("GRAY_BIN", crate::update::exe_watch::on_disk_exe()?)
        .env("GRAY_HOME", gray_core::paths::gray_home_or_temp());
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

async fn cli_manifest(binary: &Path) -> anyhow::Result<Option<Value>> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("manifest")
        .env("GRAY_HOME", gray_core::paths::gray_home_or_temp());
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .context("plugin manifest probe timed out")??;
    if !output.status.success() {
        return Ok(None);
    }
    match serde_json::from_slice::<Value>(&output.stdout)
        .ok()
        .filter(|m| m["name"].is_string())
    {
        Some(manifest) => Ok(Some(manifest)),
        None => anyhow::bail!("`{} manifest` printed no plugin manifest", binary.display()),
    }
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
    let cli_probe = cli_manifest(&binary).await.ok().flatten();
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
            removed: false,
            granted_capabilities: granted.clone(),
            capabilities_hash,
            extra: lock
                .plugins
                .get(&name)
                .map(|e| e.extra.clone())
                .unwrap_or_default(),
        },
    );

    gray_plugin::lazy::persist_manifest(
        &gray_plugin::lazy::manifest_path(&home.join("plugins"), &name),
        &manifest,
    )?;
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

pub(crate) async fn adopt_cli(
    home: &Path,
    name: &str,
    dir: &Path,
    description: Option<&str>,
) -> anyhow::Result<bool> {
    validate_name(name)?;
    let exe = gray_plugin::builder::resolve_argv(dir)
        .ok()
        .and_then(|argv| argv.into_iter().next());
    let manifest = match exe.as_deref() {
        Some(exe) => cli_manifest(Path::new(exe)).await?,
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
        Some(mut manifest) => {
            let exe = exe.expect("a manifest implies an executable");
            if blurb(&manifest).is_none()
                && let Some(description) = description
            {
                manifest["description"] = Value::String(description.to_string());
            }

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

            gray_plugin::lazy::persist_manifest(
                &gray_plugin::lazy::manifest_path(&plugins, name),
                &manifest,
            )?;
            lock.save(&lock_path)?;

            crate::providers::ProviderRegistry::refresh(home)?;
            Ok(true)
        }
        None => {
            if !entry.hash.is_empty() && entry.cli_argv.take().is_some() {
                lock.save(&lock_path)?;
                crate::providers::ProviderRegistry::refresh(home)?;
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
/// `plugins/<name>-completion.json` holding `{"completion": [...]}` and/or
/// `{"command_completions": {"/<cmd>": [...]}}` in the manifest's entry format.
/// Lets a single- or multi-command plugin offer arguments that change after
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
            let cmd_key = format!("/{command}");
            let cmd_args = dynamic
                .as_ref()
                .and_then(|d| d["command_completions"][&cmd_key].as_array())
                .or_else(|| m["command_completions"][&cmd_key].as_array())
                .map(|entries| entries.iter().filter_map(completion_entry).collect())
                .unwrap_or_else(|| args.clone());
            rows.push(CompletionRow {
                command: command.to_string(),
                description: description_in(&m, command),
                plugin_description: blurb(&m),
                args: cmd_args,
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
    if !query.contains(' ') {
        for (_, name, desc) in gray_plugin::dynamic_commands() {
            if name.starts_with(query)
                && !crate::repl::is_builtin_command(&name)
                && !table.rows.iter().any(|r| r.command == name)
                && !out.iter().any(|(n, _): &(String, String)| *n == name)
            {
                out.push((name, desc));
            }
        }
    }
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
        let query_lower = query.to_lowercase();
        for (arg, desc) in &row.args {
            let full = format!("{} {arg}", row.command);
            if query.contains(' ') && full.to_lowercase().starts_with(&query_lower) {
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

pub(crate) struct PluginOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: std::process::ExitStatus,
}

impl PluginOutput {
    pub(crate) fn failure(&self) -> String {
        fn tail(bytes: &[u8]) -> String {
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim_end();
            let mut start = text.len().saturating_sub(500);
            while start < text.len() && !text.is_char_boundary(start) {
                start += 1;
            }
            text.get(start..).unwrap_or(text).to_string()
        }
        let stderr = tail(&self.stderr);
        let stdout = tail(&self.stdout);
        let said = match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{stderr} (stdout: {stdout})"),
            (false, true) => stderr,
            (true, false) => stdout,
            (true, true) => return format!("plugin exited {}", self.status),
        };
        format!("plugin exited {} — last output: {said}", self.status)
    }
}

const STDERR_TAIL: usize = 4096;

async fn read_tail<R>(reader: &mut R, keep: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 8192];
    let mut tail: Vec<u8> = Vec::new();
    loop {
        let n = reader.read(&mut buf[..]).await?;
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > keep * 2 {
            tail.drain(..tail.len() - keep);
        }
    }
    if tail.len() > keep {
        tail.drain(..tail.len() - keep);
    }
    Ok(tail)
}

pub(crate) async fn capture_output(
    mut command: tokio::process::Command,
    ttl: std::time::Duration,
) -> anyhow::Result<PluginOutput> {
    use tokio::io::AsyncReadExt;
    #[cfg(windows)]
    command.creation_flags(0x0000_0200);
    let mut child = command
        .env("GRAY_HOME", gray_core::paths::gray_home_or_temp())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().context("missing stdout")?.take(65537);
    let mut stderr = child.stderr.take().context("missing stderr")?;
    let result = tokio::time::timeout(ttl, async {
        let mut bytes = Vec::new();
        let (out, err) = tokio::join!(
            stdout.read_to_end(&mut bytes),
            read_tail(&mut stderr, STDERR_TAIL)
        );
        out?;
        let errors = err?;
        anyhow::ensure!(bytes.len() <= 65536, "plugin output exceeds 64 KiB");
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>(PluginOutput {
            stdout: bytes,
            stderr: errors,
            status,
        })
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
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

pub(crate) async fn capture(
    command: tokio::process::Command,
    ttl: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    let output = capture_output(command, ttl).await?;
    anyhow::ensure!(output.status.success(), "{}", output.failure());
    Ok(output.stdout)
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

#[cfg(test)]
mod dynamic_completion_tests {
    use super::*;
    use gray_plugin::lock::LockEntry;

    #[test]
    fn dynamic_command_completions_override_only_their_command() {
        let dir = std::env::temp_dir().join(format!("gray-dyn-comp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins_dir = dir.join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        let mut lock = LockFile::default();
        lock.plugins.insert(
            "extras".into(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "0.4.0".into(),
                scope: "user".into(),
                argv: vec!["/bin/true".into()],
                cli_argv: Some(vec!["/bin/true".into()]),
                ..LockEntry::default()
            },
        );
        std::fs::write(
            plugins_dir.join("extras-manifest.json"),
            serde_json::json!({
                "name": "extras",
                "commands": ["/export", "/skills"],
                "command_completions": {
                    "/export": ["clip", "md"],
                    "/skills": [{"arg": "list", "description": "list installed skills"}]
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            plugins_dir.join("extras-completion.json"),
            serde_json::json!({
                "command_completions": {
                    "/skills": [
                        {"arg": "list", "description": "list installed skills"},
                        {"arg": "my-skill", "description": "project skill"}
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut stamps = Vec::new();
        let mut rows = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_lock_completions(&dir, &lock, None, false, &mut stamps, &mut rows, &mut seen);

        let export = rows.iter().find(|r| r.command == "export").unwrap();
        let export_args: Vec<&str> = export.args.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(export_args, vec!["clip", "md"]);

        let skills = rows.iter().find(|r| r.command == "skills").unwrap();
        let skills_args: Vec<&str> = skills.args.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(skills_args, vec!["list", "my-skill"]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
