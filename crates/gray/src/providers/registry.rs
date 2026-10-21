use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use gray_plugin::lock::LockEntry;
use gray_plugin::{
    AuthMethodDecl, Plugin, ProviderAuthPoll, ProviderAuthStart, ProviderChatRequest,
    ProviderChatResult, ProviderDecl, ProviderModelCatalog, ProviderModelsRequest,
    ProviderRefreshRequest, ProviderRevokeRequest, ProviderRevokeResult, ProviderRpcError,
    ProviderUsageLimits, ProviderUsageRequest, SidecarPlugin,
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
        Self::load_cached_with_user_home(home, gray_core::paths::gray_user_home().as_deref())
    }

    fn load_cached_with_user_home(home: &Path, user_home: Option<&Path>) -> Self {
        let mut cache = ProviderCache::load(&cache_path(home));
        let _ = refresh_installed_argv(home, &mut cache);
        if let Some(user) = user_home.filter(|user| differs(home, user)) {
            merge_global_entries(user, &mut cache);
        }
        Self { cache }
    }

    pub fn refresh(home: &Path) -> anyhow::Result<()> {
        let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
        let path = cache_path(home);
        let mut cache = ProviderCache::load(&path);
        let mut seen = std::collections::BTreeSet::new();
        for (name, entry) in &lock.plugins {
            seen.insert(name.clone());
            cache
                .plugins
                .insert(name.clone(), cached_from_manifest(home, name, entry));
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

pub fn plugin_home(home: &Path, plugin: &str) -> PathBuf {
    plugin_home_with_user(home, gray_core::paths::gray_user_home().as_deref(), plugin)
}

fn plugin_home_with_user(home: &Path, user_home: Option<&Path>, plugin: &str) -> PathBuf {
    let names_plugin = |dir: &Path| {
        gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(dir))
            .is_ok_and(|lock| lock.plugins.contains_key(plugin))
    };
    if names_plugin(home) {
        return home.to_path_buf();
    }
    let Some(user) = user_home.filter(|user| differs(home, user)) else {
        return home.to_path_buf();
    };
    let global = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(user))
        .ok()
        .and_then(|lock| lock.plugins.get(plugin).map(|e| e.global && e.enabled))
        .unwrap_or(false);
    if global {
        user.to_path_buf()
    } else {
        home.to_path_buf()
    }
}

pub(crate) fn differs(home: &Path, user: &Path) -> bool {
    user != home && std::fs::canonicalize(user).ok() != std::fs::canonicalize(home).ok()
}

fn merge_global_entries(user: &Path, cache: &mut ProviderCache) {
    let Ok(user_lock) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(user))
    else {
        return;
    };
    for (name, entry) in &user_lock.plugins {
        if !entry.global || !entry.enabled || cache.plugins.contains_key(name) {
            continue;
        }
        let argv = spawn_argv(name, user, entry).unwrap_or_default();
        cache.plugins.insert(
            name.clone(),
            CachedProviderPlugin {
                argv,
                ..cached_from_manifest(user, name, entry)
            },
        );
    }
}

pub fn cache_path(home: &Path) -> PathBuf {
    home.join("plugins/provider-cache.json")
}

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
    let granted_provider = provider_granted(entry, &manifest.capabilities);
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

/// Explicitly granted, or grandfathered: a plugin never asked for consent
/// gets what it declares — the rule the runtime builder and `gray plugin
/// capabilities` already apply. (Fresh index installs are never asked, so the
/// stricter check here hid devin/claude providers the runtime would allow.)
pub(crate) fn provider_granted(entry: &LockEntry, declared: &[String]) -> bool {
    entry
        .granted_capabilities
        .iter()
        .any(|capability| capability == PROVIDER_CAPABILITY)
        || gray_plugin::capabilities::granted_for(entry, declared).contains(PROVIDER_CAPABILITY)
}

fn entry_name(home: &Path, entry: &LockEntry) -> anyhow::Result<String> {
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    lock.plugins
        .into_iter()
        .find_map(|(name, candidate)| (candidate == *entry).then_some(name))
        .ok_or_else(|| anyhow::anyhow!("provider plugin entry is not present in the sidecar lock"))
}

fn cached_from_manifest(home: &Path, name: &str, entry: &LockEntry) -> CachedProviderPlugin {
    let mut cached = CachedProviderPlugin {
        identity: lock_identity(name, entry),
        enabled: entry.enabled,
        manifest_sha256: lock_identity(name, entry),
        ..Default::default()
    };
    let manifest_path = home.join("plugins").join(format!("{name}-manifest.json"));
    let Ok(text) = std::fs::read_to_string(&manifest_path) else {
        return cached;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return cached;
    };

    let raw_providers = manifest
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let declared: Vec<String> = manifest
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let granted = provider_granted(entry, &declared);
    if !raw_providers.is_empty() && granted {
        match serde_json::from_value::<Vec<ProviderDecl>>(serde_json::Value::Array(raw_providers)) {
            Ok(declared) => cached.providers = declared,
            Err(_) => cached
                .errors
                .push("provider declarations are invalid".to_string()),
        }
    } else if !raw_providers.is_empty() {
        cached
            .errors
            .push("provider capability not granted; provider declarations hidden".to_string());
    }
    cached
}

fn refresh_installed_argv(home: &Path, cache: &mut ProviderCache) -> anyhow::Result<()> {
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    for (name, entry) in &lock.plugins {
        let argv = spawn_argv(name, home, entry).unwrap_or_default();
        match cache.plugins.get_mut(name) {
            Some(cached) if cached.identity != lock_identity(name, entry) => {
                *cached = CachedProviderPlugin {
                    argv,
                    ..cached_from_manifest(home, name, entry)
                };
            }
            Some(cached) => {
                cached.enabled = entry.enabled;
                cached.argv = argv;
            }

            None => {
                cache.plugins.insert(
                    name.clone(),
                    CachedProviderPlugin {
                        argv,
                        ..cached_from_manifest(home, name, entry)
                    },
                );
            }
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
    async fn chat(
        &self,
        request: ProviderChatRequest,
    ) -> Result<ProviderChatResult, ProviderRpcError>;

    async fn usage(
        &self,
        _request: ProviderUsageRequest,
    ) -> Result<ProviderUsageLimits, ProviderRpcError> {
        Err(ProviderRpcError::Protocol(
            "provider/usage unsupported".into(),
        ))
    }
    async fn shutdown(&self);
}

#[path = "tests.rs"]
#[cfg(test)]
mod tests;
