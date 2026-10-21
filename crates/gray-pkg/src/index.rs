use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fetch::redact;

pub const DEFAULT_INDEX_URL: &str = "https://gray.alignment.id/plugins/index.json";

pub const INDEX_URL_ENV: &str = "GRAY_PLUGIN_INDEX";

pub const DEFAULT_HUB_INDEX_URL: &str = "https://gray.alignment.id/hub/index.json";

pub const HUB_INDEX_URL_ENV: &str = "GRAY_HUB_INDEX";

pub const INDEX_TTL_SECS: u64 = 24 * 60 * 60;

pub fn index_url() -> String {
    std::env::var(INDEX_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

pub fn hub_index_url() -> String {
    std::env::var(HUB_INDEX_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_HUB_INDEX_URL.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
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
    pub scope: String,

    #[serde(default)]
    pub kind: String,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub homepage: Option<String>,

    #[serde(default)]
    pub repo: Option<String>,

    #[serde(default)]
    pub official: bool,

    #[serde(default)]
    pub verified: bool,

    #[serde(default)]
    pub license: String,

    #[serde(default)]
    pub requires_plugins: Vec<String>,

    #[serde(default)]
    pub supersedes: Vec<String>,

    #[serde(default)]
    pub superseded_by: Vec<String>,

    #[serde(default)]
    pub conflicts: Vec<String>,

    #[serde(default)]
    pub commands: Vec<String>,
}

impl Entry {
    pub fn trusted(&self) -> bool {
        self.official || self.verified
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "type")]
    pub type_: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HashSpec {
    Single(String),
    PerTarget(BTreeMap<String, String>),
}

impl HashSpec {
    pub fn primary(&self) -> Option<&str> {
        match self {
            HashSpec::Single(s) => Some(s.as_str()),
            HashSpec::PerTarget(m) => m.values().next().map(|s| s.as_str()),
        }
    }
}

pub fn lookup<'a>(index: &'a Index, name: &str) -> anyhow::Result<&'a Entry> {
    index.plugins.get(name).ok_or_else(|| {
        anyhow::anyhow!("not in index: {name} (install by https URL or local path instead)")
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexCache {
    etag: Option<String>,
    fetched_at: u64,
    #[serde(default)]
    url: String,
    index: Index,
}

fn cache_path(file: &str) -> PathBuf {
    crate::plugins_dir().join(file)
}

fn read_cache(file: &str) -> anyhow::Result<IndexCache> {
    let raw = std::fs::read_to_string(cache_path(file))?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_cache(file: &str, cache: &IndexCache) -> anyhow::Result<()> {
    let path = cache_path(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string(cache)?)?;
    Ok(())
}

pub async fn fetch_index(client: &reqwest::Client) -> anyhow::Result<Index> {
    fetch_index_url(client, &index_url(), "index-cache.json").await
}

pub async fn fetch_index_uncached(client: &reqwest::Client) -> anyhow::Result<Index> {
    let url = index_url();
    crate::fetch::check_url(&url)?;
    let response = client.get(&url).send().await?;
    anyhow::ensure!(
        !response.status().is_redirection()
            || response.status() == reqwest::StatusCode::NOT_MODIFIED,
        "catalog redirect refused"
    );
    anyhow::ensure!(
        response.status() != reqwest::StatusCode::NOT_MODIFIED,
        "catalog returned 304 without a cache"
    );
    Ok(response.error_for_status()?.json().await?)
}

pub async fn fetch_hub_index(client: &reqwest::Client) -> anyhow::Result<Index> {
    fetch_index_url(client, &hub_index_url(), "hub-index-cache.json").await
}

async fn fetch_index_url(
    client: &reqwest::Client,
    url: &str,
    cache_file: &str,
) -> anyhow::Result<Index> {
    let cached = read_cache(cache_file).ok().filter(|c| c.url == url);
    if let Some(c) = &cached
        && crate::now_secs().saturating_sub(c.fetched_at) < INDEX_TTL_SECS
    {
        return Ok(c.index.clone());
    }
    let mut req = client.get(url);
    if let Some(etag) = cached.as_ref().and_then(|c| c.etag.as_deref()) {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    log::debug!("fetching plugin index from {}", redact(url));
    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        let mut cache =
            cached.ok_or_else(|| anyhow::anyhow!("index revalidated but no cache present"))?;
        cache.fetched_at = crate::now_secs();
        let index = cache.index.clone();
        write_cache(cache_file, &cache)?;
        return Ok(index);
    }
    let resp = resp.error_for_status()?;
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let index: Index = resp.json().await?;
    write_cache(
        cache_file,
        &IndexCache {
            etag,
            fetched_at: crate::now_secs(),
            url: url.to_string(),
            index: index.clone(),
        },
    )?;
    Ok(index)
}

#[path = "index_tests.rs"]
#[cfg(test)]
mod tests;
