use super::catalog::Catalog;

/// Converts a raw model ID to a friendly human-readable display name.
pub fn friendly_model_name(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    let name = model_id.split('/').last().unwrap_or(model_id);
    let words: Vec<String> = name
        .split(['-', '_', ':'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            if lower == "gpt" || lower == "glm" || lower == "ai" || lower == "api" {
                w.to_uppercase()
            } else if lower.starts_with('v') && lower.len() > 1 && lower[1..].chars().all(|c| c.is_ascii_digit() || c == '.') {
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
                        if let Some(k) = &key {
                            if !k.is_empty() {
                                req = req.header("Authorization", format!("Bearer {k}"));
                            }
                        }
                        if url.contains("openrouter") {
                            req = req.header("HTTP-Referer", "https://github.com/vstaln/gray");
                            req = req.header("X-Title", "Gray");
                        }

                        if let Ok(resp) = req.send().await {
                            if resp.status().is_success() {
                                if let Ok(json) = resp.json::<serde_json::Value>().await {
                                    let mut models = Vec::new();
                                    let items_opt = if let Some(arr) = json.as_array() {
                                        Some(arr)
                                    } else if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                                        Some(arr)
                                    } else if let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                                        Some(arr)
                                    } else {
                                        None
                                    };

                                    if let Some(items) = items_opt {
                                        for item in items {
                                            let id = item.get("id")
                                                .or_else(|| item.get("name"))
                                                .or_else(|| item.get("model"))
                                                .and_then(|v| v.as_str());
                                            if let Some(id_str) = id {
                                                let name = item.get("name")
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
                                        return models;
                                    }
                                }
                            }
                        }
                    }

                    Vec::new()
                })
            }).join().unwrap_or_default()
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

static MODEL_CONTEXT_CACHE: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, usize>>> = std::sync::OnceLock::new();

fn model_context_cache() -> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
    MODEL_CONTEXT_CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_context(model_id: &str, length: usize) {
    if length == 0 { return; }
    if let Ok(mut g) = model_context_cache().write() {
        g.insert(model_id.to_string(), length);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, length);
        }
    }
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

static USER_CONTEXT_WINDOW: std::sync::OnceLock<std::sync::RwLock<Option<usize>>> = std::sync::OnceLock::new();

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
pub fn user_keep_recent_tokens() -> usize {
    user_keep_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or(DEFAULT_KEEP_RECENT_TOKENS)
}

/// Parses a human-friendly context window string: `128000`, `128k`, `1m`, `1.5m`, `256k`, etc.
/// Accepts commas/underscores and is case-insensitive. `k` = 1_000, `m` = 1_000_000.
pub fn parse_context_window(s: &str) -> Option<usize> {
    let t = s.trim().to_lowercase().replace([',', '_'], "");
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<usize>() {
        if n > 0 {
            return Some(n);
        }
    }
    if let Some(num) = t.strip_suffix('k') {
        if let Ok(f) = num.trim().parse::<f64>() {
            let v = (f * 1_000.0).round() as usize;
            if v > 0 {
                return Some(v);
            }
        }
    }
    if let Some(num) = t.strip_suffix('m') {
        if let Ok(f) = num.trim().parse::<f64>() {
            let v = (f * 1_000_000.0).round() as usize;
            if v > 0 {
                return Some(v);
            }
        }
    }
    None
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
            } else if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    if n > 0 {
                        return Some(n);
                    }
                }
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
    // auto-fetched provider value (populated by fetch_live_provider_models)
    if let Some(cached) = get_cached_model_context(model_name) {
        return cached;
    }
    let lower = model_name.to_lowercase();
    if let Some(cached) = get_cached_model_context(&lower) {
        return cached;
    }
    fallback_context_length(&lower)
}

fn fallback_context_length(lower: &str) -> usize {
    if lower.contains("gemini-1.5-pro") || lower.contains("gemini-2.0") || lower.contains("gemini-2.5") || lower.contains("gemini-1.5-flash") || lower.contains("gemini") {
        1_048_576
    } else if lower.contains("claude-opus-4") || lower.contains("claude-sonnet-4") || lower.contains("claude-4") || lower.contains("claude-5") {
        1_000_000
    } else if lower.contains("claude-3") || lower.contains("claude") {
        200_000
    } else if lower.contains("gpt-5") || lower.contains("gpt-4.5") || lower.contains("gpt-4.1") {
        1_048_576
    } else if lower.contains("gpt-4o") || lower.contains("o1") || lower.contains("o3") || lower.contains("gpt-4-turbo") {
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
    } else if lower.contains("deepseek-chat") || lower.contains("deepseek-reasoner") || lower.contains("deepseek-v3") || lower.contains("deepseek-r1") || lower.contains("deepseek") {
        131_072
    } else if lower.contains("qwen3") {
        1_000_000
    } else if lower.contains("qwen2.5") || lower.contains("qwen") {
        131_072
    } else if lower.contains("grok-4") {
        2_000_000
    } else if lower.contains("grok-3") || lower.contains("grok-2") || lower.contains("grok") {
        131_072
    } else if lower.contains("llama-3.3") || lower.contains("llama-3.2") || lower.contains("llama-3.1") {
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
    } else if lower.contains("256k") {
        256_000
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

    /// 100 grid cells split across categories + reserve + free. Sums to 100.
    /// Order: system, project, tools, skills, messages, reserve, free.
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
            ((reserve as f64 / window as f64) * 100.0).round() as usize,
            0,
        ];
        let used: usize = cells.iter().take(6).sum();
        cells[6] = 100usize.saturating_sub(used.min(100));
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
