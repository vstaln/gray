use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

use gray_plugin::{ModelSlot, ModelVariant};
use serde::{Deserialize, Serialize};

pub(crate) type Parts = BTreeMap<String, String>;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RowShape {
    #[serde(default)]
    pub variants: Vec<ModelVariant>,
    #[serde(default)]
    pub slots: Vec<ModelSlot>,
}

impl RowShape {
    pub(crate) fn is_composite(&self) -> bool {
        !self.slots.is_empty()
    }

    pub(crate) fn option_name(&self, key: &str, option: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.key == key)?
            .options
            .iter()
            .find(|o| o.id == option)
            .map(|o| o.name.as_str())
    }

    pub(crate) fn serves(&self, parts: &Parts) -> bool {
        self.variants.iter().any(|v| v.parts == *parts)
    }

    pub(crate) fn settle_parts(&self, want: Option<&Parts>) -> Parts {
        match want {
            Some(p) if self.serves(p) => p.clone(),
            _ => self
                .variants
                .first()
                .map(|v| v.parts.clone())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn option_parts(&self, cur: &Parts, key: &str, option: &str) -> Option<Parts> {
        let slot = self.slots.iter().position(|s| s.key == key)?;
        let mut want = cur.clone();
        want.insert(key.to_string(), option.to_string());
        if self.serves(&want) {
            return Some(want);
        }
        let earlier: Vec<&String> = self.slots[..slot].iter().map(|s| &s.key).collect();
        self.variants
            .iter()
            .find(|v| {
                v.parts.get(key).map(String::as_str) == Some(option)
                    && earlier.iter().all(|k| v.parts.get(*k) == cur.get(*k))
            })
            .map(|v| v.parts.clone())
    }
}

static SHAPES: OnceLock<RwLock<HashMap<String, Arc<RowShape>>>> = OnceLock::new();

pub(crate) fn cache_row_shape(id: &str, variants: Vec<ModelVariant>, slots: Vec<ModelSlot>) {
    let Ok(mut map) = SHAPES.get_or_init(Default::default).write() else {
        return;
    };
    if variants.is_empty() && slots.is_empty() {
        map.remove(id);
    } else {
        map.insert(id.to_string(), Arc::new(RowShape { variants, slots }));
    }
}

pub(crate) fn row_shape(id: &str) -> Option<Arc<RowShape>> {
    SHAPES.get()?.read().ok()?.get(id).cloned()
}

fn norm_level(level: &str) -> &str {
    if level.eq_ignore_ascii_case("none") {
        "off"
    } else {
        level
    }
}

fn level_rank(level: &str) -> Option<usize> {
    let level = norm_level(level);
    super::THINKING_LEVELS.iter().position(|(l, _)| *l == level)
}

pub(crate) fn static_level(level: &str) -> Option<&'static str> {
    level_rank(level).map(|i| super::THINKING_LEVELS[i].0)
}

fn candidates<'a>(
    variants: &'a [ModelVariant],
    fast: bool,
    parts: &'a Parts,
) -> impl Iterator<Item = &'a ModelVariant> + 'a {
    variants
        .iter()
        .filter(move |v| v.fast == fast && v.parts == *parts)
}

pub(crate) fn has_variant(variants: &[ModelVariant], fast: bool, parts: &Parts) -> bool {
    candidates(variants, fast, parts).next().is_some()
}

pub(crate) fn effective_fast(variants: &[ModelVariant], fast: bool, parts: &Parts) -> bool {
    fast && has_variant(variants, true, parts)
}

pub(crate) fn resolve_variant<'a>(
    variants: &'a [ModelVariant],
    effort: Option<&str>,
    fast: bool,
    parts: &Parts,
) -> Option<&'a ModelVariant> {
    let pick = |fast: bool| -> Option<&'a ModelVariant> {
        let cands: Vec<&'a ModelVariant> = variants
            .iter()
            .filter(|v| v.fast == fast && v.parts == *parts)
            .collect();
        let want = effort.map(norm_level);
        if let Some(want) = want
            && let Some(v) = cands
                .iter()
                .find(|v| v.effort.as_deref().map(norm_level) == Some(want))
        {
            return Some(*v);
        }
        if let Some(v) = cands.iter().find(|v| v.effort.is_none()) {
            return Some(*v);
        }
        let Some(target) = want.and_then(level_rank) else {
            return cands.first().copied();
        };
        cands
            .iter()
            .filter_map(|v| Some((level_rank(v.effort.as_deref()?)?, *v)))
            .min_by_key(|(rank, _)| (rank.abs_diff(target), *rank < target))
            .map(|(_, v)| v)
            .or_else(|| cands.first().copied())
    };
    pick(fast).or_else(|| if fast { pick(false) } else { None })
}

pub(crate) fn variant_levels(
    variants: &[ModelVariant],
    fast: bool,
    parts: &Parts,
) -> Vec<&'static str> {
    let fast = effective_fast(variants, fast, parts);
    let mut levels: Vec<&'static str> = candidates(variants, fast, parts)
        .filter_map(|v| static_level(v.effort.as_deref()?))
        .collect();
    levels.sort_by_key(|l| level_rank(l));
    levels.dedup();
    levels
}

pub(crate) fn clamp_to_levels(levels: &[&'static str], want: &str) -> Option<&'static str> {
    if let Some(l) = levels.iter().find(|l| **l == norm_level(want)) {
        return Some(l);
    }
    let target = level_rank(want).unwrap_or(0);
    levels
        .iter()
        .filter_map(|l| Some((level_rank(l)?, *l)))
        .min_by_key(|(rank, _)| (rank.abs_diff(target), *rank < target))
        .map(|(_, l)| l)
}

fn short_name(name: &str) -> &str {
    name.strip_prefix("Claude ").unwrap_or(name)
}

pub(crate) fn composite_label_parts(
    shape: &RowShape,
    row_name: &str,
    parts: &Parts,
    effort: Option<&str>,
) -> Option<Vec<(String, Option<usize>)>> {
    if !shape.is_composite() {
        return None;
    }
    let mut segs: Vec<(String, Option<usize>)> = vec![(format!("{row_name} · "), None)];
    for (i, slot) in shape.slots.iter().enumerate() {
        let id = parts.get(&slot.key)?;
        let name = short_name(shape.option_name(&slot.key, id).unwrap_or(id));
        if i > 0 {
            segs.push((" × ".to_string(), None));
        }
        segs.push((name.to_string(), Some(i)));
        if let Some(e) = effort.filter(|e| i == 0 && *e != "off") {
            segs.push((format!(" {}", effort_word(e)), None));
        }
    }
    Some(segs)
}

pub(crate) fn effort_word(level: &str) -> String {
    if level == "xhigh" {
        return "XHigh".to_string();
    }
    let mut chars = level.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum CachedRow {
    Pair(String, String),
    Full(CachedModel),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<ModelVariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<ModelSlot>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub declared: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cliff_tokens: Option<u32>,
}

impl CachedRow {
    pub(crate) fn id(&self) -> &str {
        match self {
            CachedRow::Pair(id, _) => id,
            CachedRow::Full(m) => &m.id,
        }
    }

    pub(crate) fn pair(&self) -> (String, String) {
        match self {
            CachedRow::Pair(id, name) => (id.clone(), name.clone()),
            CachedRow::Full(m) => (
                m.id.clone(),
                if m.name.is_empty() {
                    m.id.clone()
                } else {
                    m.name.clone()
                },
            ),
        }
    }

    pub(crate) fn register(&self) {
        let CachedRow::Full(m) = self else {
            return;
        };
        if m.declared {
            if !m.reasoning_efforts.is_empty() {
                super::context::cache_model_efforts(&m.id, m.reasoning_efforts.clone());
            }
            super::context::cache_model_reasoning(&m.id, !m.reasoning_efforts.is_empty());
        }
        cache_row_shape(&m.id, m.variants.clone(), m.slots.clone());
        super::context::cache_model_price_cliff(&m.id, m.price_cliff_tokens);
    }
}

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;
