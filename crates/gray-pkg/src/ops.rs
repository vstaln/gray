//! Install/list/remove/update over the plugin lockfile.
//!
//! Two sources: the gray index (verified gray-native tarballs) and bare
//! https tarball URLs (unverified, warned).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub schema: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LockEntry>,
}

fn default_true() -> bool {
    true
}

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    #[serde(default)]
    pub ecosystem: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub adapter_version: String,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// argv for `gray <name> …` forwarding; `None` = sidecar-only. Round-tripped
    /// so `set_enabled`/`remove`/`update` never drop it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_argv: Option<Vec<String>>,
    /// Fields this crate does not own (`granted_capabilities`,
    /// `capabilities_hash`, `runtime_role`, …) — gray_plugin writes them, and a
    /// gray-pkg rewrite that dropped them would silently re-grant capabilities
    /// or re-boot a provider-only entry as a normal sidecar.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for LockEntry {
    fn default() -> Self {
        Self {
            ecosystem: String::new(),
            version: String::new(),
            hash: String::new(),
            source: String::new(),
            argv: Vec::new(),
            adapter_version: String::new(),
            installed_at: String::new(),
            scope: String::new(),
            enabled: true,
            cli_argv: None,
            extra: Default::default(),
        }
    }
}

/// Install target: a gray index name or an https tarball URL.
#[derive(Debug, Clone)]
pub enum NameOrUrl {
    Name(String),
    Url(String),
}

pub fn parse_spec(s: &str) -> NameOrUrl {
    let t = s.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        NameOrUrl::Url(t.to_string())
    } else {
        NameOrUrl::Name(t.to_string())
    }
}

#[derive(Debug, Default, Clone)]
pub struct InstallOpts {
    pub argv: Vec<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
}

fn lock_path() -> PathBuf {
    crate::plugins_dir().join("lock.json")
}

fn read_lock() -> anyhow::Result<Option<LockFile>> {
    match std::fs::read_to_string(lock_path()) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// How long a registry write waits for another process's read-modify-write
/// before failing loudly (the same doctrine as the session store's locks).
const REGISTRY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Exclusive cross-process guard for the whole read-modify-write on
/// `lock.json`: the file is kept (it is the lock itself), `WouldBlock` is
/// retried until `timeout`, and only a *contended* lock fails the op. A
/// filesystem without flock degrades to unlocked-with-warning, so an exotic
/// filesystem never breaks `install`/`remove`.
pub fn hold_registry_lock_timeout(
    timeout: std::time::Duration,
) -> anyhow::Result<Option<std::fs::File>> {
    hold_registry_lock_timeout_in(&crate::plugins_dir(), timeout)
}

/// Same guard against an explicit plugins dir, so callers carrying their
/// own home (tests, `plugin_cli`) never touch the real `~/.gray`.
pub fn hold_registry_lock_in(plugins_dir: &Path) -> anyhow::Result<Option<std::fs::File>> {
    hold_registry_lock_timeout_in(plugins_dir, REGISTRY_LOCK_TIMEOUT)
}

fn hold_registry_lock_timeout_in(
    plugins_dir: &Path,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<std::fs::File>> {
    let path = plugins_dir.join(".registry.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
            .ok();
    }
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
                eprintln!(
                    "warning: plugin registry locking unsupported on {} ({e}); proceeding unlocked",
                    path.display()
                );
                return Ok(None);
            }
        }
    }
}

pub fn hold_registry_lock() -> anyhow::Result<Option<std::fs::File>> {
    hold_registry_lock_timeout(REGISTRY_LOCK_TIMEOUT)
}

fn write_lock(lock: &LockFile) -> anyhow::Result<()> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock = lock.clone();
    lock.schema = 1;
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temporary.write_all(serde_json::to_string_pretty(&lock)?.as_bytes())?;
    temporary.persist(&path)?;
    Ok(())
}

/// Record one install in the lockfile, preserving any existing `enabled`
/// flag (a reinstall must not silently re-enable a disabled plugin).
fn record_install(
    name: &str,
    ecosystem: &str,
    version: &str,
    hash: &str,
    source: &str,
    scope: String,
    argv: Vec<String>,
) -> anyhow::Result<()> {
    let _guard = hold_registry_lock()?;
    let mut lock = read_lock()?.unwrap_or_default();
    let enabled = lock.plugins.get(name).map(|e| e.enabled).unwrap_or(true);
    // A CLI registration's `cli_argv` survives a reinstall/update the same
    // way `enabled` does — as do fields owned by gray_plugin::lock (grants,
    // consent hash, runtime role), which this rewrite must not drop.
    let cli_argv = lock.plugins.get(name).and_then(|e| e.cli_argv.clone());
    let extra = lock
        .plugins
        .get(name)
        .map(|e| e.extra.clone())
        .unwrap_or_default();
    lock.plugins.insert(
        name.to_string(),
        LockEntry {
            ecosystem: ecosystem.to_string(),
            version: version.to_string(),
            hash: hash.to_string(),
            source: source.to_string(),
            argv,
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: crate::now_secs().to_string(),
            scope,
            enabled,
            cli_argv,
            extra,
        },
    );
    write_lock(&lock)
}

/// A lock row that belongs to a user-registered executable (a `plugin add`
/// or a PATH registration): it has a `cli_argv` but no index hash. Index
/// installs carry the tarball's hash even after adopting a `cli_argv`, so
/// this is the test that keeps `update` from clobbering user registrations.
pub fn is_registered_executable(e: &LockEntry) -> bool {
    e.cli_argv.is_some() && e.hash.is_empty()
}

/// Index entries gray installs by name: curated gray-native tarballs.
/// Anything else bails with the honest reason.
fn supported_index_entry(entry: &crate::index::Entry) -> bool {
    matches!(
        (entry.ecosystem.as_str(), entry.source.type_.as_str()),
        ("gray-native", "tarball")
    )
}

/// Derive a plugin name from a URL's last path segment.
fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_suffix(".tar.gz")
        .or_else(|| last.strip_suffix(".tgz"))
        .unwrap_or(last)
        .to_string()
}

/// Reject install keys that would escape the plugins dir: empty, `.`,
/// `..`, or anything holding `/` or `\`.
pub(crate) fn validate_install_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() || key == "." || key == ".." || key.contains('/') || key.contains('\\') {
        anyhow::bail!("cannot derive a safe plugin name from package (got {key:?})");
    }
    Ok(())
}

/// Some archives wrap everything in `package/`; use it when present.
pub(crate) fn stage_root(dir: &Path) -> PathBuf {
    let wrapped = dir.join("package");
    if wrapped.is_dir() {
        wrapped
    } else {
        dir.to_path_buf()
    }
}

/// Shallow-clone `url` (under the plugins tmp dir) via the shared
/// `sources::clone_into_tmp` helper and return the keeper tempdir, the clone
/// dir, and the cloned HEAD commit sha. `--branch` only when pinned.
pub(crate) fn clone_git_repo(
    url: &str,
    git_ref: Option<&str>,
) -> anyhow::Result<(tempfile::TempDir, PathBuf, String)> {
    let branch = git_ref.filter(|r| !r.is_empty());
    let mut extra: Vec<&str> = Vec::new();
    if let Some(b) = branch {
        extra.push("--branch");
        extra.push(b);
    }
    let (staging, clone_dir) = crate::sources::clone_into_tmp(url, &extra, true)?;
    let sha_out = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()?;
    if !sha_out.status.success() {
        anyhow::bail!(
            "cloning {} failed: cannot read commit sha",
            crate::fetch::redact(url)
        );
    }
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    if sha.is_empty() {
        anyhow::bail!(
            "cloning {} failed: empty commit sha",
            crate::fetch::redact(url)
        );
    }
    Ok((staging, clone_dir, sha))
}

/// `(source, item)` identity for install failures, from the spec kind —
/// the honest attribution when the ecosystem isn't known yet.
fn install_identity(spec: &NameOrUrl) -> (String, String) {
    match spec {
        NameOrUrl::Name(n) => ("index".to_string(), n.clone()),
        NameOrUrl::Url(u) => ("url".to_string(), u.clone()),
    }
}

/// Best-effort ecosystem attribution for the error registry: the lock
/// entry's ecosystem when known, `"plugin"` otherwise (misses,
/// unreadable lock).
fn ecosystem_of(name: &str) -> String {
    read_lock()
        .ok()
        .flatten()
        .and_then(|l| l.plugins.get(name).map(|e| e.ecosystem.clone()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "plugin".to_string())
}

pub async fn install(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let (source, item) = install_identity(&spec);
    install_inner(spec, opts).await.map_err(|e| {
        crate::errors::record(&source, &item, format!("{e:#}"));
        e
    })
}

async fn install_inner(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let client = crate::fetch::client()?;
    match spec {
        NameOrUrl::Name(name) => install_index(&client, &name, opts).await,
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
    }
}

/// Install a fully staged tree, rolling back the directory if lock recording fails.
fn replace_archive(
    archive: &Path,
    name: &str,
    record: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<PathBuf> {
    validate_install_key(name)?;
    anyhow::ensure!(
        !matches!(name, "tmp" | "pi" | "lock.json" | "index-cache.json"),
        "reserved plugin name"
    );
    let root = crate::plugins_dir();
    std::fs::create_dir_all(&root)?;
    let stage = tempfile::tempdir_in(&root)?;
    let next = stage.path().join("next");
    let previous = stage.path().join("previous");
    std::fs::create_dir(&next)?;
    let unpacked = crate::fetch::unpack_tar_gz(archive, &next);
    let _ = std::fs::remove_file(archive);
    unpacked?;
    let dest = root.join(name);
    let had_previous = dest.symlink_metadata().is_ok();
    if had_previous {
        std::fs::rename(&dest, &previous)?;
    }
    let installed = std::fs::rename(&next, &dest)
        .map_err(anyhow::Error::from)
        .and_then(|()| record());
    if let Err(error) = installed {
        let rollback = (|| -> std::io::Result<()> {
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            if had_previous {
                std::fs::rename(&previous, &dest)?;
            }
            Ok(())
        })();
        if let Err(rollback) = rollback {
            let recovery = stage.keep();
            anyhow::bail!(
                "{error}; rollback failed: {rollback}; previous install saved at {}",
                recovery.display()
            );
        }
        return Err(error);
    }
    Ok(dest)
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    validate_install_key(name)?;
    let index = crate::index::fetch_index(client).await?;
    let entry = crate::index::lookup(&index, name)?;
    if !supported_index_entry(entry) {
        anyhow::bail!(
            "unsupported ecosystem '{}' source type '{}' for {name} (installable: gray-native tarballs)",
            entry.ecosystem,
            entry.source.type_
        );
    }
    let archive = crate::fetch::download(client, &entry.source.url, Some(&entry.hash)).await?;
    let scope = if entry.scope.is_empty() {
        opts.scope.clone().unwrap_or_else(|| "user".to_string())
    } else {
        entry.scope.clone()
    };
    let dest = replace_archive(&archive, name, || {
        record_install(
            name,
            &entry.ecosystem,
            &entry.version,
            entry.hash.primary().unwrap_or_default(),
            &entry.source.url,
            scope,
            opts.argv.clone(),
        )
    })?;
    Ok(Report {
        name: name.to_string(),
        version: entry.version.clone(),
        path: dest,
    })
}

async fn install_url(
    client: &reqwest::Client,
    url: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    eprintln!(
        "warning: unverified install from {} (no index hash; use an index name for verified installs)",
        crate::fetch::redact(url)
    );
    let name = name_from_url(url);
    if name.is_empty() {
        anyhow::bail!(
            "cannot derive a plugin name from URL: {}",
            crate::fetch::redact(url)
        );
    }
    validate_install_key(&name)?;
    let archive = crate::fetch::download(client, url, None).await?;
    let dest = replace_archive(&archive, &name, || {
        record_install(
            &name,
            "url",
            "0.0.0",
            "",
            url,
            opts.scope.clone().unwrap_or_else(|| "user".to_string()),
            opts.argv.clone(),
        )
    })?;
    Ok(Report {
        name,
        version: "0.0.0".to_string(),
        path: dest,
    })
}

pub fn list() -> anyhow::Result<BTreeMap<String, LockEntry>> {
    Ok(read_lock()?.map(|l| l.plugins).unwrap_or_default())
}

pub fn remove(name: &str) -> anyhow::Result<()> {
    remove_inner(name).map_err(|e| {
        crate::errors::record(&ecosystem_of(name), name, format!("{e:#}"));
        e
    })
}

fn remove_inner(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("not installed: {name}");
    }
    let _guard = hold_registry_lock()?;
    let mut lock = read_lock()?.unwrap_or_default();
    if lock.plugins.remove(name).is_none() {
        anyhow::bail!("not installed: {name}");
    }
    // Commit the registry first: a failed write must leave the files on disk
    // (a stale entry the user can re-remove), never the reverse, where the
    // registry would claim a plugin whose files are gone.
    write_lock(&lock)?;
    // Gray-native/URL installs live at `<plugins_dir>/<name>`; pi installs
    // at `<plugins_dir>/pi/<name>` (R13: sanitized keys hold no `/`).
    for dir in [
        crate::plugins_dir().join(name),
        crate::plugins_dir().join("pi").join(name),
    ] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    Ok(())
}

/// Flip a plugin's `enabled` flag (boot skips disabled entries).
/// Miss message matches `remove`.
pub fn set_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    set_enabled_inner(name, on).map_err(|e| {
        crate::errors::record(&ecosystem_of(name), name, format!("{e:#}"));
        e
    })
}

fn set_enabled_inner(name: &str, on: bool) -> anyhow::Result<()> {
    let _guard = hold_registry_lock()?;
    let mut lock = read_lock()?.unwrap_or_default();
    let Some(entry) = lock.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    entry.enabled = on;
    write_lock(&lock)?;
    Ok(())
}

/// Update one plugin (`target` = name) or all (`target` = `"all"`).
/// Only lock entries the index lists under the same ecosystem (gray-native
/// tarballs) are considered; anything else is skipped with a warning.
/// Returns per-plugin reports for the plugins that actually changed.
pub async fn update(target: &str) -> anyhow::Result<Vec<Report>> {
    update_inner(target).await.map_err(|e| {
        crate::errors::record(&ecosystem_of(target), target, format!("{e:#}"));
        e
    })
}

async fn update_inner(target: &str) -> anyhow::Result<Vec<Report>> {
    let lock = read_lock()?.unwrap_or_default();
    let names: Vec<String> = if target == "all" {
        lock.plugins.keys().cloned().collect()
    } else {
        if !lock.plugins.contains_key(target) {
            anyhow::bail!("not installed: {target}");
        }
        vec![target.to_string()]
    };
    let client = crate::fetch::client()?;
    let index = crate::index::fetch_index(&client).await?;
    let mut out = Vec::new();
    for name in &names {
        let installed = &lock.plugins[name];
        // Registered executables (cli_argv with no index hash) are
        // user-owned, not index installs — never let a same-named index
        // entry replace one. Index installs that adopted a cli_argv keep
        // their hash and update normally.
        if is_registered_executable(installed) {
            eprintln!("warning: skipping update of {name} (registered executable)");
            continue;
        }
        let entry = match crate::index::lookup(&index, name) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("warning: skipping update of {name} (not in index)");
                continue;
            }
        };
        // Update only rows install_index would write itself: a foreign row
        // the index never listed keeps its source (never rewritten).
        if installed.ecosystem != entry.ecosystem || !supported_index_entry(entry) {
            eprintln!("warning: skipping update of {name} (non-index source)");
            continue;
        }
        if entry.version == installed.version {
            continue;
        }
        let argv = installed.argv.clone();
        let scope = Some(installed.scope.clone());
        out.push(install_index(&client, name, InstallOpts { argv, scope }).await?);
    }
    Ok(out)
}

#[path = "ops_tests.rs"]
#[cfg(test)]
pub(crate) mod tests;
