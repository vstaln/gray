//! Bounded cache of installed provider declarations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use gray_plugin::lock::LockEntry;
use gray_plugin::{
    AuthMethodDecl, Plugin, ProviderAuthPoll, ProviderAuthStart, ProviderDecl,
    ProviderModelCatalog, ProviderModelsRequest, ProviderRefreshRequest, ProviderRevokeRequest,
    ProviderRevokeResult, ProviderRpcError, SidecarPlugin,
};
use serde::{Deserialize, Serialize};

pub const PROVIDER_CAPABILITY: &str = "provider.credentials";
const PROVIDER_CACHE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledProvider {
    pub plugin: String,
    pub provider: ProviderDecl,
    pub auth_method: AuthMethodDecl,
    pub profile_binding: String,
    pub argv: Vec<String>,
}

impl InstalledProvider {
    pub fn provider_id(&self) -> String {
        format!("{}:{}", self.plugin, self.provider.id)
    }

    pub fn auth_ref(&self) -> String {
        format!(
            "plugin:{}:{}:{}",
            self.plugin, self.provider.id, self.auth_method.id
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProviderCache {
    pub schema: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, CachedProviderPlugin>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CachedProviderPlugin {
    pub identity: String,
    pub enabled: bool,
    #[serde(default)]
    pub manifest_sha256: String,
    #[serde(default)]
    pub providers: Vec<ProviderDecl>,
    #[serde(default)]
    pub errors: Vec<String>,
    /// Spawn argv from the lock; filled on load, never trusted from disk.
    #[serde(skip)]
    pub argv: Vec<String>,
}

impl ProviderCache {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .filter(|cache: &Self| cache.schema == PROVIDER_CACHE_SCHEMA)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = self.clone();
        out.schema = PROVIDER_CACHE_SCHEMA;
        let text = serde_json::to_string_pretty(&out)?;
        if let Some(parent) = path.parent() {
            let tmp = parent.join(format!(".provider-cache-{}.tmp", uuid::Uuid::new_v4()));
            std::fs::write(&tmp, &text)?;
            std::fs::rename(&tmp, path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ProviderRegistry {
    cache: ProviderCache,
}

impl ProviderRegistry {
    pub fn load_cached(home: &Path) -> Self {
        let mut cache = ProviderCache::load(&cache_path(home));
        let _ = refresh_installed_argv(home, &mut cache);
        Self { cache }
    }

    /// Rebuild the provider cache from installed sidecar manifests. A
    /// plugin without a cached manifest is command-only and contributes no
    /// providers, so this never needs to spawn a sidecar.
    /// Rebuild the provider cache from installed sidecar manifests. A
    /// plugin without a cached manifest is command-only and contributes no
    /// providers, so this never needs to spawn a sidecar.
    pub fn refresh(home: &Path) -> anyhow::Result<()> {
        let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
        let path = cache_path(home);
        let mut cache = ProviderCache::load(&path);
        let mut seen = std::collections::BTreeSet::new();
        for (name, entry) in &lock.plugins {
            let manifest_path = home.join("plugins").join(format!("{name}-manifest.json"));
            let Ok(text) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let manifest_name = manifest
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            anyhow::ensure!(
                manifest_name == name,
                "cached manifest name '{manifest_name}' does not match lock entry '{name}'"
            );
            let raw_providers = manifest
                .get("providers")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let granted = entry
                .granted_capabilities
                .iter()
                .any(|capability| capability == PROVIDER_CAPABILITY);
            let mut providers = Vec::new();
            let mut errors = Vec::new();
            if !raw_providers.is_empty() && granted {
                match serde_json::from_value::<Vec<ProviderDecl>>(serde_json::Value::Array(
                    raw_providers.clone(),
                )) {
                    Ok(declared) => providers = declared,
                    Err(_) => {
                        providers.clear();
                        errors.push("provider declarations are invalid".to_string());
                    }
                }
            } else if !raw_providers.is_empty() {
                errors.push(
                    "provider capability not granted; provider declarations hidden".to_string(),
                );
            }
            let cached = CachedProviderPlugin {
                identity: lock_identity(name, entry),
                enabled: entry.enabled,
                manifest_sha256: lock_identity(name, entry),
                providers,
                errors,
                argv: Vec::new(),
            };
            seen.insert(name.clone());
            cache.plugins.insert(name.clone(), cached);
        }
        cache.plugins.retain(|name, _| seen.contains(name));
        cache.save(&path)?;
        Ok(())
    }

    pub fn cache(&self) -> &ProviderCache {
        &self.cache
    }

    pub fn installed(&self) -> Vec<InstalledProvider> {
        let mut out = Vec::new();
        for (plugin, cached) in &self.cache.plugins {
            if !cached.enabled {
                continue;
            }
            for provider in &cached.providers {
                for method in &provider.auth_methods {
                    if let Ok(binding) = provider.profile_binding(&method.id) {
                        out.push(InstalledProvider {
                            plugin: plugin.clone(),
                            provider: provider.clone(),
                            auth_method: method.clone(),
                            profile_binding: binding,
                            argv: cached.argv.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    pub fn resolve(&self, provider_id: &str, auth_ref: &str) -> Option<InstalledProvider> {
        self.installed().into_iter().find(|installed| {
            installed.provider_id() == provider_id && installed.auth_ref() == auth_ref
        })
    }
}

pub fn cache_path(home: &Path) -> PathBuf {
    home.join("plugins/provider-cache.json")
}

/// Lock identity: package identity plus executable argv, never the raw URL.
pub fn lock_identity(plugin: &str, entry: &LockEntry) -> String {
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    use sha2::Digest as _;
    hasher.update(plugin.as_bytes());
    hasher.update(b"\n");
    hasher.update(entry.version.as_bytes());
    hasher.update(b"\n");
    hasher.update(entry.hash.as_bytes());
    hasher.update(b"\n");
    for arg in &entry.argv {
        hasher.update(arg.as_bytes());
        hasher.update(b"\0");
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn spawn_argv(plugin: &str, home: &Path, entry: &LockEntry) -> anyhow::Result<Vec<String>> {
    if !entry.argv.is_empty() {
        return Ok(entry.argv.clone());
    }
    let dir = home.join("plugins").join(plugin);
    gray_plugin::builder::resolve_argv(&dir)
}

pub async fn refresh_plugin(home: &Path, entry: &LockEntry) -> anyhow::Result<ProviderCache> {
    let plugin_name = entry_name(home, entry)?;
    let path = cache_path(home);
    let mut cache = ProviderCache::load(&path);
    let mut cached = CachedProviderPlugin {
        identity: lock_identity(&plugin_name, entry),
        enabled: entry.enabled,
        ..Default::default()
    };
    let argv = spawn_argv(&plugin_name, home, entry)?;
    let plugin = SidecarPlugin::spawn(argv).await?;
    plugin.set_capabilities(entry.granted_capabilities.clone());
    let manifest = plugin.manifest();
    plugin.shutdown(std::time::Duration::from_secs(2)).await;

    cached.manifest_sha256 = lock_identity(&plugin_name, entry);
    let granted_provider = entry
        .granted_capabilities
        .iter()
        .any(|capability| capability == PROVIDER_CAPABILITY);
    if granted_provider {
        cached.providers.extend(manifest.providers);
    } else if !manifest.providers.is_empty() {
        cached
            .errors
            .push("provider capability not granted; provider declarations hidden".to_string());
    }
    for error in manifest.provider_errors {
        cached.errors.push(error.to_string());
    }
    cache.plugins.insert(plugin_name, cached);
    cache.save(&path)?;
    let _ = refresh_installed_argv(home, &mut cache);
    Ok(cache)
}

fn entry_name(home: &Path, entry: &LockEntry) -> anyhow::Result<String> {
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    lock.plugins
        .into_iter()
        .find_map(|(name, candidate)| (candidate == *entry).then_some(name))
        .ok_or_else(|| anyhow::anyhow!("provider plugin entry is not present in the sidecar lock"))
}

fn refresh_installed_argv(home: &Path, cache: &mut ProviderCache) -> anyhow::Result<()> {
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    for (name, entry) in lock.plugins {
        if let Some(cached) = cache.plugins.get_mut(&name) {
            cached.enabled = entry.enabled;
            cached.argv = spawn_argv(&name, home, &entry).unwrap_or_default();
        }
    }
    Ok(())
}

#[async_trait]
pub trait ProviderRpc: Send + Sync {
    async fn auth_start(
        &self,
        provider: &str,
        auth_method: &str,
    ) -> Result<ProviderAuthStart, ProviderRpcError>;
    async fn auth_poll(&self, operation_id: &str) -> Result<ProviderAuthPoll, ProviderRpcError>;
    async fn auth_cancel(&self, operation_id: &str) -> Result<(), ProviderRpcError>;
    async fn refresh(
        &self,
        request: ProviderRefreshRequest,
    ) -> Result<gray_core::credential::CredentialMaterial, ProviderRpcError>;
    async fn revoke(
        &self,
        request: ProviderRevokeRequest,
    ) -> Result<ProviderRevokeResult, ProviderRpcError>;
    async fn models(
        &self,
        request: ProviderModelsRequest,
    ) -> Result<ProviderModelCatalog, ProviderRpcError>;
    async fn shutdown(&self);
}

#[path = "tests.rs"]
#[cfg(test)]
mod tests;
