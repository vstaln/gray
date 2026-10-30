use super::*;

pub(super) fn model_list_candidates(trimmed_base: &str) -> Vec<String> {
    if trimmed_base.contains("openrouter.ai") {
        vec!["https://openrouter.ai/api/v1/models".to_string()]
    } else if trimmed_base.ends_with("/v1") {
        vec![
            format!("{trimmed_base}/models"),
            format!("{trimmed_base}/tags"),
        ]
    } else {
        vec![
            format!("{trimmed_base}/models"),
            format!("{trimmed_base}/v1/models"),
            format!("{trimmed_base}/api/tags"),
            format!("{trimmed_base}/api/v1/models"),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointProbe {
    Up { models: usize },
    Auth,
    Down,
}

pub(super) async fn probe_endpoint_async(base: String) -> EndpointProbe {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return EndpointProbe::Down;
    };
    for url in model_list_candidates(base.trim_end_matches('/')) {
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        let status = resp.status();
        if status.is_success() {
            let models = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .map(|json| count_models_json(&json))
                .unwrap_or(0);
            return EndpointProbe::Up { models };
        }
        if matches!(status.as_u16(), 401 | 403) {
            return EndpointProbe::Auth;
        }
    }
    EndpointProbe::Down
}

pub(super) fn count_models_json(json: &serde_json::Value) -> usize {
    if let Some(arr) = json.as_array() {
        arr.len()
    } else if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
        arr.len()
    } else {
        json.get("models")
            .and_then(|m| m.as_array())
            .map_or(0, |a| a.len())
    }
}

/// Probe an endpoint on a detached thread, reporting over a channel —
/// callers can keep reading keys (e.g. esc to cancel) while the check is in
/// flight.
pub fn probe_custom_endpoint_detached(base_url: &str) -> std::sync::mpsc::Receiver<EndpointProbe> {
    let (tx, rx) = std::sync::mpsc::channel();
    let base = base_url.to_string();
    std::thread::spawn(move || {
        let probe = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(probe_endpoint_async(base)),
            Err(_) => EndpointProbe::Down,
        };
        let _ = tx.send(probe);
    });
    rx
}

pub struct LocalEndpointSuggestion {
    pub name: String,
    pub url: String,

    pub running: bool,
}

pub fn probe_local_endpoints() -> Vec<LocalEndpointSuggestion> {
    std::thread::scope(|scope| {
        let meta = provider_meta();
        let handles: Vec<_> = meta
            .local_endpoints
            .iter()
            .map(|ep| {
                let name = ep.name.clone();
                let url = ep.url.clone();
                scope.spawn(move || {
                    let running = (|| {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .ok()?;
                        rt.block_on(async {
                            let client = reqwest::Client::builder()
                                .timeout(std::time::Duration::from_millis(250))
                                .build()
                                .ok()?;
                            client
                                .get(format!("{url}/models"))
                                .send()
                                .await
                                .ok()
                                .map(|r| r.status().is_success())
                        })
                    })()
                    .unwrap_or(false);
                    LocalEndpointSuggestion { name, url, running }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

pub(super) async fn fetch_models_async(base: String, key: Option<String>) -> Vec<(String, String)> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(3000))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    for url in model_list_candidates(base.trim_end_matches('/')) {
        let mut req = client.get(&url);
        if let Some(k) = &key
            && !k.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        if url.contains("openrouter") {
            req = req.header("HTTP-Referer", "https://github.com/vstaln/gray");
            req = req.header("X-Title", "Gray");
        }

        if let Ok(resp) = req.send().await
            && resp.status().is_success()
            && let Ok(json) = resp.json::<serde_json::Value>().await
        {
            let mut models = Vec::new();
            let items_opt = if let Some(arr) = json.as_array() {
                Some(arr)
            } else if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                Some(arr)
            } else {
                json.get("models").and_then(|m| m.as_array())
            };

            if let Some(items) = items_opt {
                for item in items {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("model"))
                        .and_then(|v| v.as_str());
                    if let Some(id_str) = id {
                        let name = item
                            .get("name")
                            .or_else(|| item.get("display_name"))
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| friendly_model_name(id_str));
                        if let Some(len) = extract_context_length_from_json(item) {
                            cache_model_context(id_str, len);
                        }

                        if let Some(r) = item
                            .get("supported_parameters")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().any(|p| p.as_str() == Some("reasoning")))
                            .or_else(|| item.get("reasoning").and_then(|v| v.as_bool()))
                        {
                            cache_model_reasoning(id_str, r);
                        }

                        let tool_call = item
                            .get("supported_parameters")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().any(|p| p.as_str() == Some("tools")));
                        let text_only = item
                            .get("architecture")
                            .and_then(|a| a.get("output_modalities"))
                            .and_then(|v| v.as_array())
                            .map(|o| o.len() == 1 && o[0].as_str() == Some("text"));
                        if tool_call.is_some() || text_only.is_some() {
                            cache_model_meta_gap_fill(
                                id_str,
                                ModelMeta {
                                    tool_call,
                                    text_only,
                                    release_date: None,
                                    family: None,
                                },
                            );
                        }
                        models.push((id_str.to_string(), name));
                    }
                }
            }
            if !models.is_empty() {
                cache_provider_model_ids(&base, &models);
                save_provider_model_list(&base, &models);
                save_models_cache_to_disk();
                return models;
            }
        }
    }

    Vec::new()
}

pub fn fetch_live_provider_models(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let base = base_url.to_string();
    let key = api_key.map(|k| k.to_string());

    let live = std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(fetch_models_async(base, key)),
                Err(_) => Vec::new(),
            }
        })
        .join()
        .unwrap_or_default()
    });

    let Ok(home) = crate::setup::catalog::gray_home() else {
        let rows: Vec<crate::setup::variants::CachedRow> = live
            .into_iter()
            .map(|(id, name)| crate::setup::variants::CachedRow::Pair(id, name))
            .collect();
        for row in &rows {
            row.register();
        }
        return rows.iter().map(|r| r.pair()).collect();
    };
    let list_key = crate::setup::catalog::normalize_custom_base_url(base_url);
    let mut rows: Vec<crate::setup::variants::CachedRow> = live
        .into_iter()
        .map(|(id, name)| crate::setup::variants::CachedRow::Pair(id, name))
        .collect();
    crate::setup::composite_rows::extend_with_composites(&home, &list_key, &mut rows);
    for row in &rows {
        row.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}

pub(crate) fn plugin_catalog_rows(
    catalog: &gray_plugin::ProviderModelCatalog,
) -> Vec<crate::setup::variants::CachedRow> {
    for m in &catalog.models {
        if let Some(w) = m.context_window {
            cache_model_context_if_absent(&m.id, w as usize);
        }
    }
    catalog
        .models
        .iter()
        .map(|m| {
            let name = m.name.trim();
            crate::setup::variants::CachedRow::Full(crate::setup::variants::CachedModel {
                id: m.id.clone(),
                name: if name.is_empty() {
                    m.id.clone()
                } else {
                    name.to_string()
                },
                reasoning_efforts: m.reasoning_efforts.clone(),
                variants: m.variants.clone(),
                slots: m.slots.clone(),
                declared: true,
                price_cliff_tokens: m.price_cliff_tokens,
            })
        })
        .collect()
}

pub(crate) fn fetch_plugin_provider_models(
    installed: crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(plugin_models_rpc(&installed)),
                Err(_) => Vec::new(),
            }
        })
        .join()
        .unwrap_or_default()
    })
}

pub(super) async fn plugin_models_rpc(
    installed: &crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    let runtime = match crate::providers::catalog::live_runtime_for(installed) {
        Some(live) => live,
        None => match crate::providers::ProviderRuntime::start(installed.clone()).await {
            Ok(fresh) => std::sync::Arc::new(fresh),
            Err(_) => return Vec::new(),
        },
    };
    plugin_models_on(&runtime, installed).await
}

/// A plugin-only model id (absent from the LiteLLM/models.dev caches) only
/// learns its context window from the provider's own catalog, which used to
/// be fetched solely by the model picker — so bare `gray -p` sized compaction
/// off the 256k fallback. Ask the catalog before the window resolves, only on
/// a cache miss (a known model never starts the sidecar), and persist it.
pub(crate) async fn warm_plugin_model_context(config: &crate::Config) {
    let Some(model) = config.model.as_deref().filter(|m| !m.is_empty()) else {
        return;
    };
    if crate::config::offline() || super::super::known_model_context(model).is_some() {
        return;
    }
    let Some(installed) = crate::setup::catalog::gray_home()
        .ok()
        .and_then(|home| crate::providers::resolve_provider_connection(config, &home))
    else {
        return;
    };
    let started = std::time::Instant::now();
    let fetch = async {
        let runtime = crate::providers::catalog::shared_runtime(installed.clone())
            .await
            .ok()?;
        Some(plugin_models_on(&runtime, &installed).await)
    };
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(10), fetch).await;
    log::debug!(
        target: "gray_timing",
        "plugin model catalog warm model={model} found={} elapsed_ms={}",
        super::super::known_model_context(model).is_some(),
        started.elapsed().as_millis()
    );
    if !matches!(fetched, Ok(Some(_))) {
        log::warn!(target: "gray_config", "plugin model catalog unavailable; {model} context falls back to the default window");
    }
}

pub(super) async fn plugin_models_on(
    runtime: &crate::providers::ProviderRuntime,
    installed: &crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    use crate::providers::registry::ProviderRpc;
    let credential = crate::setup::catalog::auth_store_path()
        .ok()
        .and_then(|path| {
            crate::auth::CredentialStore::new(path)
                .read_plugin(&installed.auth_ref())
                .ok()
                .flatten()
        })
        .or_else(|| {
            gray_core::credential::CredentialEnvelope::new(
                installed.plugin.clone(),
                installed.provider.id.clone(),
                installed.auth_method.id.clone(),
                installed.profile_binding.clone(),
                gray_core::credential::CredentialMaterial::empty(),
            )
            .ok()
        });
    let Some(credential) = credential else {
        return Vec::new();
    };
    let request = gray_plugin::ProviderModelsRequest {
        provider: installed.provider.id.clone(),
        auth_method: installed.auth_method.id.clone(),
        profile_binding: installed.profile_binding.clone(),
        credential,
    };
    let Ok(catalog) = runtime.rpc().models(request).await else {
        return Vec::new();
    };

    let rows = plugin_catalog_rows(&catalog);
    for row in &rows {
        row.register();
    }
    save_models_cache_to_disk();
    let list_key = crate::setup::catalog::plugin_models_key(&installed.provider_id());
    if !rows.is_empty() {
        save_provider_model_rows(&list_key, rows.clone());
    }

    let mut rows = rows;
    if let Ok(home) = crate::setup::catalog::gray_home() {
        crate::setup::composite_rows::extend_with_composites(&home, &list_key, &mut rows);
    }
    if let Some(last) = rows.last() {
        last.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}
