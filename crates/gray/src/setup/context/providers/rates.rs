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

pub(super) static MODEL_RATES: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelRate>>,
> = std::sync::OnceLock::new();

pub(super) fn model_rates_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, ModelRate>> {
    MODEL_RATES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub(super) static PENDING_TIERS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>>,
> = std::sync::OnceLock::new();

pub(super) fn pending_tiers_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>> {
    PENDING_TIERS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub(super) fn stash_pending_tiers(model_id: &str, tiers: Vec<RateTier>) {
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

pub(super) fn take_pending_tiers(model_id: &str) -> Vec<RateTier> {
    pending_tiers_cell()
        .write()
        .ok()
        .and_then(|mut g| g.remove(model_id))
        .unwrap_or_default()
}

pub(super) fn merge_tiers(existing: &mut Vec<RateTier>, incoming: Vec<RateTier>) {
    for t in incoming {
        match existing.iter_mut().find(|e| e.size == t.size) {
            Some(e) => *e = t,
            None => existing.push(t),
        }
    }
    existing.sort_by_key(|tier| std::cmp::Reverse(tier.size));
}

pub(super) fn cache_model_rate(model_id: &str, rate: ModelRate) {
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

pub(super) fn cache_models_dev_rate(model_id: &str, base: Option<ModelRate>, tiers: Vec<RateTier>) {
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

pub(super) fn json_rate(v: &serde_json::Value) -> Option<f64> {
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

pub(super) fn rate_at_context(r: &ModelRate, context: usize) -> (f64, f64, f64, f64, bool) {
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
