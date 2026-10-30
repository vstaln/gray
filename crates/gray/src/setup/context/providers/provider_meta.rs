use super::*;

/// Runtime provider metadata: the connect catalog, popular presets, env-key
/// hints, local endpoints, and key-check quirks. Loaded from
/// ~/.gray/cache/providers.json (refreshed by fetch_provider_metadata) when
/// present, else the snapshot compiled in from assets/providers.json — so
/// provider/model metadata ships without a release while offline first-run
/// and /models still work.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ProviderMeta {
    #[serde(default)]
    pub catalog: crate::setup::catalog::Catalog,
    #[serde(default)]
    pub popular: Vec<PopularProvider>,
    #[serde(default)]
    pub env_keys: std::collections::BTreeMap<String, EnvKeyMeta>,
    #[serde(default)]
    pub local_endpoints: Vec<LocalEndpoint>,
    #[serde(default)]
    pub key_check: KeyCheckMeta,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PopularProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sublabel: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub no_auth: bool,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct EnvKeyMeta {
    #[serde(default)]
    pub vars: Vec<String>,
    #[serde(default)]
    pub key_url: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct LocalEndpoint {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct KeyCheckMeta {
    /// base-URL substrings with their own key-check endpoint.
    #[serde(default)]
    pub url_hosts: std::collections::BTreeMap<String, String>,
    /// hosts that take x-api-key + anthropic-version instead of a bearer token.
    #[serde(default)]
    pub anthropic_hosts: Vec<String>,
    /// hosts that answer a bad key with HTTP 400 instead of 401/403.
    #[serde(default)]
    pub reject_400_hosts: Vec<String>,
}

pub(super) const PROVIDER_META_SNAPSHOT: &str = include_str!("../../../../assets/providers.json");

pub(super) static EMBEDDED_PROVIDER_META: std::sync::OnceLock<std::sync::Arc<ProviderMeta>> =
    std::sync::OnceLock::new();

pub(super) fn embedded_provider_meta() -> std::sync::Arc<ProviderMeta> {
    std::sync::Arc::clone(EMBEDDED_PROVIDER_META.get_or_init(|| {
        std::sync::Arc::new(serde_json::from_str(PROVIDER_META_SNAPSHOT).unwrap_or_default())
    }))
}

pub(crate) fn load_provider_meta_at(path: &std::path::Path) -> Option<ProviderMeta> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(super) fn backfill(mut meta: ProviderMeta) -> Option<ProviderMeta> {
    if meta.catalog.is_empty() {
        return None;
    }
    let embedded = embedded_provider_meta();
    if meta.popular.is_empty() {
        meta.popular = embedded.popular.clone();
    }
    if meta.env_keys.is_empty() {
        meta.env_keys = embedded.env_keys.clone();
    }
    if meta.local_endpoints.is_empty() {
        meta.local_endpoints = embedded.local_endpoints.clone();
    }
    if meta.key_check.url_hosts.is_empty()
        && meta.key_check.anthropic_hosts.is_empty()
        && meta.key_check.reject_400_hosts.is_empty()
    {
        meta.key_check = embedded.key_check.clone();
    }
    Some(meta)
}

pub(super) fn provider_meta_cache_path() -> Option<std::path::PathBuf> {
    crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("cache").join("providers.json"))
}

pub(super) static PROVIDER_META: std::sync::OnceLock<
    std::sync::RwLock<std::sync::Arc<ProviderMeta>>,
> = std::sync::OnceLock::new();

pub(super) fn provider_meta_cell() -> &'static std::sync::RwLock<std::sync::Arc<ProviderMeta>> {
    PROVIDER_META.get_or_init(|| {
        // Tests keep the embedded snapshot so runs never depend on the
        // developer's real ~/.gray cache.
        let disk = if cfg!(test) {
            None
        } else {
            provider_meta_cache_path()
                .and_then(|p| load_provider_meta_at(&p))
                .and_then(backfill)
        };
        std::sync::RwLock::new(disk.map_or_else(embedded_provider_meta, std::sync::Arc::new))
    })
}

pub fn provider_meta() -> std::sync::Arc<ProviderMeta> {
    provider_meta_cell()
        .read()
        .map(|g| std::sync::Arc::clone(&g))
        .unwrap_or_else(|_| embedded_provider_meta())
}

pub(super) fn apply_provider_meta(meta: ProviderMeta) -> usize {
    let Some(meta) = backfill(meta) else {
        return 0;
    };
    let n = meta.catalog.len();
    if let Ok(mut g) = provider_meta_cell().write() {
        *g = std::sync::Arc::new(meta);
    }
    n
}

pub async fn fetch_provider_metadata() -> usize {
    let url = std::env::var("GRAY_PROVIDERS_URL")
        .unwrap_or_else(|_| "https://gray.alignment.id/models/providers.json".to_string());
    load_catalog("providers.json", &url, apply_provider_meta).await
}
