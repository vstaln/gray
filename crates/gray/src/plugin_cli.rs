//! Native plugin command registration, help metadata and bounded UI requests. No model calls.
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use gray_plugin::lock::{LockEntry, LockFile};

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

fn metadata(home: &Path, name: &str) -> anyhow::Result<serde_json::Value> {
    validate_name(name)?;
    Ok(serde_json::from_slice(&std::fs::read(
        home.join("plugins").join(format!("{name}-manifest.json")),
    )?)?)
}

/// Register a separately built native plugin. No hardcoded plugin catalog.
/// Download/version resolution remains the existing `gray plugin install` API.
pub async fn install(home: &Path, name: &str) -> anyhow::Result<()> {
    validate_name(name)?;
    if let Some(path) = std::env::var_os("GRAY_PLUGIN_PATH") {
        return register_native(home, name, Path::new(&path)).await;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let path = dir.join(format!("gray-{name}{}", std::env::consts::EXE_SUFFIX));
            if path.is_file() {
                return register_native(home, name, &path).await;
            }
        }
    }
    anyhow::bail!(
        "plugin '{name}' is not available locally; put gray-{name} on PATH or set GRAY_PLUGIN_PATH to its executable"
    )
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

/// Register a user-selected native executable; registration runs its manifest.
/// The executable and widgets remain owned by the separate plugin repository.
pub async fn register_native(home: &Path, name: &str, binary: &Path) -> anyhow::Result<()> {
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
}
