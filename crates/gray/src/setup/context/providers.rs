use super::*;

pub fn friendly_model_name(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    let name = model_id.split('/').next_back().unwrap_or(model_id);
    let words: Vec<String> = name
        .split(['-', '_', ':'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            if lower == "gpt" || lower == "glm" || lower == "ai" || lower == "api" || lower == "swe"
            {
                w.to_uppercase()
            } else if lower.starts_with('v')
                && lower.len() > 1
                && lower[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                format!("v{}", &lower[1..])
            } else {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            }
        })
        .collect();

    let mut merged: Vec<String> = Vec::with_capacity(words.len());
    for w in words {
        let lone_digit = w.len() == 1 && w.bytes().next().is_some_and(|b| b.is_ascii_digit());
        if lone_digit
            && merged
                .last()
                .is_some_and(|prev| prev.ends_with(|c: char| c.is_ascii_digit()))
        {
            merged.last_mut().unwrap().push('.');
            merged.last_mut().unwrap().push_str(&w);
        } else {
            merged.push(w);
        }
    }
    merged.join(" ")
}

static MODEL_REASONING: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, bool>>,
> = std::sync::OnceLock::new();

fn model_reasoning_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
    MODEL_REASONING.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_reasoning(model_id: &str, reasoning: bool) {
    if let Ok(mut g) = model_reasoning_cell().write() {
        g.insert(model_id.to_string(), reasoning);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, reasoning);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            g.insert(suffix.to_string(), reasoning);
            g.insert(suffix.to_lowercase(), reasoning);
        }
    }
}

static MODEL_EFFORTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

fn model_efforts_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>
{
    MODEL_EFFORTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub(crate) fn cache_model_efforts(model_id: &str, efforts: Vec<String>) {
    if let Ok(mut g) = model_efforts_cell().write() {
        g.insert(model_id.to_string(), efforts.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, efforts.clone());
        }

        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            g.entry(suffix.to_string()).or_insert(efforts.clone());
            g.entry(suffix.to_lowercase()).or_insert(efforts);
        }
    }
}

fn model_efforts(model_id: &str) -> Option<Vec<String>> {
    let g = model_efforts_cell().read().ok()?;
    if let Some(v) = g.get(model_id).cloned() {
        return Some(v);
    }
    let lower = model_id.to_lowercase();
    if let Some(v) = g.get(&lower).cloned() {
        return Some(v);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(v) = g.get(suffix).cloned() {
            return Some(v);
        }
        if let Some(v) = g.get(&suffix.to_lowercase()).cloned() {
            return Some(v);
        }
    }
    None
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelMeta {
    pub tool_call: Option<bool>,

    pub text_only: Option<bool>,

    pub release_date: Option<String>,
    pub family: Option<String>,
}

static MODEL_META: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelMeta>>,
> = std::sync::OnceLock::new();

fn model_meta_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, ModelMeta>> {
    MODEL_META.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_meta(model_id: &str, meta: ModelMeta) {
    if let Ok(mut g) = model_meta_cell().write() {
        g.insert(model_id.to_string(), meta);
    }
}

pub(crate) fn cache_model_meta_gap_fill(model_id: &str, meta: ModelMeta) {
    if let Ok(mut g) = model_meta_cell().write() {
        let entry = g.entry(model_id.to_string()).or_default();
        if entry.tool_call.is_none() {
            entry.tool_call = meta.tool_call;
        }
        if entry.text_only.is_none() {
            entry.text_only = meta.text_only;
        }
        if entry.release_date.is_none() {
            entry.release_date = meta.release_date;
        }
        if entry.family.is_none() {
            entry.family = meta.family;
        }
    }
}

pub fn model_meta(model_id: &str) -> Option<ModelMeta> {
    let g = model_meta_cell().read().ok()?;
    if let Some(m) = g.get(model_id).cloned() {
        return Some(m);
    }
    let lower = model_id.to_lowercase();
    if let Some(m) = g.get(&lower).cloned() {
        return Some(m);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(m) = g.get(suffix).cloned() {
            return Some(m);
        }
        if let Some(m) = g.get(&suffix.to_lowercase()).cloned() {
            return Some(m);
        }
    }
    None
}

pub fn model_supports_reasoning(model_id: &str) -> Option<bool> {
    let g = model_reasoning_cell().read().ok()?;
    if let Some(v) = g.get(model_id).copied() {
        return Some(v);
    }
    let lower = model_id.to_lowercase();
    if let Some(v) = g.get(&lower).copied() {
        return Some(v);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(v) = g.get(suffix).copied() {
            return Some(v);
        }
        if let Some(v) = g.get(&suffix.to_lowercase()).copied() {
            return Some(v);
        }
    }
    None
}

pub fn supported_efforts(model_id: &str) -> Option<Vec<&'static str>> {
    if let Some(cached) = model_efforts(model_id) {
        let levels = super::super::THINKING_LEVELS;
        let want: Vec<&'static str> = cached
            .iter()
            .filter_map(|v| {
                let low = v.to_lowercase();
                let label = if low == "none" { "off" } else { low.as_str() };
                levels.iter().find(|(l, _)| *l == label).map(|(l, _)| *l)
            })
            .collect();
        if !want.is_empty() {
            return Some(want);
        }
    }

    if is_loopback_base(&active_provider_base_url()) {
        let id = model_id.to_lowercase();
        let leaf = id.rsplit('/').next().unwrap_or(id.as_str());
        let core = leaf
            .strip_suffix("-fast")
            .or_else(|| leaf.strip_suffix("-priority"))
            .unwrap_or(leaf);
        let seg = core.rsplit('-').next().unwrap_or("");
        if seg == "none" || super::super::THINKING_LEVELS.iter().any(|(l, _)| *l == seg) {
            return Some(vec![]);
        }
    }
    None
}

pub fn supported_thinking_levels(model_id: &str) -> Vec<(&'static str, &'static str)> {
    if model_supports_reasoning(model_id) == Some(false) {
        return vec![("off", "No reasoning")];
    }
    let Some(want) = supported_efforts(model_id) else {
        return super::super::THINKING_LEVELS.to_vec();
    };
    super::super::THINKING_LEVELS
        .iter()
        .filter(|(l, _)| *l == "off" || want.contains(l))
        .copied()
        .collect()
}

pub fn clamp_thinking_level(model_id: &str, level: &str) -> &'static str {
    let supported = supported_thinking_levels(model_id);
    if let Some((l, _)) = supported.iter().find(|(l, _)| *l == level) {
        return l;
    }
    let order: Vec<&str> = super::super::THINKING_LEVELS
        .iter()
        .map(|(l, _)| *l)
        .collect();
    let Some(req_idx) = order.iter().position(|l| *l == level) else {
        return supported.first().map(|(l, _)| *l).unwrap_or("off");
    };
    for cand in order.iter().skip(req_idx) {
        if let Some((l, _)) = supported.iter().find(|(l, _)| l == cand) {
            return l;
        }
    }
    for cand in order[..req_idx].iter().rev() {
        if let Some((l, _)) = supported.iter().find(|(l, _)| l == cand) {
            return l;
        }
    }
    supported.first().map(|(l, _)| *l).unwrap_or("off")
}

fn model_list_candidates(trimmed_base: &str) -> Vec<String> {
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

async fn probe_endpoint_async(base: String) -> EndpointProbe {
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

fn count_models_json(json: &serde_json::Value) -> usize {
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

pub fn probe_custom_endpoint(base_url: &str) -> EndpointProbe {
    let base = base_url.to_string();
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(probe_endpoint_async(base)),
                Err(_) => EndpointProbe::Down,
            }
        })
        .join()
        .unwrap_or(EndpointProbe::Down)
    })
}

pub struct LocalEndpointSuggestion {
    pub name: &'static str,
    pub url: &'static str,

    pub running: bool,
}

pub fn probe_local_endpoints() -> Vec<LocalEndpointSuggestion> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = crate::setup::catalog::KNOWN_LOCAL_ENDPOINTS
            .iter()
            .map(|&(name, url)| {
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

async fn fetch_models_async(base: String, key: Option<String>) -> Vec<(String, String)> {
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

async fn plugin_models_rpc(
    installed: &crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    use crate::providers::registry::ProviderRpc;
    let runtime = match crate::providers::catalog::live_runtime_for(installed) {
        Some(live) => live,
        None => match crate::providers::ProviderRuntime::start(installed.clone()).await {
            Ok(fresh) => std::sync::Arc::new(fresh),
            Err(_) => return Vec::new(),
        },
    };
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

    for m in &catalog.models {
        if let Some(w) = m.context_window {
            cache_model_context_if_absent(&m.id, w as usize);
        }
    }

    let rows: Vec<crate::setup::variants::CachedRow> = catalog
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
        .collect();
    for row in &rows {
        row.register();
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    Valid,
    Rejected(u16),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthStyle {
    Bearer,
    Anthropic,
}

const BAD_KEY_IS_400: &[&str] = &["generativelanguage.googleapis.com", "api.x.ai"];

pub(crate) fn key_check_request(base_url: &str) -> (String, AuthStyle, bool) {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.contains("openrouter.ai") {
        return (
            "https://openrouter.ai/api/v1/key".to_string(),
            AuthStyle::Bearer,
            false,
        );
    }
    let host = reqwest::Url::parse(trimmed)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let style = if host == "api.anthropic.com" {
        AuthStyle::Anthropic
    } else {
        AuthStyle::Bearer
    };
    (
        format!("{trimmed}/models"),
        style,
        BAD_KEY_IS_400.contains(&host.as_str()),
    )
}

pub(crate) fn classify_key_status(status: u16, reject_400: bool) -> KeyCheck {
    match status {
        200..=299 => KeyCheck::Valid,
        401 | 403 => KeyCheck::Rejected(status),
        400 if reject_400 => KeyCheck::Rejected(status),
        _ => KeyCheck::Unknown,
    }
}

async fn key_check_async(url: String, style: AuthStyle, reject_400: bool, key: String) -> KeyCheck {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return KeyCheck::Unknown;
    };
    let req = match style {
        AuthStyle::Bearer => client.get(&url).bearer_auth(key),
        AuthStyle::Anthropic => client
            .get(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
    };

    match req.send().await {
        Ok(resp) => classify_key_status(resp.status().as_u16(), reject_400),
        Err(_) => KeyCheck::Unknown,
    }
}

pub fn verify_api_key(base_url: &str, key: &str) -> KeyCheck {
    let (url, style, reject_400) = key_check_request(base_url);
    let key = key.to_string();
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(key_check_async(url, style, reject_400, key)),
                Err(_) => KeyCheck::Unknown,
            }
        })
        .join()
        .unwrap_or(KeyCheck::Unknown)
    })
}

static OLLAMA_SEEN: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

pub(crate) fn ollama_last_seen() -> Option<bool> {
    *OLLAMA_SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn ollama_running() -> bool {
    let up = probe_ollama();
    *OLLAMA_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(up);
    up
}

fn probe_ollama() -> bool {
    std::thread::scope(|s| {
        s.spawn(|| {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(300))
                        .build()
                    else {
                        return false;
                    };
                    client
                        .get("http://localhost:11434/api/tags")
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false)
                }),
                Err(_) => false,
            }
        })
        .join()
        .unwrap_or(false)
    })
}

static MODEL_PRICE_CLIFFS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, u32>>,
> = std::sync::OnceLock::new();

fn price_cliff_map() -> &'static std::sync::RwLock<std::collections::HashMap<String, u32>> {
    MODEL_PRICE_CLIFFS.get_or_init(Default::default)
}

pub fn cache_model_price_cliff(model_id: &str, cliff: Option<u32>) {
    if let Ok(mut m) = price_cliff_map().write() {
        match cliff {
            Some(c) => {
                m.insert(model_id.to_string(), c);
            }
            None => {
                m.remove(model_id);
            }
        }
    }
}

pub fn get_cached_model_price_cliff(model_id: &str) -> Option<u32> {
    let g = price_cliff_map().read().ok()?;
    if let Some(v) = g.get(model_id).copied() {
        return Some(v);
    }
    let lower = model_id.to_lowercase();
    if let Some(v) = g.get(&lower).copied() {
        return Some(v);
    }
    model_id
        .rsplit_once('/')
        .and_then(|(_, suffix)| g.get(suffix).copied())
}

static MODEL_CONTEXT_CACHE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, usize>>,
> = std::sync::OnceLock::new();

fn model_context_cache() -> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
    MODEL_CONTEXT_CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_context(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "live", true);
}

pub fn get_cached_model_context(model_id: &str) -> Option<usize> {
    if let Ok(g) = model_context_cache().read() {
        if let Some(v) = g.get(model_id).copied() {
            return Some(v);
        }
        let lower = model_id.to_lowercase();
        if let Some(v) = g.get(&lower).copied() {
            return Some(v);
        }
    }
    None
}

#[derive(Default)]
struct ProviderModels {
    active: String,
    ids: std::collections::HashMap<String, Vec<String>>,
}

static PROVIDER_MODELS: std::sync::OnceLock<std::sync::RwLock<ProviderModels>> =
    std::sync::OnceLock::new();

fn provider_models_cell() -> &'static std::sync::RwLock<ProviderModels> {
    PROVIDER_MODELS.get_or_init(|| std::sync::RwLock::new(ProviderModels::default()))
}

pub fn set_active_model_provider(base_url: &str) {
    if let Ok(mut cache) = provider_models_cell().write() {
        cache.active = base_url.trim_end_matches('/').to_string();
    }
}

fn active_provider_base_url() -> String {
    provider_models_cell()
        .read()
        .ok()
        .map(|cache| cache.active.clone())
        .unwrap_or_default()
}

pub(crate) fn cache_provider_model_ids(base_url: &str, models: &[(String, String)]) {
    if let Ok(mut cache) = provider_models_cell().write() {
        let mut ids: Vec<String> = models.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids.dedup();
        cache
            .ids
            .insert(base_url.trim_end_matches('/').to_string(), ids);
    }
}

pub fn cached_model_ids() -> Vec<String> {
    provider_models_cell()
        .read()
        .ok()
        .and_then(|cache| cache.ids.get(&cache.active).cloned())
        .unwrap_or_default()
}

fn is_loopback_base(base: &str) -> bool {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|host| is_loopback_host(&host))
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub(crate) fn save_provider_model_list_at(
    home: &std::path::Path,
    base_url: &str,
    models: &[(String, String)],
) {
    let rows = models
        .iter()
        .map(|(id, name)| crate::setup::variants::CachedRow::Pair(id.clone(), name.clone()))
        .collect();
    save_provider_model_rows_at(home, base_url, rows);
}

pub(crate) fn save_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    use crate::setup::variants::CachedRow;
    if rows.is_empty() {
        return;
    }
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    if base.is_empty() || is_loopback_base(&base) {
        return;
    }
    let path = home.join("provider_models.json");
    let mut map = read_model_entries(&path);
    let old = map.remove(&base).map(CacheEntry::rows).unwrap_or_default();
    let rows = rows
        .into_iter()
        .map(|row| match row {
            CachedRow::Pair(id, name) => match old.iter().find(|o| o.id() == id) {
                Some(CachedRow::Full(m)) => {
                    CachedRow::Full(crate::setup::variants::CachedModel { name, ..m.clone() })
                }
                _ => CachedRow::Pair(id, name),
            },
            full => full,
        })
        .collect();
    map.insert(
        base,
        CacheEntry::Stamped {
            fetched_at: unix_now(),
            rows,
        },
    );
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };

    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if std::fs::write(&tmp, s).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

const PROVIDER_MODELS_TTL_SECS: u64 = 30 * 24 * 3600;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum CacheEntry {
    Stamped {
        fetched_at: u64,
        rows: Vec<crate::setup::variants::CachedRow>,
    },
    Rows(Vec<crate::setup::variants::CachedRow>),
}

impl CacheEntry {
    fn rows(self) -> Vec<crate::setup::variants::CachedRow> {
        match self {
            CacheEntry::Stamped { rows, .. } | CacheEntry::Rows(rows) => rows,
        }
    }

    fn fresh(self) -> Option<Self> {
        match &self {
            CacheEntry::Stamped { fetched_at, .. }
                if unix_now().saturating_sub(*fetched_at) > PROVIDER_MODELS_TTL_SECS =>
            {
                None
            }
            _ => Some(self),
        }
    }
}

fn unix_now() -> u64 {
    u64::try_from(gray_core::spill::now_secs()).unwrap_or_default()
}

fn read_model_entries(path: &std::path::Path) -> std::collections::BTreeMap<String, CacheEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| {
            serde_json::from_str::<std::collections::BTreeMap<String, CacheEntry>>(&s).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, e)| e.fresh().map(|e| (k, e)))
        .collect()
}

fn read_model_list_map(
    path: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>> {
    read_model_entries(path)
        .into_iter()
        .map(|(k, e)| (k, e.rows()))
        .collect()
}

pub(crate) fn provider_model_lists(
    home: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>> {
    read_model_list_map(&home.join("provider_models.json"))
}

pub(crate) fn load_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
) -> Vec<crate::setup::variants::CachedRow> {
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    read_model_list_map(&home.join("provider_models.json"))
        .remove(&base)
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn load_provider_model_list_at(
    home: &std::path::Path,
    base_url: &str,
) -> Vec<(String, String)> {
    load_provider_model_rows_at(home, base_url)
        .iter()
        .map(|r| r.pair())
        .collect()
}

pub(crate) fn save_provider_model_list(base_url: &str, models: &[(String, String)]) {
    if cfg!(test) {
        return;
    }
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_list_at(&home, base_url, models);
    }
}

pub(crate) fn save_provider_model_rows(
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    if cfg!(test) {
        return;
    }
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_rows_at(&home, base_url, rows);
    }
}

pub(crate) fn load_provider_model_list(base_url: &str) -> Vec<(String, String)> {
    let Ok(home) = crate::setup::catalog::gray_home() else {
        return Vec::new();
    };
    let mut rows = load_provider_model_rows_at(&home, base_url);
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    crate::setup::composite_rows::extend_with_composites(&home, &base, &mut rows);
    for row in &rows {
        row.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}

pub fn cache_model_context_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "litellm", false);
}

pub fn cache_models_dev_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "models.dev", false);
}

fn cache_model_context_with_source(
    model_id: &str,
    length: usize,
    src: &'static str,
    overwrite: bool,
) {
    if length == 0 {
        return;
    }
    if let Ok(mut g) = model_context_cache().write()
        && (overwrite || !g.contains_key(model_id))
    {
        g.insert(model_id.to_string(), length);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            if overwrite {
                g.insert(lower, length);
            } else {
                g.entry(lower).or_insert(length);
            }
        }
    }
    record_context_source(model_id, src, overwrite);
}

static MODEL_CONTEXT_SOURCE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, &'static str>>,
> = std::sync::OnceLock::new();

fn model_context_source_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, &'static str>> {
    MODEL_CONTEXT_SOURCE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn record_context_source(model_id: &str, src: &'static str, overwrite: bool) {
    if let Ok(mut g) = model_context_source_cell().write() {
        if overwrite || !g.contains_key(model_id) {
            g.insert(model_id.to_string(), src);
        }
        let lower = model_id.to_lowercase();
        if lower != model_id && (overwrite || !g.contains_key(&lower)) {
            g.insert(lower, src);
        }
    }
}

fn get_cached_source(model_id: &str) -> Option<&'static str> {
    if let Ok(g) = model_context_source_cell().read() {
        if let Some(s) = g.get(model_id).copied() {
            return Some(s);
        }
        let lower = model_id.to_lowercase();
        if let Some(s) = g.get(&lower).copied() {
            return Some(s);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            if let Some(s) = g.get(suffix).copied() {
                return Some(s);
            }
            if let Some(s) = g.get(&suffix.to_lowercase()).copied() {
                return Some(s);
            }
        }
    }
    None
}

pub fn context_source(model: &str) -> &'static str {
    if get_user_context_window().is_some() {
        return "override";
    }
    ensure_disk_loaded();
    if let Some(s) = get_cached_source(model) {
        return s;
    }
    "guess"
}

fn json_usize(v: &serde_json::Value) -> Option<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| v.as_f64().map(|f| f as usize))
}

#[derive(Debug, Clone, Copy)]
pub struct RateTier {
    pub size: usize,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,

    pub has_cache_prices: bool,

    pub tiers: Vec<RateTier>,
}

static MODEL_RATES: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelRate>>,
> = std::sync::OnceLock::new();

fn model_rates_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, ModelRate>> {
    MODEL_RATES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

static PENDING_TIERS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>>,
> = std::sync::OnceLock::new();

fn pending_tiers_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>> {
    PENDING_TIERS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn stash_pending_tiers(model_id: &str, tiers: Vec<RateTier>) {
    if tiers.is_empty() {
        return;
    }
    if let Ok(mut g) = pending_tiers_cell().write() {
        merge_tiers(g.entry(model_id.to_string()).or_default(), tiers.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            merge_tiers(g.entry(lower).or_default(), tiers);
        }
    }
}

fn take_pending_tiers(model_id: &str) -> Vec<RateTier> {
    pending_tiers_cell()
        .write()
        .ok()
        .and_then(|mut g| g.remove(model_id))
        .unwrap_or_default()
}

fn merge_tiers(existing: &mut Vec<RateTier>, incoming: Vec<RateTier>) {
    for t in incoming {
        match existing.iter_mut().find(|e| e.size == t.size) {
            Some(e) => *e = t,
            None => existing.push(t),
        }
    }
    existing.sort_by_key(|tier| std::cmp::Reverse(tier.size));
}

fn cache_model_rate(model_id: &str, rate: ModelRate) {
    fn merge_into(
        map: &mut std::collections::HashMap<String, ModelRate>,
        id: String,
        rate: ModelRate,
    ) {
        match map.get_mut(&id) {
            Some(cur) => {
                merge_tiers(&mut cur.tiers, rate.tiers);
                let tiers = std::mem::take(&mut cur.tiers);
                *cur = ModelRate { tiers, ..rate };
            }
            None => {
                let mut rate = rate;
                merge_tiers(&mut rate.tiers, take_pending_tiers(&id));
                map.insert(id, rate);
            }
        }
    }
    if let Ok(mut g) = model_rates_cell().write() {
        merge_into(&mut g, model_id.to_string(), rate.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            merge_into(&mut g, lower, rate);
        }
    }
}

fn cache_models_dev_rate(model_id: &str, base: Option<ModelRate>, tiers: Vec<RateTier>) {
    if base.is_none() && tiers.is_empty() {
        return;
    }
    let mut ids = vec![model_id.to_string()];
    let lower = model_id.to_lowercase();
    if lower != model_id {
        ids.push(lower);
    }
    let mut stashed: Vec<String> = Vec::new();
    if let Ok(mut g) = model_rates_cell().write() {
        for id in &ids {
            match g.get_mut(id) {
                Some(cur) => {
                    let missing: Vec<RateTier> = tiers
                        .iter()
                        .copied()
                        .filter(|t| !cur.tiers.iter().any(|e| e.size == t.size))
                        .collect();
                    cur.tiers.extend(missing);
                    cur.tiers.sort_by_key(|tier| std::cmp::Reverse(tier.size));
                }
                None => {
                    if let Some(mut r) = base.clone() {
                        merge_tiers(&mut r.tiers, tiers.clone());
                        g.insert(id.clone(), r);
                    } else {
                        stashed.push(id.clone());
                    }
                }
            }
        }
    }
    for id in stashed {
        stash_pending_tiers(&id, tiers.clone());
    }
}

pub fn get_model_rate(model_id: &str) -> Option<ModelRate> {
    if let Ok(g) = model_rates_cell().read() {
        if let Some(r) = g.get(model_id).cloned() {
            return Some(r);
        }
        let lower = model_id.to_lowercase();
        if let Some(r) = g.get(&lower).cloned() {
            return Some(r);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/')
            && let Some(r) = g.get(suffix).cloned()
        {
            return Some(r);
        }
    }
    None
}

fn json_rate(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .or_else(|| v.as_u64().map(|n| n as f64))
}

pub fn turn_cost(usage: &gray_core::event::Usage, model: &str) -> Option<f64> {
    let r = get_model_rate(model)?;
    let (input, output, cache_read, cache_write, has_cache) =
        rate_at_context(&r, usage.input_tokens);
    let read = usage.cache_read_input_tokens as f64;
    let write = usage.cache_write_input_tokens as f64;
    let mut fresh = usage.non_cached_input_tokens as f64;
    if fresh == 0.0 {
        fresh = (usage.input_tokens as f64 - read - write).max(0.0);
    }
    let input_cost = if has_cache {
        fresh * input + read * cache_read + write * cache_write
    } else {
        (fresh + read + write) * input
    };
    Some(input_cost + usage.output_tokens as f64 * output)
}

pub fn uncached_cost(usage: &gray_core::event::Usage, model: &str) -> Option<f64> {
    let r = get_model_rate(model)?;
    let (input, output, ..) = rate_at_context(&r, usage.input_tokens);
    Some(usage.input_tokens as f64 * input + usage.output_tokens as f64 * output)
}

fn rate_at_context(r: &ModelRate, context: usize) -> (f64, f64, f64, f64, bool) {
    let Some(t) = r
        .tiers
        .iter()
        .filter(|t| context > t.size)
        .max_by_key(|t| t.size)
    else {
        return (
            r.input,
            r.output,
            r.cache_read,
            r.cache_write,
            r.has_cache_prices,
        );
    };
    (
        t.input.unwrap_or(r.input),
        t.output.unwrap_or(r.output),
        t.cache_read.unwrap_or(r.cache_read),
        t.cache_write.unwrap_or(r.cache_write),
        r.has_cache_prices || t.cache_read.is_some() || t.cache_write.is_some(),
    )
}

pub fn format_cost(usd: f64) -> String {
    if usd >= 1.0 {
        return format!("${:.2}", usd);
    }
    let trimmed = format!("{:.4}", usd)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string();
    if trimmed == "0" {
        return if usd > 0.0 {
            "<$0.0001".to_string()
        } else {
            "$0".to_string()
        };
    }
    format!("${trimmed}")
}

type Field = Option<serde_json::Value>;

#[derive(serde::Deserialize)]
pub struct LiteLlmEntry {
    max_input_tokens: Field,
    max_tokens: Field,
    input_cost_per_token: Field,
    output_cost_per_token: Field,
    cache_read_input_token_cost: Field,
    cache_creation_input_token_cost: Field,

    input_cost_per_token_above_200k_tokens: Field,
    output_cost_per_token_above_200k_tokens: Field,
    cache_read_input_token_cost_above_200k_tokens: Field,
    cache_creation_input_token_cost_above_200k_tokens: Field,
}
type LiteLlm = std::collections::BTreeMap<String, LiteLlmEntry>;

#[derive(serde::Deserialize)]
pub struct ModelsDevProvider {
    models: Option<std::collections::BTreeMap<String, ModelsDevEntry>>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevEntry {
    limit: Option<ModelsDevLimit>,
    context_window: Field,
    max_input_tokens: Field,
    reasoning: Field,
    reasoning_options: Option<Vec<ModelsDevOption>>,

    cost: Option<ModelsDevCost>,

    tool_call: Field,
    modalities: Option<ModelsDevModalities>,
    release_date: Field,
    family: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevModalities {
    output: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevCost {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
    tiers: Option<Vec<ModelsDevCostTier>>,

    context_over_200k: Option<ModelsDevTierRates>,
    #[serde(alias = "experimentalOver200K")]
    experimental_over_200k: Option<ModelsDevTierRates>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevCostTier {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
    tier: Option<ModelsDevTierBound>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevTierBound {
    #[serde(rename = "type")]
    kind: Field,
    size: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevTierRates {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevLimit {
    context: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevOption {
    #[serde(rename = "type")]
    kind: Field,
    values: Field,
}
type ModelsDev = std::collections::BTreeMap<String, ModelsDevProvider>;

#[derive(serde::Deserialize)]
pub struct OpenRouter {
    data: Vec<OpenRouterEntry>,
}
#[derive(serde::Deserialize)]
pub struct OpenRouterEntry {
    id: Field,
    pricing: Option<OpenRouterPricing>,
}
#[derive(serde::Deserialize)]
pub struct OpenRouterPricing {
    prompt: Field,
    completion: Field,
}

pub fn parse_litellm_context_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_litellm)
}

fn apply_litellm(map: LiteLlm) -> usize {
    let mut n = 0;
    for (key, entry) in &map {
        if key == "sample_spec" {
            continue;
        }
        let len = entry
            .max_input_tokens
            .as_ref()
            .and_then(json_usize)
            .or_else(|| entry.max_tokens.as_ref().and_then(json_usize));
        if let Some(len) = len.filter(|&v| v >= 1024) {
            cache_model_context_if_absent(key, len);

            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_context_if_absent(suffix, len);
            }
            n += 1;
        }

        let rate = |f: &Field| f.as_ref().and_then(json_rate);
        if let (Some(input), Some(output)) = (
            rate(&entry.input_cost_per_token),
            rate(&entry.output_cost_per_token),
        ) {
            let (cache_read, cache_write, has_cache) = match (
                rate(&entry.cache_read_input_token_cost),
                rate(&entry.cache_creation_input_token_cost),
            ) {
                (Some(r), Some(w)) => (r, w, true),
                _ => (0.0, 0.0, false),
            };
            let tiers = {
                let t_in = rate(&entry.input_cost_per_token_above_200k_tokens);
                let t_out = rate(&entry.output_cost_per_token_above_200k_tokens);
                if t_in.is_some() || t_out.is_some() {
                    vec![RateTier {
                        size: 200_000,
                        input: t_in,
                        output: t_out,
                        cache_read: rate(&entry.cache_read_input_token_cost_above_200k_tokens),
                        cache_write: rate(&entry.cache_creation_input_token_cost_above_200k_tokens),
                    }]
                } else {
                    Vec::new()
                }
            };
            let rate = ModelRate {
                input,
                output,
                cache_read,
                cache_write,
                has_cache_prices: has_cache,
                tiers,
            };
            cache_model_rate(key, rate.clone());
            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_rate(suffix, rate);
            }
        }
    }
    n
}

pub async fn fetch_litellm_context_windows() {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    if load_catalog("litellm.json", URL, apply_litellm).await > 0 {
        save_models_cache_to_disk();
    }
}

pub fn parse_models_dev_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_models_dev)
}

fn apply_models_dev(providers: ModelsDev) -> usize {
    let mut n = 0;
    for models in providers.values().filter_map(|p| p.models.as_ref()) {
        for (key, entry) in models {
            let len = entry
                .limit
                .as_ref()
                .and_then(|l| l.context.as_ref())
                .and_then(json_usize)
                .or_else(|| entry.context_window.as_ref().and_then(json_usize))
                .or_else(|| entry.max_input_tokens.as_ref().and_then(json_usize));
            if let Some(len) = len.filter(|&v| v >= 1024) {
                cache_models_dev_if_absent(key, len);
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_models_dev_if_absent(suffix, len);
                }
                n += 1;
            }

            if let Some(r) = entry.reasoning.as_ref().and_then(|v| v.as_bool()) {
                cache_model_reasoning(key, r);
            }

            let meta = ModelMeta {
                tool_call: entry.tool_call.as_ref().and_then(|v| v.as_bool()),
                text_only: entry
                    .modalities
                    .as_ref()
                    .and_then(|m| m.output.as_ref())
                    .and_then(|v| v.as_array())
                    .map(|o| o.len() == 1 && o[0].as_str() == Some("text")),
                release_date: entry
                    .release_date
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                family: entry
                    .family
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            if meta != ModelMeta::default() {
                cache_model_meta(key, meta.clone());
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_model_meta(suffix, meta);
                }
            }

            if let Some(values) = entry
                .reasoning_options
                .iter()
                .flatten()
                .find(|o| o.kind.as_ref().and_then(|t| t.as_str()) == Some("effort"))
                .and_then(|o| o.values.as_ref())
                .and_then(|v| v.as_array())
            {
                let efforts: Vec<String> = values
                    .iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.is_null().then(|| "none".to_string()))
                    })
                    .collect();
                if !efforts.is_empty() {
                    cache_model_efforts(key, efforts);
                }
            }

            if let Some(cost) = &entry.cost {
                let per_m = |f: &Field| f.as_ref().and_then(json_rate).map(|v| v / 1_000_000.0);
                let mut tiers: Vec<RateTier> = Vec::new();
                for flat in [&cost.context_over_200k, &cost.experimental_over_200k]
                    .into_iter()
                    .flatten()
                {
                    tiers.push(RateTier {
                        size: 200_000,
                        input: per_m(&flat.input),
                        output: per_m(&flat.output),
                        cache_read: per_m(&flat.cache_read),
                        cache_write: per_m(&flat.cache_write),
                    });
                }
                for t in cost.tiers.iter().flatten() {
                    let Some(bound) = t.tier.as_ref() else {
                        continue;
                    };
                    if bound.kind.as_ref().and_then(|v| v.as_str()) != Some("context") {
                        continue;
                    }
                    let Some(size) = bound.size.as_ref().and_then(json_usize) else {
                        continue;
                    };

                    tiers.retain(|e| e.size != size);
                    tiers.push(RateTier {
                        size,
                        input: per_m(&t.input),
                        output: per_m(&t.output),
                        cache_read: per_m(&t.cache_read),
                        cache_write: per_m(&t.cache_write),
                    });
                }
                let base = match (per_m(&cost.input), per_m(&cost.output)) {
                    (Some(input), Some(output)) => Some(ModelRate {
                        input,
                        output,
                        cache_read: per_m(&cost.cache_read).unwrap_or(0.0),
                        cache_write: per_m(&cost.cache_write).unwrap_or(0.0),

                        has_cache_prices: per_m(&cost.cache_read).is_some()
                            && per_m(&cost.cache_write).is_some(),
                        tiers: Vec::new(),
                    }),
                    _ => None,
                };
                cache_models_dev_rate(key, base.clone(), tiers.clone());
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_models_dev_rate(suffix, base, tiers);
                }
            }
        }
    }
    n
}

pub async fn fetch_models_dev_context() -> usize {
    const URL: &str = "https://models.dev/api.json";
    let n = load_catalog("models.dev.json", URL, apply_models_dev).await;
    if n > 0 {
        save_models_cache_to_disk();
    }
    n
}

fn cache_model_rate_if_absent(model_id: &str, rate: ModelRate) {
    if let Ok(g) = model_rates_cell().read()
        && g.contains_key(model_id)
    {
        return;
    }

    let mut rate = rate;
    merge_tiers(&mut rate.tiers, take_pending_tiers(model_id));
    cache_model_rate(model_id, rate);
}

pub fn parse_openrouter_models_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_openrouter)
}

fn apply_openrouter(list: OpenRouter) -> usize {
    fn num(v: &serde_json::Value) -> Option<f64> {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
            .filter(|f| f.is_finite() && *f >= 0.0)
    }
    let mut n = 0;
    for entry in &list.data {
        let (Some(id), Some(pricing)) =
            (entry.id.as_ref().and_then(|v| v.as_str()), &entry.pricing)
        else {
            continue;
        };
        let (Some(input), Some(output)) = (
            pricing.prompt.as_ref().and_then(num),
            pricing.completion.as_ref().and_then(num),
        ) else {
            continue;
        };
        if input == 0.0 && output == 0.0 {
            continue;
        }
        let rate = ModelRate {
            input,
            output,
            cache_read: 0.0,
            cache_write: 0.0,
            has_cache_prices: false,
            tiers: Vec::new(),
        };
        cache_model_rate_if_absent(id, rate.clone());
        let lower = id.to_lowercase();
        cache_model_rate_if_absent(&lower, rate.clone());
        if let Some((_, suffix)) = id.rsplit_once('/') {
            cache_model_rate_if_absent(suffix, rate);
        }
        n += 1;
    }
    n
}

pub async fn fetch_openrouter_rates() -> usize {
    const URL: &str = "https://openrouter.ai/api/v1/models";
    load_catalog("openrouter.json", URL, apply_openrouter).await
}

#[derive(serde::Deserialize)]
struct RecommendedModels {
    providers: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

static RECOMMENDED_MODELS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

fn recommended_cell() -> &'static std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>>
{
    RECOMMENDED_MODELS.get_or_init(|| std::sync::RwLock::new(std::collections::BTreeMap::new()))
}

pub fn curated_recommended(connect_id: &str) -> Option<Vec<String>> {
    recommended_cell().read().ok()?.get(connect_id).cloned()
}

fn apply_recommended(list: RecommendedModels) -> usize {
    let providers = list.providers.unwrap_or_default();
    let n = providers.values().map(Vec::len).sum();
    if let Ok(mut g) = recommended_cell().write() {
        *g = providers;
    }
    n
}

pub async fn fetch_recommended_models() -> usize {
    let url = std::env::var("GRAY_RECOMMENDED_URL")
        .unwrap_or_else(|_| "https://gray.alignment.id/models/recommended.json".to_string());
    load_catalog("recommended.json", &url, apply_recommended).await
}

const CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

async fn load_catalog<T: serde::de::DeserializeOwned>(
    name: &str,
    url: &str,
    apply: fn(T) -> usize,
) -> usize {
    let path = crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("cache").join(name));
    let from_disk = |max_age: std::time::Duration| -> Option<T> {
        let p = path.as_ref()?;
        let age = std::fs::metadata(p)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .unwrap_or_default();
        (age <= max_age).then_some(())?;
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    };
    let catalog = match from_disk(CATALOG_TTL) {
        Some(t) => Some(t),
        None => match fetch_body(url)
            .await
            .and_then(|body| Some((serde_json::from_slice::<T>(body.as_ref()).ok()?, body)))
        {
            Some((t, body)) => {
                if let Some(p) = &path
                    && let Some(dir) = p.parent()
                    && std::fs::create_dir_all(dir).is_ok()
                {
                    let tmp = p.with_extension("json.tmp");
                    if std::fs::write(&tmp, body.as_ref()).is_ok() {
                        let _ = std::fs::rename(&tmp, p);
                    }
                }
                Some(t)
            }
            None => from_disk(std::time::Duration::MAX),
        },
    };
    catalog.map_or(0, apply)
}

async fn fetch_body(url: &str) -> Option<impl AsRef<[u8]>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok()
}

fn models_cache_path() -> Option<std::path::PathBuf> {
    crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("models.json"))
}

pub fn load_models_cache_to_memory() -> usize {
    let Some(path) = models_cache_path() else {
        return 0;
    };
    let s = std::fs::read_to_string(path).ok().unwrap_or_default();
    if s.is_empty() {
        return 0;
    }
    let map: std::collections::HashMap<String, usize> =
        serde_json::from_str(&s).unwrap_or_default();
    let mut n = 0;
    for (k, v) in map {
        if v == 0 || get_cached_model_context(&k).is_some() {
            continue;
        }
        cache_model_context_with_source(&k, v, "disk", false);
        n += 1;
    }
    n
}

fn merged_models_cache(
    mut disk: std::collections::HashMap<String, usize>,
    mem: impl IntoIterator<Item = (String, usize)>,
) -> Option<std::collections::HashMap<String, usize>> {
    let mut dirty = false;
    for (k, v) in mem {
        if disk.get(&k) != Some(&v) {
            disk.insert(k, v);
            dirty = true;
        }
    }
    if dirty { Some(disk) } else { None }
}

pub fn save_models_cache_to_disk() {
    // Unit tests share the real $GRAY_HOME — a mocked fetch succeeding must
    // not persist the test models they seed the in-memory cache with.
    if cfg!(test) {
        return;
    }
    let Some(path) = models_cache_path() else {
        return;
    };
    let mem: Vec<(String, usize)> = model_context_cache()
        .read()
        .map(|g| g.iter().map(|(k, v)| (k.clone(), *v)).collect())
        .unwrap_or_default();
    if mem.is_empty() {
        return;
    }
    let disk: std::collections::HashMap<String, usize> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let Some(map) = merged_models_cache(disk, mem) else {
        return;
    };
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }

    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if std::fs::write(&tmp, s).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

pub(crate) fn ensure_disk_loaded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = load_models_cache_to_memory();
    });
}

pub(crate) fn warm_rates_from_disk_cache() {
    if cfg!(test) {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(dir) = crate::setup::catalog::gray_home()
            .ok()
            .map(|h| h.join("cache"))
        else {
            return;
        };
        for (name, apply) in [
            (
                "models.dev.json",
                parse_models_dev_json as fn(&serde_json::Value) -> usize,
            ),
            ("litellm.json", parse_litellm_context_json as _),
            ("openrouter.json", parse_openrouter_models_json as _),
        ] {
            if let Ok(s) = std::fs::read(dir.join(name))
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&s)
            {
                apply(&v);
            }
        }
    });
}

#[path = "providers_tests.rs"]
#[cfg(test)]
mod tests;
