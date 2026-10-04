//! Native plugin command registration, help metadata and bounded UI requests. No model calls.
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::setup::registry::{FieldKind, SetupDecl, SetupField};
use anyhow::Context;
use gray_plugin::Plugin;
use gray_plugin::lock::{LockEntry, LockFile};
use serde_json::Value;

/// One first-party plugin. `source` is a `git+<url>@<commit>` pin; the crate
/// is built with `cargo build --release --locked` and the resulting binary is
/// published under `<home>/plugins/<name>/` and spawned with `sidecar_args`.
#[derive(Debug)]
struct Catalog {
    name: &'static str,
    source: &'static str,
    /// The crate's `[[bin]]` name, as produced under `target/release/`.
    bin: &'static str,
    /// Arguments that make the binary serve the sidecar wire.
    sidecar_args: &'static [&'static str],
    /// What setup needs from the user, and how gray proves it worked.
    setup: &'static SetupDecl,
}

/// The Discord app's setup declaration: one secret from the Developer
/// Portal, one destination (pickable), one owner ID; paths are derived;
/// no budget, no quiz.
pub static DISCORD_SETUP: SetupDecl = SetupDecl {
    config_path: ".config/gray-discord/config.json",
    fields: &[
        SetupField {
            key: "token",
            kind: FieldKind::Required,
            description: "Discord bot token — Developer Portal, your app, Bot, Reset Token",
            url: Some("https://discord.com/developers/applications"),
            secret: true,
            picker: None,
        },
        SetupField {
            key: "owner_id",
            // Not Required: the app runs ownerless and admits its first human
            // through the Discord-side pairing reply (their own ID, told to
            // them by the bot), so nobody has to hunt a snowflake.
            kind: FieldKind::Optional,
            description: "Your Discord user ID (Developer Mode, Copy User ID) — gates who can trigger the bot",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "channel_id",
            kind: FieldKind::Required,
            description: "Where the bot posts — pick a channel or DM below, or paste an ID",
            url: None,
            secret: false,
            picker: Some(crate::setup::registry::PICKER_CHANNELS),
        },
        SetupField {
            key: "allowed_users",
            kind: FieldKind::Optional,
            description: "Extra user IDs allowed to trigger the bot (comma-separated)",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "gray_bin",
            kind: FieldKind::Derived,
            description: "gray binary",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "gray_home",
            kind: FieldKind::Derived,
            description: "gray home",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "workdir",
            kind: FieldKind::Derived,
            description: "working directory",
            url: None,
            secret: false,
            picker: None,
        },
    ],
    verify: &["gray-discord", "doctor"],
    post_steps: &["register", "start"],
    service: Some(&["gray-discord", "run"]),
};

/// First-party plugins, pinned by commit. Every entry is a Rust crate built
/// locally: gray itself ships no Python, and its own plugins must not
/// reintroduce one. User-written plugins may still be any executable — see
/// `GRAY_PLUGIN_PATH` in [`install`] and [`gray_plugin::builder::resolve_argv`].
const CATALOG: &[Catalog] = &[Catalog {
    name: "discord",
    source: "git+https://github.com/vstaln/gray-discord-plugin.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f",
    bin: "gray-discord",
    sidecar_args: &["sidecar"],
    setup: &DISCORD_SETUP,
}];

/// The app's setup declaration, when gray ships one.
pub fn setup_decl(name: &str) -> Option<&'static SetupDecl> {
    catalog(name).ok().map(|entry| entry.setup)
}

fn catalog(name: &str) -> anyhow::Result<&'static Catalog> {
    CATALOG
        .iter()
        .find(|entry| entry.name == name)
        .with_context(|| format!("Unknown plugin '{name}'. Available: background, discord"))
}

/// Split a catalog source into its clone URL and pinned commit. Sources are
/// always `git+<url>@<commit>`; the pin must be a full commit ID so an
/// install reproduces exactly the code that was reviewed.
fn parse_git_source(source: &str) -> anyhow::Result<(&str, &str)> {
    const SHAPE: &str = "plugin source must be git+<url>@<40-hex commit>";
    let rest = source
        .strip_prefix("git+")
        .with_context(|| format!("{SHAPE}, got {source}"))?;
    // rsplit: an ssh URL may itself contain '@' (user@host).
    let (url, pin) = rest
        .rsplit_once('@')
        .with_context(|| format!("{SHAPE}, got {source}"))?;
    anyhow::ensure!(
        pin.len() == 40 && pin.bytes().all(|b| b.is_ascii_hexdigit()),
        "{SHAPE}, got pin {pin}"
    );
    anyhow::ensure!(
        url.starts_with("https://") || url.starts_with("ssh://") || url.starts_with("git@"),
        "plugin source URL must be https or ssh, got {url}"
    );
    Ok((url, pin))
}

pub fn home() -> anyhow::Result<PathBuf> {
    Ok(crate::sys_prompt_path()?
        .parent()
        .context("cannot resolve gray home")?
        .to_path_buf())
}

fn registry_path(home: &Path) -> PathBuf {
    home.join("plugins/commands.json")
}

fn load(home: &Path) -> anyhow::Result<LockFile> {
    let registry = LockFile::load(&registry_path(home))?;
    anyhow::ensure!(
        registry.schema == 1,
        "unsupported plugin command registry schema"
    );
    Ok(registry)
}

/// How long a registry write waits for a competing plugin-manager operation
/// (the same discipline as the gray-pkg and session-store locks).
const COMMANDS_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Exclusive cross-process guard for registry writes: held for the caller's
/// whole read-modify-write via the returned handle. Advisory only (a crashed
/// holder releases on close). A *contended* lock is retried until
/// [`COMMANDS_LOCK_TIMEOUT`] and then fails the operation — the previous
/// code dropped the `try_lock` error, so two managers could edit
/// `commands.json` concurrently and one silently won. A filesystem without
/// flock still degrades to no guard rather than breaking the op.
fn hold_commands_lock_timeout(
    home: &Path,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<std::fs::File>> {
    let path = home.join("plugins/.commands.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(Some(f)),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "another plugin operation is modifying the registry ({}); try again",
                        path.display()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "plugin registry locking unsupported on {} ({e}); proceeding unlocked",
                    path.display()
                );
                return Ok(None);
            }
        }
    }
}

fn hold_commands_lock(home: &Path) -> anyhow::Result<Option<std::fs::File>> {
    hold_commands_lock_timeout(home, COMMANDS_LOCK_TIMEOUT)
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

fn metadata(home: &Path, name: &str) -> anyhow::Result<serde_json::Value> {
    validate_name(name)?;
    Ok(serde_json::from_slice(&std::fs::read(
        home.join("plugins").join(format!("{name}-manifest.json")),
    )?)?)
}

/// Install a first-party plugin from the pinned catalog. `background` keeps
/// its own prebuilt-binary path; every other entry is a Rust crate built from
/// source. A file lock serializes registry updates; temp dirs roll back
/// failed installs.
async fn install_catalog(home: &Path, name: &str, force: bool) -> anyhow::Result<()> {
    validate_name(name)?;
    if name == "background" {
        return native::install(home).await;
    }
    install_cargo(home, catalog(name)?, force).await
}

/// Build a Rust plugin from its pinned source and register it as a sidecar.
/// No interpreter is involved at any step: `cargo build --release` produces
/// the binary gray spawns, so a first-party plugin install needs neither
/// Python nor a package index.
async fn install_cargo(home: &Path, entry: &Catalog, force: bool) -> anyhow::Result<()> {
    let name = entry.name;
    let (url, pin) = parse_git_source(entry.source)?;
    let root = home.join("plugins");
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".commands.lock"))?;
    lock.try_lock()
        .context("another plugin installation is running")?;
    // Sidecars live in lock.json (commands.json holds CLI commands), so the
    // already-installed check must read the registry this kind publishes to.
    let sidecars = LockFile::load(&gray_plugin::lock::lock_path(home))?;
    anyhow::ensure!(sidecars.schema == 1, "unsupported plugin lock schema");
    if sidecars.plugins.contains_key(name) {
        println!("Plugin '{name}' is already registered. Run: gray {name} --help");
        return Ok(());
    }
    println!(
        "Installing '{name}' from {} (compiling from source; plugin code runs with your user permissions).",
        entry.source
    );
    let (_repo, repo_dir) = gray_pkg::sources::clone_into_tmp(url, &[], false)?;
    gray_pkg::sources::checkout_pinned_commit(&repo_dir, name, pin)?;
    // Scan the exact source this pin resolves to, before it is compiled:
    // building unvetted code is a bigger commitment than reading it.
    scan_or_block(&repo_dir, force)?;
    let built = build_plugin(&repo_dir, entry.bin)?;
    // Prove the artifact speaks the sidecar wire while it is still inside the
    // build tempdir: a plugin that fails this check leaves nothing behind.
    let probe = sidecar_argv(&built, entry.sidecar_args);
    let plugin = gray_plugin::SidecarPlugin::spawn(probe).await?;
    let manifest = plugin.manifest();
    plugin.shutdown(std::time::Duration::from_secs(2)).await;
    anyhow::ensure!(
        manifest.name == name,
        "built plugin reports name '{}', expected '{name}'",
        manifest.name
    );
    let (granted, capabilities_hash) = consent_capabilities(name, &manifest.capabilities);
    // Publish under <home>/plugins/<name>/ so the recorded argv stays valid
    // once the build tempdir is dropped.
    let dest = root.join(name);
    std::fs::create_dir_all(&dest)?;
    let executable = dest.join(entry.bin);
    std::fs::copy(&built, &executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    }
    let argv = sidecar_argv(&executable, entry.sidecar_args);
    let runtime_role = provider_only_role(&manifest);
    let mut sidecars = LockFile::load(&gray_plugin::lock::lock_path(home))?;
    sidecars.plugins.insert(
        name.into(),
        LockEntry {
            runtime_role,
            ecosystem: "gray-native".into(),
            version: manifest.version.clone(),
            hash: String::new(),
            source: entry.source.to_string(),
            argv,
            adapter_version: manifest.protocol.clone().unwrap_or_else(|| "1".into()),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
            granted_capabilities: granted.clone(),
            capabilities_hash,
        },
    );
    sidecars.save(&gray_plugin::lock::lock_path(home))?;
    // Same cache refresh as native registration: a catalog plugin that
    // declares providers is usable in `/connect` without a manual edit.
    crate::providers::ProviderRegistry::refresh(home)?;
    // Cache the manifest beside the install: same file `register_native`
    // writes, so `plugin capabilities` and command lookup work for
    // catalog plugins too.
    let mut cached = tempfile::NamedTempFile::new_in(&root)?;
    use std::io::Write as _;
    writeln!(cached, "{}", serde_json::to_string_pretty(&manifest)?)?;
    cached.persist(root.join(format!("{name}-manifest.json")))?;
    println!("Installed '{name}'. Next: gray {name} setup");
    Ok(())
}

/// Spawn argv for a built plugin: the binary plus its sidecar arguments.
fn sidecar_argv(executable: &Path, sidecar_args: &[&str]) -> Vec<String> {
    let mut argv = vec![executable.to_string_lossy().into_owned()];
    argv.extend(sidecar_args.iter().map(|arg| (*arg).to_string()));
    argv
}

/// `cargo build --release --locked` inside the checked-out source. The
/// lockfile must be committed: an unpinned dependency tree is not the build
/// that was reviewed. Returns the built binary.
fn build_plugin(repo_dir: &Path, bin: &str) -> anyhow::Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // Pin the target dir inside the checkout: an inherited CARGO_TARGET_DIR
    // would otherwise build the artifact somewhere else and leave the check
    // below looking in the wrong place.
    let target = repo_dir.join("target");
    let status = Command::new(&cargo)
        .args(["build", "--release", "--locked"])
        .arg("--target-dir")
        .arg(&target)
        .current_dir(repo_dir)
        .status()
        .context("could not start cargo; install Rust or point CARGO at it")?;
    anyhow::ensure!(
        status.success(),
        "cargo build failed ({status}); see output above"
    );
    let built = target
        .join("release")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
    anyhow::ensure!(
        built.is_file(),
        "cargo build did not produce {}",
        built.display()
    );
    Ok(built)
}

/// Register a separately built native plugin. No hardcoded plugin catalog.
/// Download/version resolution remains the existing `gray plugin install` API.
pub async fn install(home: &Path, name: &str, force: bool) -> anyhow::Result<()> {
    validate_name(name)?;
    if let Some(path) = std::env::var_os("GRAY_PLUGIN_PATH") {
        return register_native(home, name, Path::new(&path), force).await;
    }
    if matches!(name, "background" | "discord") {
        return install_catalog(home, name, force).await;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let path = dir.join(format!("gray-{name}{}", std::env::consts::EXE_SUFFIX));
            if path.is_file() {
                return register_native(home, name, &path, force).await;
            }
        }
    }
    anyhow::bail!(
        "Unknown plugin '{name}'. Catalog: background, discord. For a local native plugin, put gray-{name} on PATH or set GRAY_PLUGIN_PATH to its executable"
    )
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

/// Runtime role for a protocol-1.2 sidecar that owns no other surface.
/// It stays in `lock.json` for provider discovery but `active_plugins`
/// skips it, so the provider runtime is the only process it spawns.
fn provider_only_role(manifest: &gray_plugin::Manifest) -> Option<String> {
    let provider_only = manifest.protocol.as_deref() == Some("1.2")
        && !manifest.providers.is_empty()
        && manifest.tools.is_empty()
        && manifest.commands.is_empty()
        && manifest.hooks.is_empty();
    provider_only.then(|| "provider_only".to_string())
}

/// Same rule for a JSON manifest (native registration reads JSON before
/// a typed sidecar manifest is available).
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
    let sidecars = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(&home))?;
    let rows = list_managed(&home)?;
    let mut shown = 0usize;
    for row in &rows {
        if let Some(want) = only
            && row.name != want
        {
            continue;
        }
        shown += 1;
        let entry = sidecars.plugins.get(&row.name);
        let declared = entry
            .and_then(|_| metadata(&home, &row.name).ok())
            .and_then(|m| {
                m["capabilities"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let (granted, hash) = match entry {
            Some(e) => (
                gray_plugin::capabilities::granted_for(e, &declared),
                e.capabilities_hash.clone(),
            ),
            None => (Default::default(), None),
        };
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
                let drift = gray_plugin::capabilities::needs_reconsent(
                    entry.expect("hashed entry"),
                    &declared,
                );
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

/// One installed plugin in the merged manager view (`lock.json` sidecars +
/// `commands.json` native/CLI commands). `cli` is true for `commands.json`
/// entries; when both registries hold the same name the CLI entry wins at
/// runtime (see [`forward`]) so it shadows the sidecar row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPlugin {
    pub name: String,
    pub entry: LockEntry,
    pub cli: bool,
}

/// Merged manager view over both registries, sorted by name: every
/// `commands.json` entry (native/CLI commands like `discord`) plus every
/// `lock.json` sidecar not shadowed by the same name. Missing files read
/// as empty; a corrupt file is an error (callers warn the same way boot
/// does for the sidecar lock).
pub fn list_managed(home: &Path) -> anyhow::Result<Vec<ManagedPlugin>> {
    use std::collections::BTreeMap;
    let mut merged: BTreeMap<String, ManagedPlugin> = BTreeMap::new();
    for (name, entry) in load(home)?.plugins {
        merged.insert(
            name.clone(),
            ManagedPlugin {
                name,
                entry,
                cli: true,
            },
        );
    }
    for (name, entry) in
        gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?.plugins
    {
        merged.entry(name.clone()).or_insert(ManagedPlugin {
            name,
            entry,
            cli: false,
        });
    }
    Ok(merged.into_values().collect())
}

/// One display row of the merged manager view, pre-resolved: `on` already
/// folds the sidecar enable overlay (see [`enabled`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRow {
    pub name: String,
    pub version: String,
    pub scope: String,
    pub ecosystem: String,
    pub on: bool,
    pub cli: bool,
}

/// Merged display rows (sidecars + CLI commands, sorted by name) for
/// `plugin list`, `/plugin list`, and the manager picker. When the home dir
/// is unresolvable, falls back to the sidecar-only listing instead of
/// failing (the previous `plugin list` never failed here — `gray-pkg` falls
/// back to a relative `.gray` dir).
pub fn list_rows() -> anyhow::Result<Vec<ManagedRow>> {
    let Ok(home) = home() else {
        return Ok(gray_pkg::ops::list()?
            .into_iter()
            .map(|(name, e)| ManagedRow {
                name,
                version: e.version,
                scope: e.scope,
                ecosystem: e.ecosystem,
                on: e.enabled,
                cli: false,
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

/// Flip a `commands.json` entry's `enabled` flag (native/CLI commands only;
/// sidecars stay on `gray-pkg::ops`). Miss message matches `ops::remove`.
pub fn set_command_enabled(home: &Path, name: &str, on: bool) -> anyhow::Result<()> {
    validate_name(name)?;
    let _guard = hold_commands_lock(home)?;
    let mut registry = load(home)?;
    let Some(entry) = registry.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    entry.enabled = on;
    registry.save(&registry_path(home))?;
    Ok(())
}

/// Remove one `commands.json` entry (native/CLI commands only; sidecars stay
/// on `gray-pkg::ops`). Also drops its `<name>-manifest.json` so completion
/// and help stop advertising it, and clears a widget slot it owned.
/// Miss message matches `ops::remove`.
pub fn remove_command(home: &Path, name: &str) -> anyhow::Result<()> {
    validate_name(name)?;
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("not installed: {name}");
    }
    let _guard = hold_commands_lock(home)?;
    let mut registry = load(home)?;
    // `register_native` mirrors the entry into `lock.json` as a zero-tool
    // sidecar: drop the mirror too, or its ghost row outlives the remove.
    // Only a same-argv row is the mirror — an independent same-named
    // sidecar (different argv) survives.
    let Some(dropped) = registry.plugins.remove(name) else {
        anyhow::bail!("not installed: {name}");
    };
    registry.save(&registry_path(home))?;
    if let Ok(mut sidecars) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))
        && sidecars
            .plugins
            .get(name)
            .is_some_and(|s| s.argv == dropped.argv)
    {
        sidecars.plugins.remove(name);
        let _ = sidecars.save(&gray_plugin::lock::lock_path(home));
    }
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

/// Is `name` a `commands.json` native/CLI command (missing registry: no)?
pub fn is_command(home: &Path, name: &str) -> bool {
    load(home)
        .map(|r| r.plugins.contains_key(name))
        .unwrap_or(false)
}

/// Remove from whichever registry owns `name` (`commands.json` CLI entries
/// shadow sidecars at runtime, so they win ties). Both misses read
/// "not installed: {name}", matching `ops::remove`.
pub fn remove_managed(name: &str) -> anyhow::Result<()> {
    let home = home()?;
    if is_command(&home, name) {
        remove_command(&home, name)
    } else {
        gray_pkg::ops::remove(name)
    }
}

/// Enable/disable in whichever registry owns `name`, same routing as
/// [`remove_managed`].
pub fn set_managed_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    let home = home()?;
    if is_command(&home, name) {
        set_command_enabled(&home, name, on)
    } else {
        gray_pkg::ops::set_enabled(name, on)
    }
}

/// `update` only knows sidecar sources: a `commands.json` entry would fail
/// as "not installed", so skip it with the same warning shape `ops` uses
/// for non-index rows and report nothing changed.
pub async fn update_managed(target: &str) -> anyhow::Result<Vec<gray_pkg::ops::Report>> {
    let home = home()?;
    if target != "all" && is_command(&home, target) {
        eprintln!("warning: skipping update of {target} (non-index source)");
        return Ok(Vec::new());
    }
    gray_pkg::ops::update(target).await
}

/// Resolve only explicitly registered commands (never arbitrary PATH executables).
/// exec preserves terminal, signals, argument boundaries, and the child's exit code.
pub fn forward(home: &Path, name: &str, rest: &[String]) -> anyhow::Result<()> {
    validate_name(name)?;
    let registry = load(home)?;
    let entry = registry.plugins.get(name).with_context(|| {
        format!("no plugin command '{name}' — install it with: gray install plugin {name}")
    })?;
    anyhow::ensure!(
        enabled(home, name, entry),
        "plugin command '{name}' is disabled"
    );
    let (program, args) = entry
        .argv
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

/// The registered argv (binary plus its own args) for an installed command
/// plugin. The setup flow runs the app's subcommands (`doctor`, `register`)
/// through this instead of re-deriving a path.
pub fn command_argv(home: &Path, name: &str) -> anyhow::Result<Vec<String>> {
    validate_name(name)?;
    let entry = load(home)?
        .plugins
        .get(name)
        .with_context(|| {
            format!(
                "no plugin command '{name}' \u{2014} install it with: gray install plugin {name}"
            )
        })?
        .clone();
    anyhow::ensure!(
        enabled(home, name, &entry),
        "plugin command '{name}' is disabled"
    );
    anyhow::ensure!(!entry.argv.is_empty(), "plugin command has no entry point");
    Ok(entry.argv)
}

/// Register a user-selected native executable; registration runs its manifest.
/// The executable and widgets remain owned by the separate plugin repository.
pub async fn register_native(
    home: &Path,
    name: &str,
    binary: &Path,
    force: bool,
) -> anyhow::Result<()> {
    validate_name(name)?;
    let binary = std::fs::canonicalize(binary)?;
    let mut command = tokio::process::Command::new(&binary);
    command.arg("manifest");
    let bytes = capture(command, std::time::Duration::from_secs(10)).await?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        manifest["name"].as_str() == Some(name),
        "manifest name does not match requested plugin"
    );
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
    let (granted, capabilities_hash) = consent_capabilities(name, &declared);
    let runtime_role = provider_only_role_json(&manifest);
    std::fs::create_dir_all(home.join("plugins"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("plugins/.commands.lock"))?;
    lock.try_lock()
        .context("another plugin installation is running")?;
    if manifest["widget"].as_bool() == Some(true) {
        // v1 supports one above-editor plugin surface. Refuse to displace another.
        let path = home.join("plugins/widgets.json");
        if path.exists() {
            let old: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            anyhow::ensure!(
                old["name"].as_str() == Some(name),
                "another plugin owns the widget slot"
            );
        }
    }
    let mut registry = load(home)?;
    registry.plugins.insert(
        name.into(),
        LockEntry {
            runtime_role: runtime_role.clone(),
            ecosystem: "gray-native".into(),
            version: manifest["version"].as_str().unwrap_or("unknown").into(),
            hash: String::new(),
            source: binary.to_string_lossy().into_owned(),
            argv: vec![binary.to_string_lossy().into_owned()],
            adapter_version: "1".into(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
            granted_capabilities: granted.clone(),
            capabilities_hash,
        },
    );
    registry.save(&registry_path(home))?;
    // Register its zero-tool sidecar so commands and Bash guidance load normally.
    let mut sidecars = LockFile::load(&gray_plugin::lock::lock_path(home))?;
    sidecars
        .plugins
        .insert(name.into(), registry.plugins[name].clone());
    sidecars.save(&gray_plugin::lock::lock_path(home))?;
    // Refresh the provider cache from the plugin lock so provider rows are
    // discoverable by `/connect` immediately after this registration.
    crate::providers::ProviderRegistry::refresh(home)?;
    let mut metadata = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
    use std::io::Write;
    writeln!(metadata, "{}", manifest)?;
    metadata.persist(home.join("plugins").join(format!("{name}-manifest.json")))?;
    if manifest["widget"].as_bool() == Some(true) {
        let mut tmp = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
        use std::io::Write;
        writeln!(
            tmp,
            "{}",
            serde_json::json!({"name":name,"argv":[binary,"widget"]})
        )?;
        tmp.persist(home.join("plugins/widgets.json"))?;
    }
    println!(
        "Registered '{name}' from {}. Run: gray {name} settings",
        binary.display()
    );
    Ok(())
}

/// Slash commands use the registered executable without replacing the TUI.
pub async fn capture_slash(name: &str, args: &[String]) -> anyhow::Result<Option<String>> {
    let home = home()?;
    let registry = load(&home)?;
    let selected = if let Some(entry) = registry.plugins.get(name) {
        Some((name.to_string(), entry))
    } else {
        registry.plugins.iter().find_map(|(owner, entry)| {
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
    let (program, base) = entry.argv.split_first().context("empty plugin command")?;
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
    let Ok(registry) = load(&home) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in registry.plugins {
        if !enabled(&home, &name, &entry) {
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

    fn command_entry(enabled: bool) -> LockEntry {
        LockEntry {
            runtime_role: None,
            ecosystem: "gray-cli".into(),
            version: "catalog".into(),
            hash: String::new(),
            source: "git+https://example.invalid/x.git".into(),
            argv: vec!["/usr/bin/false".into()],
            adapter_version: "1".into(),
            installed_at: "2026-09-18T00:00:00Z".into(),
            scope: "user".into(),
            enabled,
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

    #[test]
    fn list_managed_merges_both_registries_with_cli_shadowing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(
            home,
            &serde_json::json!({"cli-only": command_entry(true), "both": command_entry(true)}),
        );
        let mut sidecars =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home)).unwrap();
        sidecars.plugins.insert(
            "both".into(),
            gray_plugin::lock::LockEntry {
                runtime_role: None,
                ecosystem: "gray-native".into(),
                version: "9.9.9".into(),
                hash: String::new(),
                source: "sidecar".into(),
                argv: Vec::new(),
                adapter_version: "1".into(),
                installed_at: String::new(),
                scope: "user".into(),
                // Disabled here: `register_native` mirrors native entries
                // into `lock.json`, and `enabled()` reads that overlay.
                enabled: false,
                ..Default::default()
            },
        );
        sidecars.plugins.insert(
            "sidecar-only".into(),
            gray_plugin::lock::LockEntry {
                runtime_role: None,
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                hash: String::new(),
                source: "sidecar".into(),
                argv: Vec::new(),
                adapter_version: "1".into(),
                installed_at: String::new(),
                scope: "user".into(),
                enabled: false,
                ..Default::default()
            },
        );
        sidecars.save(&gray_plugin::lock::lock_path(home)).unwrap();
        let merged = list_managed(home).unwrap();
        let names: Vec<_> = merged.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["both", "cli-only", "sidecar-only"]);
        // Same name in both registries: the CLI entry (what `forward` runs)
        // shadows the sidecar row instead of listing twice.
        let both = merged.iter().find(|p| p.name == "both").unwrap();
        assert!(both.cli);
        assert_eq!(both.entry.ecosystem, "gray-cli");
        // Effective enablement still honors the sidecar overlay (see
        // `enabled`): the mirrored `lock.json` entry disables the row, and
        // the CLI entry itself stays enabled.
        assert!(!enabled(home, "both", &both.entry));
        assert!(both.entry.enabled);
        assert!(is_command(home, "cli-only"));
        assert!(!is_command(home, "sidecar-only"));
    }

    #[test]
    fn list_managed_is_empty_when_both_registries_missing() {
        let home = tempfile::tempdir().unwrap();
        assert!(list_managed(home.path()).unwrap().is_empty());
    }

    #[test]
    fn set_command_enabled_flips_only_the_command_registry() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(home, &serde_json::json!({"demo": command_entry(true)}));
        set_command_enabled(home, "demo", false).unwrap();
        assert!(!load(home).unwrap().plugins["demo"].enabled);
        set_command_enabled(home, "demo", true).unwrap();
        assert!(load(home).unwrap().plugins["demo"].enabled);
        let miss = set_command_enabled(home, "ghost", true).unwrap_err();
        assert!(miss.to_string().contains("not installed: ghost"), "{miss}");
        let bad = set_command_enabled(home, "BAD NAME", true).unwrap_err();
        assert!(
            bad.to_string().contains("invalid plugin command name"),
            "{bad}"
        );
    }

    #[test]
    fn remove_command_drops_mirrored_sidecar_but_keeps_independent_row() {
        // `register_native` mirrors into lock.json with the same argv: the
        // mirror goes with the remove.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let entry = command_entry(true);
        write_commands(home, &serde_json::json!({"demo": entry}));
        let mut sidecars =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home)).unwrap();
        sidecars.plugins.insert(
            "demo".into(),
            gray_plugin::lock::LockEntry {
                runtime_role: None,
                ecosystem: "gray-native".into(),
                version: "catalog".into(),
                hash: String::new(),
                source: "mirror".into(),
                argv: entry.argv.clone(),
                adapter_version: "1".into(),
                installed_at: String::new(),
                scope: "user".into(),
                enabled: true,
                ..Default::default()
            },
        );
        sidecars.save(&gray_plugin::lock::lock_path(home)).unwrap();
        remove_command(home, "demo").unwrap();
        assert!(!load(home).unwrap().plugins.contains_key("demo"));
        assert!(
            !gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))
                .unwrap()
                .plugins
                .contains_key("demo")
        );
        // Same name, different argv: an independent sidecar, not the mirror —
        // it survives and keeps listing (without `[command]`).
        write_commands(home, &serde_json::json!({"demo": command_entry(true)}));
        let mut sidecars =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home)).unwrap();
        sidecars.plugins.insert(
            "demo".into(),
            gray_plugin::lock::LockEntry {
                runtime_role: None,
                ecosystem: "gray-native".into(),
                version: "2.0.0".into(),
                hash: String::new(),
                source: "other".into(),
                argv: vec!["/other/binary".into()],
                adapter_version: "1".into(),
                installed_at: String::new(),
                scope: "user".into(),
                enabled: true,
                ..Default::default()
            },
        );
        sidecars.save(&gray_plugin::lock::lock_path(home)).unwrap();
        remove_command(home, "demo").unwrap();
        let ghost = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))
            .unwrap()
            .plugins
            .remove("demo")
            .unwrap();
        assert_eq!(ghost.argv, vec!["/other/binary".to_string()]);
        let merged = list_managed(home).unwrap();
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].cli);
        assert_eq!(merged[0].entry.version, "2.0.0");
    }

    #[test]
    fn remove_command_drops_registry_manifest_and_widget_slot() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        write_commands(home, &serde_json::json!({"demo": command_entry(true)}));
        std::fs::write(
            home.join("plugins/demo-manifest.json"),
            r#"{"name":"demo","commands":["/demo"]}"#,
        )
        .unwrap();
        std::fs::write(
            home.join("plugins/widgets.json"),
            r#"{"name":"demo","argv":["demo","widget"]}"#,
        )
        .unwrap();
        remove_command(home, "demo").unwrap();
        assert!(!load(home).unwrap().plugins.contains_key("demo"));
        assert!(!home.join("plugins/demo-manifest.json").exists());
        assert!(!home.join("plugins/widgets.json").exists());
        // A widget slot owned by someone else survives.
        std::fs::write(
            home.join("plugins/widgets.json"),
            r#"{"name":"other","argv":["other","widget"]}"#,
        )
        .unwrap();
        write_commands(home, &serde_json::json!({"demo": command_entry(true)}));
        remove_command(home, "demo").unwrap();
        assert!(home.join("plugins/widgets.json").exists());
        let miss = remove_command(home, "ghost").unwrap_err();
        assert!(miss.to_string().contains("not installed: ghost"), "{miss}");
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

    #[test]
    fn git_source_splits_url_from_pinned_commit() {
        let (url, pin) = parse_git_source(
            "git+https://github.com/vstaln/gray-discord-plugin.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f",
        )
        .unwrap();
        assert_eq!(url, "https://github.com/vstaln/gray-discord-plugin.git");
        assert_eq!(pin, "07f3d2e6b5782589e4fa27826e8b42c57467c66f");
        // An ssh URL carries its own '@': the pin is the last segment.
        let (url, pin) = parse_git_source(
            "git+ssh://git@github.com/vstaln/gray-discord-plugin.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f",
        )
        .unwrap();
        assert_eq!(url, "ssh://git@github.com/vstaln/gray-discord-plugin.git");
        assert_eq!(pin, "07f3d2e6b5782589e4fa27826e8b42c57467c66f");
    }

    #[test]
    fn git_source_rejects_anything_but_a_full_commit_pin() {
        for source in [
            "https://github.com/vstaln/gray-discord-plugin.git",
            "git+https://github.com/vstaln/gray-discord-plugin.git",
            "git+https://github.com/vstaln/gray-discord-plugin.git@main",
            "git+https://github.com/vstaln/gray-discord-plugin.git@648952d",
            "git+https://github.com/vstaln/gray-discord-plugin.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f0",
            "git+http://example.invalid/x.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f",
            "git+file:///tmp/x.git@07f3d2e6b5782589e4fa27826e8b42c57467c66f",
            "git+https://github.com/vstaln/gray-discord-plugin.git@648952dc01a78a5eee031846f5f964877bfdac9g",
        ] {
            assert!(parse_git_source(source).is_err(), "{source}");
        }
    }

    #[test]
    fn catalog_discord_builds_from_rust_source() {
        let entry = catalog("discord").unwrap();
        assert_eq!(entry.name, "discord");
        assert_eq!(entry.bin, "gray-discord");
        assert_eq!(entry.sidecar_args, &["sidecar"][..]);
        let miss = catalog("nope").err().expect("an unknown name must fail");
        assert!(miss.to_string().contains("Unknown plugin"), "{miss}");
    }

    #[test]
    fn discord_setup_declaration_asks_for_exactly_what_it_needs() {
        let required: Vec<&str> = DISCORD_SETUP
            .fields
            .iter()
            .filter(|f| f.is_required())
            .map(|f| f.key)
            .collect();
        // Token and home channel only: the owner is discovered by the
        // Discord-side pairing reply instead of typed, and the extras
        // (allowed_users) plus derived paths are filled in around them.
        assert_eq!(required, ["token", "channel_id"]);
        let owner = DISCORD_SETUP.field("owner_id").unwrap();
        assert!(matches!(owner.kind, FieldKind::Optional));
        assert_eq!(
            DISCORD_SETUP.field("allowed_users").unwrap().kind,
            FieldKind::Optional
        );
        assert!(DISCORD_SETUP.field("token").unwrap().secret);
    }
}

#[path = "plugin_native.rs"]
mod native;

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn commands_lock_fails_loudly_when_contended() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("plugins")).unwrap();
        let lock_path = home.path().join("plugins/.commands.lock");
        let holder = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        holder.try_lock().unwrap();
        let err = hold_commands_lock_timeout(home.path(), std::time::Duration::from_millis(50))
            .err()
            .expect("a contended commands lock must fail the op");
        assert!(
            err.to_string().contains("another plugin operation"),
            "{err}"
        );
        drop(holder);
        assert!(
            hold_commands_lock_timeout(home.path(), std::time::Duration::from_millis(50)).is_ok()
        );
    }
}
