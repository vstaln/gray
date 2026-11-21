use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gray_core::credential::{
    ACCOUNT_METADATA, API_KEY_SECRET, CredentialError, CredentialFailure, CredentialLease,
    CredentialSource, SecretMap,
};
use tokio::sync::Mutex;

use super::store::CredentialStore;
use crate::setup::catalog::{self, AuthEntry, KeyPool};

/// How long a failure parks a key when the provider named no window.
const RATE_LIMIT_COOLDOWN_SECS: u64 = 60;
const QUOTA_COOLDOWN_SECS: u64 = 3600;
const UNAUTHORIZED_COOLDOWN_SECS: u64 = 24 * 3600;

/// Serves one API key per request out of a provider's `auth.json` key pool:
/// acquire picks the currently serving key under the shared pool rule and
/// stamps `last_used_at`, report parks a key that failed so the next
/// acquire moves on to another.
pub struct KeyPoolSource {
    provider_id: String,
    store: CredentialStore,
    /// Serialises the read-modify-write of one pool entry.
    write: Mutex<()>,
}

impl KeyPoolSource {
    pub fn new(provider_id: impl Into<String>, store: CredentialStore) -> Arc<Self> {
        Arc::new(Self {
            provider_id: provider_id.into(),
            store,
            write: Mutex::new(()),
        })
    }

    fn lease(&self, label: &str, key: &str) -> CredentialLease {
        CredentialLease {
            secrets: SecretMap::from_iter([(API_KEY_SECRET, key)]),
            metadata: std::collections::BTreeMap::from([(
                ACCOUNT_METADATA.to_string(),
                label.to_string(),
            )]),
        }
    }

    /// Seconds until the earliest cooldown ends, when every key is cooling.
    async fn reset_secs(&self) -> Option<u64> {
        let now = gray_core::time::now_secs();
        let pool = self.pool(|_| true).await;
        let earliest = pool
            .keys
            .iter()
            .map(|k| k.cooldown_until)
            .filter(|until| *until > now)
            .min()?;
        Some(earliest.saturating_sub(now))
    }

    /// The pool under this provider id, as a value: a plain string entry
    /// reads as a one-key pool.
    async fn pool(&self, keep: impl Fn(&KeyPool) -> bool) -> KeyPool {
        let entry = self
            .store
            .load()
            .ok()
            .and_then(|s| s.get(&self.provider_id).cloned());
        let pool = match entry {
            Some(AuthEntry::Key(key)) => KeyPool::from_key(&key),
            Some(AuthEntry::KeyPool(pool)) => pool,
            _ => KeyPool {
                version: 1,
                selected: None,
                strategy: String::new(),
                keys: Vec::new(),
            },
        };
        if keep(&pool) {
            pool
        } else {
            KeyPool {
                version: 1,
                selected: None,
                strategy: String::new(),
                keys: Vec::new(),
            }
        }
    }

    /// Rewrite the entry under the auth lock: `f` gets the pool as stored
    /// and returns the pool to write back. Nothing is written for a provider
    /// with no entry, so a source for an unused provider never creates one.
    async fn update<T>(&self, f: impl FnOnce(&mut KeyPool) -> Option<T>) -> Option<T> {
        let _guard = self.write.lock().await;
        let _lock = self.store.lock().ok()?;
        let mut store = self.store.load().ok()?;
        let mut pool = match store.remove(&self.provider_id)? {
            AuthEntry::Key(key) => KeyPool::from_key(&key),
            AuthEntry::KeyPool(pool) => pool,
            other => {
                store.insert(self.provider_id.clone(), other);
                return None;
            }
        };
        let out = f(&mut pool)?;
        store.insert(self.provider_id.clone(), AuthEntry::KeyPool(pool));
        self.store.replace_locked(&store).ok()?;
        Some(out)
    }
}

fn cooldown_secs(failure: CredentialFailure) -> u64 {
    match failure {
        CredentialFailure::RateLimited { retry_after } => retry_after
            .map(|d| d.as_secs())
            .unwrap_or(RATE_LIMIT_COOLDOWN_SECS),
        CredentialFailure::Quota { resets_in } => resets_in
            .map(|d| d.as_secs())
            .unwrap_or(QUOTA_COOLDOWN_SECS),
        CredentialFailure::Unauthorized => UNAUTHORIZED_COOLDOWN_SECS,
    }
}

#[async_trait]
impl CredentialSource for KeyPoolSource {
    async fn acquire(&self) -> Result<CredentialLease, CredentialError> {
        let now = gray_core::time::now_secs();
        let pool = self.pool(|_| true).await;
        if pool.keys.is_empty() {
            return Err(CredentialError::ReauthRequired(self.provider_id.clone()));
        }
        let Some(picked) = pool.pick().map(|k| (k.label.clone(), k.key.clone())) else {
            let what = format!("{} API keys", self.provider_id);
            return Err(match self.reset_secs().await {
                Some(secs) => CredentialError::cooling(&what, secs),
                None => CredentialError::Unavailable(format!("all {what} are cooling")),
            });
        };
        let (label, key) = picked;
        self.update(|pool| {
            if let Some(k) = pool.keys.iter_mut().find(|k| k.label == label) {
                k.last_used_at = now;
            }
            Some(())
        })
        .await;
        log::debug!(target: "gray_auth", "serving {} key {label}", self.provider_id);
        Ok(self.lease(&label, &key))
    }

    async fn report(&self, lease: &CredentialLease, failure: CredentialFailure) {
        let Some(label) = lease.metadata.get(ACCOUNT_METADATA).cloned() else {
            return;
        };
        let until = gray_core::time::now_secs() + cooldown_secs(failure);
        let provider = self.provider_id.clone();
        let parked = self
            .update(|pool| {
                let key = pool.keys.iter_mut().find(|k| k.label == label)?;
                key.cooldown_until = until;
                Some(())
            })
            .await;
        if parked.is_some() {
            log::info!(target: "gray_auth", "{provider} key {label} cooling: {failure:?}");
        }
    }
}

pub fn key_pool_source(provider_id: impl Into<String>, path: PathBuf) -> Arc<KeyPoolSource> {
    KeyPoolSource::new(provider_id, CredentialStore::new(path))
}

/// The auth.json a key pool lives in.
pub fn key_pool_source_for(
    provider_id: impl Into<String>,
    home: &std::path::Path,
) -> Arc<KeyPoolSource> {
    key_pool_source(provider_id, home.join("auth.json"))
}

/// Whether `provider_id` holds more than one key, the only case a pool
/// source has anything to rotate.
pub fn has_multiple_keys(provider_id: &str, path: &std::path::Path) -> bool {
    match catalog::load_mixed_store(path).remove(provider_id) {
        Some(AuthEntry::KeyPool(pool)) => pool.keys.len() > 1,
        _ => false,
    }
}
