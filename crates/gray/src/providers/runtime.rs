//! Agent/setup lifetime ownership for one provider sidecar.

use std::sync::Arc;

use async_trait::async_trait;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{
    ProviderAuthPoll, ProviderAuthStart, ProviderChatRequest, ProviderChatResult,
    ProviderModelCatalog, ProviderModelsRequest, ProviderRefreshRequest, ProviderRevokeRequest,
    ProviderRevokeResult, ProviderRpcError,
};

use super::registry::{InstalledProvider, ProviderRpc};

pub struct SidecarProviderRpc {
    installed: InstalledProvider,
    plugin: SidecarPlugin,
}

impl SidecarProviderRpc {
    pub fn new(installed: InstalledProvider, plugin: SidecarPlugin) -> Arc<Self> {
        Arc::new(Self { installed, plugin })
    }

    pub fn installed(&self) -> &InstalledProvider {
        &self.installed
    }
}

#[async_trait]
impl ProviderRpc for SidecarProviderRpc {
    async fn auth_start(
        &self,
        provider: &str,
        auth_method: &str,
    ) -> Result<ProviderAuthStart, ProviderRpcError> {
        self.plugin.provider_auth_start(provider, auth_method).await
    }

    async fn auth_poll(&self, operation_id: &str) -> Result<ProviderAuthPoll, ProviderRpcError> {
        self.plugin.provider_auth_poll(operation_id).await
    }

    async fn auth_cancel(&self, operation_id: &str) -> Result<(), ProviderRpcError> {
        self.plugin.provider_auth_cancel(operation_id).await
    }

    async fn refresh(
        &self,
        request: ProviderRefreshRequest,
    ) -> Result<gray_core::credential::CredentialMaterial, ProviderRpcError> {
        self.plugin.provider_auth_refresh(&request).await
    }

    async fn revoke(
        &self,
        request: ProviderRevokeRequest,
    ) -> Result<ProviderRevokeResult, ProviderRpcError> {
        self.plugin.provider_auth_revoke(&request).await
    }

    async fn models(
        &self,
        request: ProviderModelsRequest,
    ) -> Result<ProviderModelCatalog, ProviderRpcError> {
        self.plugin.provider_models(&request).await
    }

    async fn chat(
        &self,
        request: ProviderChatRequest,
    ) -> Result<ProviderChatResult, ProviderRpcError> {
        self.plugin.provider_chat(&request).await
    }

    async fn shutdown(&self) {
        self.plugin
            .shutdown(std::time::Duration::from_secs(2))
            .await;
    }
}

/// Starts a sidecar for setup/model discovery and drops it when this value drops.
pub struct ProviderRuntime {
    installed: InstalledProvider,
    rpc: Arc<SidecarProviderRpc>,
}

impl ProviderRuntime {
    pub async fn start(installed: InstalledProvider) -> anyhow::Result<Self> {
        let plugin = SidecarPlugin::spawn(installed.argv.clone()).await?;
        plugin.set_capabilities(vec![super::registry::PROVIDER_CAPABILITY.to_string()]);
        let rpc = SidecarProviderRpc::new(installed.clone(), plugin);
        Ok(Self { installed, rpc })
    }

    pub async fn for_agent(installed: InstalledProvider) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self::start(installed).await?))
    }

    pub fn installed(&self) -> &InstalledProvider {
        &self.installed
    }

    pub fn rpc(&self) -> Arc<SidecarProviderRpc> {
        self.rpc.clone()
    }
}

impl Drop for ProviderRuntime {
    fn drop(&mut self) {
        // `shutdown` is async; `spawn` owns a child kill-on-drop and the
        // sidecar protocol has no synchronous exit. Killing here is the
        // bounded, non-hanging behavior for agent teardown.
    }
}
