use std::collections::{BTreeMap, HashMap, HashSet};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::config::Config;
use crate::providers::{InstalledProvider, ProviderRegistry};

use super::catalog::{
    self, AUTH_MODE_API_KEY, AUTH_MODE_NONE, ConnectAuth, ConnectItem, SavedConfig,
    build_connect_items, detect_connect_candidates, load_auth_keys, load_connect_auth,
    load_custom_endpoints, load_saved_config_at, lock_saved_config_at, normalize_custom_base_url,
    plugin_models_key, save_saved_config_at, saved_config_path, sort_connect_items,
};
use super::model_modal::{
    PickerToggles, commit_row_effort, compose_fast_model, fetch_models_live, fit_chars,
    initial_effort_in, known_context, merge_models, meter_spans, model_picker_loop,
    model_refresh_due, rate_text, row_levels, saved_models_for, selection_parts,
};
use super::provider_auth::activate_plugin_connection;
use super::ui::{BackgroundSnapshot, render_dimmed_background};
use super::variants::{Parts, effort_word};

pub(crate) enum PanesOutcome {
    Picked,
    Dismissed,
    Connect(String),
    Removed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Rail,
    Models,
}

struct PaneProvider {
    item: ConnectItem,
    list_key: String,
    connected: bool,
    removable: bool,
    installed: Option<InstalledProvider>,
}

impl PaneProvider {
    fn list_key(item: &ConnectItem) -> String {
        match &item.auth {
            ConnectAuth::Plugin { .. } => plugin_models_key(&item.id),
            _ => item.base_url.clone(),
        }
    }

    fn adoptable(&self, keys: &BTreeMap<String, String>) -> bool {
        self.connected || self.item.no_auth || keys.contains_key(&self.item.id)
    }

    fn key(&self, keys: &BTreeMap<String, String>, config: &Config) -> Option<String> {
        keys.get(&self.item.id)
            .cloned()
            .or_else(|| {
                provider_is_active(self, config)
                    .then(|| config.api_key.clone())
                    .flatten()
            })
            .filter(|k| !k.is_empty())
    }
}

fn build_rail(config: &Config, ollama: bool) -> (Vec<PaneProvider>, BTreeMap<String, String>) {
    let catalog = catalog::load_catalog().unwrap_or_default();
    let providers = crate::setup::catalog::gray_home()
        .map(|home| ProviderRegistry::load_cached(&home).installed())
        .unwrap_or_default();
    let customs = load_custom_endpoints();
    let mut items = build_connect_items(&catalog, &providers, &customs);
    let auth = load_connect_auth();
    let detected = detect_connect_candidates(ollama, |k| std::env::var(k).ok());
    sort_connect_items(&mut items, config, &auth, &detected);
    let keys = load_auth_keys();

    let rail = items
        .into_iter()
        .filter(|item| item.id != "custom")
        .map(|item| {
            let installed = match &item.auth {
                ConnectAuth::Plugin { .. } => providers
                    .iter()
                    .find(|p| p.provider_id() == item.id)
                    .cloned(),
                _ => None,
            };
            PaneProvider {
                connected: item.is_connected(config, &auth),
                removable: super::connect::is_removable(&item, config, &auth),
                list_key: PaneProvider::list_key(&item),
                installed,
                item,
            }
        })
        .collect();
    (rail, keys)
}

fn active_rail_index(rail: &[PaneProvider], config: &Config) -> usize {
    rail.iter()
        .position(|p| provider_is_active(p, config))
        .unwrap_or(0)
}

fn provider_is_active(p: &PaneProvider, config: &Config) -> bool {
    if config.credential_source == "plugin" || matches!(p.item.auth, ConnectAuth::Plugin { .. }) {
        return config.credential_source == "plugin" && config.provider_id == p.item.id;
    }
    !p.item.base_url.is_empty()
        && normalize_custom_base_url(&p.item.base_url)
            == normalize_custom_base_url(&config.base_url)
}

fn scoped_config(config: &Config, p: &PaneProvider, keys: &BTreeMap<String, String>) -> Config {
    let mut scoped = config.clone();
    if !provider_is_active(p, config) {
        scoped.model = None;
    }
    if let Some(installed) = &p.installed {
        scoped.base_url = installed.provider.transport.base_url.to_string();
        scoped.provider_id = installed.provider_id();
        scoped.credential_source = "plugin".to_string();
        scoped.auth_ref = installed.auth_ref();
        scoped.api_key = None;
    } else {
        scoped.base_url = p.item.base_url.clone();
        scoped.provider_id.clear();
        scoped.credential_source.clear();
        scoped.auth_ref.clear();
        scoped.api_key = p.key(keys, config);
    }
    scoped
}

#[derive(Default)]
struct PaneEfforts {
    stepped: HashMap<String, &'static str>,
}

impl PaneEfforts {
    fn key(p: &PaneProvider, model: &str) -> String {
        format!("{}\u{1f}{model}", p.list_key)
    }

    fn shown(
        &self,
        scoped: &Config,
        saved: &SavedConfig,
        p: &PaneProvider,
        model: &str,
    ) -> Option<&'static str> {
        let levels = row_levels(model, false, &Parts::new());
        let initial = initial_effort_in(scoped, saved, model, &levels)?;
        let want = self
            .stepped
            .get(&Self::key(p, model))
            .copied()
            .unwrap_or(initial);
        if levels.contains(&want) {
            Some(want)
        } else {
            super::variants::clamp_to_levels(&levels, want).or(Some(initial))
        }
    }

    fn was_stepped(&self, p: &PaneProvider, model: &str) -> bool {
        self.stepped.contains_key(&Self::key(p, model))
    }

    fn step(
        &mut self,
        scoped: &Config,
        saved: &SavedConfig,
        p: &PaneProvider,
        model: &str,
        forward: bool,
    ) -> bool {
        let Some(cur) = self.shown(scoped, saved, p, model) else {
            return false;
        };
        let levels = row_levels(model, false, &Parts::new());
        let pos = levels.iter().position(|l| *l == cur).unwrap_or(0);
        let next = if forward {
            (pos + 1).min(levels.len().saturating_sub(1))
        } else {
            pos.saturating_sub(1)
        };
        match levels.get(next) {
            Some(level) if next != pos || levels.get(pos) != Some(&cur) => {
                self.stepped.insert(Self::key(p, model), level);
                true
            }
            _ => false,
        }
    }
}

fn is_composite(model: &str) -> bool {
    super::variants::row_shape(model).is_some_and(|s| s.is_composite())
}

fn start_row(models: &[(String, String)], active: Option<&str>) -> usize {
    active
        .and_then(|cur| models.iter().position(|(id, _)| id == cur))
        .or_else(|| models.iter().position(|(id, _)| !is_composite(id)))
        .unwrap_or(0)
}

fn models_for<'a>(
    p: &PaneProvider,
    cache: &'a mut HashMap<String, Vec<(String, String)>>,
) -> &'a [(String, String)] {
    cache
        .entry(p.list_key.clone())
        .or_insert_with(|| {
            let mut models = saved_models_for(&p.list_key, &p.item.base_url);
            if models.is_empty() {
                let alt = normalize_custom_base_url(&p.item.base_url);
                if alt != p.list_key {
                    models = saved_models_for(&alt, &p.item.base_url);
                }
            }
            models
        })
        .as_slice()
}

fn save_key_connection(config: &Config, saved: &mut SavedConfig) {
    saved.base_url = Some(config.base_url.clone());
    saved.api_key = config.api_key.clone();
    saved.auth_mode = Some(
        if config.api_key.is_none() {
            AUTH_MODE_NONE
        } else {
            AUTH_MODE_API_KEY
        }
        .into(),
    );
    saved.provider_id.clear();
    saved.credential_source.clear();
    saved.auth_ref.clear();
    saved.model = config.model.clone();
}

fn commit_pick(
    config: &mut Config,
    p: &PaneProvider,
    keys: &BTreeMap<String, String>,
    model: &str,
    shown: Option<&str>,
    stepped: bool,
    toggles: PickerToggles,
) -> anyhow::Result<()> {
    config.model_parts.clear();
    match &p.installed {
        Some(installed) => activate_plugin_connection(config, installed, model)?,
        None => *config = scoped_config(config, p, keys),
    }
    config.model = Some(model.to_string());
    let path = saved_config_path()?;
    let _lock = lock_saved_config_at(&path).ok();
    let mut saved = load_saved_config_at(&path);
    if p.installed.is_none() {
        save_key_connection(config, &mut saved);
    }
    saved.model = config.model.clone();
    commit_row_effort(config, &mut saved, shown, stepped);
    toggles.commit(config, &mut saved);
    save_saved_config_at(&path, &saved)
}

fn delegate_pick(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    config: &mut Config,
    bg: &BackgroundSnapshot,
    p: &PaneProvider,
    keys: &BTreeMap<String, String>,
    model: &str,
) -> anyhow::Result<PanesOutcome> {
    *config = scoped_config(config, p, keys);
    let picked = model_picker_loop(terminal, config, Some(bg), Some(model))?;
    match &p.installed {
        _ if !picked => {}
        Some(installed) => {
            let chosen = config.model.clone().unwrap_or_else(|| model.to_string());
            activate_plugin_connection(config, installed, &chosen)?;
        }
        None => {
            let path = saved_config_path()?;
            let _lock = lock_saved_config_at(&path).ok();
            let mut saved = load_saved_config_at(&path);
            save_key_connection(config, &mut saved);
            save_saved_config_at(&path, &saved)?;
        }
    }
    Ok(if picked {
        PanesOutcome::Picked
    } else {
        PanesOutcome::Dismissed
    })
}

const RAIL_W: u16 = 22;

struct PaneView<'a> {
    rail: &'a [PaneProvider],
    rail_sel: usize,
    rail_top: usize,
    scoped: &'a Config,
    saved: &'a SavedConfig,
    efforts: &'a PaneEfforts,
    models: &'a [(String, String)],
    model_sel: usize,
    model_top: usize,
    focus: Focus,
    filter: &'a str,
    adoptable: bool,
    toggles: PickerToggles,
    config: &'a Config,
    confirm_remove: bool,
    notice: Option<&'a str>,
}

impl PaneView<'_> {
    fn provider(&self) -> Option<&PaneProvider> {
        self.rail.get(self.rail_sel)
    }

    fn highlighted(&self) -> Option<&(String, String)> {
        self.models
            .get(clamp_sel(self.model_sel, self.models.len()))
    }

    fn shown(&self, id: &str) -> Option<&'static str> {
        self.provider()
            .and_then(|p| self.efforts.shown(self.scoped, self.saved, p, id))
    }
}

fn fast_wire(model: &str, effort: Option<&str>, models: &[(String, String)]) -> Option<String> {
    match super::variants::row_shape(model) {
        Some(shape) if !shape.variants.is_empty() => {
            let parts = selection_parts(&shape, &Parts::new());
            super::variants::has_variant(&shape.variants, true, &parts)
                .then(|| super::variants::resolve_variant(&shape.variants, effort, true, &parts))
                .flatten()
                .map(|v| v.id.clone())
        }
        _ => compose_fast_model(model, effort, models),
    }
}

fn model_label<'a>(id: &'a str, name: &'a str) -> &'a str {
    if name.is_empty() { id } else { name }
}

fn match_range(label: &str, filter: &str) -> Option<(usize, usize)> {
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = filter.chars().map(lower).collect();
    let chars: Vec<char> = label.chars().map(lower).collect();
    if needle.is_empty() {
        return None;
    }
    chars
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|i| (i, i + needle.len()))
}

fn underline_match(label: &str, filter: &str, style: Style) -> Vec<Span<'static>> {
    let Some((i, j)) = match_range(label, filter) else {
        return vec![Span::styled(label.to_string(), style)];
    };
    let chars: Vec<char> = label.chars().collect();
    [
        (0, i, style),
        (i, j, style.add_modifier(Modifier::UNDERLINED)),
        (j, chars.len(), style),
    ]
    .into_iter()
    .filter(|(a, b, _)| a < b)
    .map(|(a, b, st)| Span::styled(chars[a..b].iter().collect::<String>(), st))
    .collect()
}

fn draw(frame: &mut Frame, bg: &BackgroundSnapshot, v: &PaneView) {
    let area = frame.area();
    if area.width < 40 || area.height < 10 {
        return;
    }
    render_dimmed_background(frame, bg);
    let t = crate::theme::theme();
    let fg = |c: Color| Style::default().fg(c);
    let bold = |c: Color| fg(c).add_modifier(Modifier::BOLD);
    let fill = |c: Color| Style::default().bg(c);
    let w = area.width.clamp(48, 84).min(area.width.saturating_sub(4));
    let h = area.height.clamp(10, 24).min(area.height.saturating_sub(2));
    let outer = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    frame.render_widget(Clear, outer);
    frame.render_widget(Paragraph::new("").style(fill(t.raised_bg)), outer);
    let inner = Rect::new(outer.x + 1, outer.y + 1, w - 2, h - 2);
    let list_h = list_rows(inner.height);
    let list_y = inner.y + 2;
    let detail_y = inner.y + inner.height - if roomy(inner.height) { 3 } else { 2 };
    let rail_w = RAIL_W.min(inner.width / 3).max(14);
    let pane_x = inner.x + rail_w + 1;
    let pane_w = inner.width.saturating_sub(rail_w + 1);
    let row = |x: u16, y: u16, w: u16| Rect::new(x, y, w, 1);
    let provider = v.provider();
    let highlighted = v.highlighted();

    let filter = if v.filter.is_empty() {
        vec![Span::styled("type to filter ", fg(t.text_faint))]
    } else {
        vec![
            Span::styled("filter ", fg(t.text_faint)),
            Span::styled(
                v.filter.to_string(),
                fg(t.text_bright).add_modifier(Modifier::UNDERLINED),
            ),
            Span::raw(" "),
        ]
    };
    let mut head = vec![Span::raw(" ")];
    if let Some(p) = provider {
        head.push(Span::styled(
            p.item.name.clone(),
            bold(if v.adoptable {
                t.text_bright
            } else {
                t.text_dim
            }),
        ));
    }
    if !v.adoptable {
        head.push(Span::styled(" · not connected", fg(t.text_faint)));
    } else if let Some((id, name)) = highlighted {
        head.push(Span::styled(
            format!(" · {}", model_label(id, name)),
            fg(t.text_body),
        ));
        if !is_composite(id)
            && let Some(eff) = v.shown(id)
        {
            head.push(Span::styled(
                format!(" · {}", effort_word(eff)),
                fg(t.text_body),
            ));
        }
        if v.toggles.fast {
            head.push(Span::styled(" · fast", fg(t.accent)));
        }
    }
    let filter_w: usize = filter.iter().map(Span::width).sum();
    frame.render_widget(
        Paragraph::new(Line::from(head)),
        row(
            inner.x,
            inner.y,
            inner.width.saturating_sub(filter_w as u16 + 1),
        ),
    );
    frame.render_widget(
        Paragraph::new(Line::from(filter)).alignment(Alignment::Right),
        row(inner.x, inner.y, inner.width),
    );

    for (r, p) in v.rail.iter().enumerate().skip(v.rail_top).take(list_h) {
        let on = r == v.rail_sel;
        let lit = on && v.focus == Focus::Rail;
        let text = if lit {
            bold(t.on_selection)
        } else if on {
            bold(t.text_bright)
        } else {
            fg(if p.connected {
                t.text_body
            } else {
                t.text_faint
            })
        };
        let bar = match (lit, on) {
            (true, _) => fill(t.accent),
            (_, true) => fill(t.bezel_lo),
            _ => Style::default(),
        };
        let mark = Span::styled(
            if p.connected { " ✓ " } else { "   " },
            fg(if lit { t.on_selection } else { t.success }),
        );
        let name = fit_chars(&p.item.name, rail_w.saturating_sub(4) as usize);
        frame.render_widget(
            Paragraph::new(Line::from(vec![mark, Span::styled(name, text)])).style(bar),
            row(inner.x, list_y + (r - v.rail_top) as u16, rail_w),
        );
    }

    if let Some(p) = provider {
        if v.models.is_empty() {
            let msg = match (v.adoptable, v.filter.is_empty()) {
                (false, _) => " no models yet",
                (true, true) => " fetching models…",
                (true, false) => " no matches",
            };
            frame.render_widget(
                Paragraph::new(Span::styled(msg, fg(t.text_faint))),
                row(pane_x, list_y, pane_w),
            );
        }
        let active = provider_is_active(p, v.config);
        for (m, (id, name)) in v.models.iter().enumerate().skip(v.model_top).take(list_h) {
            let sel = m == v.model_sel && v.focus == Focus::Models;
            let current = active && v.config.model.as_deref() == Some(id);
            let base = if sel {
                fill(t.accent)
            } else {
                Style::default()
            };
            let text = base.patch(if sel {
                bold(t.on_selection)
            } else if current {
                fg(t.text_bright)
            } else {
                fg(t.text_body)
            });
            let suffix = if is_composite(id) {
                vec![Span::styled(
                    "▸ ",
                    fg(if sel { t.on_selection } else { t.text_dim }),
                )]
            } else {
                v.efforts
                    .shown(v.scoped, v.saved, p, id)
                    .map(|eff| meter_spans(&row_levels(id, false, &Parts::new()), eff, sel))
                    .unwrap_or_default()
            };
            let suffix_w: usize = suffix.iter().map(Span::width).sum();
            let name_w = (pane_w as usize).saturating_sub(3 + suffix_w);
            let label = fit_chars(model_label(id, name), name_w);
            let mut spans = vec![Span::styled(
                if current { " ✓ " } else { "   " },
                fg(if sel { t.on_selection } else { t.success }),
            )];
            spans.extend(underline_match(&label, v.filter, text));
            spans.push(Span::raw(
                " ".repeat(name_w.saturating_sub(label.chars().count())),
            ));
            spans.extend(suffix);
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(base),
                row(pane_x, list_y + (m - v.model_top) as u16, pane_w),
            );
        }
    }

    let dim = |s: String| Span::styled(s, fg(t.text_dim));
    let mut detail = vec![Span::raw(" ")];
    match highlighted {
        _ if v.confirm_remove => {
            if let Some(p) = provider {
                detail.extend([
                    dim("Remove ".into()),
                    Span::styled(p.item.name.clone(), bold(t.text_bright)),
                    dim("? its saved credential is deleted".into()),
                ]);
            }
        }
        _ if v.notice.is_some() => detail.extend(v.notice.map(|n| dim(n.to_string()))),
        _ if !v.adoptable => detail.push(dim("enter to sign in".into())),
        Some((id, _)) if is_composite(id) => detail.push(dim("enter picks its slots".into())),
        Some((id, _)) => {
            let mut parts: Vec<Vec<Span>> = Vec::new();
            if let Some(c) = known_context(id) {
                parts.push(vec![
                    dim("Context ".into()),
                    Span::styled(super::context::format_context_length(c), fg(t.text_body)),
                ]);
            }
            if fast_wire(id, v.shown(id), v.models).is_some() {
                parts.push(vec![
                    dim("Fast ".into()),
                    if v.toggles.fast {
                        Span::styled("on", fg(t.accent))
                    } else {
                        Span::styled("off", fg(t.text_body))
                    },
                ]);
            }
            if !v.toggles.show_reasoning {
                parts.push(vec![
                    dim("Reasoning ".into()),
                    Span::styled("hidden", fg(t.text_body)),
                ]);
            }
            if let Some(r) = super::context::get_model_rate(id) {
                let used: usize =
                    parts.iter().flatten().map(Span::width).sum::<usize>() + parts.len() * 3;
                let room = (inner.width as usize).saturating_sub(used + 2);
                parts.push(vec![dim(rate_text(&r, room))]);
            }
            for (i, part) in parts.into_iter().enumerate() {
                if i > 0 {
                    detail.push(Span::styled(" · ", fg(t.text_faint)));
                }
                detail.extend(part);
            }
        }
        None => {}
    }
    frame.render_widget(
        Paragraph::new(Line::from(detail)),
        row(inner.x, detail_y, inner.width),
    );

    let has_effort = highlighted.is_some_and(|(id, _)| v.shown(id).is_some());
    let has_fast = highlighted
        .is_some_and(|(id, _)| v.toggles.fast || fast_wire(id, v.shown(id), v.models).is_some());
    let enter = match (v.adoptable, v.focus) {
        (false, _) => "sign in",
        (true, Focus::Rail) => "open",
        (true, Focus::Models) => "select",
    };
    let mut keys = if v.confirm_remove {
        vec![("enter", "remove", true), ("esc", "cancel", true)]
    } else {
        let mut keys = vec![
            ("↑↓", "move", true),
            ("⇧↑↓", "provider", true),
            ("←→", "effort", has_effort),
            ("tab", "fast", has_fast),
            ("^r", "reasoning", true),
            ("enter", enter, true),
        ];
        if provider.is_some_and(|p| p.removable) {
            keys.push(("del", "remove", true));
        }
        keys.push(("esc", "close", true));
        keys
    };
    let keys_w = |keys: &[(&str, &str, bool)]| -> usize {
        keys.iter()
            .map(|(k, l, _)| k.chars().count() + l.chars().count() + 4)
            .sum::<usize>()
            + 1
    };
    for drop in ["^r", "⇧↑↓", "tab", "del", "←→"] {
        if keys_w(&keys) <= inner.width as usize {
            break;
        }
        keys.retain(|(k, _, _)| *k != drop);
    }
    let mut footer = vec![Span::raw(" ")];
    footer.extend(keys.into_iter().flat_map(|(k, label, live)| {
        [
            Span::styled(k, fg(if live { t.text_soft } else { t.text_faint })),
            Span::styled(
                format!(" {label}   "),
                fg(if live { t.text_dim } else { t.text_faint }),
            ),
        ]
    }));
    frame.render_widget(
        Paragraph::new(Line::from(footer)),
        row(inner.x, inner.y + inner.height - 1, inner.width),
    );
}

fn roomy(inner_h: u16) -> bool {
    inner_h >= 12
}

fn list_rows(inner_h: u16) -> usize {
    inner_h.saturating_sub(if roomy(inner_h) { 6 } else { 5 }) as usize
}

fn list_height(term_h: u16) -> usize {
    let h = term_h.clamp(10, 24).min(term_h.saturating_sub(2));
    list_rows(h.saturating_sub(2))
}

fn clamp_sel(sel: usize, len: usize) -> usize {
    sel.min(len.saturating_sub(1))
}

pub(crate) fn run_model_panes(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<PanesOutcome> {
    let (_session, mut terminal) = super::open_modal()?;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let opened = std::time::Instant::now();
    let (ollama_tx, ollama_rx) = std::sync::mpsc::channel::<bool>();
    let mut ollama = super::context::ollama_last_seen().unwrap_or(false);
    std::thread::spawn(move || {
        let _ = ollama_tx.send(super::context::ollama_running());
    });
    let mut first_draw = true;
    let mut before = config.clone();
    let (mut rail, mut keys) = build_rail(config, ollama);
    let mut model_cache: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut efforts = PaneEfforts::default();
    let mut toggles = PickerToggles::from_config(config);
    let mut focus = Focus::Rail;
    let mut rail_sel = active_rail_index(&rail, config);
    let mut rail_top = 0usize;
    let mut model_sel = 0usize;
    let mut model_top = 0usize;
    let mut filter = String::new();
    let mut place_current = true;
    let mut confirm_remove = false;
    let mut do_remove = false;
    let mut notice: Option<String> = None;
    let mut removed: Option<String> = None;

    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<(String, Vec<(String, String)>)>();
    let mut fetching: HashSet<String> = HashSet::new();
    let kick = |p: &PaneProvider,
                key: Option<String>,
                fetching: &mut HashSet<String>,
                tx: &std::sync::mpsc::Sender<(String, Vec<(String, String)>)>| {
        if !fetching.insert(p.list_key.clone()) {
            return;
        }
        let installed = p.installed.clone();
        let base = p.item.base_url.clone();
        let list_key = p.list_key.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let live = fetch_models_live(&list_key, installed, &base, key.as_deref());
            let _ = tx.send((list_key, live));
        });
    };

    let committed = (|| -> anyhow::Result<PanesOutcome> {
        let saved = saved_config_path()
            .map(|path| {
                let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
                load_saved_config_at(&path)
            })
            .unwrap_or_default();
        loop {
            if let Ok(up) = ollama_rx.try_recv()
                && up != ollama
            {
                ollama = up;
                let keep = rail.get(rail_sel).map(|p| p.item.id.clone());
                (rail, keys) = build_rail(config, ollama);
                rail_sel = keep
                    .and_then(|id| rail.iter().position(|p| p.item.id == id))
                    .unwrap_or(rail_sel)
                    .min(rail.len().saturating_sub(1));
            }
            while let Ok((list_key, live)) = refresh_rx.try_recv() {
                if !live.is_empty()
                    && let Some(p) = rail.iter().find(|p| p.list_key == list_key)
                {
                    model_cache.insert(list_key.clone(), merge_models(&p.item.base_url, live));
                }
            }

            if std::mem::take(&mut do_remove)
                && let Some(p) = rail.get(rail_sel)
            {
                let id = p.item.id.clone();
                let name = super::connect::forget_provider(&p.item, config)?;
                before = config.clone();
                (rail, keys) = build_rail(config, ollama);
                rail_sel = rail
                    .iter()
                    .position(|q| q.item.id == id)
                    .unwrap_or(rail_sel)
                    .min(rail.len().saturating_sub(1));
                place_current = true;
                model_top = 0;
                notice = Some(format!("Removed {name}"));
                removed = Some(name);
            }
            let provider = rail.get(rail_sel);
            let adoptable = provider.is_some_and(|p| p.adoptable(&keys));
            let all_models = provider
                .map(|p| models_for(p, &mut model_cache).to_vec())
                .unwrap_or_default();
            if let Some(p) = provider
                && adoptable
                && (all_models.is_empty() || model_refresh_due(&p.list_key))
            {
                kick(p, p.key(&keys, config), &mut fetching, &refresh_tx);
            }
            let models: Vec<(String, String)> = if filter.is_empty() {
                all_models
            } else {
                let f = filter.to_lowercase();
                all_models
                    .into_iter()
                    .filter(|(id, name)| {
                        id.to_lowercase().contains(&f) || name.to_lowercase().contains(&f)
                    })
                    .collect()
            };
            if place_current {
                place_current = false;
                let active = provider
                    .filter(|p| provider_is_active(p, config))
                    .and(config.model.as_deref());
                model_sel = start_row(&models, active);
            }
            model_sel = clamp_sel(model_sel, models.len());
            let scoped = provider
                .map(|p| scoped_config(config, p, &keys))
                .unwrap_or_else(|| config.clone());

            let list_h = list_height(terminal.size()?.height);
            if rail_sel < rail_top {
                rail_top = rail_sel;
            } else if rail_sel >= rail_top + list_h {
                rail_top = rail_sel + 1 - list_h;
            }
            if model_sel < model_top {
                model_top = model_sel;
            } else if model_sel >= model_top + list_h {
                model_top = model_sel + 1 - list_h;
            }

            let highlighted = models.get(model_sel).map(|(id, _)| id.clone());
            let shown = provider
                .zip(highlighted.as_deref())
                .and_then(|(p, id)| efforts.shown(&scoped, &saved, p, id));
            let fast_supported = toggles.fast
                || highlighted
                    .as_deref()
                    .is_some_and(|id| fast_wire(id, shown, &models).is_some());

            terminal.draw(|f| {
                draw(
                    f,
                    &bg_snapshot,
                    &PaneView {
                        rail: &rail,
                        rail_sel,
                        rail_top,
                        scoped: &scoped,
                        saved: &saved,
                        efforts: &efforts,
                        models: &models,
                        model_sel,
                        model_top,
                        focus,
                        filter: &filter,
                        adoptable,
                        toggles,
                        config,
                        confirm_remove,
                        notice: notice.as_deref(),
                    },
                )
            })?;
            if std::mem::take(&mut first_draw) {
                log::debug!(target: "gray_timing", "model_panes first_draw elapsed_ms={}", opened.elapsed().as_millis());
            }

            if !poll(std::time::Duration::from_millis(200))? {
                continue;
            }
            let Event::Key(key) = read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            notice = None;
            if std::mem::take(&mut confirm_remove) {
                do_remove = key.code == KeyCode::Enter;
                continue;
            }
            let last_model = models.len().saturating_sub(1);
            match key.code {
                KeyCode::Esc if !filter.is_empty() => filter.clear(),
                KeyCode::Esc if focus == Focus::Models => focus = Focus::Rail,
                KeyCode::Esc => return Ok(PanesOutcome::Dismissed),
                KeyCode::Char('c') if ctrl => return Ok(PanesOutcome::Dismissed),
                KeyCode::Char('r') if ctrl => toggles.toggle_reasoning(),
                KeyCode::Delete => confirm_remove = provider.is_some_and(|p| p.removable),
                KeyCode::Char('d') if ctrl => {
                    confirm_remove = provider.is_some_and(|p| p.removable);
                }
                KeyCode::Enter => match focus {
                    _ if !adoptable => {
                        if let Some(p) = provider {
                            return Ok(PanesOutcome::Connect(p.item.id.clone()));
                        }
                    }
                    Focus::Rail => focus = Focus::Models,
                    Focus::Models => {
                        let (Some(p), Some(id)) = (provider, highlighted.as_deref()) else {
                            continue;
                        };
                        if is_composite(id) {
                            return delegate_pick(
                                &mut terminal,
                                config,
                                &bg_snapshot,
                                p,
                                &keys,
                                id,
                            );
                        }
                        let stepped = efforts.was_stepped(p, id);
                        commit_pick(config, p, &keys, id, shown, stepped, toggles)?;
                        return Ok(PanesOutcome::Picked);
                    }
                },
                KeyCode::Up | KeyCode::Down if shift || focus == Focus::Rail => {
                    rail_sel = if key.code == KeyCode::Up {
                        rail_sel.saturating_sub(1)
                    } else {
                        (rail_sel + 1).min(rail.len().saturating_sub(1))
                    };
                    place_current = true;
                    model_top = 0;
                }
                KeyCode::Up => model_sel = model_sel.saturating_sub(1),
                KeyCode::Down => model_sel = (model_sel + 1).min(last_model),
                KeyCode::PageUp => model_sel = model_sel.saturating_sub(list_h.max(1)),
                KeyCode::PageDown => model_sel = (model_sel + list_h.max(1)).min(last_model),
                KeyCode::Left | KeyCode::Right if focus == Focus::Models => {
                    let forward = key.code == KeyCode::Right;
                    let moved = matches!((provider, highlighted.as_deref()), (Some(p), Some(id))
                        if !is_composite(id) && efforts.step(&scoped, &saved, p, id, forward));
                    if !moved && !forward {
                        focus = Focus::Rail;
                    }
                }
                KeyCode::Right => focus = Focus::Models,
                KeyCode::Tab => toggles.toggle_fast(fast_supported),
                KeyCode::BackTab => {
                    focus = match focus {
                        Focus::Rail => Focus::Models,
                        Focus::Models => Focus::Rail,
                    };
                }
                KeyCode::Backspace => {
                    filter.pop();
                    place_current = true;
                }
                KeyCode::Char(c) if !ctrl => {
                    filter.push(c);
                    place_current = true;
                }
                _ => {}
            }
        }
    })();
    if !matches!(committed, Ok(PanesOutcome::Picked)) {
        *config = before;
    }
    committed.map(|outcome| match (outcome, removed) {
        (PanesOutcome::Dismissed, Some(name)) => PanesOutcome::Removed(name),
        (outcome, _) => outcome,
    })
}

#[path = "model_panes_tests.rs"]
#[cfg(test)]
mod tests;
