use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Row {
    Model(usize),
    Divider,

    Opt(usize),
}

pub(super) fn head_first<'a>(
    list: &[&'a (String, String)],
    head: &[String],
) -> Vec<&'a (String, String)> {
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

pub(super) fn skip_divider(rows: &[Row], sel: usize, down: bool) -> usize {
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

pub(super) fn is_composite_id(id: &str) -> bool {
    super::super::variants::row_shape(id).is_some_and(|s| s.is_composite())
}

pub(super) fn composites_first<'a>(list: &[&'a (String, String)]) -> Vec<&'a (String, String)> {
    let (mut out, plain): (Vec<_>, Vec<_>) = list
        .iter()
        .copied()
        .partition(|(id, _)| is_composite_id(id));
    out.extend(plain);
    out
}

pub(super) fn gap_possible(models: &[(String, String)]) -> bool {
    let composites = models.iter().filter(|(id, _)| is_composite_id(id)).count();
    composites > 0 && composites < models.len()
}

pub(super) fn slot_of(row: usize, gap: Option<usize>) -> usize {
    row + usize::from(gap.is_some_and(|g| row >= g))
}

pub(super) fn row_at(slot: usize, gap: Option<usize>) -> Option<usize> {
    match gap {
        Some(g) if slot == g => None,
        Some(g) if slot > g => Some(slot - 1),
        _ => Some(slot),
    }
}

pub(super) fn rows_before(slot: usize, gap: Option<usize>) -> usize {
    slot - usize::from(gap.is_some_and(|g| slot > g))
}

pub(super) fn composite_gap(rows: &[Row], lead: usize) -> Option<usize> {
    if lead == 0 {
        return None;
    }
    rows.iter()
        .position(|row| matches!(row, Row::Model(mi) if *mi >= lead))
}

pub(crate) fn row_levels(
    model: &str,
    fast: bool,
    parts: &super::super::variants::Parts,
) -> Vec<&'static str> {
    if let Some(shape) = super::super::variants::row_shape(model)
        && !shape.variants.is_empty()
    {
        let parts = selection_parts(&shape, parts);
        return super::super::variants::variant_levels(&shape.variants, fast, &parts);
    }
    super::super::context::supported_thinking_levels(model)
        .iter()
        .map(|(l, _)| *l)
        .collect()
}

pub(super) fn has_declared_variants(model: &str) -> bool {
    super::super::variants::row_shape(model).is_some_and(|s| !s.variants.is_empty())
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
        super::super::variants::clamp_to_levels(levels, &want)
    } else {
        Some(super::super::context::clamp_thinking_level(model, &want))
    }
}

#[derive(Default)]
pub(super) struct RowEfforts {
    pub(super) stepped: std::collections::HashMap<String, &'static str>,

    pub(super) fast: bool,

    pub(super) parts: std::collections::HashMap<String, super::super::variants::Parts>,
}

impl RowEfforts {
    pub(super) fn parts_of(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
    ) -> super::super::variants::Parts {
        let Some(shape) = super::super::variants::row_shape(model).filter(|s| s.is_composite())
        else {
            return super::super::variants::Parts::new();
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

    pub(super) fn key(model: &str, parts: &super::super::variants::Parts) -> String {
        match super::super::variants::row_shape(model)
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

    pub(super) fn shown_with(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        parts: &super::super::variants::Parts,
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
            super::super::variants::clamp_to_levels(&levels, want).or(Some(initial))
        }
    }

    pub(super) fn shown(
        &self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
    ) -> Option<&'static str> {
        let parts = self.parts_of(config, saved, model);
        self.shown_with(config, saved, model, &parts)
    }

    pub(super) fn was_stepped(&self, model: &str) -> bool {
        self.stepped
            .keys()
            .any(|k| k == model || k.starts_with(&format!("{model}\u{1f}")))
    }

    pub(super) fn step_with(
        &mut self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        parts: &super::super::variants::Parts,
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

    pub(super) fn step(
        &mut self,
        config: &Config,
        saved: &SavedConfig,
        model: &str,
        forward: bool,
    ) {
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
        _ => super::super::provider_auth::apply_connection_effort(config, saved),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerToggles {
    pub(crate) fast: bool,
    pub(crate) show_reasoning: bool,
    pub(super) opened_fast: bool,
    pub(super) opened_show: bool,
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
pub(super) enum PickerKey {
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

pub(super) fn picker_key(
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<PickerKey> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    if let Some(up) = crate::keymap::select_move(&KeyEvent::new(code, modifiers)) {
        return Some(if up { PickerKey::Up } else { PickerKey::Down });
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        return match code {
            KeyCode::Char('c') => Some(PickerKey::Cancel),
            KeyCode::Char('r') => Some(PickerKey::ToggleReasoning),
            _ => None,
        };
    }
    Some(match code {
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

pub(super) fn effort_label(level: &str) -> String {
    super::super::variants::effort_word(level)
}
