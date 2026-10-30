use super::*;

pub(crate) const VARIANT_EFFORT_WORDS: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max"];

pub(crate) const FAST_SUFFIXES: &[&str] = &["fast", "priority"];

pub(super) fn effort_variant_shape(model: &str) -> Option<(&str, &str)> {
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

pub(super) fn split_fast_variant(
    model: &str,
    known: &[(String, String)],
) -> Option<(String, &'static str)> {
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
    shape: &super::super::variants::RowShape,
    parts: &super::super::variants::Parts,
) -> super::super::variants::Parts {
    if shape.is_composite() {
        shape.settle_parts(Some(parts))
    } else {
        super::super::variants::Parts::new()
    }
}

pub(crate) fn wire_model_for(config: &Config) -> Option<String> {
    let model = config.model.as_deref()?;

    let rows = canonical_model_rows(config);
    let fast = config.fast_mode == Some(true);
    if let Some(shape) = super::super::variants::row_shape(model)
        && !shape.variants.is_empty()
    {
        let parts = selection_parts(&shape, &config.model_parts);
        let effort = config.thinking_effort.as_deref();
        return Some(
            super::super::variants::resolve_variant(&shape.variants, effort, fast, &parts)
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
            .unwrap_or_else(|| super::super::context::friendly_model_name(model)),
    )
}

pub(crate) fn composite_label_for(config: &Config) -> Option<String> {
    composite_label_parts_for(config).map(|segs| segs.into_iter().map(|(t, _)| t).collect())
}

pub(crate) fn composite_label_parts_for(config: &Config) -> Option<Vec<(String, Option<usize>)>> {
    let model = config.model.as_deref()?;
    let shape = super::super::variants::row_shape(model)?;
    let name = load_provider_names(config)
        .into_iter()
        .find(|(id, _)| id == model)
        .map(|(_, n)| n)
        .unwrap_or_else(|| super::super::context::friendly_model_name(model));
    let parts = selection_parts(&shape, &config.model_parts);
    super::super::variants::composite_label_parts(
        &shape,
        &name,
        &parts,
        config.thinking_effort.as_deref(),
    )
}

pub(super) fn load_provider_names(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, _) = picker_scope(config);
    super::super::context::load_provider_model_list(&list_key)
}

pub(super) fn variant_owner(
    wire: &str,
    known: &[(String, String)],
) -> Option<(String, ModelVariantRef)> {
    if known.iter().any(|(id, _)| id == wire) {
        return None;
    }
    known.iter().find_map(|(id, _)| {
        let shape = super::super::variants::row_shape(id)?;
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

pub(super) struct ModelVariantRef {
    pub(super) effort: Option<String>,
    pub(super) fast: bool,
    pub(super) parts: super::super::variants::Parts,
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
            let live = super::super::context::fetch_plugin_provider_models(installed);
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

pub(super) fn recent_head_for(base_url: &str) -> (Option<String>, Vec<String>) {
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

pub(super) fn recent_prefix_len<'a>(
    current: Option<&str>,
    recent: &[String],
    ids: impl Iterator<Item = &'a str>,
) -> usize {
    ids.take_while(|id| Some(*id) == current || recent.iter().any(|m| m.as_str() == *id))
        .count()
}
