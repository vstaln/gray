mod providers;

pub(crate) use providers::ensure_disk_loaded;
pub use providers::{
    ModelRate, cache_model_context, cache_model_context_if_absent, cache_model_reasoning,
    cache_models_dev_if_absent, cached_model_ids, clamp_thinking_level, context_source,
    fetch_litellm_context_windows, fetch_live_provider_models, fetch_models_dev_context,
    fetch_openrouter_rates, format_cost, friendly_model_name, get_cached_model_context,
    get_model_rate, load_models_cache_to_memory, model_supports_reasoning,
    parse_litellm_context_json, parse_models_dev_json, parse_openrouter_models_json,
    save_models_cache_to_disk, set_active_model_provider, supported_efforts,
    supported_thinking_levels, turn_cost,
};

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

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
