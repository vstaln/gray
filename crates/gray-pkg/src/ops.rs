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
        }
    }
}

/// Install target: index name, https URL, or npm package.
#[derive(Debug, Clone)]
pub enum NameOrUrl {
    Name(String),
    Url(String),
    Npm {
        name: String,
        version: Option<String>,
    },
}

/// Split an `npm:<pkg>[@<version>]` body on the LAST `@` (regex-free).
/// Scoped names keep their leading `@`: `@scope/bar` is unpinned,
/// `@scope/bar@1.2.3` pins `1.2.3`. A trailing `@` (`foo@`) is unpinned.
fn parse_npm_spec(body: &str) -> NameOrUrl {
    match body.rfind('@') {
        Some(i) if i > 0 => {
            let ver = &body[i + 1..];
            if ver.is_empty() {
                NameOrUrl::Npm {
                    name: body[..i].to_string(),
                    version: None,
                }
            } else {
                NameOrUrl::Npm {
                    name: body[..i].to_string(),
                    version: Some(ver.to_string()),
                }
            }
        }
        _ => NameOrUrl::Npm {
            name: body.to_string(),
            version: None,
        },
    }
}

pub fn parse_spec(s: &str) -> NameOrUrl {
    let t = s.trim();
    if let Some(body) = t.strip_prefix("npm:") {
        return parse_npm_spec(body);
    }
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

/// Default npm registry (overridden by [`NPM_REGISTRY_ENV`]).
pub const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// Env var overriding the npm registry base URL (tests point at loopback).
pub const NPM_REGISTRY_ENV: &str = "GRAY_NPM_REGISTRY";

pub fn npm_registry_base() -> String {
    std::env::var(NPM_REGISTRY_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_NPM_REGISTRY.to_string())
}

/// Registry metadata URL for a package (`/` in scoped names is `%2F`).
fn npm_metadata_url(base: &str, name: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        name.replace('/', "%2F")
    )
}

struct NpmResolved {
    tarball: String,
    integrity: String,
    version: String,
}

async fn npm_resolve(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<NpmResolved> {
    if name.is_empty() {
        anyhow::bail!("npm package name is empty");
    }
    let url = npm_metadata_url(&npm_registry_base(), name);
    crate::fetch::check_url(&url)?;
    log::debug!(
        "resolving npm package {name} via {}",
        crate::fetch::redact(&url)
    );
    let meta: serde_json::Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let want = match version {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => meta
            .get("dist-tags")
            .and_then(|t| t.get("latest"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("npm package {name} has no latest version"))?
            .to_string(),
    };
    let dist = meta
        .get("versions")
        .and_then(|v| v.get(&want))
        .and_then(|v| v.get("dist"))
        .ok_or_else(|| anyhow::anyhow!("npm package {name} has no version {want}"))?;
    let tarball = dist
        .get("tarball")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("npm package {name}@{want} has no tarball"))?
        .to_string();
    let integrity = match dist.get("integrity").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            let shasum = dist
                .get("shasum")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("npm package {name}@{want} has no integrity"))?;
            format!("sha256:{shasum}")
        }
    };
    Ok(NpmResolved {
        tarball,
        integrity,
        version: want,
    })
}

/// Resolve `name[@version]` via registry metadata to `(tarball_url, integrity)`.
/// Unpinned resolves `dist-tags.latest`; integrity falls back to `dist.shasum`.
// In-flight (P2-2 consumer): silenced for CI -D warnings; wire up or delete.
#[allow(dead_code)]
pub(crate) async fn npm_tarball_url(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<(String, String)> {
    let r = npm_resolve(client, name, version).await?;
    Ok((r.tarball, r.integrity))
}

/// Verified + unpacked npm tarball awaiting P2-2's extractor. `dir` owns the
/// staging tempdir (deleted on drop); the lock write happens in P2-2 after
/// extraction, so a failed extract leaves no half-state.
// In-flight (P2-2 consumer): silenced for CI -D warnings; wire up or delete.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct StagedPkg {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) integrity: String,
}

pub(crate) async fn stage_npm_package(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<StagedPkg> {
    let resolved = npm_resolve(client, name, version).await?;
    let spec = crate::index::HashSpec::Single(resolved.integrity.clone());
    let archive = crate::fetch::download(client, &resolved.tarball, Some(&spec)).await?;
    let tmp_root = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_root)?;
    let dir = tempfile::tempdir_in(&tmp_root)?;
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, dir.path()) {
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    Ok(StagedPkg {
        dir,
        name: name.to_string(),
        version: resolved.version,
        integrity: resolved.integrity,
    })
}

/// `Npm` arm: resolve → verified download → staging unpack. Skill extraction
/// and the lock write land in Task P2-2; until then this bails honestly after
/// staging (the staging dir cleans itself, nothing is recorded).
async fn install_npm(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
    _opts: InstallOpts,
) -> anyhow::Result<Report> {
    let staged = stage_npm_package(client, name, version).await?;
    anyhow::bail!(
        "npm package {}@{} verified (integrity {}) but skill extraction lands in Task P2-2",
        staged.name,
        staged.version,
        staged.integrity
    )
}

pub async fn install(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let client = crate::fetch::client()?;
    match spec {
        NameOrUrl::Name(name) => install_index(&client, &name, opts).await,
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
        NameOrUrl::Npm { name, version } => {
            install_npm(&client, &name, version.as_deref(), opts).await
        }
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
    let enabled = lock.plugins.get(name).map(|e| e.enabled).unwrap_or(true);
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
            enabled,
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
    let enabled = lock.plugins.get(&name).map(|e| e.enabled).unwrap_or(true);
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
            enabled,
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
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("not installed: {name}");
    }
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

/// Flip a plugin's `enabled` flag (boot skips disabled entries).
/// Miss message matches `remove`.
pub fn set_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    let mut lock = read_lock()?.unwrap_or_default();
    let Some(entry) = lock.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    entry.enabled = on;
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
    #![allow(clippy::await_holding_lock)]
    use super::*;

    // Serializes the process-global GRAY_HOME mutation within this test
    // binary (cargo runs tests in one process on multiple threads).
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_entry_is_enabled() {
        assert!(LockEntry::default().enabled);
    }

    #[test]
    fn spec_splits_names_and_urls() {
        assert!(matches!(parse_spec("foo"), NameOrUrl::Name(_)));
        assert!(matches!(parse_spec("  foo  "), NameOrUrl::Name(_)));
        assert!(matches!(
            parse_spec("https://h/x.tar.gz"),
            NameOrUrl::Url(_)
        ));
        assert!(matches!(parse_spec("http://h/x.tar.gz"), NameOrUrl::Url(_)));
        assert_eq!(name_from_url("https://h/plugins/foo.tar.gz?x=1"), "foo");
    }

    #[test]
    fn spec_parses_npm_forms() {
        match parse_spec("npm:pi-foo") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "pi-foo");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("npm:@scope/bar@1.2.3") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "@scope/bar");
                assert_eq!(version, Some("1.2.3".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn npm_spec_edge_cases() {
        // Scoped without version stays unpinned.
        match parse_spec("npm:@scope/bar") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "@scope/bar");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // Trailing `@` is unpinned, not an empty version.
        match parse_spec("npm:foo@") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "foo");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // Pinned unscoped.
        match parse_spec("npm:pi-foo@0.2.0") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "pi-foo");
                assert_eq!(version, Some("0.2.0".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn npm_metadata_url_encodes_scope() {
        assert_eq!(
            npm_metadata_url("https://registry.npmjs.org", "@scope/bar"),
            "https://registry.npmjs.org/@scope%2Fbar"
        );
        assert_eq!(
            npm_metadata_url("http://127.0.0.1:1/", "pi-foo"),
            "http://127.0.0.1:1/pi-foo"
        );
    }

    // --- npm registry stub helpers (loopback only, no live registry) ---

    fn b64_encode(bytes: &[u8]) -> String {
        const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for ch in bytes.chunks(3) {
            let mut n: u32 = 0;
            for &b in ch {
                n = (n << 8) | u32::from(b);
            }
            n <<= 8 * (3 - ch.len());
            s.push(ALPH[((n >> 18) & 63) as usize] as char);
            s.push(ALPH[((n >> 12) & 63) as usize] as char);
            s.push(if ch.len() > 1 {
                ALPH[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            s.push(if ch.len() > 2 {
                ALPH[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        s
    }

    fn sha512_integrity(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha512::new();
        h.update(bytes);
        format!("sha512-{}", b64_encode(&h.finalize()))
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        format!("sha256:{:x}", h.finalize())
    }

    fn tiny_tgz() -> Vec<u8> {
        use std::io::Write;
        let bytes = br#"{"name":"pi-foo"}"#;
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(bytes.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_mtime(0);
        hdr.set_cksum();
        let mut ar = tar::Builder::new(Vec::new());
        ar.append_data(&mut hdr, "package/package.json", &bytes[..])
            .unwrap();
        let tar_bytes = ar.into_inner().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    async fn spawn_tarball(tgz: Vec<u8>) -> String {
        use axum::{Router, routing::get};
        let router = Router::new().route(
            "/pi-foo.tgz",
            get(move || {
                let tgz = tgz.clone();
                async move { tgz }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/pi-foo.tgz")
    }

    async fn spawn_registry(meta: serde_json::Value) -> String {
        use axum::{Json, Router, routing::get};
        let router = Router::new().route(
            "/pi-foo",
            get(move || {
                let meta = meta.clone();
                async move { Json(meta) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    /// Point `GRAY_HOME` at a fresh tempdir and the npm registry at the stub.
    /// Must be called under `ENV_GUARD`.
    fn use_npm_env(registry_base: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
            std::env::set_var(NPM_REGISTRY_ENV, registry_base);
        }
        home
    }

    #[tokio::test]
    async fn npm_resolve_picks_exact_version() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "2.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
                "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let (url, integrity) = npm_tarball_url(&client, "pi-foo", Some("1.0.0"))
            .await
            .unwrap();
        assert_eq!(url, tarball);
        assert_eq!(integrity, sha512_integrity(b"old"));
    }

    #[tokio::test]
    async fn npm_resolve_falls_back_to_latest() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "2.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
                "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let (url, integrity) = npm_tarball_url(&client, "pi-foo", None).await.unwrap();
        assert_eq!(url, tarball);
        assert_eq!(integrity, sha512_integrity(b"new"));
    }

    #[tokio::test]
    async fn npm_resolve_shasum_fallback() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let (_, integrity) = npm_tarball_url(&client, "pi-foo", None).await.unwrap();
        assert_eq!(integrity, "sha256:abc123");
    }

    #[tokio::test]
    async fn npm_resolve_missing_version_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = npm_tarball_url(&client, "pi-foo", Some("9.9.9"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("9.9.9"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn npm_resolve_missing_integrity_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = npm_tarball_url(&client, "pi-foo", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no integrity"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn stage_npm_package_downloads_verifies_unpacks() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = tiny_tgz();
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let staged = stage_npm_package(&client, "pi-foo", None).await.unwrap();
        assert_eq!(staged.name, "pi-foo");
        assert_eq!(staged.version, "1.0.0");
        assert_eq!(staged.integrity, integrity);
        assert_eq!(
            std::fs::read(staged.dir.path().join("package/package.json")).unwrap(),
            br#"{"name":"pi-foo"}"#
        );
    }

    #[tokio::test]
    async fn stage_npm_package_hash_mismatch_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"other bytes")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = stage_npm_package(&client, "pi-foo", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("hash mismatch"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn download_verifies_sha512_base64() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let body = b"plugin bytes".to_vec();
        let url = spawn_tarball(body.clone()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha512_integrity(&body));
        let path = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn download_verifies_sha256_hex() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let body = b"plugin bytes".to_vec();
        let url = spawn_tarball(body.clone()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha256_hex(&body));
        let path = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn download_rejects_sha512_mismatch() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let url = spawn_tarball(b"plugin bytes".to_vec()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha512_integrity(b"other bytes"));
        let err = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("hash mismatch"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn download_rejects_unknown_algorithm() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let url = spawn_tarball(b"plugin bytes".to_vec()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single("md5:abc".to_string());
        let err = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported hash algorithm"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn install_npm_defers_extraction_to_p2_2() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = tiny_tgz();
        let tarball = spawn_tarball(tgz.clone()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(&tgz)}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let err = install(parse_spec("npm:pi-foo"), InstallOpts::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("P2-2"), "unexpected error: {err}");
        // Nothing recorded: no half-state.
        assert!(list().unwrap().is_empty());
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
                enabled: true,
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

    #[test]
    fn lock_enabled_defaults_true_and_roundtrips_false() {
        // Old locks without `enabled` load as enabled.
        let old: LockFile = serde_json::from_str(
            r#"{"schema":1,"plugins":{"demo":{"ecosystem":"gray-native","version":"1.0.0","hash":"sha256:abc","source":"https://h/demo.tar.gz","argv":[],"adapter_version":"0.1.0","installed_at":"1","scope":"user"}}}"#,
        )
        .unwrap();
        assert!(old.plugins["demo"].enabled);
        // New locks round-trip an explicit `false`.
        let mut lock = old.clone();
        lock.plugins.get_mut("demo").unwrap().enabled = false;
        let back: LockFile = serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
        assert!(!back.plugins["demo"].enabled);
    }

    #[test]
    fn set_enabled_flips_flag_and_bails_on_miss() {
        // GRAY_HOME points at a tempdir so the real lockfile is never
        // disturbed (serialized by ENV_GUARD).
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                enabled: true,
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        set_enabled("demo", false).unwrap();
        assert!(!list().unwrap()["demo"].enabled);
        set_enabled("demo", true).unwrap();
        assert!(list().unwrap()["demo"].enabled);
        let err = set_enabled("nope", false).unwrap_err();
        assert_eq!(err.to_string(), "not installed: nope");
    }

    #[test]
    fn remove_rejects_traversal_and_empty_names() {
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        for bad in ["../evil", "a/b", "..", ""] {
            let err = remove(bad).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("not installed: {bad}"),
                "name {bad:?}"
            );
        }
        // The lock entry and the plugins dir survive the rejections.
        assert!(list().unwrap().contains_key("demo"));
    }

    #[test]
    fn remove_deletes_entry_and_dir() {
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        let dir = crate::plugins_dir().join("demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.sh"), "#!/bin/sh\n").unwrap();
        remove("demo").unwrap();
        assert!(!list().unwrap().contains_key("demo"));
        assert!(!dir.exists());
    }
}
