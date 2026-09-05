//! Plugin index client: fetch over HTTPS, ETag-cache, lookup.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fetch::redact;

/// Default index URL (overridden by [`INDEX_URL_ENV`]).
pub const DEFAULT_INDEX_URL: &str = "https://gray.alignment.id/plugins/index.json";
/// Env var overriding the index URL (tests point this at loopback).
pub const INDEX_URL_ENV: &str = "GRAY_PLUGIN_INDEX";
/// Freshness window for the on-disk index cache.
pub const INDEX_TTL_SECS: u64 = 24 * 60 * 60;

pub fn index_url() -> String {
    std::env::var(INDEX_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub plugins: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ecosystem: String,
    pub version: String,
    pub source: Source,
    pub hash: HashSpec,
    #[serde(default)]
    pub caps: Vec<String>,
    #[serde(default)]
    pub adapter_min: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "type")]
    pub type_: String,
    pub url: String,
    #[serde(default)]
    pub subdir: Option<String>,
}

/// Index hash: either `"sha256:<hex>"` or a per-target map.
/// Per-target selection lands later; until then [`HashSpec::primary`] wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HashSpec {
    Single(String),
    PerTarget(BTreeMap<String, String>),
}

impl HashSpec {
    /// Primary hash string: the single value, or the first map entry.
    pub fn primary(&self) -> Option<&str> {
        match self {
            HashSpec::Single(s) => Some(s.as_str()),
            HashSpec::PerTarget(m) => m.values().next().map(|s| s.as_str()),
        }
    }
}

/// Look up a plugin by name. Miss message is part of the CLI contract.
pub fn lookup<'a>(index: &'a Index, name: &str) -> anyhow::Result<&'a Entry> {
    index
        .plugins
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("not in index: {name} (try /plugin install <https-url>)"))
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexCache {
    etag: Option<String>,
    fetched_at: u64,
    index: Index,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> PathBuf {
    crate::plugins_dir().join("index-cache.json")
}

fn read_cache() -> anyhow::Result<IndexCache> {
    let raw = std::fs::read_to_string(cache_path())?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_cache(cache: &IndexCache) -> anyhow::Result<()> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string(cache)?)?;
    Ok(())
}

/// Fetch the index, using the on-disk cache when fresh (<24h) and
/// revalidating with `If-None-Match` otherwise (304 keeps the cache).
pub async fn fetch_index(client: &reqwest::Client) -> anyhow::Result<Index> {
    let url = index_url();
    let cached = read_cache().ok();
    if let Some(c) = &cached
        && now_secs().saturating_sub(c.fetched_at) < INDEX_TTL_SECS
    {
        return Ok(c.index.clone());
    }
    let mut req = client.get(&url);
    if let Some(etag) = cached.as_ref().and_then(|c| c.etag.as_deref()) {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    log::debug!("fetching plugin index from {}", redact(&url));
    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        let mut cache =
            cached.ok_or_else(|| anyhow::anyhow!("index revalidated but no cache present"))?;
        cache.fetched_at = now_secs();
        let index = cache.index.clone();
        write_cache(&cache)?;
        return Ok(index);
    }
    let resp = resp.error_for_status()?;
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let index: Index = resp.json().await?;
    write_cache(&IndexCache {
        etag,
        fetched_at: now_secs(),
        index: index.clone(),
    })?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miss_error_is_exact() {
        let index = Index {
            schema: 1,
            generated: String::new(),
            plugins: BTreeMap::new(),
        };
        let err = lookup(&index, "foo").unwrap_err().to_string();
        assert_eq!(err, "not in index: foo (try /plugin install <https-url>)");
    }

    #[test]
    fn hash_spec_parses_string_or_map() {
        let s: HashSpec = serde_json::from_str(r#""sha256:abc""#).unwrap();
        assert_eq!(s.primary(), Some("sha256:abc"));
        let m: HashSpec = serde_json::from_str(r#"{"x86_64-linux": "sha256:def"}"#).unwrap();
        assert_eq!(m.primary(), Some("sha256:def"));
    }
}
