//! Plugin-backed provider resolution, dynamic provider construction, and model discovery.

use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;
use gray_core::credential::CredentialSource;
use gray_plugin::{
    ProviderHeaderDecl, ProviderHeaderSourceDecl, ProviderModel, ProviderModelsRequest,
};
use gray_provider::{
    OpenAiAuthorization, OpenAiHeader, OpenAiHeaderSource, OpenAiProviderProfile,
    OpenAiRequestPolicy, OpenAiWire,
};

use crate::auth::{CredentialStore, shared_plugin_source_with_model};
use crate::config::Config;
use crate::providers::registry::ProviderRpc;
use crate::providers::registry::{InstalledProvider, ProviderRegistry};
use crate::providers::runtime::ProviderRuntime;

/// One dynamic provider: the declared profile plus the credential source the
/// agent, setup modal, and background refreshes all share.
pub struct DynamicProvider {
    installed: InstalledProvider,
    profile: OpenAiProviderProfile,
    source: Arc<dyn CredentialSource>,
    _runtime: Arc<ProviderRuntime>,
}

impl DynamicProvider {
    pub fn installed(&self) -> &InstalledProvider {
        &self.installed
    }

    pub fn profile(&self) -> &OpenAiProviderProfile {
        &self.profile
    }

    pub fn credential_source(&self) -> Arc<dyn CredentialSource> {
        self.source.clone()
    }
}

/// Resolve the active config to one installed provider/auth-method pair.
pub fn resolve_provider_connection(config: &Config, home: &Path) -> Option<InstalledProvider> {
    if !config.uses_plugin_credentials() {
        return None;
    }
    ProviderRegistry::load_cached(home).resolve(&config.provider_id, &config.auth_ref)
}

/// Start the provider sidecar and build its dynamic credential source.
pub async fn connect_dynamic_provider(
    config: &Config,
    home: &Path,
) -> anyhow::Result<DynamicProvider> {
    let installed = resolve_provider_connection(config, home).ok_or_else(|| {
        anyhow!(
            "selected provider connection is not installed or enabled: {}",
            config.provider_id
        )
    })?;
    let profile = profile_for_provider(&installed)?;
    let runtime = shared_runtime(installed.clone()).await?;
    let store = CredentialStore::new(home.join("auth.json"));
    let source: Arc<dyn CredentialSource> = shared_plugin_source_with_model(
        installed.clone(),
        store,
        Arc::clone(&runtime.rpc()) as Arc<dyn ProviderRpc>,
        config.model.clone(),
    );
    Ok(DynamicProvider {
        installed,
        profile,
        source,
        _runtime: runtime,
    })
}

/// The provider sidecar of the live connection. A model switch on the same
/// provider and auth method reuses it: each fresh spawn waited for its
/// manifest before the switch returned, and handed the new model a cold
/// process whose first chat could stall.
static LIVE_RUNTIME: std::sync::Mutex<Option<Arc<ProviderRuntime>>> = std::sync::Mutex::new(None);

async fn shared_runtime(installed: InstalledProvider) -> anyhow::Result<Arc<ProviderRuntime>> {
    let live = LIVE_RUNTIME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(live) = live
        && same_connection(live.installed(), &installed)
    {
        log::debug!(target: "gray_timing", "provider sidecar reused plugin={}", installed.plugin);
        return Ok(live);
    }
    let started = std::time::Instant::now();
    let fresh = Arc::new(ProviderRuntime::start(installed).await?);
    log::debug!(target: "gray_timing", "provider sidecar start elapsed_ms={}", started.elapsed().as_millis());
    // Replacing the slot drops the previous runtime, which kills its child.
    *LIVE_RUNTIME.lock().unwrap_or_else(|e| e.into_inner()) = Some(fresh.clone());
    Ok(fresh)
}

/// Same sidecar identity: the process would answer the same provider.
fn same_connection(a: &InstalledProvider, b: &InstalledProvider) -> bool {
    a.plugin == b.plugin
        && a.provider.id == b.provider.id
        && a.auth_method.id == b.auth_method.id
        && a.profile_binding == b.profile_binding
        && a.argv == b.argv
}

pub fn profile_for_provider(
    installed: &InstalledProvider,
) -> anyhow::Result<OpenAiProviderProfile> {
    let transport = &installed.provider.transport;
    anyhow::ensure!(
        transport.kind == "openai-responses",
        "provider transport {} is not an OpenAI Responses endpoint",
        transport.kind
    );
    anyhow::ensure!(
        transport.authorization.kind == "bearer",
        "provider authorization {} is not a bearer credential",
        transport.authorization.kind
    );
    anyhow::ensure!(
        matches!(transport.base_url.scheme(), "https" | "http"),
        "provider base URL must use HTTP or HTTPS"
    );
    let authorization = OpenAiAuthorization::Bearer {
        secret_name: transport.authorization.secret_name.clone(),
    };
    let headers = transport
        .headers
        .iter()
        .filter_map(declared_header)
        .collect::<Vec<_>>();
    let request = &transport.request;
    let profile = OpenAiProviderProfile {
        base_url: transport.base_url.clone(),
        wire: OpenAiWire::Responses,
        authorization,
        headers,
        request: OpenAiRequestPolicy {
            prompt_cache_key: request.prompt_cache_key,
            store: request.store,
            include_reasoning_encrypted: request.include_reasoning_encrypted,
            previous_response_id: request.previous_response_id,
            tool_choice: request.tool_choice.clone(),
            parallel_tool_calls: request.parallel_tool_calls,
            text_verbosity: request.text_verbosity.clone(),
        },
        follow_redirects: false,
    };
    Ok(profile)
}

fn declared_header(header: &ProviderHeaderDecl) -> Option<OpenAiHeader> {
    let source = if let Some(value) = &header.value {
        Some(OpenAiHeaderSource::Static(value.clone()))
    } else {
        match header.source.as_ref() {
            Some(ProviderHeaderSourceDecl::Static { value }) => {
                Some(OpenAiHeaderSource::Static(value.clone()))
            }
            Some(ProviderHeaderSourceDecl::Metadata { name }) => {
                Some(OpenAiHeaderSource::Metadata(name.clone()))
            }
            Some(ProviderHeaderSourceDecl::SessionId) => Some(OpenAiHeaderSource::SessionId),
            None => None,
        }
    }?;
    Some(OpenAiHeader {
        name: header.name.clone(),
        source,
        required: header.required,
    })
}

/// Plugin model discovery goes through the provider RPC with a real credential;
/// the generic HTTP catalog client never sees plugin requests.
pub async fn fetch_models_for_config(config: &Config) -> Vec<(String, String)> {
    if !config.uses_plugin_credentials() {
        return Vec::new();
    }
    let home = match crate::setup::gray_home() {
        Ok(home) => home,
        Err(_) => return Vec::new(),
    };
    let Some(installed) = resolve_provider_connection(config, &home) else {
        return Vec::new();
    };
    let Ok(runtime) = ProviderRuntime::start(installed.clone()).await else {
        return Vec::new();
    };
    // Relay providers (`chat` operation): the per-turn loopback URL only
    // exists after `provider/chat`, so discovery calls `provider/models`
    // directly with an empty envelope (the sidecar ignores it) instead of
    // going through `acquire`, which would burn a relay on a listing.
    let is_relay = installed
        .auth_method
        .operations
        .iter()
        .any(|operation| operation == "chat");
    let request = if is_relay {
        ProviderModelsRequest {
            provider: installed.provider.id.clone(),
            auth_method: installed.auth_method.id.clone(),
            profile_binding: installed.profile_binding.clone(),
            credential: gray_core::credential::CredentialEnvelope::new(
                installed.plugin.clone(),
                installed.provider.id.clone(),
                installed.auth_method.id.clone(),
                installed.profile_binding.clone(),
                gray_core::credential::CredentialMaterial::empty(),
            )
            .expect("relay models envelope"),
        }
    } else {
        let store = CredentialStore::new(home.join("auth.json"));
        let source = shared_plugin_source_with_model(
            installed.clone(),
            store.clone(),
            Arc::clone(&runtime.rpc()) as Arc<dyn ProviderRpc>,
            config.model.clone(),
        );
        if source.acquire().await.is_err() {
            return Vec::new();
        }
        let Ok(Some(credential)) = store.read_plugin(&installed.auth_ref()) else {
            return Vec::new();
        };
        ProviderModelsRequest {
            provider: installed.provider.id.clone(),
            auth_method: installed.auth_method.id.clone(),
            profile_binding: installed.profile_binding.clone(),
            credential,
        }
    };
    match runtime.rpc().models(request).await {
        Ok(catalog) => catalog
            .models
            .into_iter()
            .map(model_tuple)
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    }
}

fn model_tuple(model: ProviderModel) -> (String, String) {
    let name = if model.name.trim().is_empty() {
        model.id.clone()
    } else {
        model.name
    };
    (model.id, name)
}
