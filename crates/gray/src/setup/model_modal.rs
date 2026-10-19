use super::*;

pub(crate) fn provider_models_for_config(config: &Config) -> (String, Vec<(String, String)>) {
    let (name, _key, plugin) = picker_scope(config);
    let models = match plugin {
        Some(installed) => super::context::fetch_plugin_provider_models(installed),
        None => picker_models_for(&config.base_url, config.api_key.as_deref()),
    };
    (name, models)
}

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

pub(crate) fn saved_models_for(list_key: &str, sort_base: &str) -> Vec<(String, String)> {
    let mut models = super::context::load_provider_model_list(list_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(sort_base, &mut models);
    }
    models
}

pub(crate) const VARIANT_EFFORT_WORDS: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max"];

pub(crate) const FAST_SUFFIXES: &[&str] = &["fast", "priority"];

fn effort_variant_shape(model: &str) -> Option<(&str, &str)> {
    let (base, tier) = model.rsplit_once('-')?;
    (!base.is_empty() && VARIANT_EFFORT_WORDS.contains(&tier)).then_some((base, tier))
}

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

fn split_fast_variant(model: &str, known: &[(String, String)]) -> Option<(String, &'static str)> {
    let (stem, leaf) = model.rsplit_once('-')?;
    if stem.is_empty() {
        return None;
    }
    let suffix = FAST_SUFFIXES
        .iter()
        .find(|s| leaf.eq_ignore_ascii_case(s))?;

    let stem = match known.iter().find(|(id, _)| id.eq_ignore_ascii_case(stem)) {
        Some((id, _)) => id.clone(),
        None if split_effort_variant(stem, known).is_some() => stem.to_string(),
        None => return None,
    };
    Some((stem, *suffix))
}

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

pub(crate) fn wire_model_for(config: &Config) -> Option<String> {
    let model = config.model.as_deref()?;

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

pub(crate) fn selected_model_label(config: &Config) -> Option<String> {
    let model = config.model.as_deref()?;
    if let Some(label) = composite_label_for(config) {
        return Some(label);
    }
    Some(
        load_provider_names(config)
            .into_iter()
            .find(|(id, _)| id == model)
            .map(|(_, name)| name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| super::context::friendly_model_name(model)),
    )
}

pub(crate) fn composite_label_for(config: &Config) -> Option<String> {
    composite_label_parts_for(config).map(|segs| segs.into_iter().map(|(t, _)| t).collect())
}

pub(crate) fn composite_label_parts_for(config: &Config) -> Option<Vec<(String, Option<usize>)>> {
    let model = config.model.as_deref()?;
    let shape = super::variants::row_shape(model)?;
    let name = load_provider_names(config)
        .into_iter()
        .find(|(id, _)| id == model)
        .map(|(_, n)| n)
        .unwrap_or_else(|| super::context::friendly_model_name(model));
    let parts = selection_parts(&shape, &config.model_parts);
    super::variants::composite_label_parts(&shape, &name, &parts, config.thinking_effort.as_deref())
}

fn load_provider_names(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, _) = picker_scope(config);
    super::context::load_provider_model_list(&list_key)
}

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

struct ModelVariantRef {
    effort: Option<String>,
    fast: bool,
    parts: super::variants::Parts,
}

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
    let stale_variant = config.model.as_deref().is_some_and(|m| {
        effort_variant_shape(m).is_some()
            && !known.iter().any(|(id, _)| id == m)
            && decompose_model_variant(m, &known).is_none()
    });
    match (stale_variant, plugin) {
        (true, Some(installed)) => {
            let live = super::context::fetch_plugin_provider_models(installed);
            if live.is_empty() { known } else { live }
        }
        _ => known,
    }
}

pub(crate) fn canonicalize_adopted_model(config: &mut Config) {
    let rows = canonical_model_rows(config);
    canonicalize_effort_variant(config, &rows);
}

pub(crate) fn canonicalize_effort_variant(
    config: &mut Config,
    known: &[(String, String)],
) -> Option<(String, Option<String>, bool)> {
    let model = config.model.clone()?;

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
            if saved.model.as_deref() == Some(model.as_str()) {
                saved.model = Some(row.clone());
                let key =
                    crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &row);
                if let Some(e) = &v.effort {
                    saved.remember_effort(&key, e);
                }
                saved.remember_parts(&key, &v.parts);
                saved.fast_mode = config.fast_mode;
                let _ = save_saved_config_at(&path, &saved);
            }
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
        if saved.model.as_deref() == Some(model.as_str()) {
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
    }
    Some((base, tier, fast))
}

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

fn recent_prefix_len<'a>(
    current: Option<&str>,
    recent: &[String],
    ids: impl Iterator<Item = &'a str>,
) -> usize {
    ids.take_while(|id| Some(*id) == current || recent.iter().any(|m| m.as_str() == *id))
        .count()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Model(usize),
    Divider,

    Opt(usize),
}

fn head_first<'a>(list: &[&'a (String, String)], head: &[String]) -> Vec<&'a (String, String)> {
    let mut out = Vec::with_capacity(list.len());
    for h in head {
        if let Some(entry) = list.iter().find(|(id, _)| id == h) {
            out.push(*entry);
        }
    }
    out.extend(
        list.iter()
            .copied()
            .filter(|(id, _)| !head.iter().any(|h| h == id)),
    );
    out
}

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

fn is_composite_id(id: &str) -> bool {
    super::variants::row_shape(id).is_some_and(|s| s.is_composite())
}

fn composites_first<'a>(list: &[&'a (String, String)]) -> Vec<&'a (String, String)> {
    let (mut out, plain): (Vec<_>, Vec<_>) = list
        .iter()
        .copied()
        .partition(|(id, _)| is_composite_id(id));
    out.extend(plain);
    out
}

fn gap_possible(models: &[(String, String)]) -> bool {
    let composites = models.iter().filter(|(id, _)| is_composite_id(id)).count();
    composites > 0 && composites < models.len()
}

fn slot_of(row: usize, gap: Option<usize>) -> usize {
    row + usize::from(gap.is_some_and(|g| row >= g))
}

fn row_at(slot: usize, gap: Option<usize>) -> Option<usize> {
    match gap {
        Some(g) if slot == g => None,
        Some(g) if slot > g => Some(slot - 1),
        _ => Some(slot),
    }
}

fn rows_before(slot: usize, gap: Option<usize>) -> usize {
    slot - usize::from(gap.is_some_and(|g| slot > g))
}

fn composite_gap(rows: &[Row], lead: usize) -> Option<usize> {
    if lead == 0 {
        return None;
    }
    rows.iter()
        .position(|row| matches!(row, Row::Model(mi) if *mi >= lead))
}

pub(crate) fn row_levels(
    model: &str,
    fast: bool,
    parts: &super::variants::Parts,
) -> Vec<&'static str> {
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

fn has_declared_variants(model: &str) -> bool {
    super::variants::row_shape(model).is_some_and(|s| !s.variants.is_empty())
}

#[cfg(test)]
fn initial_row_effort(config: &Config, saved: &SavedConfig, model: &str) -> Option<&'static str> {
    let levels = row_levels(model, false, &config.model_parts);
    initial_effort_in(config, saved, model, &levels)
}

pub(crate) fn initial_effort_in(
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

#[derive(Default)]
struct RowEfforts {
    stepped: std::collections::HashMap<String, &'static str>,

    fast: bool,

    parts: std::collections::HashMap<String, super::variants::Parts>,
}

impl RowEfforts {
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

    fn shown_with(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        parts: &super::variants::Parts,
    ) -> Option<&'static str> {
        let levels = row_levels(model, self.fast, parts);

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

    fn shown(&self, config: &Config, saved: &SavedConfig, model: &str) -> Option<&'static str> {
        let parts = self.parts_of(config, saved, model);
        self.shown_with(config, saved, model, &parts)
    }

    fn was_stepped(&self, model: &str) -> bool {
        self.stepped
            .keys()
            .any(|k| k == model || k.starts_with(&format!("{model}\u{1f}")))
    }

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

    fn step(&mut self, config: &Config, saved: &SavedConfig, model: &str, forward: bool) {
        let parts = self.parts_of(config, saved, model);
        self.step_with(config, saved, model, &parts, forward);
    }
}

pub(crate) fn commit_row_effort(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerToggles {
    pub(crate) fast: bool,
    pub(crate) show_reasoning: bool,
    opened_fast: bool,
    opened_show: bool,
}

impl PickerToggles {
    pub(crate) fn from_config(config: &Config) -> Self {
        let fast = config.fast_mode == Some(true);
        let show_reasoning = config.show_reasoning.unwrap_or(true);
        Self {
            fast,
            show_reasoning,
            opened_fast: fast,
            opened_show: show_reasoning,
        }
    }

    pub(crate) fn toggle_fast(&mut self, supported: bool) {
        if supported {
            self.fast = !self.fast;
        }
    }

    pub(crate) fn toggle_reasoning(&mut self) {
        self.show_reasoning = !self.show_reasoning;
    }

    pub(crate) fn commit(&self, config: &mut Config, saved: &mut SavedConfig) {
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

fn effort_label(level: &str) -> String {
    super::variants::effort_word(level)
}

const METER_W: usize = 6;

fn meter_fill(levels: &[&str], level: &str, width: usize) -> usize {
    let tiers: Vec<&str> = levels.iter().copied().filter(|l| *l != "off").collect();
    let Some(k) = tiers.iter().position(|l| *l == level) else {
        return 0;
    };
    let n = tiers.len();
    ((k + 1) * width * 2 + n) / (2 * n)
}

fn list_window(total: usize, sel: usize, top: usize, cap: usize) -> (usize, usize) {
    if total <= cap {
        return (0, total);
    }
    let sel = sel.min(total - 1);
    if cap < 3 {
        let top = top.min(sel).max((sel + 1).saturating_sub(cap));
        return (top.min(total - cap), cap);
    }

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

fn per_million(rate: f64) -> String {
    let s = format!("{:.3}", rate * 1_000_000.0);
    format!("${}", s.trim_end_matches('0').trim_end_matches('.'))
}

pub(crate) const PRICE_DOTS: usize = 5;

pub(crate) fn price_level(blended_per_million: f64) -> usize {
    const BANDS: [f64; PRICE_DOTS - 1] = [0.5, 2.0, 8.0, 20.0];
    1 + BANDS.iter().filter(|&&b| blended_per_million > b).count()
}

pub(crate) fn rate_text(r: &super::context::ModelRate, room: usize) -> String {
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

pub(crate) fn price_gradient(i: usize, w: usize) -> ratatui::style::Color {
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

pub(crate) fn known_context(model: &str) -> Option<usize> {
    super::context::ensure_disk_loaded();
    let tail = model.rsplit('/').next().unwrap_or(model);
    [model, tail]
        .into_iter()
        .find_map(super::context::get_cached_model_context)
}

pub(crate) fn fit_chars(s: &str, max: usize) -> String {
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

pub(super) fn cached_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let cached = saved_models_for(base_url, base_url);
    if cached.is_empty() {
        return picker_models_for(base_url, api_key);
    }
    let (base, key) = (base_url.to_string(), api_key.map(str::to_string));

    std::thread::spawn(move || super::context::fetch_live_provider_models(&base, key.as_deref()));
    cached
}

const MODEL_REFRESH_TTL: std::time::Duration = std::time::Duration::from_secs(300);

static MODEL_REFRESHED: std::sync::Mutex<Vec<(String, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

pub(crate) fn model_refresh_due(list_key: &str) -> bool {
    let seen = MODEL_REFRESHED.lock().unwrap_or_else(|e| e.into_inner());
    !seen
        .iter()
        .any(|(k, at)| k == list_key && at.elapsed() < MODEL_REFRESH_TTL)
}

fn note_model_refreshed(list_key: &str) {
    let mut seen = MODEL_REFRESHED.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    match seen.iter_mut().find(|(k, _)| k == list_key) {
        Some(entry) => entry.1 = now,
        None => seen.push((list_key.to_string(), now)),
    }
}

pub(crate) fn fetch_models_live(
    list_key: &str,
    plugin: Option<crate::providers::InstalledProvider>,
    base_url: &str,
    api_key: Option<&str>,
) -> Vec<(String, String)> {
    let started = std::time::Instant::now();
    let live = match plugin {
        Some(installed) => super::context::fetch_plugin_provider_models(installed),
        None => super::context::fetch_live_provider_models(base_url, api_key),
    };
    if !live.is_empty() {
        note_model_refreshed(list_key);
    }
    log::debug!(target: "gray_timing", "model list refresh list_key={list_key} rows={} elapsed_ms={}", live.len(), started.elapsed().as_millis());
    live
}

pub(crate) fn merge_models(base_url: &str, live: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut models = live;
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

pub(super) fn picker_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut models = fetch_live_provider_models(base_url, api_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

fn row_name(m: &(String, String)) -> &str {
    if m.1.is_empty() { &m.0 } else { &m.1 }
}

fn put_line(frame: &mut ratatui::Frame, at: ratatui::layout::Rect, line: ratatui::text::Line<'_>) {
    let base = ratatui::style::Style::default().bg(crate::theme::theme().surface_bg);
    frame.render_widget(ratatui::widgets::Paragraph::new(line).style(base), at);
}

const PICKER_CHROME: usize = 8;

fn lists_fast_variants(models: &[(String, String)]) -> bool {
    models.iter().any(|(id, _)| {
        super::variants::row_shape(id).is_some_and(|s| s.variants.iter().any(|v| v.fast))
            || split_fast_variant(id, models).is_some()
    })
}

fn listed_slots(models: &[(String, String)]) -> usize {
    models
        .iter()
        .filter_map(|(id, _)| super::variants::row_shape(id))
        .map(|s| s.slots.len())
        .max()
        .unwrap_or(0)
}

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

const ROW_TAIL: usize = 2 + 1 + 1 + 2 + METER_W + 2 + 1 + 7;

const RECOMMENDED_HEAD: usize = 5;

fn meter_glyphs() -> (&'static str, &'static str) {
    if super::icons::has_nerd_font() {
        ("⬢", "⬡")
    } else {
        ("■", "□")
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DropState {
    row: String,
    slot: usize,
    sel: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickLevel {
    List,
    Slot(usize),
}

fn enter_level(level: PickLevel, slots: usize) -> (PickLevel, bool) {
    match level {
        PickLevel::List if slots > 0 => (PickLevel::Slot(0), false),
        PickLevel::List => (PickLevel::List, true),
        PickLevel::Slot(i) if i + 1 < slots => (PickLevel::Slot(i + 1), false),
        PickLevel::Slot(_) => (PickLevel::List, true),
    }
}

fn esc_level(level: PickLevel) -> Option<PickLevel> {
    match level {
        PickLevel::List => None,
        PickLevel::Slot(0) => Some(PickLevel::List),
        PickLevel::Slot(i) => Some(PickLevel::Slot(i - 1)),
    }
}

fn option_parts(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
    option: &str,
) -> Option<super::variants::Parts> {
    let key = &shape.slots.get(slot)?.key;
    shape.option_parts(cur, key, option)
}

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

    all_head: bool,

    gap: Option<usize>,
}

impl PickerView<'_> {
    fn highlighted(&self, sel: usize) -> Option<&str> {
        match self.rows.get(sel) {
            Some(Row::Model(mi)) => self.filtered.get(*mi).map(|(id, _)| id.as_str()),
            Some(Row::Opt(_)) => self.drop.map(|d| d.row.as_str()),
            _ => None,
        }
    }

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
                format!("  {}", fit_chars(&summary.join(" × "), room)),
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

    if selected {
        let fill = w.saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>());
        spans.push(Span::styled(
            " ".repeat(fill),
            Style::default().bg(t.accent),
        ));
    }
    Line::from(spans)
}

pub(crate) fn meter_spans(
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
        Span::styled(
            format!("{:<7}", effort_label(level)),
            sel(fg(if selected { on } else { t.text_muted })),
        ),
    ]
}

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

fn footer_width(segs: &[(&str, &str, bool)]) -> usize {
    segs.iter()
        .map(|(k, d, _)| k.chars().count() + usize::from(!d.is_empty()) + d.chars().count())
        .sum::<usize>()
        + 3 * segs.len().saturating_sub(1)
}

fn footer_floor_width() -> usize {
    footer_width(&footer_tiers(true, true, "confirm", "cancel", "show reasoning")[1])
}

fn picker_footer(v: &PickerView<'_>, sel: usize, w: usize) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let highlighted = v.highlighted(sel);
    let has_fast = highlighted.is_some_and(|id| v.fast_variant(id).is_some());

    let has_effort = highlighted.is_some_and(|id| {
        let parts = v.live_parts(id);
        v.drop.is_none_or(|d| d.slot == 0)
            && v.efforts
                .shown_with(v.config, v.saved, id, &parts)
                .is_some()
    });

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

    let avail = area.height.saturating_sub(2) as usize;

    let want = (v.models.len() + 1 + usize::from(gap_possible(v.models))).clamp(4, 12);
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

    let mut sub = vec![Span::styled(
        fit_chars(v.provider, w.saturating_sub(14)),
        fg(t.text_dim),
    )];
    if v.refreshing {
        sub.push(Span::styled(" · refreshing…", fg(t.text_faint)));
    }
    put_line(frame, at(y0 + 2), Line::from(sub));

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

        let slots = v.rows.len() + usize::from(v.gap.is_some());
        let (top, shown) = list_window(slots, slot_of(safe_sel, v.gap), *scroll_top, list_h);
        *scroll_top = top;
        let end = top + shown;
        let models_in = |rows: &[Row]| rows.iter().filter(|r| matches!(r, Row::Model(_))).count();
        let hint = |text: String| Line::from(Span::styled(text, fg(t.text_dim)));
        let mut y = list_y;
        if top > 0 {
            let above = models_in(&v.rows[..rows_before(top, v.gap)]);
            put_line(frame, at(y), hint(format!("  ↑ {above} more above")));
            y += 1;
        }
        for slot in top..end {
            let line = match row_at(slot, v.gap) {
                None => Line::default(),
                Some(ri) => match v.rows[ri] {
                    Row::Divider if v.all_head => Line::from(Span::styled(
                        format!("  ─ all models {}", "─".repeat(row_w.saturating_sub(15))),
                        fg(t.text_faint),
                    )),
                    Row::Divider => Line::from(Span::styled(
                        format!("  {}", "─".repeat(row_w.saturating_sub(2))),
                        fg(t.text_faint),
                    )),
                    Row::Model(mi) => picker_row_line(v, v.filtered[mi], ri == safe_sel, name_w, w),
                    Row::Opt(oi) => match v.drop {
                        Some(d) => picker_option_line(v, d, oi, ri == safe_sel, name_w, w),
                        None => Line::default(),
                    },
                },
            };
            put_line(frame, at(y), line);
            y += 1;
        }
        let below = models_in(&v.rows[rows_before(end, v.gap)..]);
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

pub(crate) fn model_picker_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyEvent, KeyEventKind, poll, read};
    use std::time::Duration;

    let (item_name, list_key, plugin) = picker_scope(config);
    let mut models = saved_models_for(&list_key, &config.base_url);
    let (mut current_id, mut recent_ids) = recent_head_for(&config.base_url);
    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();

    let _refresh = (models.is_empty() || model_refresh_due(&list_key)).then(|| {
        let plugin = plugin.clone();
        let list_key = list_key.clone();
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let tx = refresh_tx;
        std::thread::spawn(move || {
            let live = fetch_models_live(&list_key, plugin, &base_url, api_key.as_deref());
            let _ = tx.send(live);
        })
    });

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let mut place_on_current = true;
    let focus = focus.map(str::to_string);

    let effort_saved = saved_config_path()
        .map(|path| {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
            load_saved_config_at(&path)
        })
        .unwrap_or_default();
    let mut efforts = RowEfforts::default();
    let mut toggles = PickerToggles::from_config(config);

    let mut drop: Option<DropState> = None;

    let curated = crate::setup::active_connect_id(config)
        .and_then(|id| super::context::curated_recommended(&id));

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        let mut refreshing = _refresh.is_some();
        loop {
            if refreshing && let Ok(live) = refresh_rx.try_recv() {
                refreshing = false;
                if !live.is_empty() {
                    models = merge_models(&config.base_url, live);
                    (current_id, recent_ids) = recent_head_for(&config.base_url);
                }
            }
            let f = filter.to_lowercase();
            let matched: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    f.is_empty()
                        || m_id.to_lowercase().contains(&f)
                        || m_name.to_lowercase().contains(&f)
                })
                .collect();

            let lead = matched.iter().filter(|(id, _)| is_composite_id(id)).count();
            let mut ordered = composites_first(&matched);
            let mut plain = ordered.split_off(lead);

            let sep = recent_prefix_len(
                current_id.as_deref(),
                &recent_ids,
                plain.iter().map(|(id, _)| id.as_str()),
            );

            let mut all_head = false;
            let mut div_at = sep;
            if sep == 0 && f.is_empty() {
                let head: Vec<String> = super::recommend::recommended_ids(
                    &models,
                    curated.as_deref(),
                    super::context::model_meta,
                    RECOMMENDED_HEAD,
                )
                .into_iter()
                .filter(|id| !is_composite_id(id))
                .collect();
                plain = head_first(&plain, &head);
                if !head.is_empty() && head.len() < plain.len() {
                    div_at = head.len();
                    all_head = true;
                }
            }

            let div = (div_at > 0 && div_at < plain.len()).then_some(lead + div_at);
            ordered.extend(plain);
            let filtered_models = ordered;
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

            if let Some(d) = &drop {
                match rows.iter().position(|row| match row {
                    Row::Model(mi) => filtered_models[*mi].0 == d.row,
                    _ => false,
                }) {
                    Some(pos) => sel = pos,
                    None => drop = None,
                }
            }

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

            let gap = composite_gap(&rows, lead);

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
                all_head,
                gap,
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

                    if filtered_models.is_empty() && filter.is_empty() && config.model.is_some() {
                        return Ok(false);
                    }

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
                        validate_direct_model_id(filter.trim(), &models)
                            .unwrap_or_else(|_| filter.trim().to_string())
                    } else {
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

    toggles.commit(config, &mut saved);
    save_saved_config_at(&path, &saved)?;
    Ok(())
}

pub(crate) fn validate_direct_model_id(
    raw: &str,
    models: &[(String, String)],
) -> Result<String, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("usage: /models <model-id> — type /models to browse models".to_string());
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
                "ambiguous model '{input}' — matches {} models ({shown}{}); type /models to pick",
                ids.len(),
                if ids.len() > 5 { ", …" } else { "" },
            ));
        }
    }
    Err(format!(
        "unknown model '{input}' — no match; type /models to browse {} models",
        models.len(),
    ))
}

#[path = "model_modal_tests.rs"]
#[cfg(test)]
mod tests;
