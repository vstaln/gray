use super::*;

pub(super) static MODEL_PRICE_CLIFFS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, u32>>,
> = std::sync::OnceLock::new();

pub(super) fn price_cliff_map() -> &'static std::sync::RwLock<std::collections::HashMap<String, u32>>
{
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

pub(super) static MODEL_CONTEXT_CACHE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, usize>>,
> = std::sync::OnceLock::new();

pub(super) fn model_context_cache()
-> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
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

pub fn cache_model_context_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "litellm", false);
}

pub fn cache_models_dev_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "models.dev", false);
}

pub(super) fn cache_model_context_with_source(
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

pub(super) static MODEL_CONTEXT_SOURCE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, &'static str>>,
> = std::sync::OnceLock::new();

pub(super) fn model_context_source_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, &'static str>> {
    MODEL_CONTEXT_SOURCE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub(super) fn record_context_source(model_id: &str, src: &'static str, overwrite: bool) {
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

pub(super) fn get_cached_source(model_id: &str) -> Option<&'static str> {
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

pub(super) fn json_usize(v: &serde_json::Value) -> Option<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| v.as_f64().map(|f| f as usize))
}
