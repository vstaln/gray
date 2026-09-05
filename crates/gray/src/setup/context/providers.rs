//! Provider models, context caches, rates (split from `context`).

use super::*;

/// Converts a raw model ID to a friendly human-readable display name.
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
            if lower == "gpt" || lower == "glm" || lower == "ai" || lower == "api" {
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
    words.join(" ")
}

/// Returns the models list for a provider from the catalog.
pub fn get_provider_models(_provider_id: &str, _catalog: &Catalog) -> Vec<(String, String)> {
    Vec::new()
}

/// Dynamically queries the provider's live /models endpoint (e.g. OpenAI, OpenRouter, Ollama, vLLM, LMStudio, etc.).
pub fn fetch_live_provider_models(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let base = base_url.to_string();
        let key = api_key.map(|k| k.to_string());
        std::thread::scope(|s| {
            s.spawn(move || {
                handle.block_on(async move {
                    let client = match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(3000))
                        .user_agent("gray/0.1.0")
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return Vec::new(),
                    };

                    let trimmed_base = base.trim_end_matches('/');
                    let endpoints = if trimmed_base.contains("openrouter.ai") {
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
                    };

                    for url in endpoints {
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
                                        models.push((id_str.to_string(), name));
                                    }
                                }
                            }
                            if !models.is_empty() {
                                save_models_cache_to_disk();
                                return models;
                            }
                        }
                    }

                    Vec::new()
                })
            })
            .join()
            .unwrap_or_default()
        })
    } else {
        Vec::new()
    }
}

/// Returns the models list for a provider dynamically from live endpoint.
pub fn get_provider_models_with_live(
    _provider_id: &str,
    base_url: &str,
    api_key: Option<&str>,
    _catalog: &Catalog,
) -> Vec<(String, String)> {
    fetch_live_provider_models(base_url, api_key)
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

/// Gap-fill insert: leaves an existing entry (e.g. provider-fetched) alone.
/// Provider values always win over the LiteLLM table regardless of arrival order.
pub fn cache_model_context_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "litellm", false);
}

/// Gap-fill insert for models.dev values (same provider-wins semantics).
pub fn cache_models_dev_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "models.dev", false);
}

/// Shared insert behind the cache fns above; also fans out the source tag.
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

/// Tags a cached window with its origin. Gap-fill callers pass
/// `overwrite: false` so a live value keeps its "live" tag.
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

/// Where the effective window for `model` came from.
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

/// USD-per-token rates from LiteLLM's table (same source as the context
/// windows — and the one T3 Code prices against). Base tier, like T3.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// False when the entry had no cache prices — price all input at `input`.
    pub has_cache_prices: bool,
}

static MODEL_RATES: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelRate>>,
> = std::sync::OnceLock::new();

fn model_rates_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, ModelRate>> {
    MODEL_RATES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn cache_model_rate(model_id: &str, rate: ModelRate) {
    if let Ok(mut g) = model_rates_cell().write() {
        g.insert(model_id.to_string(), rate);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, rate);
        }
    }
}

/// Rate for a model id, with the same `provider/model` tail fallback as the
/// context cache. None = unpriced (LiteLLM has no rate for it).
pub fn get_model_rate(model_id: &str) -> Option<ModelRate> {
    if let Ok(g) = model_rates_cell().read() {
        if let Some(r) = g.get(model_id).copied() {
            return Some(r);
        }
        let lower = model_id.to_lowercase();
        if let Some(r) = g.get(&lower).copied() {
            return Some(r);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/')
            && let Some(r) = g.get(suffix).copied()
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

/// Turn cost in USD, or None when the model is unpriced. Cache-aware: fresh
/// input at `input`, cached reads/writes at their prices; providers that only
/// fill inclusive `input_tokens` get it all priced fresh.
pub fn turn_cost(usage: &gray_core::event::Usage, model: &str) -> Option<f64> {
    let r = get_model_rate(model)?;
    let read = usage.cache_read_input_tokens as f64;
    let write = usage.cache_write_input_tokens as f64;
    let mut fresh = usage.non_cached_input_tokens as f64;
    if fresh == 0.0 {
        fresh = (usage.input_tokens as f64 - read - write).max(0.0);
    }
    let input_cost = if r.has_cache_prices {
        fresh * r.input + read * r.cache_read + write * r.cache_write
    } else {
        (fresh + read + write) * r.input
    };
    Some(input_cost + usage.output_tokens as f64 * r.output)
}

/// `$0.004`, `$0.41`, `$1.50` — 4 decimals trimmed, 2 minimum past a dollar.
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

/// Projects LiteLLM's public `model_prices_and_context_window.json` (the same
/// table T3 Code / ccusage price against) into the context cache.
/// `max_input_tokens` is the window; legacy `max_tokens` is the fallback
/// (on new entries it can mean output size, so it loses). Gap-fill only.
/// Returns the number of models cached.
pub fn parse_litellm_context_json(val: &serde_json::Value) -> usize {
    let Some(map) = val.as_object() else {
        return 0;
    };
    let mut n = 0;
    for (key, entry) in map {
        if key == "sample_spec" {
            continue;
        }
        let len = entry
            .get("max_input_tokens")
            .and_then(json_usize)
            .or_else(|| entry.get("max_tokens").and_then(json_usize));
        if let Some(len) = len.filter(|&v| v >= 1024) {
            cache_model_context_if_absent(key, len);
            // Keys are usually bare (`gpt-4o`); index the tail too so
            // `provider/model` lookups hit without knowing every prefix.
            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_context_if_absent(suffix, len);
            }
            n += 1;
        }
        // Rates ride the same loop. Both base rates required — a half-priced
        // model silently under-reports, which is worse than unpriced.
        if let (Some(input), Some(output)) = (
            entry.get("input_cost_per_token").and_then(json_rate),
            entry.get("output_cost_per_token").and_then(json_rate),
        ) {
            let (cache_read, cache_write, has_cache) = match (
                entry.get("cache_read_input_token_cost").and_then(json_rate),
                entry
                    .get("cache_creation_input_token_cost")
                    .and_then(json_rate),
            ) {
                (Some(r), Some(w)) => (r, w, true),
                _ => (0.0, 0.0, false),
            };
            let rate = ModelRate {
                input,
                output,
                cache_read,
                cache_write,
                has_cache_prices: has_cache,
            };
            cache_model_rate(key, rate);
            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_rate(suffix, rate);
            }
        }
    }
    n
}

/// Fetches LiteLLM's model table in the background and caches context windows.
/// Fire-and-forget: callers `tokio::spawn` this at boot next to the provider
/// `/models` fetch. Failures are silent — the hardcoded fallback covers offline.
pub async fn fetch_litellm_context_windows() {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("gray/0.1.0")
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let Ok(resp) = client.get(URL).send().await else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }
    if let Ok(json) = resp.json::<serde_json::Value>().await
        && parse_litellm_context_json(&json) > 0
    {
        save_models_cache_to_disk();
    }
}

/// Projects models.dev's public `api.json` (`providers -> models ->
/// `limit.context`, the same shape opencode's provider.ts consumes) into the
/// context cache. Also accepts `context_window` / `max_input_tokens` keys.
/// Gap-fill only. Returns the number of models cached.
pub fn parse_models_dev_json(val: &serde_json::Value) -> usize {
    let Some(providers) = val.as_object() else {
        return 0;
    };
    let mut n = 0;
    for (_pid, pval) in providers {
        let Some(models) = pval.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        for (key, entry) in models {
            let len = entry
                .get("limit")
                .and_then(|l| l.get("context"))
                .and_then(json_usize)
                .or_else(|| entry.get("context_window").and_then(json_usize))
                .or_else(|| entry.get("max_input_tokens").and_then(json_usize));
            if let Some(len) = len.filter(|&v| v >= 1024) {
                cache_models_dev_if_absent(key, len);
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_models_dev_if_absent(suffix, len);
                }
                n += 1;
            }
        }
    }
    n
}

/// Fetches models.dev's table in the background and caches context windows.
/// Same fire-and-forget contract as `fetch_litellm_context_windows`.
/// Returns the number of models cached (0 on any failure).
pub async fn fetch_models_dev_context() -> usize {
    const URL: &str = "https://models.dev/api.json";
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("gray/0.1.0")
        .build()
    {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let Ok(resp) = client.get(URL).send().await else {
        return 0;
    };
    if !resp.status().is_success() {
        return 0;
    }
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return 0;
    };
    let n = parse_models_dev_json(&json);
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
    cache_model_rate(model_id, rate);
}

/// OpenRouter's `/models` carries per-model `pricing` (USD/token as strings).
/// Gap-fill only — LiteLLM stays authoritative; this just covers models too
/// new for LiteLLM's table (e.g. Muse Spark at launch).
pub fn parse_openrouter_models_json(val: &serde_json::Value) -> usize {
    let Some(list) = val.get("data").and_then(|d| d.as_array()) else {
        return 0;
    };
    fn num(v: &serde_json::Value) -> Option<f64> {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
            .filter(|f| f.is_finite() && *f >= 0.0)
    }
    let mut n = 0;
    for entry in list {
        let (Some(id), Some(pricing)) = (
            entry.get("id").and_then(|v| v.as_str()),
            entry.get("pricing"),
        ) else {
            continue;
        };
        let (Some(input), Some(output)) = (
            pricing.get("prompt").and_then(num),
            pricing.get("completion").and_then(num),
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
        };
        cache_model_rate_if_absent(id, rate);
        let lower = id.to_lowercase();
        cache_model_rate_if_absent(&lower, rate);
        if let Some((_, suffix)) = id.rsplit_once('/') {
            cache_model_rate_if_absent(suffix, rate);
        }
        n += 1;
    }
    n
}

/// Same fire-and-forget contract as `fetch_litellm_context_windows`.
/// No auth needed — OpenRouter's model list is public.
pub async fn fetch_openrouter_rates() -> usize {
    const URL: &str = "https://openrouter.ai/api/v1/models";
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("gray/0.1.0")
        .build()
    {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let Ok(resp) = client.get(URL).send().await else {
        return 0;
    };
    if !resp.status().is_success() {
        return 0;
    }
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return 0;
    };
    parse_openrouter_models_json(&json)
}
/// A previous session's live/litellm/models.dev values beat the hardcoded
/// guess on cold boot, before any fetch completes.
/// On-disk context cache (`~/.gray/models.json`, `{ "model-id": tokens }`).
fn models_cache_path() -> Option<std::path::PathBuf> {
    crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("models.json"))
}

/// Loads the disk cache into memory (gap-fill, source "disk"). Best-effort.
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

/// Persists the in-memory cache to disk (read-modify-write, best-effort).
/// Called after any successful fetch so cold boot beats the guess.
pub fn save_models_cache_to_disk() {
    let Some(path) = models_cache_path() else {
        return;
    };
    let mut map: std::collections::HashMap<String, usize> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if let Ok(g) = model_context_cache().read() {
        for (k, v) in g.iter() {
            map.insert(k.clone(), *v);
        }
    }
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = std::fs::write(path, s);
}

/// One-shot cold-boot load so disk values are present before first resolve.
/// (The only startup hook reachable without touching other modules.)
pub(crate) fn ensure_disk_loaded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = load_models_cache_to_memory();
    });
}
