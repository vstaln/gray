//! Structured model variants from plugin catalogs: one picker row owns the
//! concrete wire ids behind its effort / fast / composite-part knobs
//! (`ProviderModel::variants` + `slots`). Rows without variants keep the
//! legacy path — the row id is the wire id, effort travels as a parameter
//! and fast mode composes heuristically ([`super::compose_fast_model`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

use gray_plugin::{ModelSlot, ModelVariant};
use serde::{Deserialize, Serialize};

/// Slot key → option id for a composite row (`{"lead": …, "sidekick": …}`).
pub(crate) type Parts = BTreeMap<String, String>;

/// What a row declares beyond its id and name.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RowShape {
    pub variants: Vec<ModelVariant>,
    pub slots: Vec<ModelSlot>,
}

impl RowShape {
    pub(crate) fn is_composite(&self) -> bool {
        !self.slots.is_empty()
    }

    /// Display name of `option` in slot `key`, if declared.
    pub(crate) fn option_name(&self, key: &str, option: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.key == key)?
            .options
            .iter()
            .find(|o| o.id == option)
            .map(|o| o.name.as_str())
    }

    /// Whether some variant serves exactly `parts`.
    pub(crate) fn serves(&self, parts: &Parts) -> bool {
        self.variants.iter().any(|v| v.parts == *parts)
    }

    /// The parts a fresh selection opens at: `want` when a variant serves
    /// it, else the first variant's (the catalog's own default).
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
}

static SHAPES: OnceLock<RwLock<HashMap<String, Arc<RowShape>>>> = OnceLock::new();

/// Record what a catalog declared for row `id`. A declaration with neither
/// variants nor slots is authoritative too: it clears an older shape.
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

/// The declared shape of row `id`, when its catalog had variants or slots.
pub(crate) fn row_shape(id: &str) -> Option<Arc<RowShape>> {
    SHAPES.get()?.read().ok()?.get(id).cloned()
}

/// `none` is the wire spelling of `off` in variant ids; both mean no
/// thinking.
fn norm_level(level: &str) -> &str {
    if level.eq_ignore_ascii_case("none") {
        "off"
    } else {
        level
    }
}

/// Position in `THINKING_LEVELS` (`off` = 0 … `max`).
fn level_rank(level: &str) -> Option<usize> {
    let level = norm_level(level);
    super::THINKING_LEVELS.iter().position(|(l, _)| *l == level)
}

/// The `THINKING_LEVELS` spelling of `level` (`'static`, for picker state).
pub(crate) fn static_level(level: &str) -> Option<&'static str> {
    level_rank(level).map(|i| super::THINKING_LEVELS[i].0)
}

/// The variants a selection may resolve to: same fast flag, same parts.
fn candidates<'a>(
    variants: &'a [ModelVariant],
    fast: bool,
    parts: &'a Parts,
) -> impl Iterator<Item = &'a ModelVariant> + 'a {
    variants
        .iter()
        .filter(move |v| v.fast == fast && v.parts == *parts)
}

/// Whether any variant serves `parts` with this `fast` flag.
pub(crate) fn has_variant(variants: &[ModelVariant], fast: bool, parts: &Parts) -> bool {
    candidates(variants, fast, parts).next().is_some()
}

/// The fast flag a selection actually runs at: fast only where a fast
/// variant exists for `parts` (resolution falls back to standard serving).
pub(crate) fn effective_fast(variants: &[ModelVariant], fast: bool, parts: &Parts) -> bool {
    fast && has_variant(variants, true, parts)
}

/// Resolve a row's selection to its concrete wire variant.
///
/// Candidates are the variants whose `fast` and `parts` match. Among them:
/// an exact `effort` match wins; else one pinned to no effort (`None`, the
/// provider default); else the nearest effort in `THINKING_LEVELS` order
/// (ties go up — the clamp's direction). With no fast candidate the
/// selection falls back to `fast = false`. `None` when nothing serves
/// `parts` at all.
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

/// Effort levels a selection can step through: the pinned efforts among
/// its candidates, in `THINKING_LEVELS` order (fast falls back like
/// [`resolve_variant`]). Empty = no effort knob.
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

/// `want` if `levels` has it, else the nearest level (ties go up). `None`
/// for an empty list.
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

/// Drop a leading vendor word so labels stay short (`Claude Opus 5.5` →
/// `Opus 5.5`).
fn short_name(name: &str) -> &str {
    name.strip_prefix("Claude ").unwrap_or(name)
}

/// Footer / status label for a composite selection: `Fusion · Opus 5.5
/// High + SWE-2 High` — row name, then each slot's option name, the first
/// (lead) carrying the effort it runs at. `None` for a non-composite row.
pub(crate) fn composite_label(
    shape: &RowShape,
    row_name: &str,
    parts: &Parts,
    effort: Option<&str>,
) -> Option<String> {
    if !shape.is_composite() {
        return None;
    }
    let mut names = Vec::new();
    for (i, slot) in shape.slots.iter().enumerate() {
        let id = parts.get(&slot.key)?;
        let name = short_name(shape.option_name(&slot.key, id).unwrap_or(id));
        match effort.filter(|e| i == 0 && *e != "off") {
            Some(e) => names.push(format!("{name} {}", effort_word(e))),
            None => names.push(name.to_string()),
        }
    }
    Some(format!("{row_name} · {}", names.join(" + ")))
}

/// `high` → `High`, `xhigh` → `XHigh`.
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

/// A `provider_models.json` row: the legacy `["id", "name"]` pair, or an
/// object carrying the catalog metadata plugin rows declare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum CachedRow {
    Pair(String, String),
    Full(CachedModel),
}

/// The object form of a cached row (only written when there is metadata).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Plugin catalogs declare efforts authoritatively: an empty list means
    /// "no effort knob", which a reload must restore too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub declared: bool,
}

impl CachedRow {
    pub(crate) fn id(&self) -> &str {
        match self {
            CachedRow::Pair(id, _) => id,
            CachedRow::Full(m) => &m.id,
        }
    }

    /// The `(id, name)` pair every list consumer works with.
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

    /// Feed a cached object's metadata to the in-memory caches the picker
    /// and the request builder read (a pair carries none).
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
    }
}

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;
