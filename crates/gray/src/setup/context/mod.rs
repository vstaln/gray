mod providers;

pub(crate) use providers::{
    EndpointProbe, LocalEndpointSuggestion, cache_model_efforts, cache_model_price_cliff,
    ensure_disk_loaded, fetch_plugin_provider_models, load_provider_model_list, ollama_last_seen,
    ollama_running, probe_custom_endpoint, probe_local_endpoints, provider_model_lists,
    save_provider_model_list, warm_rates_from_disk_cache,
};
pub use providers::{
    KeyCheck, ModelMeta, ModelRate, cache_model_context, cache_model_context_if_absent,
    cache_model_meta, cache_model_reasoning, cache_models_dev_if_absent, cached_model_ids,
    clamp_thinking_level, context_source, curated_recommended, fetch_litellm_context_windows,
    fetch_live_provider_models, fetch_models_dev_context, fetch_openrouter_rates,
    fetch_recommended_models, format_cost, friendly_model_name, get_cached_model_context,
    get_cached_model_price_cliff, get_model_rate, load_models_cache_to_memory, model_meta,
    model_supports_reasoning, parse_litellm_context_json, parse_models_dev_json,
    parse_openrouter_models_json, save_models_cache_to_disk, set_active_model_provider,
    supported_efforts, supported_thinking_levels, turn_cost, uncached_cost, verify_api_key,
};

static USER_CONTEXT_WINDOW: std::sync::OnceLock<std::sync::RwLock<Option<usize>>> =
    std::sync::OnceLock::new();

fn user_context_window_cell() -> &'static std::sync::RwLock<Option<usize>> {
    USER_CONTEXT_WINDOW.get_or_init(|| std::sync::RwLock::new(None))
}

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

pub fn set_user_reserve_tokens(v: Option<usize>) {
    if let Ok(mut g) = user_reserve_cell().write() {
        *g = v.filter(|&n| n > 0);
    }
}

pub fn set_user_keep_recent_tokens(v: Option<usize>) {
    if let Ok(mut g) = user_keep_cell().write() {
        *g = v;
    }
}

pub fn user_keep_recent_tokens() -> usize {
    user_keep_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or(DEFAULT_KEEP_RECENT_TOKENS)
}

pub fn default_reserve_for_window(window: usize) -> usize {
    (window / 16).clamp(4096, 65_536)
}
pub fn default_keep_for_window(window: usize) -> usize {
    (window / 13).clamp(4096, 65_536)
}

pub fn user_reserve_tokens_for(window: usize) -> usize {
    user_reserve_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| default_reserve_for_window(window))
}

pub fn user_keep_for(window: usize) -> usize {
    user_keep_cell()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| default_keep_for_window(window))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactAt {
    Auto,

    Tokens(usize),
}

pub fn parse_compact_at(s: &str) -> Option<CompactAt> {
    if s.trim().eq_ignore_ascii_case("auto") {
        return Some(CompactAt::Auto);
    }
    parse_context_window(s).map(CompactAt::Tokens)
}

impl serde::Serialize for CompactAt {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => ser.serialize_str("auto"),
            Self::Tokens(n) => ser.serialize_u64(*n as u64),
        }
    }
}

impl<'de> serde::Deserialize<'de> for CompactAt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = CompactAt;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a token count or \"auto\"")
            }
            fn visit_u64<E>(self, n: u64) -> Result<CompactAt, E> {
                Ok(CompactAt::Tokens(n as usize))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<CompactAt, E> {
                u64::try_from(n)
                    .map(|n| CompactAt::Tokens(n as usize))
                    .map_err(|_| E::custom("compact_at must be positive"))
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<CompactAt, E> {
                parse_compact_at(s).ok_or_else(|| E::custom("expected \"auto\" or a token count"))
            }
        }
        d.deserialize_any(V)
    }
}

pub fn parse_context_window(s: &str) -> Option<usize> {
    let t = s.trim().to_lowercase().replace([',', '_', ' '], "");
    if t.is_empty() {
        return None;
    }

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
        num.replace('.', "")
    } else {
        num.to_string()
    };
    if mult == 1.0 {
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
    if let Some(user) = get_user_context_window() {
        return user;
    }
    model_max_context(model_name)
}

pub fn model_price_cliff(model_name: &str) -> Option<u32> {
    ensure_disk_loaded();
    get_cached_model_price_cliff(model_name)
}

pub fn model_max_context(model_name: &str) -> usize {
    ensure_disk_loaded();

    if let Some(cached) = get_cached_model_context(model_name) {
        return cached;
    }
    let lower = model_name.to_lowercase();
    if let Some(cached) = get_cached_model_context(&lower) {
        return cached;
    }

    if let Some((_, suffix)) = model_name.rsplit_once('/')
        && let Some(cached) = get_cached_model_context(suffix)
    {
        return cached;
    }
    256_000
}

pub fn model_context_info(model_name: &str) -> (usize, String) {
    let tokens = resolve_model_context_length(model_name);
    let label = format_context_length(tokens);
    (tokens, label)
}

pub fn estimate_str_tokens(s: &str) -> usize {
    (s.len() as f64 / 4.0).ceil() as usize
}

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
            0,
            ((reserve as f64 / window as f64) * 100.0).round() as usize,
        ];
        let used: usize = cells[0] + cells[1] + cells[2] + cells[3] + cells[4] + cells[6];
        cells[5] = 100usize.saturating_sub(used.min(100));

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
