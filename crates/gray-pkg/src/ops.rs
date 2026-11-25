use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use gray_plugin::lock::{LockEntry, LockFile};

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
    pub description: Option<String>,
}

fn lock_path() -> PathBuf {
    crate::plugins_dir().join("lock.json")
}

fn read_lock() -> anyhow::Result<LockFile> {
    LockFile::load(&lock_path())
}

const REGISTRY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
    hold_registry_lock_timeout_in(&crate::plugins_dir(), REGISTRY_LOCK_TIMEOUT)
}

fn write_lock(lock: &LockFile) -> anyhow::Result<()> {
    lock.save(&lock_path())
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
    let mut lock = read_lock()?;
    let enabled = lock.plugins.get(name).map(|e| e.enabled).unwrap_or(true);
    let own_dir = crate::plugins_dir().join(name);

    let cli_argv = match lock.plugins.get(name).and_then(|e| e.cli_argv.clone()) {
        Some(prior)
            if prior
                .first()
                .is_some_and(|p| Path::new(p).starts_with(&own_dir)) =>
        {
            argv.first().cloned().map(|exe| vec![exe])
        }
        prior => prior,
    };
    let prior = lock.plugins.get(name).cloned().unwrap_or_default();
    lock.clear_tombstone(name);
    lock.plugins.insert(
        name.to_string(),
        LockEntry {
            ecosystem: ecosystem.to_string(),
            version: version.to_string(),
            hash: hash.to_string(),
            source: source.to_string(),
            argv,
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: gray_core::time::now_secs().to_string(),
            scope,
            enabled,
            cli_argv,
            global: lock.plugins.get(name).is_some_and(|e| e.global),
            removed: false,
            granted_capabilities: prior.granted_capabilities,
            capabilities_hash: prior.capabilities_hash,
            runtime_role: prior.runtime_role,
            extra: prior.extra,
        },
    );
    write_lock(&lock)
}

fn description_of(entry: &crate::index::Entry) -> Option<String> {
    let description = entry.description.trim();
    (!description.is_empty()).then(|| description.to_string())
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

fn install_identity(spec: &NameOrUrl) -> (String, String) {
    match spec {
        NameOrUrl::Name(n) => ("index".to_string(), n.clone()),
        NameOrUrl::Url(u) => ("url".to_string(), u.clone()),
    }
}

fn ecosystem_of(name: &str) -> String {
    read_lock()
        .ok()
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
        NameOrUrl::Name(name) => {
            let lookup = canonical_name(&name);
            install_index(&client, &lookup, opts, false, &mut BTreeSet::new()).await
        }
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
    }
}

fn replace_archive(
    archive: &Path,
    name: &str,
    backup: bool,
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
    if backup && had_previous {
        let bak = root.join(format!("{name}.bak"));
        if bak.symlink_metadata().is_ok() {
            std::fs::remove_dir_all(&bak)?;
        }
        std::fs::rename(&previous, &bak)?;
    }
    Ok(dest)
}

const MAX_DEP_DEPTH: usize = 8;

fn installed_entries() -> anyhow::Result<BTreeMap<String, LockEntry>> {
    let path = lock_path();
    read_lock()
        .map(|l| l.plugins)
        .map_err(|e| anyhow::anyhow!("{}: {e:#}", path.display()))
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
    backup: bool,
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
        "skill" => {
            let owner = canonical_name("skills");
            anyhow::bail!("'{name}' is a skill — `gray plugin install {owner}` adds skill support")
        }
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

    let installed = installed_entries()?;
    let mut deps = Vec::new();
    for dep in &entry.requires_plugins {
        if installed.contains_key(dep) || visited.contains(dep) {
            continue;
        }
        let report = Box::pin(install_index(client, dep, opts.clone(), backup, visited))
            .await
            .map_err(|e| anyhow::anyhow!("{name} requires plugin '{dep}': {e:#}"))?;
        deps.extend(report.deps.iter().cloned());
        deps.push(dep.clone());
    }

    let installed = installed_entries()?;
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
    let dest = replace_archive(&archive, name, backup, || {
        // Register only a dir spawn_lock_plugin can resolve; an empty or
        // wrong-platform payload must roll back instead of churning respawns.
        let argv = if opts.argv.is_empty() {
            gray_plugin::builder::resolve_argv(&crate::plugins_dir().join(name)).map_err(|e| {
                anyhow::anyhow!("package '{name}' has no runnable plugin on this platform: {e:#}")
            })?
        } else {
            opts.argv.clone()
        };
        record_install(
            name,
            &entry.ecosystem,
            &entry.version,
            entry.hash.primary().unwrap_or_default(),
            &entry.source.url,
            scope,
            argv,
        )
    })?;
    Ok(Report {
        name: name.to_string(),
        version: entry.version.clone(),
        path: dest,
        deps,
        description: description_of(&entry),
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
    let dest = replace_archive(&archive, &name, false, || {
        if opts.argv.is_empty() {
            gray_plugin::builder::resolve_argv(&crate::plugins_dir().join(&name)).map_err(|e| {
                anyhow::anyhow!("package '{name}' has no runnable plugin on this platform: {e:#}")
            })?;
        }
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
        description: None,
    })
}

pub fn list() -> anyhow::Result<BTreeMap<String, LockEntry>> {
    Ok(read_lock()?.plugins)
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
    let mut lock = read_lock()?;
    if !lock.remove(name) {
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
    let mut lock = read_lock()?;
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
    let mut lock = read_lock()?;
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
    NeedsUpdate(String),
    Failed(String),
}

pub struct UpdateRow {
    pub name: String,
    pub action: UpdateAction,
    pub path: Option<PathBuf>,
    pub description: Option<String>,
}

impl UpdateRow {
    fn new(name: &str, action: UpdateAction) -> Self {
        Self {
            name: name.to_string(),
            action,
            path: None,
            description: None,
        }
    }

    pub fn detail(&self) -> String {
        match &self.action {
            UpdateAction::Installed(v) => format!("updated to {v}"),
            UpdateAction::Rebuilt => "rebuilt from repo HEAD".to_string(),
            UpdateAction::Stale(why) => format!("stale: {why}"),
            UpdateAction::Current => "up to date".to_string(),
            UpdateAction::Skipped(why) => format!("skipped: {why}"),
            UpdateAction::NeedsUpdate(why) => format!("needs update: {why}"),
            UpdateAction::Failed(e) => format!("failed: {e}"),
        }
    }

    pub fn changed(&self) -> bool {
        matches!(
            self.action,
            UpdateAction::Installed(_) | UpdateAction::Rebuilt
        )
    }

    pub fn obstructed(&self) -> bool {
        matches!(
            self.action,
            UpdateAction::Failed(_) | UpdateAction::NeedsUpdate(_)
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

#[derive(Default)]
pub struct Renames {
    bundle: BTreeMap<String, String>,
    parts: BTreeMap<String, Vec<String>>,
}

impl Renames {
    pub fn from_index(index: &crate::index::Index) -> Self {
        let mut bundle = BTreeMap::new();
        for (name, entry) in &index.plugins {
            if !entry.superseded_by.is_empty() {
                continue;
            }
            for old in entry.supersedes.iter().filter(|o| *o != name) {
                bundle.insert(old.clone(), name.clone());
            }
        }
        for (name, entry) in &index.plugins {
            if let Some(target) = entry.superseded_by.first().filter(|t| *t != name) {
                bundle.entry(name.clone()).or_insert_with(|| target.clone());
            }
        }
        let direct = bundle.clone();
        for (old, target) in bundle.iter_mut() {
            for _ in 0..8 {
                match direct.get(target.as_str()) {
                    Some(next) if next != old => *target = next.clone(),
                    _ => break,
                }
            }
        }
        let mut parts = BTreeMap::new();
        for (old, target) in &bundle {
            let Some(entry) = index.plugins.get(target) else {
                continue;
            };
            let hit: Vec<String> = entry
                .parts
                .iter()
                .filter(|p| p.name == *old || p.aliases.iter().any(|a| a == old))
                .map(|p| p.name.clone())
                .collect();
            if !hit.is_empty() {
                parts.insert(old.clone(), hit);
            }
        }
        Renames { bundle, parts }
    }
}

static RENAMES: std::sync::RwLock<Option<std::sync::Arc<Renames>>> = std::sync::RwLock::new(None);

pub(crate) fn invalidate_renames() {
    if let Ok(mut guard) = RENAMES.write() {
        *guard = None;
    }
}

pub fn renames() -> std::sync::Arc<Renames> {
    if let Ok(guard) = RENAMES.read()
        && let Some(cached) = guard.as_ref()
    {
        return std::sync::Arc::clone(cached);
    }
    let built = std::sync::Arc::new(Renames::from_index(&crate::index::cached()));
    if !built.bundle.is_empty()
        && let Ok(mut guard) = RENAMES.write()
    {
        *guard = Some(std::sync::Arc::clone(&built));
    }
    built
}

pub fn renamed_to(old: &str) -> Option<String> {
    renames().bundle.get(old).cloned()
}

pub fn aliases_of(bundle: &str) -> Vec<String> {
    renames()
        .bundle
        .iter()
        .filter(|(_, b)| b.as_str() == bundle)
        .map(|(old, _)| old.clone())
        .collect()
}

pub fn part_of(old: &str) -> Option<(String, String)> {
    let r = renames();
    match r.parts.get(old)?.as_slice() {
        [part] => Some((r.bundle.get(old)?.clone(), part.clone())),
        _ => None,
    }
}

pub fn alias_parts(old: &str) -> Option<(String, Vec<String>)> {
    let r = renames();
    let parts = r.parts.get(old)?;
    (parts.len() > 1)
        .then(|| (r.bundle.get(old).cloned(), parts.clone()))
        .and_then(|(b, p)| Some((b?, p)))
}

pub fn alias_part_names(old: &str) -> Vec<(String, String)> {
    let r = renames();
    let Some(bundle) = r.bundle.get(old) else {
        return Vec::new();
    };
    r.parts
        .get(old)
        .map(|parts| parts.iter().map(|p| (bundle.clone(), p.clone())).collect())
        .unwrap_or_default()
}

pub fn canonical_name(name: &str) -> std::borrow::Cow<'_, str> {
    match renamed_to(name) {
        Some(target) => std::borrow::Cow::Owned(target),
        None => std::borrow::Cow::Borrowed(name),
    }
}

pub fn migrate_retired_alias(bundle: &str, old: &str, enabled: bool) {
    let overrides = crate::parts::read();
    let rows = migration_rows(old, enabled, &overrides);
    if !rows.is_empty() {
        let mut on = crate::parts::bundle(bundle);
        for (_, target, part, part_on) in rows {
            if target == bundle {
                on.entry(part).or_insert(part_on);
            }
        }
        if let Err(e) = crate::parts::replace(bundle, &on) {
            log::warn!("migrated bundle {bundle}: parts.json not written: {e:#}");
        }
    }
    if alias_parts(old).is_some()
        && let Err(e) = crate::parts::drop_bundle(old)
    {
        log::warn!("migrated bundle {bundle}: old parts key {old} not removed: {e:#}");
    }
}

fn migration_rows(
    name: &str,
    enabled: bool,
    overrides: &crate::parts::Overrides,
) -> Vec<(String, String, String, bool)> {
    let old = overrides.get(name);
    alias_part_names(name)
        .into_iter()
        .map(|(bundle, part)| {
            let on = old.and_then(|m| m.get(&part)).copied().unwrap_or(true);
            (name.to_string(), bundle, part, enabled && on)
        })
        .collect()
}

async fn update_inner(target: &str, check: bool) -> anyhow::Result<Vec<UpdateRow>> {
    let lock = read_lock()?;
    let before = lock.clone();
    let overrides = crate::parts::read();
    let mut migrations: Vec<(String, String, String, bool)> = Vec::new();
    let mut retired_aliases: Vec<(String, String, bool)> = Vec::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let names: Vec<String> = if target == "all" {
        lock.plugins.keys().cloned().collect()
    } else {
        let group = canonical_name(target).into_owned();
        let mut names: Vec<String> = lock
            .plugins
            .keys()
            .filter(|k| canonical_name(k) == group.as_str())
            .cloned()
            .collect();
        if names.is_empty() {
            anyhow::bail!("not installed: {target}");
        }
        names.sort_by_key(|k| k.as_str() != group.as_str());
        names
    };
    let client = crate::fetch::client()?;
    let mut hub: Option<crate::index::Index> = None;
    let mut comm: Option<crate::index::Index> = None;
    let mut out = Vec::new();
    for name in &names {
        let installed = &lock.plugins[name];
        let mode = update_mode(name, installed);
        match &mode {
            UpdateMode::Url => {
                out.push(UpdateRow::new(
                    name,
                    UpdateAction::Skipped(
                        "installed from a URL — reinstall the same URL to refresh".into(),
                    ),
                ));
                continue;
            }
            UpdateMode::NoExecutable => {
                out.push(UpdateRow::new(
                    name,
                    UpdateAction::Skipped("registered without argv".into()),
                ));
                continue;
            }
            UpdateMode::RepoBuild { exe, repo } => {
                out.push(repo_build_row(name, exe, repo, check).await);
                continue;
            }
            UpdateMode::Index | UpdateMode::Prebuilt | UpdateMode::Foreign { .. } => {}
        }
        if hub.is_none() && comm.is_none() {
            hub = crate::index::fetch_hub_index(&client).await.ok();
        }
        let lookup = canonical_name(name).into_owned();
        let renamed = (lookup.as_str() != name.as_str()).then_some(lookup.as_str());
        if done.contains(&lookup) {
            if renamed.is_some() {
                retired_aliases.push((name.clone(), lookup.clone(), installed.enabled));
                migrations.extend(migration_rows(name, installed.enabled, &overrides));
                retire_alias(name);
            }
            continue;
        }
        let entry = match hub.as_ref().and_then(|i| i.plugins.get(&lookup)) {
            Some(e) => e,
            None => {
                if comm.is_none() {
                    comm = Some(crate::index::fetch_index(&client).await?);
                }
                match comm.as_ref().unwrap().plugins.get(&lookup) {
                    Some(e) => e,
                    None => {
                        let near = crate::index::near_names(comm.as_ref().unwrap(), &lookup);
                        out.push(UpdateRow::new(
                            name,
                            UpdateAction::Skipped(if near.is_empty() {
                                "not in index".to_string()
                            } else {
                                format!("not in index — did you mean {}?", near.join(", "))
                            }),
                        ));
                        continue;
                    }
                }
            }
        };
        let prebuilt = matches!(mode, UpdateMode::Prebuilt);
        if (!prebuilt && installed.ecosystem != entry.ecosystem) || !supported_index_entry(entry) {
            out.push(UpdateRow::new(
                name,
                UpdateAction::Skipped("non-index source".into()),
            ));
            continue;
        }
        let index_hash = entry.hash.primary().unwrap_or_default();
        let drift = renamed.is_some()
            || entry.version != installed.version
            || index_hash != installed.hash.as_str();
        if let UpdateMode::Foreign { exe } = &mode {
            out.push(if drift {
                UpdateRow::new(
                    name,
                    UpdateAction::NeedsUpdate(format!(
                        "registered executable {} is not gray's copy — reinstall it from the index to let gray update it",
                        exe.display()
                    )),
                )
            } else {
                UpdateRow::new(name, UpdateAction::Current)
            });
            continue;
        }
        if !drift {
            done.insert(lookup.clone());
            out.push(UpdateRow::new(name, UpdateAction::Current));
            continue;
        }
        if check {
            out.push(UpdateRow::new(
                name,
                UpdateAction::Stale(match renamed {
                    Some(new) => format!("renamed to {new} {} (index)", entry.version),
                    None => format!("{} -> {} (index)", installed.version, entry.version),
                }),
            ));
            continue;
        }
        let own_dir = crate::plugins_dir().join(name);
        let argv = if installed
            .argv
            .first()
            .is_some_and(|p| Path::new(p).starts_with(&own_dir))
        {
            Vec::new()
        } else {
            installed.argv.clone()
        };
        let scope = Some(installed.scope.clone());
        match install_index(
            &client,
            &lookup,
            InstallOpts { argv, scope },
            prebuilt,
            &mut BTreeSet::new(),
        )
        .await
        {
            Ok(r) => {
                if renamed.is_some() {
                    retired_aliases.push((name.clone(), lookup.clone(), installed.enabled));
                    migrations.extend(migration_rows(name, installed.enabled, &overrides));
                    retire_alias(name);
                }
                done.insert(lookup.clone());
                let v = r.version.clone();
                out.push(UpdateRow {
                    name: r.name,
                    action: UpdateAction::Installed(v),
                    path: Some(r.path),
                    description: r.description,
                });
            }
            Err(e) => out.push(UpdateRow::new(name, UpdateAction::Failed(format!("{e:#}")))),
        }
    }
    migrate_parts(&before, &migrations, &retired_aliases);
    Ok(out)
}

fn retire_alias(name: &str) {
    if let Err(e) = remove(name) {
        log::warn!("renamed plugin {name}: old entry not removed: {e:#}");
    }
    let dir = crate::plugins_dir();
    let _ = std::fs::remove_file(dir.join(format!("{name}-manifest.json")));
    let _ = std::fs::remove_file(dir.join(format!("{name}-completion.json")));
    let Some(home) = dir.parent() else {
        return;
    };
    let settings = home.join("plugin-settings");
    let old = settings.join(format!("{name}.json"));
    let new = settings.join(format!("{}.json", canonical_name(name)));
    if old.exists() && !new.exists() {
        let _ = std::fs::create_dir_all(&settings);
        let _ = std::fs::rename(&old, &new);
    }
}

fn migrate_parts(
    before: &LockFile,
    migrations: &[(String, String, String, bool)],
    retired_aliases: &[(String, String, bool)],
) {
    let mut groups: BTreeMap<&str, Vec<&(String, String, String, bool)>> = BTreeMap::new();
    for m in migrations {
        groups.entry(m.1.as_str()).or_default().push(m);
    }
    let mut retired_by_bundle: BTreeMap<&str, Vec<(&str, bool)>> = BTreeMap::new();
    for (alias, bundle, enabled) in retired_aliases {
        retired_by_bundle
            .entry(bundle.as_str())
            .or_default()
            .push((alias.as_str(), *enabled));
        groups.entry(bundle.as_str()).or_default();
    }
    let Ok(after) = read_lock() else {
        return;
    };
    for (bundle, migrated) in groups {
        if !after.plugins.contains_key(bundle) {
            continue;
        }
        let preexisting = before.plugins.contains_key(bundle);
        let mut on = crate::parts::bundle(bundle);
        for (_, _, part, enabled) in &migrated {
            if preexisting && on.contains_key(part) {
                continue;
            }
            on.insert(part.clone(), *enabled);
        }
        if !preexisting {
            for alias in aliases_of(bundle) {
                if migrated
                    .iter()
                    .any(|(a, _, _, _)| a.as_str() == alias.as_str())
                {
                    continue;
                }
                for (_, part) in alias_part_names(&alias) {
                    on.entry(part).or_insert(false);
                }
            }
            let retired_here = retired_by_bundle
                .get(bundle)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let any_on = migrated.iter().any(|(_, _, _, enabled)| *enabled)
                || retired_here.iter().any(|(alias, enabled)| {
                    *enabled && !migrated.iter().any(|(a, _, _, _)| a.as_str() == *alias)
                });
            if let Err(e) = set_enabled_inner(bundle, any_on) {
                log::warn!("migrated bundle {bundle}: could not set enabled={any_on}: {e:#}");
            }
        }
        if !on.is_empty()
            && let Err(e) = crate::parts::replace(bundle, &on)
        {
            log::warn!("migrated bundle {bundle}: parts.json not written: {e:#}");
        }
        let olds: BTreeSet<&str> = migrated
            .iter()
            .map(|(a, _, _, _)| a.as_str())
            .filter(|a| alias_parts(a).is_some())
            .collect();
        for old in olds {
            if let Err(e) = crate::parts::drop_bundle(old) {
                log::warn!("migrated bundle {bundle}: old parts key {old} not removed: {e:#}");
            }
        }
    }
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
    let dir = gray_core::paths::gray_home_or_temp().join("logs");
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
        let _ = writeln!(
            f,
            "=== {} cargo build --release ===",
            gray_core::time::now_secs()
        );
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateMode {
    Index,
    Prebuilt,
    RepoBuild { exe: PathBuf, repo: PathBuf },
    Foreign { exe: PathBuf },
    NoExecutable,
    Url,
}

fn update_mode(name: &str, e: &LockEntry) -> UpdateMode {
    if e.ecosystem == "url" {
        return UpdateMode::Url;
    }
    if !is_registered_executable(e) {
        return UpdateMode::Index;
    }
    let Some(exe) = registered_exe_path(e) else {
        return UpdateMode::NoExecutable;
    };
    if exe.starts_with(crate::plugins_dir().join(name)) {
        return UpdateMode::Prebuilt;
    }
    match target_repo(&exe) {
        Some(repo) => UpdateMode::RepoBuild { exe, repo },
        None => UpdateMode::Foreign { exe },
    }
}

async fn repo_build_row(name: &str, exe: &Path, repo: &Path, check: bool) -> UpdateRow {
    let head = match head_commit_secs(repo) {
        Ok(t) => t,
        Err(e) => return UpdateRow::new(name, UpdateAction::Failed(e)),
    };
    let mtime = match std::fs::metadata(exe).and_then(|m| m.modified()) {
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
    let repo2 = repo.to_path_buf();
    let name2 = name.to_string();
    let built = tokio::task::spawn_blocking(move || rebuild_registered(&name2, &repo2)).await;
    match built {
        Ok(Ok(())) => {
            reconcile_mtime(exe, head);
            UpdateRow {
                name: name.to_string(),
                action: UpdateAction::Rebuilt,
                path: Some(exe.to_path_buf()),
                description: None,
            }
        }
        Ok(Err(e)) => UpdateRow::new(name, UpdateAction::Failed(e)),
        Err(e) => UpdateRow::new(name, UpdateAction::Failed(format!("{e}"))),
    }
}
