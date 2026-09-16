//! Generic catalog installation and terminal command forwarding. No model calls.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use gray_plugin::lock::{LockEntry, LockFile};

// Package code executes with user privileges, just like other installed plugins.
// Pin reviewed source; dependency resolution remains the package's responsibility.
const CATALOG: &[(&str, &str, &str)] = &[(
    "discord",
    "git+https://github.com/vstaln/gray-discord-plugin.git@08470b8",
    "gray_discord",
)];

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

fn run(program: impl AsRef<OsStr>, args: &[&OsStr]) -> anyhow::Result<()> {
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

/// Private per-plugin venv, published only after installation and import succeed.
/// TempDir rolls back failed installs; a file lock serializes registry updates.
pub fn install(home: &Path, name: &str) -> anyhow::Result<()> {
    validate_name(name)?;
    // A separately installed native plugin may be registered by name without
    // coupling the host to its implementation or an unpublished remote URL.
    if let Some(path) = std::env::var_os("GRAY_PLUGIN_PATH") {
        return register_native(home, name, Path::new(&path));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let path = dir.join(format!("gray-{name}"));
            if path.is_file() {
                return register_native(home, name, &path);
            }
        }
    }
    let (_, source, module) = CATALOG
        .iter()
        .find(|(n, _, _)| *n == name)
        .with_context(|| format!("Unknown plugin '{name}'. Available: discord"))?;
    let root = home.join("plugins/cli");
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("plugins/.commands.lock"))?;
    lock.try_lock()
        .context("another plugin installation is running")?;
    let mut registry = load(home)?;
    if registry.plugins.contains_key(name) {
        println!("Plugin '{name}' is already registered. Run: gray {name} --help");
        return Ok(());
    }
    let env = tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempdir_in(root)?;
    let python = std::env::var_os("GRAY_PLUGIN_PYTHON").unwrap_or_else(|| "python3".into());
    println!("Installing '{name}' from {source} (plugin code runs with your user permissions).");
    run(
        python,
        &[OsStr::new("-m"), OsStr::new("venv"), env.path().as_os_str()],
    )?;
    let executable = env.path().join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    run(
        &executable,
        &[
            OsStr::new("-m"),
            OsStr::new("pip"),
            OsStr::new("install"),
            OsStr::new(source),
        ],
    )?;
    run(
        &executable,
        &[OsStr::new("-m"), OsStr::new(module), OsStr::new("--help")],
    )?;
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
        },
    );
    registry.save(&registry_path(home))?;
    // Keep the original venv path: moving it would invalidate Python entry-point shebangs.
    let _ = env.keep();
    println!("Installed '{name}'. Next: gray {name} setup");
    Ok(())
}

/// Resolve only explicitly registered commands (never arbitrary PATH executables).
/// exec preserves terminal, signals, argument boundaries, and the child's exit code.
pub fn forward(home: &Path, name: &str, rest: &[String]) -> anyhow::Result<()> {
    validate_name(name)?;
    let registry = load(home)?;
    let entry = registry.plugins.get(name).with_context(|| {
        format!("no plugin command '{name}' — install it with: gray install plugin {name}")
    })?;
    anyhow::ensure!(entry.enabled, "plugin command '{name}' is disabled");
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

/// Register a user-selected native executable; registration runs its manifest.
/// The executable and widgets remain owned by the separate plugin repository.
pub fn register_native(home: &Path, name: &str, binary: &Path) -> anyhow::Result<()> {
    validate_name(name)?;
    let binary = std::fs::canonicalize(binary)?;
    let out = Command::new(&binary).arg("manifest").output()?;
    anyhow::ensure!(out.status.success(), "plugin manifest failed");
    let manifest: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    anyhow::ensure!(
        manifest["name"].as_str() == Some(name),
        "manifest name does not match requested plugin"
    );
    std::fs::create_dir_all(home.join("plugins"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("plugins/.commands.lock"))?;
    lock.try_lock()
        .context("another plugin installation is running")?;
    let mut registry = load(home)?;
    registry.plugins.insert(
        name.into(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: manifest["version"].as_str().unwrap_or("unknown").into(),
            hash: String::new(),
            source: binary.to_string_lossy().into_owned(),
            argv: vec![binary.to_string_lossy().into_owned()],
            adapter_version: "1".into(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
        },
    );
    registry.save(&registry_path(home))?;
    // Register its zero-tool sidecar so commands and Bash guidance load normally.
    let mut sidecars = LockFile::load(&gray_plugin::lock::lock_path(home))?;
    sidecars
        .plugins
        .insert(name.into(), registry.plugins[name].clone());
    sidecars.save(&gray_plugin::lock::lock_path(home))?;
    let mut metadata = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
    use std::io::Write;
    writeln!(metadata, "{}", manifest)?;
    metadata.persist(home.join("plugins").join(format!("{name}-manifest.json")))?;
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
        let mut tmp = tempfile::NamedTempFile::new_in(home.join("plugins"))?;
        use std::io::Write;
        writeln!(
            tmp,
            "{}",
            serde_json::json!({"name":name,"argv":[binary,"widget"]})
        )?;
        tmp.persist(path)?;
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
    let canonical = if registry.plugins.contains_key(name) {
        name.to_string()
    } else {
        format!("{name}s")
    };
    let Some(entry) = registry.plugins.get(&canonical) else {
        return Ok(None);
    };
    anyhow::ensure!(entry.enabled, "plugin command is disabled");
    let (program, base) = entry.argv.split_first().context("empty plugin command")?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(program)
            .args(base)
            .args(if args.is_empty() {
                vec!["status".to_string()]
            } else {
                args.to_vec()
            })
            .env("GRAY_BIN", std::env::current_exe()?)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Only explicitly installed command metadata participates in completion.
pub(crate) fn completions(query: &str) -> Vec<(String, String)> {
    let Ok(home) = home() else { return Vec::new() };
    let Ok(registry) = load(&home) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in registry.plugins {
        if !entry.enabled {
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
