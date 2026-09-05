//! Install/list/remove/update over the plugin lockfile.
//!
//! Honest but thin: only gray-native-shaped sources install until the
//! adapters land in Task 2.3.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub schema: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LockEntry>,
}

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}

/// Install target: index name or https URL.
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

impl std::str::FromStr for NameOrUrl {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(parse_spec(s))
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
    pub unverified: bool,
}

fn lock_path() -> PathBuf {
    crate::plugins_dir().join("lock.json")
}

fn now_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

fn read_lock() -> anyhow::Result<Option<LockFile>> {
    match std::fs::read_to_string(lock_path()) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_lock(lock: &LockFile) -> anyhow::Result<()> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock = lock.clone();
    lock.schema = 1;
    std::fs::write(&path, serde_json::to_string_pretty(&lock)?)?;
    Ok(())
}

fn ensure_gray_native(ecosystem: &str, type_: &str) -> anyhow::Result<()> {
    if ecosystem != "gray-native" {
        anyhow::bail!(
            "unsupported ecosystem '{ecosystem}' (only gray-native sources are installable until Task 2.3)"
        );
    }
    if type_ != "tarball" {
        anyhow::bail!(
            "unsupported source type '{type_}' (only gray-native tarballs are installable until Task 2.3)"
        );
    }
    Ok(())
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

pub async fn install(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let client = crate::fetch::client()?;
    match spec {
        NameOrUrl::Name(name) => install_index(&client, &name, opts).await,
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
    }
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    let index = crate::index::fetch_index(client).await?;
    let entry = crate::index::lookup(&index, name)?;
    ensure_gray_native(&entry.ecosystem, &entry.source.type_)?;
    let archive = crate::fetch::download(client, &entry.source.url, Some(&entry.hash)).await?;
    let dest = crate::plugins_dir().join(name);
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    let scope = if entry.scope.is_empty() {
        opts.scope.clone().unwrap_or_else(|| "user".to_string())
    } else {
        entry.scope.clone()
    };
    let mut lock = read_lock()?.unwrap_or_default();
    lock.plugins.insert(
        name.to_string(),
        LockEntry {
            ecosystem: entry.ecosystem.clone(),
            version: entry.version.clone(),
            hash: entry.hash.primary().unwrap_or_default().to_string(),
            source: entry.source.url.clone(),
            argv: opts.argv.clone(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: now_secs(),
            scope,
        },
    );
    write_lock(&lock)?;
    Ok(Report {
        name: name.to_string(),
        version: entry.version.clone(),
        path: dest,
        unverified: false,
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
    let archive = crate::fetch::download(client, url, None).await?;
    let dest = crate::plugins_dir().join(&name);
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    let mut lock = read_lock()?.unwrap_or_default();
    lock.plugins.insert(
        name.clone(),
        LockEntry {
            ecosystem: "url".to_string(),
            version: "0.0.0".to_string(),
            hash: String::new(),
            source: url.to_string(),
            argv: opts.argv.clone(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: now_secs(),
            scope: opts.scope.clone().unwrap_or_else(|| "user".to_string()),
        },
    );
    write_lock(&lock)?;
    Ok(Report {
        name,
        version: "0.0.0".to_string(),
        path: dest,
        unverified: true,
    })
}

pub fn list() -> anyhow::Result<BTreeMap<String, LockEntry>> {
    Ok(read_lock()?.map(|l| l.plugins).unwrap_or_default())
}

pub fn remove(name: &str) -> anyhow::Result<()> {
    let mut lock = read_lock()?.unwrap_or_default();
    if lock.plugins.remove(name).is_none() {
        anyhow::bail!("not installed: {name}");
    }
    let dir = crate::plugins_dir().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    write_lock(&lock)?;
    Ok(())
}

/// Update one plugin (`target` = name) or all (`target` = `"all"`).
/// Only gray-native lock entries with an index entry are considered;
/// anything else is skipped with a warning. Returns per-plugin reports
/// for the plugins that actually changed.
pub async fn update(target: &str) -> anyhow::Result<Vec<Report>> {
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
        if installed.ecosystem != "gray-native" {
            eprintln!("warning: skipping update of {name} (non-index source)");
            continue;
        }
        let entry = match crate::index::lookup(&index, name) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("warning: skipping update of {name} (not in index)");
                continue;
            }
        };
        if entry.version == installed.version {
            continue;
        }
        let argv = installed.argv.clone();
        let scope = Some(installed.scope.clone());
        out.push(install_index(&client, name, InstallOpts { argv, scope }).await?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_splits_names_and_urls() {
        assert!(matches!(parse_spec("foo"), NameOrUrl::Name(_)));
        assert!(matches!(
            parse_spec("https://h/x.tar.gz"),
            NameOrUrl::Url(_)
        ));
        assert_eq!(name_from_url("https://h/plugins/foo.tar.gz?x=1"), "foo");
    }

    #[test]
    fn lock_roundtrips_exact_shape() {
        let mut lock = LockFile {
            schema: 1,
            plugins: BTreeMap::new(),
        };
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                hash: "sha256:abc".into(),
                source: "https://h/demo.tar.gz".into(),
                argv: vec![],
                adapter_version: "0.1.0".into(),
                installed_at: "1".into(),
                scope: "user".into(),
            },
        );
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
        assert_eq!(v["schema"], 1);
        let e = &v["plugins"]["demo"];
        for k in [
            "ecosystem",
            "version",
            "hash",
            "source",
            "argv",
            "adapter_version",
            "installed_at",
            "scope",
        ] {
            assert!(e.get(k).is_some(), "missing {k}");
        }
    }
}
