//! Agent/shared credential source for plugin-backed providers.

use std::sync::Arc;

use gray_core::credential::{
    CredentialEnvelope, CredentialError, CredentialLease, CredentialMaterial, SecretMap,
};
use gray_plugin::{ProviderChatRequest, ProviderRefreshRequest, ProviderRpcError};
use tokio::sync::Mutex;

use super::store::CredentialStore;
use crate::providers::registry::{InstalledProvider, ProviderRpc};

const REFRESH_MARGIN_SECS: u64 = 60;

/// Agent-lifetime source for one installed provider/auth-method pair.
pub struct PluginCredentialSource {
    installed: InstalledProvider,
    store: CredentialStore,
    rpc: Arc<dyn ProviderRpc>,
    refresh: Mutex<()>,
    /// Model id for the per-turn relay handshake (`provider/chat`); `None`
    /// preserves the stored-credential flow for providers without a relay.
    model: Option<String>,
}

/// Process-wide handle so the broker, agent, setup, and model discovery share
/// one credential source and cannot run a second rotation beside it.
pub fn shared_plugin_source(
    installed: InstalledProvider,
    store: CredentialStore,
    rpc: Arc<dyn ProviderRpc>,
) -> Arc<PluginCredentialSource> {
    shared_plugin_source_with_model(installed, store, rpc, None)
}

/// Same as [`shared_plugin_source`], plus the model id the per-turn relay
/// handshake (`provider/chat`) needs. `None` preserves the stored-credential
/// flow for providers without a relay.
pub fn shared_plugin_source_with_model(
    installed: InstalledProvider,
    store: CredentialStore,
    rpc: Arc<dyn ProviderRpc>,
    model: Option<String>,
) -> Arc<PluginCredentialSource> {
    Arc::new(PluginCredentialSource {
        installed,
        store,
        rpc,
        refresh: Mutex::new(()),
        model,
    })
}

impl PluginCredentialSource {
    pub fn installed(&self) -> &InstalledProvider {
        &self.installed
    }

    pub fn auth_ref(&self) -> String {
        format!(
            "plugin:{}:{}:{}",
            self.installed.plugin, self.installed.provider.id, self.installed.auth_method.id
        )
    }

    fn identity(&self) -> String {
        format!(
            "{}/{}",
            self.installed.provider.id, self.installed.auth_method.id
        )
    }

    fn map_error(&self, error: ProviderRpcError) -> CredentialError {
        let identity = self.identity();
        match error {
            ProviderRpcError::Rpc(failure) => match failure.code.as_str() {
                "invalid_grant" | "refresh_reuse" | "invalid_account" | "account_changed" => {
                    if let Err(error) = self.store.remove(&self.auth_ref()) {
                        return CredentialError::Store(error.to_string());
                    }
                    CredentialError::ReauthRequired(identity)
                }
                "profile_mismatch" => {
                    CredentialError::Invalid(format!("{identity} profile binding changed"))
                }
                _ => CredentialError::Unavailable(identity),
            },
            _ => CredentialError::Unavailable(identity),
        }
    }

    fn is_transient(error: &ProviderRpcError) -> bool {
        match error {
            ProviderRpcError::Rpc(failure) => !matches!(
                failure.code.as_str(),
                "invalid_grant"
                    | "refresh_reuse"
                    | "invalid_account"
                    | "account_changed"
                    | "profile_mismatch"
            ),
            ProviderRpcError::Unavailable(_) | ProviderRpcError::Protocol(_) => true,
            _ => false,
        }
    }

    async fn rotate(
        &self,
        current: CredentialEnvelope,
    ) -> Result<CredentialMaterial, CredentialError> {
        let request = ProviderRefreshRequest {
            provider: self.installed.provider.id.clone(),
            auth_method: self.installed.auth_method.id.clone(),
            profile_binding: self.installed.profile_binding.clone(),
            credential: current.clone(),
        };
        match self.rpc.refresh(request).await {
            Ok(next) => {
                if next.secrets.is_empty() {
                    return Err(CredentialError::Invalid(self.identity()));
                }
                Ok(next)
            }
            Err(error) => {
                if Self::is_transient(&error) && valid_lease(&current, 0).is_some() {
                    return Ok(current.credential.clone());
                }
                Err(self.map_error(error))
            }
        }
    }
}

#[async_trait::async_trait]
impl gray_core::credential::CredentialSource for PluginCredentialSource {
    async fn acquire(&self) -> Result<CredentialLease, CredentialError> {
        if self
            .installed
            .auth_method
            .operations
            .iter()
            .any(|operation| operation == "chat")
        {
            return self.acquire_relay().await;
        }
        let identity = self.identity();
        let auth_ref = self.auth_ref();
        let stored = self
            .store
            .read_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
            .ok_or_else(|| CredentialError::ReauthRequired(identity.clone()))?;
        if stored.plugin != self.installed.plugin
            || stored.provider != self.installed.provider.id
            || stored.auth_method != self.installed.auth_method.id
            || stored.profile_binding != self.installed.profile_binding
        {
            return Err(CredentialError::Invalid(format!(
                "{identity} profile binding changed"
            )));
        }
        if let Some(lease) = valid_lease(&stored, REFRESH_MARGIN_SECS) {
            return Ok(lease);
        }
        if stored.credential.expires_at.is_none() {
            return Ok(CredentialLease {
                secrets: stored.credential.secrets.clone(),
                metadata: stored.credential.metadata.clone(),
            });
        }
        if valid_lease(&stored, 0).is_none() {
            self.store
                .remove(&auth_ref)
                .map_err(|e| CredentialError::Store(e.to_string()))?;
            return Err(CredentialError::ReauthRequired(identity));
        }
        if !self
            .installed
            .auth_method
            .operations
            .iter()
            .any(|operation| operation == "refresh")
        {
            return Err(CredentialError::ReauthRequired(identity));
        }

        // One rotation at a time: the second caller waits, then re-reads the
        // already-rotated entry instead of calling refresh a second time.
        let _guard = self.refresh.lock().await;
        if let Some(saved) = self
            .store
            .read_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
            && let Some(lease) = valid_lease(&saved, REFRESH_MARGIN_SECS)
        {
            return Ok(lease);
        }
        let current = self
            .store
            .read_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
            .ok_or_else(|| CredentialError::ReauthRequired(identity.clone()))?;
        let next = self.rotate(current).await?;
        self.store
            .replace_plugin_if_bound(&auth_ref, &self.installed.profile_binding, next.clone())
            .map_err(|e| CredentialError::Store(e.to_string()))?;
        Ok(CredentialLease {
            secrets: next.secrets,
            metadata: next.metadata,
        })
    }
}

impl PluginCredentialSource {
    /// Per-turn relay handshake: the sidecar opens a loopback relay and hands
    /// back its URL + bearer. Core stays agnostic — a URL plus a secret,
    /// never the relay wire shape. The per-turn bearer goes in
    /// `secrets[relay_token]` (the manifest's declared `secret_name`); the
    /// URL rides in `metadata[relay_url]` for the dynamic POST override.
    /// Marker envelope proving `/connect` verified this login: a relay-only
    /// login has no stored credential, so `connect_dynamic_provider` calls this
    /// after a successful `provider/chat` and the row counts as connected.
    pub async fn mark_relay_verified(&self) -> Result<(), CredentialError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let material = CredentialMaterial {
            secrets: SecretMap::from_iter([("relay_verified", "1")]),
            metadata: [("verified_at".to_string(), now.to_string())]
                .into_iter()
                .collect(),
            expires_at: None,
        };
        let envelope = CredentialEnvelope::new(
            self.installed.plugin.clone(),
            self.installed.provider.id.clone(),
            self.installed.auth_method.id.clone(),
            self.installed.profile_binding.clone(),
            material,
        )
        .map_err(|e| CredentialError::ReauthRequired(format!("{e}")))?;
        self.store
            .put_plugin(envelope)
            .map_err(|e| CredentialError::ReauthRequired(format!("{e}")))?;
        Ok(())
    }

    async fn acquire_relay(&self) -> Result<CredentialLease, CredentialError> {
        let identity = self.identity();
        let request = ProviderChatRequest {
            provider: self.installed.provider.id.clone(),
            auth_method: self.installed.auth_method.id.clone(),
            model: self.model.clone().unwrap_or_default(),
        };
        let result = self
            .rpc
            .chat(request)
            .await
            .map_err(|e| self.map_error(e))?;
        if result.relay_url.trim().is_empty() || result.relay_token.trim().is_empty() {
            return Err(
                self.map_error(gray_plugin::ProviderRpcError::Unavailable(identity.clone()))
            );
        }
        let secret_name = self
            .installed
            .provider
            .transport
            .authorization
            .secret_name
            .clone();
        let mut secrets = std::collections::BTreeMap::new();
        secrets.insert(secret_name, result.relay_token);
        Ok(CredentialLease {
            secrets: gray_core::credential::SecretMap::from_iter(secrets),
            metadata: [("relay_url".to_string(), result.relay_url)]
                .into_iter()
                .collect(),
        })
    }
}

fn valid_lease(saved: &CredentialEnvelope, margin: u64) -> Option<CredentialLease> {
    let expires_at = saved.credential.expires_at?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if expires_at > now.saturating_add(margin) {
        Some(CredentialLease {
            secrets: saved.credential.secrets.clone(),
            metadata: saved.credential.metadata.clone(),
        })
    } else {
        None
    }
}

#[path = "broker_tests.rs"]
#[cfg(test)]
mod tests;
