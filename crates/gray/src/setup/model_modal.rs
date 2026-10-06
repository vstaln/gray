//! Provider picker modal (split from `setup`).

use super::*;

/// Direct `/model <id>` validation for the live connection — same candidate
/// list as the picker: `provider/models` for plugin connections, a live
/// `/models` fetch otherwise.
pub(crate) fn provider_models_for_config(config: &Config) -> (String, Vec<(String, String)>) {
    let (name, _key, plugin) = picker_scope(config);
    let models = match plugin {
        Some(installed) => super::context::fetch_plugin_provider_models(installed),
        None => picker_models_for(&config.base_url, config.api_key.as_deref()),
    };
    (name, models)
}

/// The provider behind the live config, as the picker sees it. Plugin
/// connections own their list via the sidecar's `provider/models` RPC —
/// the declared base_url is a shared relay placeholder an HTTP fetch can
/// never satisfy — so the cache key is `plugin:<provider_id>` (see
/// [`super::catalog::plugin_models_key`]) and the title is the plugin's own
/// name rather than a catalog lookup that can only ever say "Custom".
/// Returns (title, `provider_models.json` key, installed plugin provider).
pub(crate) fn picker_scope(
    config: &Config,
) -> (String, String, Option<crate::providers::InstalledProvider>) {
    if config.uses_plugin_credentials()
        && let Ok(home) = super::catalog::gray_home()
        && let Some(installed) = crate::providers::ProviderRegistry::load_cached(&home)
            .installed()
            .into_iter()
            .find(|p| p.provider_id() == config.provider_id)
    {
        let key = super::catalog::plugin_models_key(&installed.provider_id());
        return (installed.provider.name.clone(), key, Some(installed));
    }
    let catalog = load_catalog().unwrap_or_default();
    let name = catalog
        .iter()
        .find(|(_, p)| p.base_url == config.base_url)
        .map(|(_, p)| p.name.clone())
        .unwrap_or_else(|| "Custom".to_string());
    (name, config.base_url.clone(), None)
}

/// Models we already know without touching the network: the last fetched
/// list for this provider, persisted to disk after every successful fetch.
/// The picker paints from this immediately and refreshes in the background —
/// opening a modal should never wait on a round-trip.
/// `list_key` is the `provider_models.json` slot — a `plugin:` pseudo-key
/// for plugin providers; `sort_base` stays the configured base_url so the
/// current model and recents resolve under their normal key.
pub(crate) fn saved_models_for(list_key: &str, sort_base: &str) -> Vec<(String, String)> {
    let mut models = super::context::load_provider_model_list(list_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(sort_base, &mut models);
    }
    models
}

/// Effort words that may ride at the end of a model id (`swe-2-max`):
/// the tier vocabulary minus `off` — a `-none` row is its own product,
/// never a level of the base. Mirrors the devin-sub `EFFORT_VARIANTS`
/// list and the clamp's known levels.
pub(crate) const VARIANT_EFFORT_WORDS: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max"];

/// Leaf segments naming a fast-serving variant at the end of a model id:
/// `-fast` on claude/swe-family rows, `-priority` on gpt-family rows
/// (the devin-sub catalog labels them "… Fast" / "… Thinking Fast").
/// Compared case-insensitively — upstream catalogs mix case (`GLM-5.2-Fast`).
pub(crate) const FAST_SUFFIXES: &[&str] = &["fast", "priority"];

/// `Some((base, tier))` when `model` reads as `<base>-<effort>`: the split
/// itself, checked only against the effort vocabulary — no row lookup.
fn effort_variant_shape(model: &str) -> Option<(&str, &str)> {
    let (base, tier) = model.rsplit_once('-')?;
    (!base.is_empty() && VARIANT_EFFORT_WORDS.contains(&tier)).then_some((base, tier))
}

/// A stored or typed id like `swe-2-max`: when the provider's known rows
/// declare `<base>` but not the id itself, the trailing word is the tier —
/// the family collapsed into one picker row whose `reasoning_efforts`
/// own the level. A declared row always wins (`qwen3-max` listed as its
/// own model is never split); `None` when no declared base matches.
pub(crate) fn split_effort_variant(
    model: &str,
    known: &[(String, String)],
) -> Option<(String, String)> {
    let (base, tier) = effort_variant_shape(model)?;
    if known.iter().any(|(id, _)| id == model) {
        return None;
    }
    known
        .iter()
        .any(|(id, _)| id == base)
        .then(|| (base.to_string(), tier.to_string()))
}

/// `Some((stem, suffix))` when `model` ends in a [`FAST_SUFFIXES`] leaf AND
/// the stem resolves back to a declared base — directly (`swe-1-6-fast` →
/// `swe-1-6`) or through one effort tier (`claude-opus-5-5-high-fast` →
/// `claude-opus-5-5-high` → `claude-opus-5-5`). Unlike the effort split a
/// declared `...-fast` row still decomposes: the fast row IS a variant of
/// its base, and keeping base+tier+flag separate preserves the effort knob.
fn split_fast_variant(model: &str, known: &[(String, String)]) -> Option<(String, &'static str)> {
    let (stem, leaf) = model.rsplit_once('-')?;
    if stem.is_empty() {
        return None;
    }
    let suffix = FAST_SUFFIXES
        .iter()
        .find(|s| leaf.eq_ignore_ascii_case(s))?;
    // The stem's canonical form: a declared row wins (its id keeps the
    // catalog's case); an effort-bearing stem resolves through the tier
    // split.
    let stem = match known.iter().find(|(id, _)| id.eq_ignore_ascii_case(stem)) {
        Some((id, _)) => id.clone(),
        None if split_effort_variant(stem, known).is_some() => stem.to_string(),
        None => return None,
    };
    Some((stem, *suffix))
}

/// Decompose a catalog id into `(base, tier, fast)` — a `-fast`/`-priority`
/// leaf peels first (even on declared rows), then the standard
/// `<base>-<effort>` split applies to the stem. Either half may be absent:
/// `swe-1-6-fast` is base+fast with no tier, `swe-2-max` is base+tier with
/// no fast. `None` when nothing decomposes.
pub(crate) fn decompose_model_variant(
    model: &str,
    known: &[(String, String)],
) -> Option<(String, Option<String>, bool)> {
    let (work, fast) = match split_fast_variant(model, known) {
        Some((stem, _)) => (stem, true),
        None => (model.to_string(), false),
    };
    let (base, tier) = split_effort_variant(&work, known)
        .map(|(b, t)| (b, Some(t)))
        .unwrap_or((work, None));
    (fast || tier.is_some()).then_some((base, tier, fast))
}

/// The provider's fast-serving sibling of `model` at `effort`, spelled the
/// way the catalog declares it: `claude-opus-5-5` + `high` →
/// `claude-opus-5-5-high-fast`; `gpt-6-sol` + `off` →
/// `gpt-6-sol-none-priority`; `swe-1-6` + anything → `swe-1-6-fast`.
/// `None` when the catalog has no fast row for the target — the caller
/// then sends `model` unchanged. An id already carrying the suffix is
/// already the fast product and never re-composes.
pub(crate) fn compose_fast_model(
    model: &str,
    effort: Option<&str>,
    known: &[(String, String)],
) -> Option<String> {
    if split_fast_variant(model, known).is_some() {
        return None;
    }
    let mut candidates: Vec<String> = Vec::with_capacity(4);
    let level = match effort {
        Some("off") | Some("") | None => Some("none"),
        Some(l) => Some(l),
    };
    if let Some(l) = level {
        for s in FAST_SUFFIXES {
            candidates.push(format!("{model}-{l}-{s}"));
        }
    }
    for s in FAST_SUFFIXES {
        candidates.push(format!("{model}-{s}"));
    }
    for cand in candidates {
        if let Some((id, _)) = known.iter().find(|(id, _)| id.eq_ignore_ascii_case(&cand)) {
            return Some(id.clone());
        }
    }
    None
}

/// The parts a selection on `shape` resolves with: none for a plain row
/// (a stale `model_parts` from a composite never leaks onto it), the given
/// parts for a composite when a variant serves them, else its default.
pub(crate) fn selection_parts(
    shape: &super::variants::RowShape,
    parts: &super::variants::Parts,
) -> super::variants::Parts {
    if shape.is_composite() {
        shape.settle_parts(Some(parts))
    } else {
        super::variants::Parts::new()
    }
}

/// The wire id a request sends for the live selection. A row with declared
/// variants resolves (effort, fast, parts) to its concrete id
/// ([`super::variants::resolve_variant`]); other rows keep the legacy path —
/// the row id, or its heuristic fast sibling while fast mode is on.
pub(crate) fn wire_model_for(config: &Config) -> Option<String> {
    let model = config.model.as_deref()?;
    // Loads (and registers) the cached catalog the shapes come from.
    let rows = canonical_model_rows(config);
    let fast = config.fast_mode == Some(true);
    if let Some(shape) = super::variants::row_shape(model)
        && !shape.variants.is_empty()
    {
        let parts = selection_parts(&shape, &config.model_parts);
        let effort = config.thinking_effort.as_deref();
        return Some(
            super::variants::resolve_variant(&shape.variants, effort, fast, &parts)
                .map_or_else(|| model.to_string(), |v| v.id.clone()),
        );
    }
    if fast && let Some(id) = compose_fast_model(model, config.thinking_effort.as_deref(), &rows) {
        return Some(id);
    }
    Some(model.to_string())
}

/// Footer / status label for the live selection when it is a composite
/// row (`Fusion · Opus 5.5 High + SWE-2 High`); `None` otherwise.
pub(crate) fn composite_label_for(config: &Config) -> Option<String> {
    let model = config.model.as_deref()?;
    let shape = super::variants::row_shape(model)?;
    let name = load_provider_names(config)
        .into_iter()
        .find(|(id, _)| id == model)
        .map(|(_, n)| n)
        .unwrap_or_else(|| super::context::friendly_model_name(model));
    let parts = selection_parts(&shape, &config.model_parts);
    super::variants::composite_label(&shape, &name, &parts, config.thinking_effort.as_deref())
}

/// The cached `(id, name)` list for the live provider (no network).
fn load_provider_names(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, _) = picker_scope(config);
    super::context::load_provider_model_list(&list_key)
}

/// The row whose declared variants include `wire` (a stored concrete id like
/// `gpt-6-sol-high-priority`), with that variant — `None` when no listed
/// row declares it, or `wire` is itself a row.
fn variant_owner(wire: &str, known: &[(String, String)]) -> Option<(String, ModelVariantRef)> {
    if known.iter().any(|(id, _)| id == wire) {
        return None;
    }
    known.iter().find_map(|(id, _)| {
        let shape = super::variants::row_shape(id)?;
        let v = shape.variants.iter().find(|v| v.id == wire)?;
        Some((
            id.clone(),
            ModelVariantRef {
                effort: v.effort.clone(),
                fast: v.fast,
                parts: v.parts.clone(),
            },
        ))
    })
}

/// The selection a concrete variant id stands for.
struct ModelVariantRef {
    effort: Option<String>,
    fast: bool,
    parts: super::variants::Parts,
}

/// The rows canonicalization may split against: the persisted picker
/// list, plus — for a plugin connection whose stored id still reads as a
/// variant it can't confirm — one live `provider/models` fetch (the
/// persisted list may predate the family collapse, or never have run).
/// Only the variant pattern pays the fetch; a clean id returns instantly.
/// Status-line effort chip: `high` normally, `high·fast` while the fast
/// model variant is on. Display only — the wire id composes in build_agent.
pub(crate) fn effort_chip(eff: &str, config: &Config) -> String {
    if config.fast_mode == Some(true) && eff != "off" {
        format!("{eff}·fast")
    } else {
        eff.to_string()
    }
}

pub(crate) fn canonical_model_rows(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, plugin) = picker_scope(config);
    let known = saved_models_for(&list_key, &config.base_url);
    let stale_variant = config
        .model
        .as_deref()
        .is_some_and(|m| effort_variant_shape(m).is_some() && !known.iter().any(|(id, _)| id == m));
    match (stale_variant, plugin) {
        (true, Some(installed)) => {
            let live = super::context::fetch_plugin_provider_models(installed);
            if live.is_empty() { known } else { live }
        }
        _ => known,
    }
}

/// Adopt a `<base>-<tier>` id as `base` + `tier` effort: a collapsed
/// family row owns the level through the picker, so a stale variant id
/// (`swe-2-max` saved before the collapse) shows the real model, makes
/// the effort knob honest — the variant IS the level it runs at — and
/// keeps the upstream call identical (the plugin maps `base`+`tier` back
/// to the same native id). Returns the applied `(base, tier)`; the model
/// and this target's remembered effort persist under the canonical key.
/// `GRAY_THINKING_EFFORT` stays a user override: the model still
/// canonicalizes but the env level wins.
pub(crate) fn canonicalize_effort_variant(
    config: &mut Config,
    known: &[(String, String)],
) -> Option<(String, Option<String>, bool)> {
    let model = config.model.clone()?;
    // A declared variant id maps back exactly: its row, effort, fast flag
    // and composite parts — no suffix guessing.
    if let Some((row, v)) = variant_owner(&model, known) {
        config.model = Some(row.clone());
        if let Some(e) = &v.effort
            && std::env::var_os("GRAY_THINKING_EFFORT").is_none()
        {
            config.thinking_effort = Some(e.clone());
        }
        if std::env::var_os("GRAY_FAST").is_none() {
            config.fast_mode = Some(v.fast);
        }
        config.model_parts = v.parts.clone();
        if let Ok(path) = saved_config_path() {
            let _cfg_lock = lock_saved_config_at(&path).ok();
            let mut saved = load_saved_config_at(&path);
            saved.model = Some(row.clone());
            let key = crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &row);
            if let Some(e) = &v.effort {
                saved.remember_effort(&key, e);
            }
            saved.remember_parts(&key, &v.parts);
            saved.fast_mode = config.fast_mode;
            let _ = save_saved_config_at(&path, &saved);
        }
        return Some((row, v.effort, v.fast));
    }
    let (base, tier, fast) = decompose_model_variant(&model, known)?;
    config.model = Some(base.clone());
    if let Some(t) = &tier
        && std::env::var_os("GRAY_THINKING_EFFORT").is_none()
    {
        config.thinking_effort = Some(t.clone());
    }
    if fast && std::env::var_os("GRAY_FAST").is_none() {
        config.fast_mode = Some(true);
    }
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = lock_saved_config_at(&path).ok();
        let mut saved = load_saved_config_at(&path);
        saved.model = Some(base.clone());
        if let Some(t) = &tier {
            saved.remember_effort(
                &crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &base),
                t,
            );
        }
        saved.thinking_effort = config.thinking_effort.clone();
        if fast {
            saved.fast_mode = Some(true);
        }
        let _ = save_saved_config_at(&path, &saved);
    }
    Some((base, tier, fast))
}

/// The saved current model + recents for this provider: the same source
/// `sort_models` parks first, so the count below is the divider position.
fn recent_head_for(base_url: &str) -> (Option<String>, Vec<String>) {
    let Ok(path) = saved_config_path() else {
        return (None, Vec::new());
    };
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let saved = load_saved_config_at(&path);
    let base = normalize_custom_base_url(base_url);
    let current = saved
        .base_url
        .as_deref()
        .filter(|url| normalize_custom_base_url(url) == base)
        .and(saved.model.clone());
    let recent = saved.recent_models.get(&base).cloned().unwrap_or_default();
    (current, recent)
}

/// Leading run of the (already sorted) list that is the current model or a
/// recent one. 0 or full-length means no divider.
fn recent_prefix_len<'a>(
    current: Option<&str>,
    recent: &[String],
    ids: impl Iterator<Item = &'a str>,
) -> usize {
    ids.take_while(|id| Some(*id) == current || recent.iter().any(|m| m.as_str() == *id))
        .count()
}

/// A picker row: a model by index into the filtered list, or the
/// recent/all divider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Model(usize),
    Divider,
    /// Option `n` of the open composite dropdown (inline under its row).
    Opt(usize),
}

/// Selection may never rest on the divider: after any move, step once more
/// in the direction of travel. The divider always has models on both sides
/// (it only renders when `0 < sep < len`), so one step suffices.
fn skip_divider(rows: &[Row], sel: usize, down: bool) -> usize {
    if rows.get(sel) == Some(&Row::Divider) {
        if down {
            sel.saturating_add(1).min(rows.len().saturating_sub(1))
        } else {
            sel.saturating_sub(1)
        }
    } else {
        sel
    }
}

/// Levels a row offers: for a row with declared variants, the efforts of
/// the variants matching `fast` and its (settled) `parts`; otherwise the
/// model's `supported_thinking_levels`.
fn row_levels(model: &str, fast: bool, parts: &super::variants::Parts) -> Vec<&'static str> {
    if let Some(shape) = super::variants::row_shape(model)
        && !shape.variants.is_empty()
    {
        let parts = selection_parts(&shape, parts);
        return super::variants::variant_levels(&shape.variants, fast, &parts);
    }
    super::context::supported_thinking_levels(model)
        .iter()
        .map(|(l, _)| *l)
        .collect()
}

/// Whether `model` is a row with declared variants.
fn has_declared_variants(model: &str) -> bool {
    super::variants::row_shape(model).is_some_and(|s| !s.variants.is_empty())
}

/// The effort a picker row opens at: the live level for the current model
/// (what it actually runs at — and every row while `GRAY_THINKING_EFFORT`
/// overrides), else the model's remembered level, else the `high` default,
/// clamped to what the model accepts. `None` when the model has no levels
/// to choose between — the row shows no effort control at all.
#[cfg(test)]
fn initial_row_effort(config: &Config, saved: &SavedConfig, model: &str) -> Option<&'static str> {
    let levels = row_levels(model, false, &config.model_parts);
    initial_effort_in(config, saved, model, &levels)
}

/// [`initial_row_effort`] over an explicit level list (variant rows offer
/// different levels per fast flag and composite parts).
fn initial_effort_in(
    config: &Config,
    saved: &SavedConfig,
    model: &str,
    levels: &[&'static str],
) -> Option<&'static str> {
    if levels.len() <= 1 {
        return None;
    }
    let live = (config.model.as_deref() == Some(model)
        || std::env::var_os("GRAY_THINKING_EFFORT").is_some())
    .then(|| config.thinking_effort.clone())
    .flatten();
    let want = live
        .or_else(|| {
            saved.remembered_effort(&crate::setup::effort_memory_key(
                &config.provider_id,
                &config.base_url,
                model,
            ))
        })
        .unwrap_or_else(|| "high".to_string());
    if has_declared_variants(model) {
        super::variants::clamp_to_levels(levels, &want)
    } else {
        Some(super::context::clamp_thinking_level(model, &want))
    }
}

/// Per-row picker state while the picker is open: ←/→ steps the
/// highlighted row through its levels; untouched rows show
/// [`initial_row_effort`]. Variant rows read their levels from the pending
/// fast flag and composite parts held here. Nothing persists until Enter.
#[derive(Default)]
struct RowEfforts {
    /// Stepped level per effort key ([`RowEfforts::key`]).
    stepped: std::collections::HashMap<String, &'static str>,
    /// The picker's pending fast toggle (variant rows offer the efforts of
    /// the matching fast set).
    fast: bool,
    /// Pending composite selections, per row.
    parts: std::collections::HashMap<String, super::variants::Parts>,
}

impl RowEfforts {
    /// The composite parts `model`'s row shows: picked this session, else
    /// the live selection (current model), else the row's remembered parts,
    /// else the catalog default. Empty for non-composite rows.
    fn parts_of(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
    ) -> super::variants::Parts {
        let Some(shape) = super::variants::row_shape(model).filter(|s| s.is_composite()) else {
            return super::variants::Parts::new();
        };
        if let Some(p) = self.parts.get(model) {
            return p.clone();
        }
        let want = if config.model.as_deref() == Some(model) {
            Some(config.model_parts.clone())
        } else {
            saved
                .parts_memory
                .get(&crate::setup::effort_memory_key(
                    &config.provider_id,
                    &config.base_url,
                    model,
                ))
                .cloned()
        };
        shape.settle_parts(want.as_ref())
    }

    /// Effort state key: the row id, plus the lead option for a composite
    /// (each lead keeps its own level, as in Devin's picker).
    fn key(model: &str, parts: &super::variants::Parts) -> String {
        match super::variants::row_shape(model)
            .filter(|s| s.is_composite())
            .and_then(|s| {
                s.slots
                    .first()
                    .and_then(|slot| parts.get(&slot.key))
                    .cloned()
            }) {
            Some(lead) => format!("{model}\u{1f}{lead}"),
            None => model.to_string(),
        }
    }

    /// The level `model` shows with `parts` (and Enter applies there).
    fn shown_with(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        parts: &super::variants::Parts,
    ) -> Option<&'static str> {
        let levels = row_levels(model, self.fast, parts);
        // Knob existence rides the standard serving: fast narrows into a
        // single level but the row still has a knob, while a single-level
        // standard serving means no knob at all.
        let knob_levels = row_levels(model, false, parts);
        let initial = initial_effort_in(config, saved, model, &knob_levels)?;
        let want = self
            .stepped
            .get(&Self::key(model, parts))
            .copied()
            .unwrap_or(initial);
        if levels.contains(&want) {
            Some(want)
        } else {
            super::variants::clamp_to_levels(&levels, want).or(Some(initial))
        }
    }

    /// The level `model`'s row shows (and Enter applies).
    fn shown(&self, config: &Config, saved: &SavedConfig, model: &str) -> Option<&'static str> {
        let parts = self.parts_of(config, saved, model);
        self.shown_with(config, saved, model, &parts)
    }

    /// Whether ←/→ was pressed on `model`'s row this session — an explicit
    /// pick, which wins over `GRAY_THINKING_EFFORT` on commit.
    fn was_stepped(&self, model: &str) -> bool {
        self.stepped
            .keys()
            .any(|k| k == model || k.starts_with(&format!("{model}\u{1f}")))
    }

    /// One level up (`forward`) or down at `parts`, clamped at the ends. A
    /// selection without levels ignores the step.
    fn step_with(
        &mut self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        parts: &super::variants::Parts,
        forward: bool,
    ) {
        let Some(cur) = self.shown_with(config, saved, model, parts) else {
            return;
        };
        let levels = row_levels(model, self.fast, parts);
        let pos = levels.iter().position(|l| *l == cur).unwrap_or(0);
        let next = if forward {
            (pos + 1).min(levels.len().saturating_sub(1))
        } else {
            pos.saturating_sub(1)
        };
        if let Some(level) = levels.get(next) {
            self.stepped.insert(Self::key(model, parts), level);
        }
    }

    /// [`RowEfforts::step_with`] at the row's own parts.
    fn step(&mut self, config: &Config, saved: &SavedConfig, model: &str, forward: bool) {
        let parts = self.parts_of(config, saved, model);
        self.step_with(config, saved, model, &parts, forward);
    }
}

/// Commit the effort an Enter picks alongside the model: the level the row
/// showed lands on `config` and in `effort_memory` under the (canonical)
/// chosen model. A row nobody stepped defers to `GRAY_THINKING_EFFORT`;
/// a model without levels restores its remembered level or the default
/// ([`super::provider_auth::apply_connection_effort`]).
fn commit_row_effort(
    config: &mut Config,
    saved: &mut SavedConfig,
    shown: Option<&str>,
    stepped: bool,
) {
    match shown {
        Some(eff) if stepped || std::env::var_os("GRAY_THINKING_EFFORT").is_none() => {
            config.thinking_effort = Some(eff.to_string());
            saved.remember_effort(
                &crate::setup::effort_memory_key(
                    &config.provider_id,
                    &config.base_url,
                    config.model.as_deref().unwrap_or_default(),
                ),
                eff,
            );
        }
        _ => super::provider_auth::apply_connection_effort(config, saved),
    }
}

/// Pending picker toggles: Tab flips fast mode, ctrl+r the reasoning-text
/// display. Both stay local until Enter commits them with the model, so
/// Esc really cancels everything the picker touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PickerToggles {
    fast: bool,
    show_reasoning: bool,
    opened_fast: bool,
    opened_show: bool,
}

impl PickerToggles {
    fn from_config(config: &Config) -> Self {
        let fast = config.fast_mode == Some(true);
        let show_reasoning = config.show_reasoning.unwrap_or(true);
        Self {
            fast,
            show_reasoning,
            opened_fast: fast,
            opened_show: show_reasoning,
        }
    }

    /// Tab: flip fast mode — only while the highlighted model has a fast
    /// variant to send, so the toggle never claims a speedup it can't route.
    fn toggle_fast(&mut self, supported: bool) {
        if supported {
            self.fast = !self.fast;
        }
    }

    fn toggle_reasoning(&mut self) {
        self.show_reasoning = !self.show_reasoning;
    }

    /// Enter: land what changed on `config` and the saved file; an
    /// untouched toggle leaves both alone.
    fn commit(&self, config: &mut Config, saved: &mut SavedConfig) {
        if self.fast != self.opened_fast {
            config.fast_mode = Some(self.fast);
            saved.fast_mode = Some(self.fast);
        }
        if self.show_reasoning != self.opened_show {
            config.show_reasoning = Some(self.show_reasoning);
            saved.show_reasoning = Some(self.show_reasoning);
        }
    }
}

/// What a key does in the model picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerKey {
    Up,
    Down,
    PageUp,
    PageDown,
    EffortDown,
    EffortUp,
    ToggleFast,
    ToggleReasoning,
    Confirm,
    Cancel,
    Type(char),
    Backspace,
}

/// The picker keymap. ←/→ never edit the filter (it has no cursor), so
/// they are free for effort; ctrl+r is free inside the modal.
fn picker_key(
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<PickerKey> {
    use crossterm::event::{KeyCode, KeyModifiers};
    if modifiers.contains(KeyModifiers::CONTROL) {
        return match code {
            KeyCode::Char('c') => Some(PickerKey::Cancel),
            KeyCode::Char('p') => Some(PickerKey::Up),
            KeyCode::Char('n') => Some(PickerKey::Down),
            KeyCode::Char('r') => Some(PickerKey::ToggleReasoning),
            _ => None,
        };
    }
    Some(match code {
        KeyCode::Up => PickerKey::Up,
        KeyCode::Down => PickerKey::Down,
        KeyCode::PageUp => PickerKey::PageUp,
        KeyCode::PageDown => PickerKey::PageDown,
        KeyCode::Left => PickerKey::EffortDown,
        KeyCode::Right => PickerKey::EffortUp,
        KeyCode::Tab => PickerKey::ToggleFast,
        KeyCode::Enter => PickerKey::Confirm,
        KeyCode::Esc => PickerKey::Cancel,
        KeyCode::Backspace => PickerKey::Backspace,
        KeyCode::Char(ch) => PickerKey::Type(ch),
        _ => return None,
    })
}

/// Display word for an effort level: `high` → `High`, `xhigh` → `XHigh`.
fn effort_label(level: &str) -> String {
    super::variants::effort_word(level)
}

/// Cells in a row's effort meter: one per non-`off` tier in
/// `THINKING_LEVELS`, so no two levels of any model draw the same bar.
const METER_W: usize = 6;

/// Filled cells of a `width`-cell meter for `level` among a model's
/// supported `levels`: `off` (or a level the model lacks) draws empty, the
/// top level full, the ones between spread evenly — with `width` at least
/// the tier count, every step moves the bar.
fn meter_fill(levels: &[&str], level: &str, width: usize) -> usize {
    let tiers: Vec<&str> = levels.iter().copied().filter(|l| *l != "off").collect();
    let Some(k) = tiers.iter().position(|l| *l == level) else {
        return 0;
    };
    let n = tiers.len();
    ((k + 1) * width * 2 + n) / (2 * n)
}

/// The visible slice of a `total`-row list in `cap` slots that keeps `sel`
/// in view, as `(top, shown)`. On overflow a slot at either end turns into
/// a `↑ more above` / `↓ more below` hint — only on a side that actually
/// hides rows — instead of clipping silently.
fn list_window(total: usize, sel: usize, top: usize, cap: usize) -> (usize, usize) {
    if total <= cap {
        return (0, total);
    }
    let sel = sel.min(total - 1);
    if cap < 3 {
        // No room for hints: plain scrolling.
        let top = top.min(sel).max((sel + 1).saturating_sub(cap));
        return (top.min(total - cap), cap);
    }
    // Never scroll past the point where the tail fills the slots.
    let mut top = top.min(sel).min(total - (cap - 1));
    loop {
        let room = cap - usize::from(top > 0);
        let shown = room - usize::from(top + room < total);
        if sel < top + shown {
            return (top, shown);
        }
        top += sel + 1 - (top + shown);
    }
}

/// `$3`, `$0.3`, `$1.25`, `$0.075`: a per-token USD rate as dollars per
/// million tokens, trailing zeros trimmed.
fn per_million(rate: f64) -> String {
    let s = format!("{:.3}", rate * 1_000_000.0);
    format!("${}", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Cells in the price-level gauge.
const PRICE_DOTS: usize = 5;

/// The model's price level, 1..=PRICE_DOTS, on absolute blended (input +
/// output) $/1M bands — not the listed spread, which put every mid-priced
/// model at the top of the gauge (a $18/1M model sat one dot under the
/// priciest row). A level means the same thing in every provider's list.
fn price_level(blended_per_million: f64) -> usize {
    /// Band edges, roughly log-spaced: sub-$0.50 → 1, …, over $20 → 5.
    const BANDS: [f64; PRICE_DOTS - 1] = [0.5, 2.0, 8.0, 20.0];
    1 + BANDS.iter().filter(|&&b| blended_per_million > b).count()
}

/// `in $2 · cached $0.2 · out $10 / 1M`: the model's per-million-token
/// rates on one line. The cached segment drops first when narrow, then
/// the `/ 1M` suffix; past that the text truncates.
fn rate_text(r: &super::context::ModelRate, room: usize) -> String {
    let build = |cached: bool, suffix: bool| {
        let mut segs = vec![format!("in {}", per_million(r.input))];
        if cached && r.has_cache_prices {
            segs.push(format!("cached {}", per_million(r.cache_read)));
        }
        segs.push(format!("out {}", per_million(r.output)));
        let mut s = segs.join(" · ");
        if suffix {
            s.push_str(" / 1M");
        }
        s
    };
    for (cached, suffix) in [(true, true), (false, true), (false, false)] {
        let s = build(cached, suffix);
        if s.chars().count() <= room {
            return s;
        }
    }
    fit_chars(&build(false, false), room)
}

/// Cell `i` of a `w`-cell price band: green (cheap) through the peach
/// accent to rose (pricey), mixed from the palette.
fn price_gradient(i: usize, w: usize) -> ratatui::style::Color {
    use ratatui::style::Color;
    let t = crate::theme::theme();
    let stops = [t.success, t.accent, t.error_soft];
    let x = if w <= 1 {
        0.0
    } else {
        i as f64 / (w - 1) as f64
    };
    let seg = (x * 2.0).min(1.999);
    let (a, b, f) = (stops[seg as usize], stops[seg as usize + 1], seg.fract());
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * f).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => a,
    }
}

/// A model's context window when some source reported it (live list,
/// models.dev, LiteLLM, the disk cache) — never the family guess.
fn known_context(model: &str) -> Option<usize> {
    super::context::ensure_disk_loaded();
    let tail = model.rsplit('/').next().unwrap_or(model);
    [model, tail]
        .into_iter()
        .find_map(super::context::get_cached_model_context)
}

/// Keep the first `max` chars of `s`, marking a cut with `…`.
fn fit_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

/// Connect-modal list: the last good list now, refreshed in the background.
/// Only a provider never fetched before waits on the network (up to 3s per
/// probed endpoint), which is what made /connect feel stuck.
pub(super) fn cached_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let cached = saved_models_for(base_url, base_url);
    if cached.is_empty() {
        return picker_models_for(base_url, api_key);
    }
    let (base, key) = (base_url.to_string(), api_key.map(str::to_string));
    // Detached: the fetch persists the fresh list to disk for the next open.
    std::thread::spawn(move || super::context::fetch_live_provider_models(&base, key.as_deref()));
    cached
}

/// Merge a live list into what the picker already shows, keeping the
/// saved ordering. Same result as [`picker_models_for`], minus the wait.
fn merge_models(base_url: &str, live: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut models = live;
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

/// Shared ordering for /model and every connect-modal model list.
pub(super) fn picker_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut models = fetch_live_provider_models(base_url, api_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

/// A row's display name: the catalog name, else the raw id.
fn row_name(m: &(String, String)) -> &str {
    if m.1.is_empty() { &m.0 } else { &m.1 }
}

/// Paint one line of the picker panel on its surface background.
fn put_line(frame: &mut ratatui::Frame, at: ratatui::layout::Rect, line: ratatui::text::Line<'_>) {
    let base = ratatui::style::Style::default().bg(crate::theme::theme().surface_bg);
    frame.render_widget(ratatui::widgets::Paragraph::new(line).style(base), at);
}

/// Panel rows around the list: top/bottom padding, the title, the search
/// line between its two rules, and the footer with its gap.
const PICKER_CHROME: usize = 8;

/// Whether the provider serves any fast variant (`-fast` / `-priority`
/// rows that resolve to a listed base) — the gate for fast mode's detail
/// slot.
fn lists_fast_variants(models: &[(String, String)]) -> bool {
    models.iter().any(|(id, _)| {
        super::variants::row_shape(id).is_some_and(|s| s.variants.iter().any(|v| v.fast))
            || split_fast_variant(id, models).is_some()
    })
}

/// Slot lines the detail block needs: the most slots any listed composite
/// row declares.
fn listed_slots(models: &[(String, String)]) -> usize {
    models
        .iter()
        .filter_map(|(id, _)| super::variants::row_shape(id))
        .map(|s| s.slots.len())
        .max()
        .unwrap_or(0)
}

/// Rows the detail block reserves for this list: a leading gap, a line per
/// composite slot, fast mode (and its gap) when the provider lists any fast
/// variant, then one line for price and one for context — each only when
/// some listed model knows it. Fixed per list, so moving the selection
/// never resizes the panel.
fn detail_rows(models: &[(String, String)]) -> usize {
    let fast = lists_fast_variants(models);
    let priced = models
        .iter()
        .any(|(id, _)| super::context::get_model_rate(id).is_some());
    let ctxed = models.iter().any(|(id, _)| known_context(id).is_some());
    let metrics = usize::from(priced) + usize::from(ctxed);
    let head = listed_slots(models) + usize::from(fast);
    let head = if head > 0 {
        head + usize::from(metrics > 0)
    } else {
        0
    };
    match head + metrics {
        0 => 0,
        n => n + 1,
    }
}

/// Cells after a row's name column: gap, the ✓ badge, gap, the meter
/// between its arrows, gap, the effort label (`Minimal` is the widest).
const ROW_TAIL: usize = 2 + 1 + 1 + 2 + METER_W + 2 + 1 + 7;

/// The effort meter's cell pair: filled/hollow hexagons — the family
/// /context already uses for used vs free cells — on a Nerd Font, plain
/// squares everywhere else.
fn meter_glyphs() -> (&'static str, &'static str) {
    if super::icons::has_nerd_font() {
        ("⬢", "⬡")
    } else {
        ("■", "□")
    }
}

/// Secondary shade inside the accent selection bar: the peach pulled
/// toward black so hollow meter cells and dim suffixes stay readable on
/// the bar without fighting the bold primary text.
fn bar_soft() -> ratatui::style::Color {
    use ratatui::style::Color;
    match crate::theme::theme().accent {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.45) as u8,
            (g as f32 * 0.45) as u8,
            (b as f32 * 0.45) as u8,
        ),
        c => c,
    }
}

/// An open composite dropdown: which row, which slot (0 = lead), and the
/// highlighted option.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DropState {
    row: String,
    slot: usize,
    sel: usize,
}

/// Where Enter / Esc move the picker: the list, or slot `n`'s dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickLevel {
    List,
    Slot(usize),
}

/// Enter at `level` on a row with `slots` slots: the next level, and
/// whether the whole selection applies now. A plain row applies from the
/// list; a composite walks its slots (lead → sidekick) and applies on the
/// last.
fn enter_level(level: PickLevel, slots: usize) -> (PickLevel, bool) {
    match level {
        PickLevel::List if slots > 0 => (PickLevel::Slot(0), false),
        PickLevel::List => (PickLevel::List, true),
        PickLevel::Slot(i) if i + 1 < slots => (PickLevel::Slot(i + 1), false),
        PickLevel::Slot(_) => (PickLevel::List, true),
    }
}

/// Esc at `level`: back one level (sidekick → lead → list); `None` at the
/// list cancels the picker.
fn esc_level(level: PickLevel) -> Option<PickLevel> {
    match level {
        PickLevel::List => None,
        PickLevel::Slot(0) => Some(PickLevel::List),
        PickLevel::Slot(i) => Some(PickLevel::Slot(i - 1)),
    }
}

/// The parts choosing `option` in slot `slot` leads to, keeping `cur` where
/// a variant still serves it, else the first variant that agrees with the
/// earlier slots and this option. `None` = no variant serves the option
/// (shown dimmed, skipped).
fn option_parts(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
    option: &str,
) -> Option<super::variants::Parts> {
    let key = &shape.slots.get(slot)?.key;
    let mut want = cur.clone();
    want.insert(key.clone(), option.to_string());
    if shape.serves(&want) {
        return Some(want);
    }
    let earlier: Vec<&String> = shape.slots[..slot].iter().map(|s| &s.key).collect();
    shape
        .variants
        .iter()
        .find(|v| {
            v.parts.get(key).map(String::as_str) == Some(option)
                && earlier.iter().all(|k| v.parts.get(*k) == cur.get(*k))
        })
        .map(|v| v.parts.clone())
}

/// The next selectable option from `from` stepping by `dir` (±1), skipping
/// options no variant serves; stays put at the ends.
fn step_option(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
    from: usize,
    down: bool,
) -> usize {
    let Some(s) = shape.slots.get(slot) else {
        return from;
    };
    let ok = |i: usize| option_parts(shape, cur, slot, &s.options[i].id).is_some();
    let mut i = from;
    loop {
        let next = if down {
            if i + 1 >= s.options.len() {
                return from;
            }
            i + 1
        } else {
            if i == 0 {
                return from;
            }
            i - 1
        };
        if ok(next) {
            return next;
        }
        i = next;
    }
}

/// Where slot `slot`'s dropdown opens: the current option when servable,
/// else the first servable one.
fn open_option(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
) -> usize {
    let Some(s) = shape.slots.get(slot) else {
        return 0;
    };
    let servable = |i: &usize| option_parts(shape, cur, slot, &s.options[*i].id).is_some();
    s.options
        .iter()
        .position(|o| cur.get(&s.key) == Some(&o.id))
        .filter(servable)
        .or_else(|| (0..s.options.len()).find(servable))
        .unwrap_or(0)
}

/// Everything one picker frame paints.
struct PickerView<'a> {
    provider: &'a str,
    refreshing: bool,
    filter: &'a str,
    rows: &'a [Row],
    filtered: &'a [&'a (String, String)],
    models: &'a [(String, String)],
    config: &'a Config,
    saved: &'a SavedConfig,
    efforts: &'a RowEfforts,
    toggles: PickerToggles,
    drop: Option<&'a DropState>,
}

impl PickerView<'_> {
    /// The model id under the selection, if the selection is on a model.
    fn highlighted(&self, sel: usize) -> Option<&str> {
        match self.rows.get(sel) {
            Some(Row::Model(mi)) => self.filtered.get(*mi).map(|(id, _)| id.as_str()),
            Some(Row::Opt(_)) => self.drop.map(|d| d.row.as_str()),
            _ => None,
        }
    }

    /// The parts the detail block and fast line describe for `model`: the
    /// highlighted dropdown option's parts while one is open on it.
    fn live_parts(&self, model: &str) -> super::variants::Parts {
        let parts = self.efforts.parts_of(self.config, self.saved, model);
        if let Some(d) = self.drop.filter(|d| d.row == model)
            && let Some(shape) = super::variants::row_shape(model)
            && let Some(opt) = shape.slots.get(d.slot).and_then(|s| s.options.get(d.sel))
            && let Some(p) = option_parts(&shape, &parts, d.slot, &opt.id)
        {
            return p;
        }
        parts
    }

    /// The wire id Tab's fast mode would send for `model` at its row's
    /// effort — declared fast variants for rows that have them, else the
    /// catalog-spelled fast sibling and `build_agent` compose.
    fn fast_variant(&self, model: &str) -> Option<String> {
        if let Some(shape) = super::variants::row_shape(model)
            && !shape.variants.is_empty()
        {
            let parts = selection_parts(&shape, &self.live_parts(model));
            if !super::variants::has_variant(&shape.variants, true, &parts) {
                return None;
            }
            let effort = self
                .efforts
                .shown_with(self.config, self.saved, model, &parts);
            return super::variants::resolve_variant(&shape.variants, effort, true, &parts)
                .map(|v| v.id.clone());
        }
        let effort = self.efforts.shown(self.config, self.saved, model);
        compose_fast_model(model, effort, self.models)
    }
}

/// One model row: the name, the ✓ current badge, then — for models with
/// levels to pick — the effort meter (arrows on the selected row) and its
/// label. The selected row is the accent bar every gray modal uses: black
/// bold text on peach, secondary marks in the bar's own shade. Rows carry
/// no raw id, except a highlighted row whose display name another row
/// shares: it names its id after the label.
fn picker_row_line(
    v: &PickerView<'_>,
    m: &(String, String),
    selected: bool,
    name_w: usize,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: Color| Style::default().fg(c);
    // `on` is the bar's primary ink; `on_dim` its quiet ink (hollow meter
    // cells, the parts summary, a disambiguated id).
    let (on, on_dim) = (t.on_selection, bar_soft());
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let (id, name) = (m.0.as_str(), row_name(m));

    let mut spans = vec![
        Span::styled(
            if selected {
                format!("{} ", super::icon("arrow"))
            } else {
                "  ".to_string()
            },
            sel(fg(on)),
        ),
        Span::styled(
            fit_chars(name, name_w),
            sel(if selected {
                fg(on).add_modifier(Modifier::BOLD)
            } else {
                fg(Color::White)
            }),
        ),
    ];
    let pad = name_w.saturating_sub(spans[1].content.chars().count()) + 2;
    spans.push(Span::styled(" ".repeat(pad), sel(Style::default())));
    let current = v.config.model.as_deref() == Some(id);
    spans.push(Span::styled(
        if current { "✓" } else { " " },
        sel(fg(if selected { on } else { t.success })),
    ));
    spans.push(Span::styled(" ", sel(Style::default())));
    let mut used = 2 + name_w + 2 + 1 + 1;

    let parts = v.efforts.parts_of(v.config, v.saved, id);
    if let Some(level) = v.efforts.shown_with(v.config, v.saved, id, &parts) {
        let levels = row_levels(id, v.efforts.fast, &parts);
        spans.extend(meter_spans(&levels, level, selected));
        used += 2 + METER_W + 2 + 1 + 7;
    }
    let suffix = sel(fg(if selected { on_dim } else { t.text_dim }));
    let shape = super::variants::row_shape(id).filter(|s| s.is_composite());
    if let Some(shape) = shape {
        // A composite row names its current parts after the meter.
        let summary: Vec<&str> = shape
            .slots
            .iter()
            .filter_map(|slot| {
                let opt = parts.get(&slot.key)?;
                Some(shape.option_name(&slot.key, opt).unwrap_or(opt))
            })
            .collect();
        let room = w.saturating_sub(used + 2);
        if room >= 8 && !summary.is_empty() {
            spans.push(Span::styled(
                format!("  {}", fit_chars(&summary.join(" + "), room)),
                suffix,
            ));
        }
    } else if selected && name != id && v.models.iter().filter(|o| row_name(o) == name).count() > 1
    {
        let room = w.saturating_sub(used + 2);
        if room >= 8 {
            spans.push(Span::styled(format!("  {}", fit_chars(id, room)), suffix));
        }
    }
    // Fill the rest of the bar so selection reads as one solid surface.
    if selected {
        let fill = w.saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>());
        spans.push(Span::styled(
            " ".repeat(fill),
            Style::default().bg(t.accent),
        ));
    }
    Line::from(spans)
}

/// The effort meter cells after a row's badge: step arrows on the selected
/// row, the filled/hollow cells, and the 7-cell label. On the accent bar
/// everything goes black, hollows to the bar's shade.
fn meter_spans(
    levels: &[&'static str],
    level: &'static str,
    selected: bool,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let (cell, hollow) = meter_glyphs();
    let fill = meter_fill(levels, level, METER_W);
    let pos = levels.iter().position(|l| *l == level);
    let (on, on_dim) = (t.on_selection, bar_soft());
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let arrow = |live: bool| {
        sel(fg(if selected {
            if live { on } else { on_dim }
        } else if live {
            t.accent
        } else {
            t.text_faint
        }))
    };
    let (left, right) = if selected {
        ("← ", " →")
    } else {
        ("  ", "  ")
    };
    vec![
        Span::styled(left, arrow(pos.is_some_and(|p| p > 0))),
        Span::styled(
            cell.repeat(fill),
            sel(fg(if selected { on } else { t.text_muted })),
        ),
        Span::styled(
            hollow.repeat(METER_W - fill),
            sel(fg(if selected { on_dim } else { t.text_faint })),
        ),
        Span::styled(right, arrow(pos.is_some_and(|p| p + 1 < levels.len()))),
        Span::styled(" ", sel(Style::default())),
        // Padded to the widest label, so trailing text starts in one column.
        Span::styled(
            format!("{:<7}", effort_label(level)),
            sel(fg(if selected { on } else { t.text_muted })),
        ),
    ]
}

/// One option of an open composite dropdown, indented under its row: the
/// option name, ✓ on the current pick, and — in the lead slot — the effort
/// meter that option runs at. Options no variant serves are dimmed; the
/// highlighted option is the same accent bar as a selected model row,
/// just nested one step deeper.
fn picker_option_line(
    v: &PickerView<'_>,
    drop: &DropState,
    opt: usize,
    selected: bool,
    name_w: usize,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let on = t.on_selection;
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let Some(shape) = super::variants::row_shape(&drop.row) else {
        return Line::default();
    };
    let Some(slot) = shape.slots.get(drop.slot) else {
        return Line::default();
    };
    let Some(option) = slot.options.get(opt) else {
        return Line::default();
    };
    let cur = v.efforts.parts_of(v.config, v.saved, &drop.row);
    let parts = option_parts(&shape, &cur, drop.slot, &option.id);
    let style = sel(match (&parts, selected) {
        (None, _) => fg(t.text_faint),
        (Some(_), true) => fg(on).add_modifier(Modifier::BOLD),
        (Some(_), false) => fg(t.text_soft),
    });
    let inner_w = name_w.saturating_sub(2);
    let name = fit_chars(&option.name, inner_w);
    let pad = inner_w.saturating_sub(name.chars().count()) + 2;
    let badge = if selected {
        on
    } else if parts.is_some() {
        t.success
    } else {
        t.text_faint
    };
    let mut spans = vec![
        Span::styled("  ", sel(Style::default())),
        Span::styled(
            if selected {
                format!("{} ", super::icon("arrow"))
            } else {
                "  ".to_string()
            },
            sel(fg(on)),
        ),
        Span::styled(name, style),
        Span::styled(" ".repeat(pad), sel(Style::default())),
        Span::styled(
            if cur.get(&slot.key) == Some(&option.id) {
                "✓"
            } else {
                " "
            },
            sel(fg(badge)),
        ),
        Span::styled(" ", sel(Style::default())),
    ];
    // Only the first slot (the lead) carries the effort.
    if drop.slot == 0
        && let Some(parts) = &parts
        && let Some(level) = v.efforts.shown_with(v.config, v.saved, &drop.row, parts)
    {
        let levels = row_levels(&drop.row, v.efforts.fast, parts);
        spans.extend(meter_spans(&levels, level, selected));
    }
    if selected {
        let fill = w.saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>());
        spans.push(Span::styled(
            " ".repeat(fill),
            Style::default().bg(t.accent),
        ));
    }
    Line::from(spans)
}

/// The highlighted model's details, top-packed: composite slot picks, fast
/// mode (only when the catalog has a fast variant), then the Price and
/// Context lines — each only when some listed model knows it. A datum the
/// list has but this model lacks reads `—`, so the block never leaves a
/// blank hole inside it.
fn picker_detail_lines(
    v: &PickerView<'_>,
    id: &str,
    w: usize,
) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // A composite names its picks, one line per slot (`Lead … ▾`); the lead
    // carries the effort it runs at.
    if let Some(shape) = super::variants::row_shape(id).filter(|s| s.is_composite()) {
        let parts = v.live_parts(id);
        let effort = v.efforts.shown_with(v.config, v.saved, id, &parts);
        for (i, slot) in shape.slots.iter().enumerate() {
            let mut name = parts
                .get(&slot.key)
                .map(|o| shape.option_name(&slot.key, o).unwrap_or(o).to_string())
                .unwrap_or_default();
            if i == 0
                && let Some(e) = effort.filter(|e| *e != "off")
            {
                name = format!("{name} {}", effort_label(e));
            }
            let open = v.drop.is_some_and(|d| d.row == id && d.slot == i);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{:<10}", slot.label), fg(t.text_soft)),
                Span::styled(
                    fit_chars(&name, w.saturating_sub(16)),
                    if open {
                        fg(t.accent).add_modifier(Modifier::BOLD)
                    } else {
                        fg(t.text_body)
                    },
                ),
                Span::styled(
                    if open { " ▾" } else { " ▸" },
                    fg(if open { t.accent } else { t.text_dim }),
                ),
            ]));
        }
    }

    if let Some(wire) = v.fast_variant(id) {
        let mut spans = vec![Span::raw("  "), Span::styled("Fast Mode ", fg(t.text_soft))];
        if v.toggles.fast {
            spans.push(Span::styled(
                "On",
                fg(t.accent).add_modifier(Modifier::BOLD),
            ));
            let room = w.saturating_sub(2 + "Fast Mode On".len() + 4);
            if room > 8 {
                spans.push(Span::styled(
                    format!("  → {}", fit_chars(&wire, room)),
                    fg(t.text_dim),
                ));
            }
        } else {
            spans.push(Span::styled("Off", fg(t.text_dim)));
        }
        lines.push(Line::from(spans));
    } else if lists_fast_variants(v.models) {
        // The provider has fast rows, just none for this model at this
        // effort: hold the slot so the table below never jumps.
        lines.push(Line::default());
    }

    let rate = super::context::get_model_rate(id);
    let ctx = known_context(id);
    let priced = v
        .models
        .iter()
        .any(|(mid, _)| super::context::get_model_rate(mid).is_some());
    let ctxed = v.models.iter().any(|(mid, _)| known_context(mid).is_some());
    if !priced && !ctxed {
        return lines;
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let label = |l: &'static str| {
        vec![
            Span::raw("  "),
            Span::styled(format!("{l:<10}"), fg(t.text_soft)),
        ]
    };
    if priced {
        let mut spans = label("Price");
        match &rate {
            None => spans.push(Span::styled("—", fg(t.text_faint))),
            Some(r) => {
                // Absolute bands + a per-dot gradient: cheap dots stay
                // green even when a pricey model fills them.
                let level = (w >= 2 + 10 + PRICE_DOTS + 2 + 12)
                    .then(|| price_level((r.input + r.output) * 1_000_000.0));
                if let Some(level) = level {
                    for i in 0..PRICE_DOTS {
                        spans.push(Span::styled(
                            if i < level { "●" } else { "○" },
                            if i < level {
                                fg(price_gradient(i, PRICE_DOTS))
                            } else {
                                fg(t.text_faint)
                            },
                        ));
                    }
                    spans.push(Span::raw("  "));
                }
                let room = w
                    .saturating_sub(2 + 10)
                    .saturating_sub(usize::from(level.is_some()) * (PRICE_DOTS + 2));
                spans.push(Span::styled(rate_text(r, room), fg(t.text_body)));
            }
        }
        lines.push(Line::from(spans));
    }
    if ctxed {
        let mut spans = label("Context");
        spans.push(match ctx {
            Some(c) => Span::styled(super::context::format_context_length(c), fg(t.text_body)),
            None => Span::styled("—", fg(t.text_faint)),
        });
        lines.push(Line::from(spans));
    }
    lines
}

/// The footer's four width tiers, widest first: each drops more words off
/// the key descriptions until only glyphs remain. `live` flags fade keys
/// that can't act right now instead of removing them, so the line never
/// jumps; every tier but the last still names ctrl+r.
fn footer_tiers<'a>(
    has_fast: bool,
    has_effort: bool,
    enter: &'a str,
    esc: &'a str,
    reasoning: &'a str,
) -> [Vec<(&'static str, &'a str, bool)>; 4] {
    [
        vec![
            ("↑↓", "select", true),
            ("tab", "fast mode", has_fast),
            ("←→", "effort", has_effort),
            ("↵", enter, true),
            ("esc", esc, true),
            ("ctrl+r", reasoning, true),
        ],
        vec![
            ("↑↓", "select", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", enter, true),
            ("esc", "", true),
            ("ctrl+r", "reasoning", true),
        ],
        vec![
            ("↑↓", "", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", "", true),
            ("esc", "", true),
            ("^r", "reasoning", true),
        ],
        vec![
            ("↑↓", "", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", "", true),
            ("esc", "", true),
        ],
    ]
}

/// Display width of one footer tier: segments joined by ` · `.
fn footer_width(segs: &[(&str, &str, bool)]) -> usize {
    segs.iter()
        .map(|(k, d, _)| k.chars().count() + usize::from(!d.is_empty()) + d.chars().count())
        .sum::<usize>()
        + 3 * segs.len().saturating_sub(1)
}

/// Inner width the panel needs for the footer to still name every action:
/// the second tier with every key live. Panels narrower than this fall to
/// bare-glyph tiers.
fn footer_floor_width() -> usize {
    footer_width(&footer_tiers(true, true, "confirm", "cancel", "show reasoning")[1])
}

/// The one-line key hint: key bold, what it does dim. A key with nothing
/// to act on (no fast variant, no effort levels) fades instead of
/// vanishing, so the line never jumps; ctrl+r joins only when it fits,
/// and a narrow panel drops the self-evident words.
fn picker_footer(v: &PickerView<'_>, sel: usize, w: usize) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let highlighted = v.highlighted(sel);
    let has_fast = highlighted.is_some_and(|id| v.fast_variant(id).is_some());
    // Inside a dropdown only the lead slot carries effort, at the option
    // under the cursor.
    let has_effort = highlighted.is_some_and(|id| {
        let parts = v.live_parts(id);
        v.drop.is_none_or(|d| d.slot == 0)
            && v.efforts
                .shown_with(v.config, v.saved, id, &parts)
                .is_some()
    });
    // Enter walks a composite's slots (`↵ lead`, `↵ sidekick`, then
    // `↵ apply`); Esc backs out of a dropdown before it cancels.
    let shape = highlighted
        .and_then(super::variants::row_shape)
        .filter(|s| s.is_composite());
    let (enter, esc) = match (&shape, v.drop) {
        (Some(shape), Some(d)) => match shape.slots.get(d.slot + 1) {
            Some(next) => (next.label.to_lowercase(), "back"),
            None => ("apply".to_string(), "back"),
        },
        (Some(shape), None) => (
            shape
                .slots
                .first()
                .map_or("confirm".to_string(), |s| s.label.to_lowercase()),
            "cancel",
        ),
        (None, _) => ("confirm".to_string(), "cancel"),
    };
    let (enter, esc) = (enter.as_str(), esc);
    let reasoning = if v.toggles.show_reasoning {
        "hide reasoning"
    } else {
        "show reasoning"
    };
    let [first, second, third, last] = footer_tiers(has_fast, has_effort, enter, esc, reasoning);
    let segs = [first, second, third]
        .into_iter()
        .find(|segs| footer_width(segs) <= w)
        .unwrap_or(last);
    let mut spans = Vec::new();
    for (i, (key, does, live)) in segs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(t.text_faint)));
        }
        // Live keys are bright white like the other modals' footers.
        let (kc, dc) = if live {
            (ratatui::style::Color::White, t.text_dim)
        } else {
            (t.text_faint, t.text_faint)
        };
        spans.push(Span::styled(
            key,
            Style::default().fg(kc).add_modifier(Modifier::BOLD),
        ));
        if !does.is_empty() {
            spans.push(Span::styled(format!(" {does}"), Style::default().fg(dc)));
        }
    }
    Line::from(spans)
}

/// Paint the picker: a flat panel over the dimmed transcript — the
/// title/`esc` cap, the provider subline, the search row, the model list
/// with scroll hints, the highlighted model's details, and the key line.
fn draw_picker(
    frame: &mut ratatui::Frame,
    bg: &BackgroundSnapshot,
    v: &PickerView<'_>,
    sel: usize,
    scroll_top: &mut usize,
) {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear};

    let area = frame.area();
    if area.width < 24 || area.height < 10 {
        return;
    }
    render_dimmed_background(frame, bg);
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);

    // Sized to the whole list, so typing a filter never resizes the panel;
    // a short terminal gives up the detail block before the list.
    let avail = area.height.saturating_sub(2) as usize;
    let want = (v.models.len() + 1).clamp(4, 12);
    let detail_need = detail_rows(v.models);
    let detail_h = if avail >= PICKER_CHROME + detail_need + 6 {
        detail_need
    } else {
        0
    };
    let list_h = want
        .min(avail.saturating_sub(PICKER_CHROME + detail_h))
        .max(1);
    let modal_h = ((PICKER_CHROME + list_h + detail_h) as u16).min(area.height);
    // Width follows the content — the widest row (name column + meter
    // tail), floored at the footer's widest tier that still names every
    // key. The fixed 92 left the right half empty on short-name lists.
    let longest = v
        .models
        .iter()
        .map(|m| row_name(m).chars().count())
        .max()
        .unwrap_or(0);
    let inner_want = (2 + longest + ROW_TAIL).max(footer_floor_width()).max(24);
    let modal_w = ((inner_want + 6) as u16)
        .min(92)
        .min(area.width.saturating_sub(4))
        .max(40)
        .min(area.width);
    let pad_x: u16 = if modal_w >= 60 { 3 } else { 2 };
    let x0 = area.x + (area.width - modal_w) / 2;
    let y0 = area.y + (area.height - modal_h) / 3;
    let rect = Rect::new(x0, y0, modal_w, modal_h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default().style(Style::default().bg(t.surface_bg)),
        rect,
    );
    let (ix, iw) = (x0 + pad_x, modal_w - pad_x * 2);
    let w = iw as usize;
    let at = |y: u16| Rect::new(ix, y, iw, 1);

    // The cap every gray modal wears: bold title left, `esc` right.
    let title = "Select model";
    let gap = w.saturating_sub(title.chars().count() + 3);
    put_line(
        frame,
        at(y0 + 1),
        Line::from(vec![
            Span::styled(title, fg(t.text_bright).add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(gap)),
            Span::styled("esc", fg(t.text_dim)),
        ]),
    );

    // Subline: the provider the list serves, with the refresh note on it.
    let mut sub = vec![Span::styled(
        fit_chars(v.provider, w.saturating_sub(14)),
        fg(t.text_dim),
    )];
    if v.refreshing {
        sub.push(Span::styled(" · refreshing…", fg(t.text_faint)));
    }
    put_line(frame, at(y0 + 2), Line::from(sub));

    // Search, the connect modal's chrome: bold accent label, bright text,
    // the ▎ caret — no framing rules.
    let label = Span::styled("Search: ", fg(t.accent).add_modifier(Modifier::BOLD));
    let search = if v.filter.is_empty() {
        Line::from(vec![label, Span::styled("type to filter", fg(t.text_dim))])
    } else {
        Line::from(vec![
            label,
            Span::styled(
                fit_chars(v.filter, w.saturating_sub(9)),
                fg(t.text_bright).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▎", fg(t.accent)),
        ])
    };
    put_line(frame, at(y0 + 3), search);
    // y0 + 4 is air between the search row and the list.

    // The name column fits the longest name in the whole list (so it holds
    // still while filtering), leaving the meter and label their fixed cells.
    let list_y = y0 + 5;
    let name_w = longest.clamp(8, w.saturating_sub(2 + ROW_TAIL).max(8));
    let row_w = (2 + name_w + ROW_TAIL).min(w);
    let mut safe_sel = 0;
    if v.filtered.is_empty() {
        let line = if v.filter.is_empty() {
            Line::from(Span::styled(
                "  No models listed — press Enter to continue",
                fg(t.text_dim),
            ))
        } else {
            Line::from(vec![
                Span::styled("  Use custom model: ", fg(t.text_dim)),
                Span::styled(
                    fit_chars(v.filter, w.saturating_sub(22)),
                    fg(t.accent).add_modifier(Modifier::BOLD),
                ),
            ])
        };
        put_line(frame, at(list_y), line);
    } else {
        safe_sel = skip_divider(v.rows, sel.min(v.rows.len() - 1), false);
        let (top, shown) = list_window(v.rows.len(), safe_sel, *scroll_top, list_h);
        *scroll_top = top;
        let models_in = |rows: &[Row]| rows.iter().filter(|r| matches!(r, Row::Model(_))).count();
        let hint = |text: String| Line::from(Span::styled(text, fg(t.text_dim)));
        let mut y = list_y;
        if top > 0 {
            let above = models_in(&v.rows[..top]);
            put_line(frame, at(y), hint(format!("  ↑ {above} more above")));
            y += 1;
        }
        for (off, row) in v.rows[top..top + shown].iter().enumerate() {
            let line = match row {
                Row::Divider => Line::from(Span::styled(
                    format!("  {}", "─".repeat(row_w.saturating_sub(2))),
                    fg(t.text_faint),
                )),
                Row::Model(mi) => {
                    picker_row_line(v, v.filtered[*mi], top + off == safe_sel, name_w, w)
                }
                Row::Opt(oi) => match v.drop {
                    Some(d) => picker_option_line(v, d, *oi, top + off == safe_sel, name_w, w),
                    None => Line::default(),
                },
            };
            put_line(frame, at(y), line);
            y += 1;
        }
        let below = models_in(&v.rows[top + shown..]);
        if below > 0 {
            put_line(frame, at(y), hint(format!("  ↓ {below} more below")));
        }
    }

    if detail_h > 0
        && let Some(id) = v.highlighted(safe_sel)
    {
        let detail_y = list_y + list_h as u16 + 1;
        for (i, line) in picker_detail_lines(v, id, w)
            .into_iter()
            .take(detail_h - 1)
            .enumerate()
        {
            put_line(frame, at(detail_y + i as u16), line);
        }
    }

    put_line(frame, at(y0 + modal_h - 2), picker_footer(v, safe_sel, w));
}

pub fn run_model_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    let (_session, mut terminal) = super::open_modal()?;
    model_picker_loop(&mut terminal, config, bg, focus)
}

/// The picker's body on an already-open modal terminal — `/connect` reuses
/// it as its model step so both surfaces share rows, meters, effort keys
/// and the commit path. `config` must already point at the provider being
/// browsed: its fields are what the picker scopes and commits against.
pub(crate) fn model_picker_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyEvent, KeyEventKind, poll, read};
    use std::time::Duration;

    // The provider identity is local work; only the model list needs I/O,
    // and that no longer blocks the first frame. Plugin connections scope
    // by provider id — the placeholder base can never serve a list.
    let (item_name, list_key, plugin) = picker_scope(config);
    let mut models = saved_models_for(&list_key, &config.base_url);
    let (mut current_id, mut recent_ids) = recent_head_for(&config.base_url);
    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
    // Detached on purpose: a late reply lands in a channel nobody reads.
    let _refresh = {
        let plugin = plugin.clone();
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let tx = refresh_tx;
        std::thread::spawn(move || {
            let live = match plugin {
                Some(installed) => super::context::fetch_plugin_provider_models(installed),
                None => super::context::fetch_live_provider_models(&base_url, api_key.as_deref()),
            };
            let _ = tx.send(live);
        })
    };

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;
    // Open on the focused row (`/fusion`) or the live model's row;
    // dropped at the first keypress so a late refresh never yanks the
    // selection away.
    let mut place_on_current = true;
    let focus = focus.map(str::to_string);
    // Remembered per-model efforts, read once: rows show them before any
    // step, and Enter re-reads the file under the lock before writing.
    let effort_saved = saved_config_path()
        .map(|path| {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
            load_saved_config_at(&path)
        })
        .unwrap_or_default();
    let mut efforts = RowEfforts::default();
    let mut toggles = PickerToggles::from_config(config);
    // The open composite dropdown, if any.
    let mut drop: Option<DropState> = None;

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        let mut refreshing = true;
        loop {
            // A finished refresh updates the open list in place; the modal
            // was already usable while it ran.
            if refreshing && let Ok(live) = refresh_rx.try_recv() {
                refreshing = false;
                if !live.is_empty() {
                    models = merge_models(&config.base_url, live);
                    (current_id, recent_ids) = recent_head_for(&config.base_url);
                }
            }
            let f = filter.to_lowercase();
            let filtered_models: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    f.is_empty()
                        || m_id.to_lowercase().contains(&f)
                        || m_name.to_lowercase().contains(&f)
                })
                .collect();
            // One divider after the recent head; selection skips it.
            let sep = recent_prefix_len(
                current_id.as_deref(),
                &recent_ids,
                filtered_models.iter().map(|(id, _)| id.as_str()),
            );
            let div = (sep > 0 && sep < filtered_models.len()).then_some(sep);
            let mut rows: Vec<Row> = Vec::with_capacity(filtered_models.len() + 1);
            for i in 0..filtered_models.len() {
                if div == Some(i) {
                    rows.push(Row::Divider);
                }
                rows.push(Row::Model(i));
            }
            if place_on_current {
                let target = focus.as_deref().or(config.model.as_deref());
                if let Some(pos) = rows.iter().position(|row| match row {
                    Row::Model(mi) => target == Some(filtered_models[*mi].0.as_str()),
                    _ => false,
                }) {
                    sel = pos;
                    place_on_current = false;
                    // `/fusion`: a focused composite opens straight into
                    // its first slot.
                    if let Some(id) = focus.as_deref()
                        && let Some(shape) =
                            super::variants::row_shape(id).filter(|s| s.is_composite())
                    {
                        let cur = efforts.parts_of(config, &effort_saved, id);
                        drop = Some(DropState {
                            row: id.to_string(),
                            slot: 0,
                            sel: open_option(&shape, &cur, 0),
                        });
                    }
                }
            }
            // An open dropdown follows its row (a refresh may reorder the
            // list) and closes if the row went away.
            if let Some(d) = &drop {
                match rows.iter().position(|row| match row {
                    Row::Model(mi) => filtered_models[*mi].0 == d.row,
                    _ => false,
                }) {
                    Some(pos) => sel = pos,
                    None => drop = None,
                }
            }
            // An open dropdown lists its options inline under its row.
            let drop_shape = drop
                .as_ref()
                .and_then(|d| super::variants::row_shape(&d.row));
            let mut draw_sel = sel;
            if let (Some(d), Some(shape)) = (&drop, &drop_shape) {
                let n = shape.slots.get(d.slot).map_or(0, |s| s.options.len());
                let at = sel.min(rows.len()) + 1;
                for i in (0..n).rev() {
                    rows.insert(at.min(rows.len()), Row::Opt(i));
                }
                draw_sel = at + d.sel;
            }
            efforts.fast = toggles.fast;

            let view = PickerView {
                provider: &item_name,
                refreshing,
                filter: &filter,
                rows: &rows,
                filtered: &filtered_models,
                models: &models,
                config,
                saved: &effort_saved,
                efforts: &efforts,
                toggles,
                drop: drop.as_ref(),
            };
            terminal
                .draw(|frame| draw_picker(frame, &bg_snapshot, &view, draw_sel, &mut scroll_top))?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            if drop.is_none() {
                if rows.is_empty() {
                    sel = 0;
                } else {
                    sel = skip_divider(&rows, sel.min(rows.len() - 1), false);
                }
            }

            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = read()?
            else {
                continue;
            };
            place_on_current = false;
            if kind != KeyEventKind::Press {
                continue;
            }
            let Some(key) = picker_key(code, modifiers) else {
                continue;
            };
            let highlighted: Option<String> = view.highlighted(draw_sel).map(str::to_string);
            let live_parts = highlighted
                .as_deref()
                .map(|id| view.live_parts(id))
                .unwrap_or_default();
            let fast_supported =
                highlighted
                    .as_deref()
                    .is_some_and(|id| match super::variants::row_shape(id) {
                        Some(shape) if !shape.variants.is_empty() => {
                            let parts = selection_parts(&shape, &live_parts);
                            let eff = super::variants::effective_fast(
                                &shape.variants,
                                toggles.fast,
                                &parts,
                            );
                            super::variants::has_variant(&shape.variants, !eff, &parts)
                        }
                        _ => toggles.fast || view.fast_variant(id).is_some(),
                    });

            // ── inside a composite dropdown ──
            if let (Some(d), Some(shape)) = (drop.clone(), drop_shape.clone()) {
                let cur = efforts.parts_of(config, &effort_saved, &d.row);
                match key {
                    PickerKey::Cancel if code != crossterm::event::KeyCode::Esc => {
                        return Ok(false);
                    }
                    PickerKey::Cancel => {
                        drop = match esc_level(PickLevel::Slot(d.slot)) {
                            Some(PickLevel::Slot(i)) => Some(DropState {
                                row: d.row.clone(),
                                slot: i,
                                sel: open_option(&shape, &cur, i),
                            }),
                            _ => None,
                        };
                    }
                    PickerKey::Up | PickerKey::PageUp => {
                        let s = step_option(&shape, &cur, d.slot, d.sel, false);
                        drop = Some(DropState { sel: s, ..d });
                    }
                    PickerKey::Down | PickerKey::PageDown => {
                        let s = step_option(&shape, &cur, d.slot, d.sel, true);
                        drop = Some(DropState { sel: s, ..d });
                    }
                    // ←/→ set the effort of the lead under the cursor.
                    PickerKey::EffortDown | PickerKey::EffortUp if d.slot == 0 => {
                        let up = key == PickerKey::EffortUp;
                        efforts.step_with(config, &effort_saved, &d.row, &live_parts, up);
                    }
                    PickerKey::ToggleFast => toggles.toggle_fast(fast_supported),
                    PickerKey::ToggleReasoning => toggles.toggle_reasoning(),
                    PickerKey::Confirm => {
                        let Some(opt) = shape.slots.get(d.slot).and_then(|s| s.options.get(d.sel))
                        else {
                            continue;
                        };
                        let Some(parts) = option_parts(&shape, &cur, d.slot, &opt.id) else {
                            continue;
                        };
                        efforts.parts.insert(d.row.clone(), parts.clone());
                        match enter_level(PickLevel::Slot(d.slot), shape.slots.len()) {
                            (PickLevel::Slot(i), false) => {
                                drop = Some(DropState {
                                    row: d.row.clone(),
                                    slot: i,
                                    sel: open_option(&shape, &parts, i),
                                });
                            }
                            _ => {
                                let shown = efforts.shown(config, &effort_saved, &d.row);
                                let stepped = efforts.was_stepped(&d.row);
                                commit_selection(
                                    config,
                                    &models,
                                    d.row.clone(),
                                    (shown, stepped),
                                    parts,
                                    toggles,
                                )?;
                                return Ok(true);
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            let last = rows.len().saturating_sub(1);
            match key {
                PickerKey::Cancel => return Ok(false),
                PickerKey::Up => sel = skip_divider(&rows, sel.saturating_sub(1), false),
                PickerKey::Down => sel = skip_divider(&rows, (sel + 1).min(last), true),
                PickerKey::PageUp => sel = skip_divider(&rows, sel.saturating_sub(8), false),
                PickerKey::PageDown => sel = skip_divider(&rows, (sel + 8).min(last), true),
                // ←/→ step the highlighted model's effort.
                PickerKey::EffortDown | PickerKey::EffortUp => {
                    if let Some(m_id) = &highlighted {
                        let up = key == PickerKey::EffortUp;
                        efforts.step(config, &effort_saved, m_id, up);
                    }
                }
                PickerKey::ToggleFast => toggles.toggle_fast(fast_supported),
                PickerKey::ToggleReasoning => toggles.toggle_reasoning(),
                PickerKey::Type(ch) => {
                    filter.push(ch);
                    sel = 0;
                }
                PickerKey::Backspace => {
                    filter.pop();
                    sel = 0;
                }
                PickerKey::Confirm => {
                    // Enter on a composite opens its first slot instead of
                    // applying.
                    if let Some(id) = highlighted.as_deref()
                        && let Some(shape) =
                            super::variants::row_shape(id).filter(|s| s.is_composite())
                        && let (PickLevel::Slot(0), false) =
                            enter_level(PickLevel::List, shape.slots.len())
                    {
                        let cur = efforts.parts_of(config, &effort_saved, id);
                        drop = Some(DropState {
                            row: id.to_string(),
                            slot: 0,
                            sel: open_option(&shape, &cur, 0),
                        });
                        continue;
                    }
                    // An empty list has nothing to select. With a model
                    // already configured, Enter dismisses — the old
                    // literal "default" fallback silently clobbered the
                    // live model whenever a provider answered with no
                    // list (plugin providers hit exactly that).
                    if filtered_models.is_empty() && filter.is_empty() && config.model.is_some() {
                        return Ok(false);
                    }
                    // The row's effort, read while `config.model` still
                    // names the old model: a row's opening level keys
                    // off which model is live.
                    let row_effort = highlighted
                        .as_deref()
                        .map(|m_id| {
                            (
                                efforts.shown(config, &effort_saved, m_id),
                                efforts.was_stepped(m_id),
                            )
                        })
                        .unwrap_or((None, false));
                    let chosen_model = if let Some(id) = highlighted {
                        id
                    } else if !filter.is_empty() {
                        // Canonicalize known ids (case/tail); unknown
                        // filters still fall through as custom models —
                        // the picker keeps its "Use custom model" path.
                        // Direct `/model <id>` callers must use
                        // `validate_direct_model_id` and reject `Err`.
                        validate_direct_model_id(filter.trim(), &models)
                            .unwrap_or_else(|_| filter.trim().to_string())
                    } else {
                        // Reached only with nothing configured: first
                        // run needs *a* value to continue.
                        "default".to_string()
                    };
                    commit_selection(
                        config,
                        &models,
                        chosen_model,
                        row_effort,
                        super::variants::Parts::new(),
                        toggles,
                    )?;
                    return Ok(true);
                }
            }
        }
    })();

    let _ = terminal.clear();

    result
}

/// Land a picker selection: the row as `config.model`, its effort (as the
/// row showed it — stepped or the model's own remembered level), composite
/// `parts` (empty clears them), and the pending Tab / ctrl+r toggles —
/// live and on disk, under the config lock.
fn commit_selection(
    config: &mut Config,
    models: &[(String, String)],
    chosen_model: String,
    row_effort: (Option<&str>, bool),
    parts: super::variants::Parts,
    toggles: PickerToggles,
) -> anyhow::Result<()> {
    config.model = Some(chosen_model);
    config.model_parts = parts;
    // A variant pick (`base-tier`, `base-tier-fast`) decomposes: the
    // tier/fast it names wins over the remembered values applied below.
    let variant = canonicalize_effort_variant(config, models);
    let path = saved_config_path()?;
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let mut saved = load_saved_config_at(&path);
    saved.base_url = Some(config.base_url.clone());
    saved.model = config.model.clone();
    if variant.as_ref().and_then(|(_, t, _)| t.as_ref()).is_none() {
        let (shown, stepped) = row_effort;
        commit_row_effort(config, &mut saved, shown, stepped);
    }
    let key = crate::setup::effort_memory_key(
        &config.provider_id,
        &config.base_url,
        config.model.as_deref().unwrap_or_default(),
    );
    saved.remember_parts(&key, &config.model_parts);
    // Tab / ctrl+r land only now, with the model.
    toggles.commit(config, &mut saved);
    save_saved_config_at(&path, &saved)?;
    Ok(())
}

/// Validates a directly-typed `/model <id>` against the picker's known list
/// (live `/models` + catalog snapshot behind `fetch_live_provider_models`).
/// Exact id (or display name) wins; case-insensitive and unique `provider/`
/// tail matches canonicalize. Unknown ids are rejected with a hint mirroring
/// the picker empty-state (`type /model to browse ...`) instead of being
/// silently accepted until the first prompt fails. Empty known-list fails
/// open (offline/custom endpoints like Ollama accept any id).
pub(crate) fn validate_direct_model_id(
    raw: &str,
    models: &[(String, String)],
) -> Result<String, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("usage: /model <model-id> — type /model to browse models".to_string());
    }
    if models.is_empty() {
        return Ok(input.to_string());
    }
    if let Some((id, _)) = models.iter().find(|(id, _)| id == input) {
        return Ok(id.clone());
    }
    let lower = input.to_lowercase();
    if let Some((id, _)) = models.iter().find(|(id, _)| id.to_lowercase() == lower) {
        return Ok(id.clone());
    }
    if let Some((id, _)) = models.iter().find(|(_, name)| name.to_lowercase() == lower) {
        return Ok(id.clone());
    }
    if !input.contains('/') {
        let tails: Vec<&(String, String)> = models
            .iter()
            .filter(|(id, _)| {
                id.rsplit('/')
                    .next()
                    .is_some_and(|t| t.to_lowercase() == lower)
            })
            .collect();
        if tails.len() == 1 {
            return Ok(tails[0].0.clone());
        }
        if tails.len() > 1 {
            let mut ids: Vec<&str> = tails.iter().map(|(id, _)| id.as_str()).collect();
            ids.sort();
            let shown = ids.iter().take(5).copied().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "ambiguous model '{input}' — matches {} models ({shown}{}); type /model to pick",
                ids.len(),
                if ids.len() > 5 { ", …" } else { "" },
            ));
        }
    }
    Err(format!(
        "unknown model '{input}' — no match; type /model to browse {} models",
        models.len(),
    ))
}

#[path = "model_modal_tests.rs"]
#[cfg(test)]
mod tests;
