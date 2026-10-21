use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub schema: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LockEntry>,
}

fn default_true() -> bool {
    true
}

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_argv: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub global: bool,

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
            global: false,
            extra: Default::default(),
        }
    }
}

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
    pub deps: Vec<String>,
}

fn read_lock() -> anyhow::Result<Option<LockFile>> {
    read_lock_in(&crate::plugins_dir())
}

pub(crate) fn read_lock_in(plugins: &Path) -> anyhow::Result<Option<LockFile>> {
    match std::fs::read_to_string(plugins.join("lock.json")) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

const REGISTRY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn hold_registry_lock_timeout(
    timeout: std::time::Duration,
) -> anyhow::Result<Option<std::fs::File>> {
    hold_registry_lock_timeout_in(&crate::plugins_dir(), timeout)
}

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
    write_lock_in(&crate::plugins_dir(), lock)
}

pub(crate) fn write_lock_in(plugins: &Path, lock: &LockFile) -> anyhow::Result<()> {
    let path = plugins.join("lock.json");
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
            global: lock.plugins.get(name).is_some_and(|e| e.global),
            extra,
        },
    );
    write_lock(&lock)
}

pub fn is_registered_executable(e: &LockEntry) -> bool {
    e.cli_argv.is_some() && e.hash.is_empty()
}

fn supported_index_entry(entry: &crate::index::Entry) -> bool {
    matches!(
        (entry.ecosystem.as_str(), entry.source.type_.as_str()),
        ("gray-native", "tarball")
    )
}

fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_suffix(".tar.gz")
        .or_else(|| last.strip_suffix(".tgz"))
        .unwrap_or(last)
        .to_string()
}

pub(crate) fn validate_install_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() || key == "." || key == ".." || key.contains('/') || key.contains('\\') {
        anyhow::bail!("cannot derive a safe plugin name from package (got {key:?})");
    }
    Ok(())
}

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

fn install_identity(spec: &NameOrUrl) -> (String, String) {
    match spec {
        NameOrUrl::Name(n) => ("index".to_string(), n.clone()),
        NameOrUrl::Url(u) => ("url".to_string(), u.clone()),
    }
}

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
        NameOrUrl::Name(name) => install_index(&client, &name, opts, &mut BTreeSet::new()).await,
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
    }
}

fn replace_archive(
    archive: &Path,
    name: &str,
    record: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<PathBuf> {
    validate_install_key(name)?;
    anyhow::ensure!(
        !matches!(
            name,
            "tmp" | "lock.json" | "index-cache.json" | "hub-index-cache.json"
        ),
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

const MAX_DEP_DEPTH: usize = 8;

fn installed_plugin_names() -> BTreeMap<String, LockEntry> {
    read_lock()
        .ok()
        .flatten()
        .map(|l| l.plugins)
        .unwrap_or_default()
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
    visited: &mut BTreeSet<String>,
) -> anyhow::Result<Report> {
    validate_install_key(name)?;

    let hub_index = crate::index::fetch_hub_index(client).await.ok();
    let hub_hit = hub_index
        .as_ref()
        .and_then(|index| index.plugins.get(name).cloned());
    let (entry, community_index) = match hub_hit {
        Some(entry) => (entry, None),
        None => {
            let index = crate::index::fetch_index(client).await?;
            (crate::index::lookup(&index, name)?.clone(), Some(index))
        }
    };
    match entry.kind.as_str() {
        "" | "plugin" => {}
        "skill" => return crate::skills_ops::install_hub_entry(client, name, &entry).await,
        other => {
            anyhow::bail!("package '{name}' is kind '{other}' — not installable yet")
        }
    }
    if !supported_index_entry(&entry) {
        anyhow::bail!(
            "unsupported ecosystem '{}' source type '{}' for {name} (installable: gray-native tarballs)",
            entry.ecosystem,
            entry.source.type_
        );
    }

    if visited.len() >= MAX_DEP_DEPTH {
        anyhow::bail!("{name}: plugin dependency chain exceeds {MAX_DEP_DEPTH} levels");
    }
    visited.insert(name.to_string());

    let installed = installed_plugin_names();
    let mut deps = Vec::new();
    for dep in &entry.requires_plugins {
        if installed.contains_key(dep) || visited.contains(dep) {
            continue;
        }
        let report = Box::pin(install_index(client, dep, opts.clone(), visited))
            .await
            .map_err(|e| anyhow::anyhow!("{name} requires plugin '{dep}': {e:#}"))?;
        deps.extend(report.deps.iter().cloned());
        deps.push(dep.clone());
    }

    let installed = installed_plugin_names();
    let conflicts: Vec<String> = entry
        .conflicts
        .iter()
        .filter(|c| installed.contains_key(c.as_str()))
        .cloned()
        .collect();
    if !conflicts.is_empty() {
        if std::env::var_os("GRAY_PLUGIN_FORCE").is_some() {
            eprintln!(
                "warning: {name} conflicts with installed plugin(s): {} — continuing (GRAY_PLUGIN_FORCE)",
                conflicts.join(", ")
            );
        } else {
            anyhow::bail!(
                "{name} conflicts with installed plugin(s): {} — remove them first (GRAY_PLUGIN_FORCE=1 to override)",
                conflicts.join(", ")
            );
        }
    }

    let mut superseded: Vec<String> = entry
        .supersedes
        .iter()
        .filter(|s| installed.contains_key(s.as_str()))
        .cloned()
        .collect();
    for pname in installed.keys() {
        if pname == name || superseded.contains(pname) {
            continue;
        }
        let peer = hub_index
            .as_ref()
            .and_then(|i| i.plugins.get(pname))
            .or_else(|| community_index.as_ref().and_then(|i| i.plugins.get(pname)));
        if peer.is_some_and(|e| e.superseded_by.iter().any(|s| s == name)) {
            superseded.push(pname.clone());
        }
    }
    if !superseded.is_empty() {
        eprintln!(
            "note: {name} supersedes installed plugin(s) {} — double-hooking possible",
            superseded.join(", ")
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
        deps,
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
        deps: Vec::new(),
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

    write_lock(&lock)?;
    let dir = crate::plugins_dir().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

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
    if !on {
        entry.global = false;
    }
    write_lock(&lock)?;
    Ok(())
}

pub fn set_global(name: &str, global: bool) -> anyhow::Result<()> {
    set_global_inner(name, global).map_err(|e| {
        crate::errors::record(&ecosystem_of(name), name, format!("{e:#}"));
        e
    })
}

fn set_global_inner(name: &str, global: bool) -> anyhow::Result<()> {
    let _guard = hold_registry_lock()?;
    let mut lock = read_lock()?.unwrap_or_default();
    let Some(entry) = lock.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    entry.global = global;
    write_lock(&lock)?;
    Ok(())
}

#[derive(Debug)]
pub enum UpdateAction {
    Installed(String),
    Rebuilt,
    Stale(String),
    Current,
    Skipped(String),
    Failed(String),
}

pub struct UpdateRow {
    pub name: String,
    pub action: UpdateAction,
    pub path: Option<PathBuf>,
}

impl UpdateRow {
    fn new(name: &str, action: UpdateAction) -> Self {
        Self {
            name: name.to_string(),
            action,
            path: None,
        }
    }

    pub fn detail(&self) -> String {
        match &self.action {
            UpdateAction::Installed(v) => format!("updated to {v}"),
            UpdateAction::Rebuilt => "rebuilt from repo HEAD".to_string(),
            UpdateAction::Stale(why) => format!("stale: {why}"),
            UpdateAction::Current => "up to date".to_string(),
            UpdateAction::Skipped(why) => format!("skipped: {why}"),
            UpdateAction::Failed(e) => format!("failed: {e}"),
        }
    }

    pub fn changed(&self) -> bool {
        matches!(
            self.action,
            UpdateAction::Installed(_) | UpdateAction::Rebuilt
        )
    }
}

pub async fn update(target: &str) -> anyhow::Result<Vec<UpdateRow>> {
    update_inner(target, false).await.map_err(|e| {
        crate::errors::record(&ecosystem_of(target), target, format!("{e:#}"));
        e
    })
}

pub async fn check_update(target: &str) -> anyhow::Result<Vec<UpdateRow>> {
    update_inner(target, true).await.map_err(|e| {
        crate::errors::record(&ecosystem_of(target), target, format!("{e:#}"));
        e
    })
}

async fn update_inner(target: &str, check: bool) -> anyhow::Result<Vec<UpdateRow>> {
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
    let mut hub: Option<crate::index::Index> = None;
    let mut comm: Option<crate::index::Index> = None;
    let mut out = Vec::new();
    for name in &names {
        let installed = &lock.plugins[name];
        if is_registered_executable(installed) {
            out.push(registered_exe_row(name, installed, check).await);
            continue;
        }
        if hub.is_none() && comm.is_none() {
            hub = crate::index::fetch_hub_index(&client).await.ok();
        }
        let entry = match hub.as_ref().and_then(|i| i.plugins.get(name)) {
            Some(e) => e,
            None => {
                if comm.is_none() {
                    comm = Some(crate::index::fetch_index(&client).await?);
                }
                match comm.as_ref().unwrap().plugins.get(name) {
                    Some(e) => e,
                    None => {
                        out.push(UpdateRow::new(
                            name,
                            UpdateAction::Skipped("not in index".into()),
                        ));
                        continue;
                    }
                }
            }
        };
        if installed.ecosystem != entry.ecosystem || !supported_index_entry(entry) {
            out.push(UpdateRow::new(
                name,
                UpdateAction::Skipped("non-index source".into()),
            ));
            continue;
        }
        let index_hash = entry.hash.primary().unwrap_or_default();
        let drift = entry.version != installed.version || index_hash != installed.hash.as_str();
        if !drift {
            out.push(UpdateRow::new(name, UpdateAction::Current));
            continue;
        }
        if check {
            out.push(UpdateRow::new(
                name,
                UpdateAction::Stale(format!(
                    "{} -> {} (index)",
                    installed.version, entry.version
                )),
            ));
            continue;
        }
        let argv = installed.argv.clone();
        let scope = Some(installed.scope.clone());
        match install_index(
            &client,
            name,
            InstallOpts { argv, scope },
            &mut BTreeSet::new(),
        )
        .await
        {
            Ok(r) => {
                let v = r.version.clone();
                out.push(UpdateRow {
                    name: r.name,
                    action: UpdateAction::Installed(v),
                    path: Some(r.path),
                });
            }
            Err(e) => out.push(UpdateRow::new(name, UpdateAction::Failed(format!("{e:#}")))),
        }
    }
    Ok(out)
}

fn registered_exe_path(e: &LockEntry) -> Option<PathBuf> {
    e.cli_argv
        .as_ref()
        .and_then(|a| a.first())
        .or_else(|| e.argv.first())
        .map(PathBuf::from)
}

fn target_repo(exe: &Path) -> Option<PathBuf> {
    let profile = exe.parent()?;
    let name = profile.file_name()?.to_str()?;
    if name != "release" && name != "debug" {
        return None;
    }
    let target = profile.parent()?;
    if target.file_name()?.to_str()? != "target" {
        return None;
    }
    let repo = target.parent()?;
    repo.join(".git").exists().then(|| repo.to_path_buf())
}

fn head_commit_secs(repo: &Path) -> Result<u64, String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["log", "-1", "--format=%ct"])
        .output()
        .map_err(|e| format!("git log: {e}"))?;
    if !out.status.success() {
        return Err(format!("git log failed in {}", repo.display()));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|e| format!("git log %ct parse: {e}"))
}

const REBUILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

fn reconcile_mtime(exe: &Path, head_secs: u64) {
    let still_old = std::fs::metadata(exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() < head_secs)
        .unwrap_or(false);
    if !still_old {
        return;
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("touch").arg(exe).status();
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::File::options()
            .write(true)
            .open(exe)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
}

fn rebuild_log(name: &str) -> Option<PathBuf> {
    let dir = crate::gray_home().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("plugin-build-{name}.log")))
}

fn rebuild_registered(name: &str, repo: &Path) -> Result<(), String> {
    use std::io::Write as _;
    let log_path = rebuild_log(name);
    let log_file = log_path.as_ref().and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut cmd = rebuild_command();
    cmd.current_dir(repo)
        .env("CARGO_BUILD_JOBS", "4")
        .stdin(std::process::Stdio::null());
    match &log_file {
        Some(f) => {
            let f2 = f.try_clone().map_err(|e| format!("log clone: {e}"))?;
            cmd.stdout(std::process::Stdio::from(f2))
                .stderr(std::process::Stdio::from(
                    f.try_clone().map_err(|e| format!("{e}"))?,
                ));
        }
        None => {
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
    }
    if let Some(mut f) = log_file {
        let _ = writeln!(f, "=== {} cargo build --release ===", crate::now_secs());
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn build: {e}"))?;
    let deadline = std::time::Instant::now() + REBUILD_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(match &log_path {
                    Some(p) => format!("cargo build failed ({status}); log: {}", p.display()),
                    None => format!("cargo build failed ({status})"),
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("build timed out (30m)".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

#[cfg(unix)]
fn rebuild_command() -> std::process::Command {
    let mut c = std::process::Command::new("nice");
    c.args([
        "-n",
        "19",
        "ionice",
        "-c3",
        "flock",
        "/tmp/cargo.lock",
        "cargo",
        "build",
        "--release",
    ]);
    c
}

#[cfg(not(unix))]
fn rebuild_command() -> std::process::Command {
    let mut c = std::process::Command::new("cargo");
    c.args(["build", "--release"]);
    c
}

async fn registered_exe_row(name: &str, installed: &LockEntry, check: bool) -> UpdateRow {
    let Some(exe) = registered_exe_path(installed) else {
        return UpdateRow::new(
            name,
            UpdateAction::Skipped("registered without argv".into()),
        );
    };
    let Some(repo) = target_repo(&exe) else {
        return UpdateRow::new(
            name,
            UpdateAction::Skipped("registered executable is not a repo target/ build".into()),
        );
    };
    let head = match head_commit_secs(&repo) {
        Ok(t) => t,
        Err(e) => return UpdateRow::new(name, UpdateAction::Failed(e)),
    };
    let mtime = match std::fs::metadata(&exe).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => return UpdateRow::new(name, UpdateAction::Failed(format!("stat {exe:?}: {e}"))),
    };
    let mtime_secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if head <= mtime_secs {
        return UpdateRow::new(name, UpdateAction::Current);
    }
    if check {
        return UpdateRow::new(
            name,
            UpdateAction::Stale(format!("repo HEAD newer than binary ({})", repo.display())),
        );
    }
    let repo2 = repo.clone();
    let name2 = name.to_string();
    let built = tokio::task::spawn_blocking(move || rebuild_registered(&name2, &repo2)).await;
    match built {
        Ok(Ok(())) => {
            reconcile_mtime(&exe, head);
            UpdateRow {
                name: name.to_string(),
                action: UpdateAction::Rebuilt,
                path: Some(exe),
            }
        }
        Ok(Err(e)) => UpdateRow::new(name, UpdateAction::Failed(e)),
        Err(e) => UpdateRow::new(name, UpdateAction::Failed(format!("{e}"))),
    }
}

#[path = "ops_tests.rs"]
#[cfg(test)]
pub(crate) mod tests;
