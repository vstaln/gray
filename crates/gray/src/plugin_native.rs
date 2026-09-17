//! Prebuilt background sidecar installation. No compiler/interpreter on user machines.
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use gray_plugin::Plugin;
use gray_plugin::lock::{LockEntry, LockFile, lock_path};

const VERSION: &str = "0.1.0";
const RELEASE: &str = "https://github.com/vstaln/gray-background/releases/download/v0.1.0";

fn target(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => {
            bail!("background has no binary for {os}/{arch}; supported: Linux/macOS x86_64/aarch64")
        }
    }
}

fn checksum(text: &str, asset: &str) -> Result<String> {
    let parts: Vec<_> = text.split_whitespace().collect();
    ensure!(
        parts.len() == 2 && parts[1] == asset,
        "invalid background checksum file"
    );
    ensure!(
        parts[0].len() == 64 && parts[0].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid background SHA-256"
    );
    Ok(format!("sha256:{}", parts[0]))
}

// Publish only after verification. If the registry cannot be saved, restore the
// previous binary directory. Like gray-pkg's archive installer, but no extraction.
fn publish(
    stage: tempfile::TempDir,
    dest: &Path,
    record: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let previous = stage.path().join("previous");
    let had_previous = dest.symlink_metadata().is_ok();
    if had_previous {
        std::fs::rename(dest, &previous)?;
    }
    let result = std::fs::rename(stage.path().join("next"), dest)
        .map_err(anyhow::Error::from)
        .and_then(|()| record());
    if let Err(error) = result {
        let rollback = (|| -> std::io::Result<()> {
            if dest.exists() {
                std::fs::remove_dir_all(dest)?;
            }
            if had_previous {
                std::fs::rename(&previous, dest)?;
            }
            Ok(())
        })();
        if let Err(rollback) = rollback {
            let recovery = stage.keep();
            bail!(
                "{error}; rollback failed: {rollback}; previous install saved at {}",
                recovery.display()
            );
        }
        return Err(error);
    }
    Ok(())
}

pub(super) async fn install(home: &Path) -> Result<()> {
    let platform = target(std::env::consts::OS, std::env::consts::ARCH)?;
    let asset = format!("background-{platform}");
    // Explicit release mirror for development/offline deployments. Never shell commands.
    let base = std::env::var("GRAY_BACKGROUND_RELEASE_BASE").unwrap_or_else(|_| RELEASE.into());
    let parsed = reqwest::Url::parse(&base).context("invalid background release URL")?;
    ensure!(
        parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))),
        "background release must use HTTPS (loopback HTTP allowed for testing)"
    );
    ensure!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "release URL must be a base directory URL"
    );
    let source = format!("{}/{}", base.trim_end_matches('/'), asset);
    let root = home.join("plugins");
    std::fs::create_dir_all(&root)?;
    let _lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".background-install.lock"))?;
    _lock
        .try_lock()
        .context("another background installation is running")?;
    // Refuse corrupt registries before downloading or executing anything.
    let existing = LockFile::load(&lock_path(home))?;
    ensure!(existing.schema == 1, "unsupported plugin lock schema");
    let dest = root.join("background");
    let executable = dest.join("background");
    if existing.plugins.get("background").is_some_and(|entry| {
        entry.version == VERSION
            && entry.source == source
            && entry.enabled
            && entry.ecosystem == "gray-native"
            && entry.argv.is_empty()
    }) && executable.is_file()
    {
        println!("Background is already installed. Start a new Gray session and use /background.");
        return Ok(());
    }
    let client = gray_pkg::fetch::client()?;
    println!(
        "Installing background {VERSION} for {platform} (plugin code runs with your user permissions)."
    );
    let mut response = client
        .get(format!("{source}.sha256"))
        .send()
        .await
        .context("background release unavailable; its binary assets must be published first")?
        .error_for_status()
        .context("background release unavailable; its binary assets must be published first")?;
    ensure!(
        !response.status().is_redirection(),
        "checksum redirected to disallowed URL"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024,
            "checksum response exceeds 1 KiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let digest = checksum(std::str::from_utf8(&bytes)?, &asset)?;
    let hash = gray_pkg::index::HashSpec::Single(digest.clone());
    let downloaded = gray_pkg::fetch::download(&client, &source, Some(&hash)).await?;
    let downloaded = tempfile::TempPath::try_from_path(downloaded)?;
    let stage = tempfile::tempdir_in(&root)?;
    let next = stage.path().join("next");
    std::fs::create_dir(&next)?;
    let candidate = next.join("background");
    std::fs::copy(&downloaded, &candidate)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755))?;
    }
    let plugin =
        gray_plugin::SidecarPlugin::spawn(vec![candidate.to_string_lossy().into_owned()]).await?;
    let manifest = plugin.manifest();
    plugin.shutdown(Duration::from_secs(2)).await;
    ensure!(
        manifest.name == "background"
            && manifest.version == VERSION
            && manifest.protocol.as_deref() == Some("1.1")
            && manifest.commands == ["/background"]
            && manifest.tools.is_empty(),
        "download is not the expected background plugin"
    );
    // Reload immediately before commit so unrelated registrations survive.
    let mut registry = LockFile::load(&lock_path(home))?;
    ensure!(registry.schema == 1, "unsupported plugin lock schema");
    registry.plugins.insert(
        "background".into(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: VERSION.into(),
            hash: digest,
            source,
            argv: Vec::new(),
            adapter_version: "1.1".into(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            scope: "user".into(),
            enabled: true,
        },
    );
    publish(stage, &dest, || registry.save(&lock_path(home)))?;
    println!(
        "Installed background globally. Restart Gray, then use /background <image> [opacity] [gradient]."
    );
    Ok(())
}

#[cfg(test)]
#[path = "plugin_native_tests.rs"]
mod tests;
