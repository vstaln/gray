use super::catalog::Catalog;

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
    super::catalog::gray_home()
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
fn ensure_disk_loaded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = load_models_cache_to_memory();
    });
}

static USER_CONTEXT_WINDOW: std::sync::OnceLock<std::sync::RwLock<Option<usize>>> =
    std::sync::OnceLock::new();

fn user_context_window_cell() -> &'static std::sync::RwLock<Option<usize>> {
    USER_CONTEXT_WINDOW.get_or_init(|| std::sync::RwLock::new(None))
}

/// Sets the user-configured global context window override (highest priority).
/// `None` clears the override (auto-fetch / hardcoded fallback resumes).
pub fn set_user_context_window(v: Option<usize>) {
    if let Ok(mut g) = user_context_window_cell().write() {
        *g = v.filter(|&n| n > 0);
    }
}

pub fn get_user_context_window() -> Option<usize> {
    user_context_window_cell().read().ok().and_then(|g| *g)
}

pub const DEFAULT_RESERVE_TOKENS: usize = 16_384;
pub const DEFAULT_KEEP_RECENT_TOKENS: usize = 20_000;

static USER_RESERVE_TOKENS: std::sync::OnceLock<std::sync::RwLock<Option<usize>>> =
    std::sync::OnceLock::new();
static USER_KEEP_TOKENS: std::sync::OnceLock<std::sync::RwLock<Option<usize>>> =
    std::sync::OnceLock::new();

fn user_reserve_cell() -> &'static std::sync::RwLock<Option<usize>> {
    USER_RESERVE_TOKENS.get_or_init(|| std::sync::RwLock::new(None))
}
fn user_keep_cell() -> &'static std::sync::RwLock<Option<usize>> {
    USER_KEEP_TOKENS.get_or_init(|| std::sync::RwLock::new(None))
}

/// User override for auto-compact reserve (`None` = default 16k).
pub fn set_user_reserve_tokens(v: Option<usize>) {
    if let Ok(mut g) = user_reserve_cell().write() {
        *g = v.filter(|&n| n > 0);
    }
}
/// Effective reserve: override or default.
// Legacy flat default; new code prefers `user_reserve_tokens_for(window)`.
pub fn user_reserve_tokens() -> usize {
    user_reserve_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or(DEFAULT_RESERVE_TOKENS)
}
/// User override for keep-recent tail (`None` = default 20k).
pub fn set_user_keep_recent_tokens(v: Option<usize>) {
    if let Ok(mut g) = user_keep_cell().write() {
        *g = v;
    }
}
/// Effective keep-recent: override or default.
// Legacy flat default; new code prefers `user_keep_for(window)`.
pub fn user_keep_recent_tokens() -> usize {
    user_keep_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or(DEFAULT_KEEP_RECENT_TOKENS)
}

/// Proportional budgets: reserve ≈ window/16, keep ≈ window/13, clamped to
/// [4k, 64k]. 256k → 16k / ~19.7k (≈ legacy 16384 / 20000).
pub fn default_reserve_for_window(window: usize) -> usize {
    (window / 16).clamp(4096, 65_536)
}
pub fn default_keep_for_window(window: usize) -> usize {
    (window / 13).clamp(4096, 65_536)
}

/// Effective reserve for a window: override or proportional default.
pub fn user_reserve_tokens_for(window: usize) -> usize {
    user_reserve_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| default_reserve_for_window(window))
}
/// Effective keep-recent for a window: override or proportional default.
pub fn user_keep_for(window: usize) -> usize {
    user_keep_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| default_keep_for_window(window))
}

/// Parses a human-friendly context window string: `128000`, `1,000,000`,
/// `1.000.000`, `128k`, `1000k`, `1m`, `1.5m`, etc. Case-insensitive,
/// `k` = 1_000, `m` = 1_000_000. Commas/underscores/spaces are thousand
/// separators; dots are too when grouped (`1.000.000`) — a single dot stays
/// a decimal point (`1.5m`), and bare decimals (`1.5`) are rejected.
pub fn parse_context_window(s: &str) -> Option<usize> {
    let t = s.trim().to_lowercase().replace([',', '_', ' '], "");
    if t.is_empty() {
        return None;
    }
    // Strip one k/m suffix first so `1000k` / `1.000.000m` work uniformly.
    let (num, mult) = if let Some(n) = t.strip_suffix('k') {
        (n, 1_000.0)
    } else if let Some(n) = t.strip_suffix('m') {
        (n, 1_000_000.0)
    } else {
        (t.as_str(), 1.0)
    };
    let num = num.trim();
    if num.is_empty() {
        return None;
    }
    let dots = num.bytes().filter(|&b| b == b'.').count();
    let plain = if dots > 1 {
        // `1.000.000` — unambiguous thousand separators.
        num.replace('.', "")
    } else {
        num.to_string()
    };
    if mult == 1.0 {
        // No suffix: integers only. A lone dot must be grouped thousands.
        if dots == 1 && !is_grouped_thousands(num) {
            return None;
        }
        if let Ok(n) = plain.parse::<usize>()
            && n > 0
        {
            return Some(n);
        }
        return None;
    }
    if let Ok(f) = plain.parse::<f64>() {
        let v = (f * mult).round() as usize;
        if v > 0 {
            return Some(v);
        }
    }
    None
}

/// `1.000.000`-style grouping: 1–3 leading digits then `.ddd` groups.
fn is_grouped_thousands(num: &str) -> bool {
    let mut parts = num.split('.');
    match parts.next() {
        Some(first)
            if (1..=3).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_digit()) => {}
        _ => return false,
    }
    let mut groups = 0;
    for p in parts {
        if p.len() != 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        groups += 1;
    }
    groups > 0
}

pub fn extract_context_length_from_json(val: &serde_json::Value) -> Option<usize> {
    const KEYS: &[&str] = &[
        "context_length",
        "context_window",
        "max_context_length",
        "max_context_window",
        "max_position_embeddings",
        "max_model_len",
        "max_input_tokens",
        "max_sequence_length",
        "max_seq_len",
        "n_ctx_train",
        "n_ctx",
        "ctx_size",
    ];
    for key in KEYS {
        if let Some(v) = val.get(*key) {
            if let Some(n) = v.as_u64() {
                if n > 0 {
                    return Some(n as usize);
                }
            } else if let Some(s) = v.as_str()
                && let Ok(n) = s.parse::<usize>()
                && n > 0
            {
                return Some(n);
            }
        }
    }
    None
}

pub fn format_context_length(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        let val = tokens as f64 / 1_000_000.0;
        let rounded = val.round();
        if (val - rounded).abs() < 0.05 {
            format!("{:.0}M", rounded)
        } else {
            format!("{:.1}M", val)
        }
    } else if tokens >= 1_000 {
        let val = tokens as f64 / 1000.0;
        let rounded = val.round();
        if (val - rounded).abs() < 0.05 {
            format!("{:.0}k", rounded)
        } else {
            let val_kibi = tokens as f64 / 1024.0;
            let rounded_kibi = val_kibi.round();
            if (val_kibi - rounded_kibi).abs() < 0.05 {
                format!("{:.0}k", rounded_kibi)
            } else {
                format!("{:.1}k", val)
            }
        }
    } else {
        tokens.to_string()
    }
}

pub fn resolve_model_context_length(model_name: &str) -> usize {
    // 1) explicit user override wins over everything (CLI --context-window / GRAY_CONTEXT_WINDOW / config.json)
    if let Some(user) = get_user_context_window() {
        return user;
    }
    model_max_context(model_name)
}

/// Model max ignoring the user override: live cache → hardcoded fallback.
/// Use for clamping user input so effective window never exceeds what the model supports.
pub fn model_max_context(model_name: &str) -> usize {
    ensure_disk_loaded();
    // auto-fetched provider value (populated by fetch_live_provider_models)
    if let Some(cached) = get_cached_model_context(model_name) {
        return cached;
    }
    let lower = model_name.to_lowercase();
    if let Some(cached) = get_cached_model_context(&lower) {
        return cached;
    }
    // `provider/model` ids vs bare cache keys (`gpt-4o`): try the tail.
    if let Some((_, suffix)) = model_name.rsplit_once('/')
        && let Some(cached) = get_cached_model_context(suffix)
    {
        return cached;
    }
    fallback_context_length(&lower)
}

// Guess fallback; live/models.dev/litellm/disk cache wins when present.
fn fallback_context_length(lower: &str) -> usize {
    if lower.contains("gemini-1.5-pro")
        || lower.contains("gemini-2.0")
        || lower.contains("gemini-2.5")
        || lower.contains("gemini-1.5-flash")
        || lower.contains("gemini")
    {
        1_048_576
    } else if lower.contains("claude-opus-4")
        || lower.contains("claude-sonnet-4")
        || lower.contains("claude-4")
        || lower.contains("claude-5")
    {
        1_000_000
    } else if lower.contains("claude-3") || lower.contains("claude") {
        200_000
    } else if lower.contains("gpt-5") || lower.contains("gpt-4.5") || lower.contains("gpt-4.1") {
        1_048_576
    } else if lower.contains("gpt-4o")
        || lower.contains("o1")
        || lower.contains("o3")
        || lower.contains("gpt-4-turbo")
    {
        128_000
    } else if lower.contains("gpt-4-32k") {
        32_768
    } else if lower.contains("gpt-4") {
        8_192
    } else if lower.contains("gpt-3.5-turbo-16k") {
        16_384
    } else if lower.contains("gpt-3.5") {
        4_096
    } else if lower.contains("deepseek-v4") {
        1_000_000
    } else if lower.contains("deepseek-chat")
        || lower.contains("deepseek-reasoner")
        || lower.contains("deepseek-v3")
        || lower.contains("deepseek-r1")
        || lower.contains("deepseek")
    {
        131_072
    } else if lower.contains("qwen3") {
        1_000_000
    } else if lower.contains("qwen2.5") || lower.contains("qwen") {
        131_072
    } else if lower.contains("grok-4") {
        2_000_000
    } else if lower.contains("grok-3")
        || lower.contains("grok-2")
        || lower.contains("grok")
        || lower.contains("llama-3.3")
        || lower.contains("llama-3.2")
        || lower.contains("llama-3.1")
    {
        131_072
    } else if lower.contains("llama-3") {
        8_192
    } else if lower.contains("mistral-large") || lower.contains("codestral") {
        128_000
    } else if lower.contains("kimi-k3") {
        1_048_576
    } else if lower.contains("kimi") {
        262_144
    } else if lower.contains("glm-5") {
        1_048_576
    } else if lower.contains("glm") {
        128_000
    } else if lower.contains("1m") {
        1_000_000
    } else if lower.contains("2m") {
        2_000_000
    } else if lower.contains("128k") {
        128_000
    } else {
        256_000
    }
}

/// Returns the model context limit in tokens and a human-friendly label (e.g. 256_000, "256k").
pub fn model_context_info(model_name: &str) -> (usize, String) {
    let tokens = resolve_model_context_length(model_name);
    let label = format_context_length(tokens);
    (tokens, label)
}

/// Token estimate for an arbitrary string, matching `compact::estimate_tokens`
/// (`chars/4` ceiling) so `/context` breakdown stays consistent.
pub fn estimate_str_tokens(s: &str) -> usize {
    (s.len() as f64 / 4.0).ceil() as usize
}

/// Estimated per-category usage for the `/context` visual. All estimates.
#[derive(Debug, Clone, Default)]
pub struct ContextParts {
    pub system_prompt: usize,
    pub project_context: usize,
    pub tools: usize,
    pub skills: usize,
    pub messages: usize,
}

impl ContextParts {
    pub fn used(&self) -> usize {
        self.system_prompt
            .saturating_add(self.project_context)
            .saturating_add(self.tools)
            .saturating_add(self.skills)
            .saturating_add(self.messages)
    }

    pub fn free(&self, window: usize, reserve: usize) -> usize {
        window.saturating_sub(self.used().saturating_add(reserve))
    }

    /// 100 grid cells split across categories + free + reserve. Sums to 100.
    /// Order: system, project, tools, skills, messages, free, reserve.
    pub fn grid_cells(&self, window: usize, reserve: usize) -> [usize; 7] {
        if window == 0 {
            return [0; 7];
        }
        let mut cells = [
            ((self.system_prompt as f64 / window as f64) * 100.0).round() as usize,
            ((self.project_context as f64 / window as f64) * 100.0).round() as usize,
            ((self.tools as f64 / window as f64) * 100.0).round() as usize,
            ((self.skills as f64 / window as f64) * 100.0).round() as usize,
            ((self.messages as f64 / window as f64) * 100.0).round() as usize,
            0, // free takes the rounding remainder
            ((reserve as f64 / window as f64) * 100.0).round() as usize,
        ];
        let used: usize = cells[0] + cells[1] + cells[2] + cells[3] + cells[4] + cells[6];
        cells[5] = 100usize.saturating_sub(used.min(100));
        // Shrink overflow from the largest bucket so the row always sums to 100.
        let total: usize = cells.iter().sum();
        if total > 100 {
            let mut over = total - 100;
            let mut idx: Vec<usize> = (0..7).collect();
            idx.sort_by_key(|&i| std::cmp::Reverse(cells[i]));
            for i in idx {
                if over == 0 {
                    break;
                }
                let cut = cells[i].min(over);
                cells[i] -= cut;
                over -= cut;
            }
        }
        cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_context_window_accepts_separators() {
        assert_eq!(parse_context_window("1000000"), Some(1_000_000));
        assert_eq!(parse_context_window("1,000,000"), Some(1_000_000));
        assert_eq!(parse_context_window("1.000.000"), Some(1_000_000));
        assert_eq!(parse_context_window("1_000_000"), Some(1_000_000));
        assert_eq!(parse_context_window("1000k"), Some(1_000_000));
        assert_eq!(parse_context_window("128k"), Some(128_000));
        assert_eq!(parse_context_window("1.5m"), Some(1_500_000));
        assert_eq!(parse_context_window("1.5"), None); // bare decimals rejected
        assert_eq!(parse_context_window(""), None);
        assert_eq!(parse_context_window("abc"), None);
    }

    #[test]
    fn model_max_ignores_user_override() {
        set_user_context_window(Some(8_000));
        // gpt-4o fallback is 128k; max must ignore the 8k override
        assert_eq!(model_max_context("gpt-4o"), 128_000);
        set_user_context_window(None);
    }

    #[test]
    fn compaction_defaults_match_legacy() {
        assert_eq!(user_reserve_tokens(), 16_384);
        assert_eq!(user_keep_recent_tokens(), 20_000);
    }

    #[test]
    fn litellm_parse_fills_gaps_only() {
        let v: serde_json::Value = serde_json::json!({
            "sample_spec": {"max_input_tokens": 1},
            "test-litellm-alpha": {"max_input_tokens": 123000, "max_tokens": 16000},
            "test-litellm-beta": {"max_tokens": 64000},
            "test-litellm-tiny": {"max_tokens": 16},
            "prov/test-litellm-gamma": {"max_input_tokens": 50000},
        });
        assert_eq!(parse_litellm_context_json(&v), 3);
        // max_input_tokens wins over output-sized max_tokens
        assert_eq!(model_max_context("test-litellm-alpha"), 123_000);
        assert_eq!(model_max_context("test-litellm-beta"), 64_000);
        // suffix fallback: `some-provider/model` hits the bare key
        assert_eq!(model_max_context("someprov/test-litellm-alpha"), 123_000);
        assert_eq!(model_max_context("prov/test-litellm-gamma"), 50_000);
        // provider values win over litellm gaps regardless of order
        cache_model_context("test-litellm-alpha", 999_000);
        assert_eq!(parse_litellm_context_json(&v), 3);
        assert_eq!(model_max_context("test-litellm-alpha"), 999_000);
    }

    #[test]
    fn litellm_rates_and_turn_cost() {
        let v: serde_json::Value = serde_json::json!({
            "test-rate-full": {
                "max_input_tokens": 200000,
                "input_cost_per_token": 0.000003,
                "output_cost_per_token": 0.000015,
                "cache_read_input_token_cost": 0.0000003,
                "cache_creation_input_token_cost": 0.00000375,
            },
            "test-rate-nocache": {
                "max_input_tokens": 128000,
                "input_cost_per_token": 0.000002,
                "output_cost_per_token": 0.000008,
            },
            "test-rate-half": {
                "max_input_tokens": 64000,
                "input_cost_per_token": 0.000001,
            },
        });
        parse_litellm_context_json(&v);
        // full entry: cache-aware
        let r = get_model_rate("someprov/test-rate-full").expect("rate with suffix fallback");
        assert!(r.has_cache_prices);
        let u = gray_core::event::Usage {
            input_tokens: 10_000,
            output_tokens: 2_000,
            non_cached_input_tokens: 6_000,
            cache_read_input_tokens: 3_000,
            cache_write_input_tokens: 1_000,
            ..Default::default()
        };
        let cost = turn_cost(&u, "test-rate-full").expect("priced");
        let want =
            6_000.0 * 0.000003 + 3_000.0 * 0.0000003 + 1_000.0 * 0.00000375 + 2_000.0 * 0.000015;
        assert!((cost - want).abs() < 1e-9, "got {cost}, want {want}");
        // no cache prices: everything at input rate
        let u2 = gray_core::event::Usage::new(10_000, 2_000);
        let cost2 = turn_cost(&u2, "test-rate-nocache").expect("priced");
        assert!((cost2 - (10_000.0 * 0.000002 + 2_000.0 * 0.000008)).abs() < 1e-9);
        // inclusive-only providers: all input priced fresh
        let u3 = gray_core::event::Usage {
            input_tokens: 5_000,
            output_tokens: 0,
            ..Default::default()
        };
        assert!(turn_cost(&u3, "test-rate-full").expect("priced") > 0.0);
        // half entry dropped, unknown model unpriced
        assert!(get_model_rate("test-rate-half").is_none());
        assert!(turn_cost(&u, "no-such-model").is_none());
        // formatting
        assert_eq!(format_cost(0.004), "$0.004");
        assert_eq!(format_cost(0.41), "$0.41");
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(0.0), "$0");
    }

    #[test]
    fn openrouter_rates_gapfill() {
        let v: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "test-or-new-model", "pricing": {"prompt": "0.000003", "completion": "0.000015"}},
                {"id": "test-or-claimed", "pricing": {"prompt": "0.99", "completion": "0.99"}},
                {"id": "test-or-free", "pricing": {"prompt": "0", "completion": "0"}},
            ]
        });
        assert_eq!(parse_openrouter_models_json(&v), 2);
        let r = get_model_rate("test-or-new-model").expect("openrouter rate cached");
        assert!(!r.has_cache_prices);
        // gap-fill only: second parse can't overwrite an existing rate
        let v2: serde_json::Value = serde_json::json!({
            "data": [{"id": "test-or-new-model", "pricing": {"prompt": "0.99", "completion": "0.99"}}]
        });
        parse_openrouter_models_json(&v2);
        let kept = get_model_rate("test-or-new-model").expect("rate kept");
        assert!((kept.input - 0.000003).abs() < 1e-12);
    }

    #[test]
    fn breakdown_free_and_grid_sum_to_window() {
        let p = ContextParts {
            system_prompt: 2_300,
            project_context: 1_600,
            tools: 16_700,
            skills: 279,
            messages: 42_200,
        };
        let window = 200_000;
        let reserve = 45_000;
        assert_eq!(p.used(), 2_300 + 1_600 + 16_700 + 279 + 42_200);
        assert_eq!(p.free(window, reserve), window - p.used() - reserve);
        assert_eq!(p.grid_cells(window, reserve).iter().sum::<usize>(), 100);
        // saturates instead of underflowing when over budget
        assert_eq!(p.free(10_000, reserve), 0);
    }
}
