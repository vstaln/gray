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

pub(super) static MODEL_REASONING: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, bool>>,
> = std::sync::OnceLock::new();

pub(super) fn model_reasoning_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
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

pub(super) static MODEL_EFFORTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

pub(super) fn model_efforts_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<String>>> {
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

pub(super) fn model_efforts(model_id: &str) -> Option<Vec<String>> {
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

pub(super) static MODEL_META: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelMeta>>,
> = std::sync::OnceLock::new();

pub(super) fn model_meta_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, ModelMeta>> {
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
        let levels = super::super::super::THINKING_LEVELS;
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
        if seg == "none"
            || super::super::super::THINKING_LEVELS
                .iter()
                .any(|(l, _)| *l == seg)
        {
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
        return super::super::super::THINKING_LEVELS.to_vec();
    };
    super::super::super::THINKING_LEVELS
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
    let order: Vec<&str> = super::super::super::THINKING_LEVELS
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
