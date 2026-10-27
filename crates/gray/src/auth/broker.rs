use std::sync::Arc;

use gray_core::credential::{
    CredentialEnvelope, CredentialError, CredentialLease, CredentialMaterial, SecretMap,
};
use gray_plugin::{ProviderChatRequest, ProviderRefreshRequest, ProviderRpcError};
use tokio::sync::Mutex;

use super::store::CredentialStore;
use crate::providers::registry::{InstalledProvider, ProviderRpc};

const REFRESH_MARGIN_SECS: u64 = 60;

pub struct PluginCredentialSource {
    installed: InstalledProvider,
    store: CredentialStore,
    rpc: Arc<dyn ProviderRpc>,
    refresh: Mutex<()>,

    model: Option<String>,
}

pub fn shared_plugin_source(
    installed: InstalledProvider,
    store: CredentialStore,
    rpc: Arc<dyn ProviderRpc>,
) -> Arc<PluginCredentialSource> {
    shared_plugin_source_with_model(installed, store, rpc, None)
}

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
                _ => {
                    if failure.message.is_empty() {
                        CredentialError::Unavailable(identity)
                    } else {
                        CredentialError::Unavailable(format!("{identity}: {}", failure.message))
                    }
                }
            },
            ProviderRpcError::Unavailable(message) => {
                CredentialError::Unavailable(format!("{identity}: {message}"))
            }
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
        // The pool picks the serving account: a usable pin, else the
        // strategy over non-cooled accounts. Every account cooling is a
        // retryable "come back at <reset>" — not a missing login.
        let (account_label, stored) = match self
            .store
            .serving_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
        {
            Some(served) => served,
            None => {
                if let Ok(Some(hint)) = self.store.pool_retry_hint(&auth_ref) {
                    return Err(CredentialError::Unavailable(format!(
                        "all {identity} accounts are cooling ({hint})"
                    )));
                }
                return Err(CredentialError::ReauthRequired(identity.clone()));
            }
        };
        if stored.plugin != self.installed.plugin
            || stored.provider != self.installed.provider.id
            || stored.auth_method != self.installed.auth_method.id
            || stored.profile_binding != self.installed.profile_binding
        {
            return Err(CredentialError::Invalid(format!(
                "{identity} profile binding changed"
            )));
        }
        if let Some(mut lease) = valid_lease(&stored, REFRESH_MARGIN_SECS) {
            lease
                .metadata
                .insert("account".to_string(), account_label.clone());
            return Ok(lease);
        }
        if stored.credential.expires_at.is_none() {
            let mut metadata = stored.credential.metadata.clone();
            metadata.insert("account".to_string(), account_label.clone());
            return Ok(CredentialLease {
                secrets: stored.credential.secrets.clone(),
                metadata,
            });
        }

        if !self
            .installed
            .auth_method
            .operations
            .iter()
            .any(|operation| operation == "refresh")
        {
            if valid_lease(&stored, 0).is_none() {
                self.store
                    .remove(&auth_ref)
                    .map_err(|e| CredentialError::Store(e.to_string()))?;
            }
            return Err(CredentialError::ReauthRequired(identity));
        }

        let _guard = self.refresh.lock().await;
        if let Some((saved_label, saved)) = self
            .store
            .serving_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
            && let Some(mut lease) = valid_lease(&saved, REFRESH_MARGIN_SECS)
        {
            lease.metadata.insert("account".to_string(), saved_label);
            return Ok(lease);
        }
        let (current_label, current) = self
            .store
            .serving_plugin(&auth_ref)
            .map_err(|e| CredentialError::Store(e.to_string()))?
            .ok_or_else(|| CredentialError::ReauthRequired(identity.clone()))?;
        let next = self.rotate(current).await?;
        self.store
            .replace_plugin_if_bound(
                &auth_ref,
                &self.installed.profile_binding,
                next.clone(),
                Some(current_label),
            )
            .map_err(|e| CredentialError::Store(e.to_string()))?;
        let mut metadata = next.metadata;
        metadata.insert("account".to_string(), account_label);
        Ok(CredentialLease {
            secrets: next.secrets,
            metadata,
        })
    }
}

impl PluginCredentialSource {
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
        let result = self.rpc.chat(request).await.map_err(|e| match e {
            // The sidecar itself answered `unavailable`: it has no usable
            // login ("run agy once and complete the Google sign-in"). The
            // stored row is stale — prompt for reauth, carrying its hint.
            gray_plugin::ProviderRpcError::Rpc(failure) if failure.code == "unavailable" => {
                CredentialError::ReauthRequired(if failure.message.is_empty() {
                    identity.clone()
                } else {
                    failure.message
                })
            }
            // The request never got an answer (sidecar died mid-call, timed
            // out, was respawned): a transport failure, not a login
            // problem — surfaced as Unavailable so the turn retries.
            gray_plugin::ProviderRpcError::Unavailable(message) => {
                CredentialError::Unavailable(format!("{identity}: {message}"))
            }
            other => self.map_error(other),
        })?;
        if result.relay_url.trim().is_empty() || result.relay_token.trim().is_empty() {
            return Err(CredentialError::ReauthRequired(identity));
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
