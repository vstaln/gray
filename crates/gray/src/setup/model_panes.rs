use std::collections::{BTreeMap, HashMap, HashSet};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::config::Config;
use crate::providers::{InstalledProvider, ProviderRegistry};
use crate::quality_of_life::providers::detect_connect_candidates;

use super::catalog::{
    self, AUTH_MODE_API_KEY, AUTH_MODE_NONE, AuthEntry, ConnectAuth, ConnectItem, SavedConfig,
    build_connect_items, load_auth_keys, load_connect_auth, load_custom_endpoints,
    load_saved_config_at, lock_saved_config_at, normalize_custom_base_url, plugin_models_key,
    save_saved_config_at, saved_config_path, sort_connect_items,
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
    Install(String),
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
    detected: bool,
    /// Plugin index key when this row advertises a provider plugin that is
    /// not installed yet (installable login).
    install: Option<String>,
}

impl PaneProvider {
    fn list_key(item: &ConnectItem) -> String {
        match &item.auth {
            ConnectAuth::Plugin { .. } => plugin_models_key(&item.id),
            _ => item.base_url.clone(),
        }
    }

    fn adoptable(&self, keys: &BTreeMap<String, String>) -> bool {
        self.install.is_none()
            && (self.connected || self.item.no_auth || keys.contains_key(&self.item.id))
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

/// An index entry earns an installable provider row by declaring
/// `providers` — the same manifest field installed plugins announce.
/// Entries without the declaration never reach the rail; the hardcoded
/// `gray-*-sub` name list this replaced is gone.
/// Plugin names that are already installed (enabled or not), from the same
/// lock files /plugins reads.
fn installed_plugin_names(home: &std::path::Path) -> HashSet<String> {
    crate::plugin_cli::list_managed(home)
        .map(|rows| rows.into_iter().map(|p| p.name).collect())
        .unwrap_or_default()
}

/// Fetch the plugin index the way `gray plugin install <name>` resolves it:
/// hub index first, community index on failure.
fn fetch_provider_index() -> Option<gray_pkg::index::Index> {
    let client = gray_pkg::fetch::client().ok()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let secs = std::time::Duration::from_secs(5);
        if let Ok(Ok(index)) =
            tokio::time::timeout(secs, gray_pkg::index::fetch_hub_index(&client)).await
        {
            return Some(index);
        }
        tokio::time::timeout(secs, gray_pkg::index::fetch_index(&client))
            .await
            .ok()?
            .ok()
    })
}

fn build_rail(
    config: &Config,
    ollama: bool,
    index: Option<&gray_pkg::index::Index>,
) -> (Vec<PaneProvider>, BTreeMap<String, String>) {
    let catalog = catalog::load_catalog().unwrap_or_default();
    let home = crate::setup::catalog::gray_home().ok();
    let providers = home
        .as_ref()
        .map(|home| ProviderRegistry::load_cached(home).installed())
        .unwrap_or_default();
    let customs = load_custom_endpoints();
    let mut items = build_connect_items(&catalog, &providers, &customs);
    let mut auth = load_connect_auth();
    let registry_token = catalog::load_registry_token();
    if let Some(token) = registry_token.clone() {
        auth.entry(catalog::GRAY_PORTAL_ID.to_string())
            .or_insert(AuthEntry::Key(token));
    }
    let detected = detect_connect_candidates(ollama, |k| std::env::var(k).ok());
    sort_connect_items(&mut items, config, &auth, &detected);
    let mut keys = load_auth_keys();
    if let Some(token) = registry_token {
        keys.entry(catalog::GRAY_PORTAL_ID.to_string())
            .or_insert(token);
    }

    let mut rail: Vec<PaneProvider> = items
        .into_iter()
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
                detected: detected.contains_key(&item.id),
                installed,
                install: None,
                item,
            }
        })
        .collect();

    if let (Some(index), Some(home)) = (index, home.as_ref()) {
        let installed_names = installed_plugin_names(home);
        for (key, entry) in &index.plugins {
            if installed_names.contains(key.as_str()) || entry.providers.is_empty() {
                continue;
            }
            rail.push(PaneProvider {
                item: ConnectItem {
                    id: format!("plugin:{key}"),
                    name: entry
                        .providers
                        .first()
                        .map(|d| d.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| key.clone()),
                    sublabel: "(not installed)".to_string(),
                    base_url: String::new(),
                    no_auth: false,
                    auth: ConnectAuth::None,
                },
                list_key: format!("plugin:{key}"),
                connected: false,
                removable: false,
                installed: None,
                detected: false,
                install: Some(key.to_string()),
            });
        }
    }
    (rail, keys)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailRow {
    Provider(usize),
    Group(GroupId),
    Spacer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupId {
    Logins,
    OAuth,
    ApiKey,
}

impl GroupId {
    const ALL: [GroupId; 3] = [GroupId::Logins, GroupId::OAuth, GroupId::ApiKey];

    fn index(self) -> usize {
        match self {
            GroupId::Logins => 0,
            GroupId::OAuth => 1,
            GroupId::ApiKey => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            GroupId::Logins => "Logins",
            GroupId::OAuth => "OAuth",
            GroupId::ApiKey => "API keys",
        }
    }
}

/// Collapse state per rail group: the login/oauth sections stay open (they're
/// short and show what's set up); the API-key catalog starts closed.
struct GroupStates([bool; 3]);

impl Default for GroupStates {
    fn default() -> Self {
        GroupStates([true, true, false])
    }
}

impl GroupStates {
    fn get(&self, g: GroupId) -> bool {
        self.0[g.index()]
    }

    fn set(&mut self, g: GroupId, open: bool) {
        self.0[g.index()] = open;
    }

    fn toggle(&mut self, g: GroupId) {
        let i = g.index();
        self.0[i] = !self.0[i];
    }
}

enum Place {
    Flat,
    Group(GroupId),
}

/// Where a provider lives on the rail: ready-to-use key/no-auth entries stay
/// flat up top; plugin logins and unused API-key catalog entries fold into
/// labeled groups so the sections don't mix.
fn place(p: &PaneProvider) -> Place {
    if p.install.is_some() {
        return Place::Group(GroupId::Logins);
    }
    match &p.item.auth {
        ConnectAuth::Plugin { .. } => {
            let oauth = p
                .installed
                .as_ref()
                .is_some_and(|i| i.auth_method.kind == "oauth");
            Place::Group(if oauth {
                GroupId::OAuth
            } else {
                GroupId::Logins
            })
        }
        ConnectAuth::ApiKey => {
            if p.item.id == "custom"
                || p.connected
                || p.detected
                || p.item.id.starts_with("custom:")
            {
                Place::Flat
            } else {
                Place::Group(GroupId::ApiKey)
            }
        }
        ConnectAuth::None => Place::Flat,
    }
}

/// Display rows for the rail: usable providers first, the "+ Custom endpoint"
/// toggle after them, then the collapsible groups. Closed groups collapse to
/// just their header.
fn rail_rows(rail: &[PaneProvider], groups: &GroupStates) -> Vec<RailRow> {
    let mut rows = Vec::new();
    let mut members: [Vec<usize>; 3] = Default::default();
    let mut custom_row = None;
    let mut portal_row = None;
    for (i, p) in rail.iter().enumerate() {
        if p.item.id == catalog::GRAY_PORTAL_ID {
            portal_row = Some(i);
        } else if p.item.id == "custom" {
            custom_row = Some(i);
        } else {
            match place(p) {
                Place::Flat => rows.push(RailRow::Provider(i)),
                Place::Group(g) => members[g.index()].push(i),
            }
        }
    }
    if let Some(i) = custom_row {
        rows.push(RailRow::Provider(i));
    }
    for g in GroupId::ALL {
        if members[g.index()].is_empty() {
            continue;
        }
        rows.push(RailRow::Group(g));
        if groups.get(g) {
            rows.extend(members[g.index()].iter().map(|&i| RailRow::Provider(i)));
        }
    }
    if let Some(i) = portal_row {
        rows.insert(0, RailRow::Spacer);
        rows.insert(0, RailRow::Provider(i));
    }
    rows
}

/// True when the typed filter names at least one provider: the rail then
/// narrows to those providers. Otherwise the filter applies to models only.
fn rail_filter_hits(rail: &[PaneProvider], filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    !f.is_empty() && rail.iter().any(|p| p.item.name.to_lowercase().contains(&f))
}

/// `rail_rows`, narrowed to providers whose name matches the filter (groups
/// forced open, empty groups dropped) when the filter names any provider.
fn rail_rows_for(rail: &[PaneProvider], groups: &GroupStates, filter: &str) -> Vec<RailRow> {
    if !rail_filter_hits(rail, filter) {
        return rail_rows(rail, groups);
    }
    let f = filter.trim().to_lowercase();
    let mut out = Vec::new();
    let mut header = None;
    for r in rail_rows(rail, &GroupStates([true; 3])) {
        match r {
            RailRow::Group(_) => header = Some(r),
            RailRow::Provider(i) if rail[i].item.name.to_lowercase().contains(&f) => {
                out.extend(header.take());
                out.push(r);
            }
            RailRow::Provider(_) | RailRow::Spacer => {}
        }
    }
    out
}

fn group_count(rail: &[PaneProvider], g: GroupId) -> usize {
    rail.iter()
        .filter(|p| matches!(place(p), Place::Group(id) if id == g))
        .count()
}

/// Index of the group header that owns `sel`, if `sel` sits inside an open
/// group.
fn enclosing_group(rows: &[RailRow], sel: usize) -> Option<usize> {
    let mut parent = None;
    for (i, r) in rows.iter().enumerate() {
        if i == sel {
            return parent;
        }
        if matches!(r, RailRow::Group(_)) {
            parent = Some(i);
        }
    }
    None
}

fn row_provider<'a>(
    rail: &'a [PaneProvider],
    rows: &[RailRow],
    sel: usize,
) -> Option<&'a PaneProvider> {
    match rows.get(sel) {
        Some(RailRow::Provider(i)) => rail.get(*i),
        _ => None,
    }
}

/// Move the rail cursor one row, skipping spacer rows.
fn rail_step(rows: &[RailRow], sel: usize, up: bool) -> usize {
    let mut s = sel;
    loop {
        s = if up {
            s.saturating_sub(1)
        } else {
            (s + 1).min(rows.len().saturating_sub(1))
        };
        if !matches!(rows.get(s), Some(RailRow::Spacer)) || s == 0 || s + 1 >= rows.len() {
            return s;
        }
    }
}

fn active_rail_index(rail: &[PaneProvider], config: &Config) -> usize {
    rail.iter()
        .position(|p| provider_is_active(p, config))
        .unwrap_or(0)
}

/// Row (not provider) index holding the currently-active provider.
fn active_row_index(rail: &[PaneProvider], rows: &[RailRow], config: &Config) -> usize {
    let target = active_rail_index(rail, config);
    rows.iter()
        .position(|r| *r == RailRow::Provider(target))
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

/// The saved model lists, parsed once per modal: every rail move onto a new
/// provider used to re-read provider_models.json (twice for an empty list).
struct SavedLists {
    home: std::path::PathBuf,
    lists: BTreeMap<String, Vec<super::variants::CachedRow>>,
}

impl SavedLists {
    fn load() -> Option<Self> {
        let home = catalog::gray_home().ok()?;
        let lists = super::context::provider_model_lists(&home);
        Some(Self { home, lists })
    }

    /// Same rows and order as [`saved_models_for`].
    fn models(
        &self,
        saved: &SavedConfig,
        list_key: &str,
        sort_base: &str,
    ) -> Vec<(String, String)> {
        let mut models = super::context::provider_model_list_in(&self.home, &self.lists, list_key);
        saved.sort_models(sort_base, &mut models);
        models
    }
}

fn models_for<'a>(
    p: &PaneProvider,
    cache: &'a mut HashMap<String, Vec<(String, String)>>,
    lists: Option<&SavedLists>,
    saved: &SavedConfig,
) -> &'a [(String, String)] {
    let load = |key: &str| match lists {
        Some(l) => l.models(saved, key, &p.item.base_url),
        None => saved_models_for(key, &p.item.base_url),
    };
    cache
        .entry(p.list_key.clone())
        .or_insert_with(|| {
            let mut models = load(&p.list_key);
            if models.is_empty() {
                let alt = normalize_custom_base_url(&p.item.base_url);
                if alt != p.list_key {
                    models = load(&alt);
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
    terminal: &mut super::ModalTerminal,
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
    rows: &'a [RailRow],
    groups: &'a GroupStates,
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
        row_provider(self.rail, self.rows, self.rail_sel)
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
    if provider.is_some_and(|p| p.install.is_some()) {
        head.push(Span::styled(" · not installed", fg(t.accent)));
    } else if !v.adoptable && provider.is_some() {
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

    for (r, kind) in v.rows.iter().enumerate().skip(v.rail_top).take(list_h) {
        let on = r == v.rail_sel;
        let lit = on && v.focus == Focus::Rail;
        let bar = match (lit, on) {
            (true, _) => fill(t.accent),
            (_, true) => fill(t.bezel_lo),
            _ => Style::default(),
        };
        let y = list_y + (r - v.rail_top) as u16;
        let RailRow::Provider(i) = *kind else {
            let RailRow::Group(g) = *kind else { continue };
            // A provider-name filter force-opens groups: show what's on screen.
            let shown = v.rows[r + 1..]
                .iter()
                .take_while(|x| matches!(x, RailRow::Provider(_)))
                .count();
            let icon = if v.groups.get(g) || shown > 0 {
                " ▾ "
            } else {
                " ▸ "
            };
            let text = if lit {
                bold(t.on_selection)
            } else if on {
                bold(t.text_bright)
            } else {
                bold(t.text_body)
            };
            let total = group_count(v.rail, g);
            let count = if shown > 0 && shown < total {
                format!(" {shown}/{total}")
            } else {
                format!(" {total}")
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(icon, fg(if lit { t.on_selection } else { t.accent })),
                    Span::styled(g.label(), text),
                    Span::styled(count, fg(if lit { t.on_selection } else { t.text_faint })),
                ]))
                .style(bar),
                row(inner.x, y, rail_w),
            );
            continue;
        };
        let p = &v.rail[i];
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
        let in_group = enclosing_group(v.rows, r).is_some();
        let mark = Span::styled(
            if p.item.id == "custom" {
                " + "
            } else if p.install.is_some() {
                " ○ "
            } else if p.connected {
                " ✓ "
            } else {
                "   "
            },
            fg(if lit {
                t.on_selection
            } else if p.item.id == "custom" {
                t.accent
            } else {
                t.success
            }),
        );
        let name = if p.item.id == "custom" {
            "Custom endpoint".to_string()
        } else {
            fit_chars(
                &p.item.name,
                rail_w.saturating_sub(if in_group { 6 } else { 4 }) as usize,
            )
        };
        let mut spans = vec![mark];
        if in_group {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(name, text));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(bar),
            row(inner.x, y, rail_w),
        );
    }

    if let Some(p) = provider {
        if v.models.is_empty() {
            let group_row = matches!(
                v.rows.get(v.rail_sel),
                Some(RailRow::Group(g)) if v.groups.get(*g)
            );
            let msg = if group_row {
                " ← to collapse"
            } else if matches!(v.rows.get(v.rail_sel), Some(RailRow::Group(_))) {
                " enter/→ to expand"
            } else {
                match (v.adoptable, v.filter.is_empty()) {
                    (false, _) if provider.is_some_and(|p| p.install.is_some()) => {
                        " enter to install"
                    }
                    (false, _) => " no models yet",
                    (true, true) => " fetching models…",
                    (true, false) => " no matches",
                }
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
        _ if provider.is_some_and(|p| p.install.is_some()) => {
            detail.push(dim("not installed — enter to install".into()))
        }
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
        (false, _) if provider.is_some_and(|p| p.install.is_some()) => "install",
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
    // ^r is the only way to hide/show thinking — keep it visible longest.
    for drop in ["⇧↑↓", "tab", "del", "^r", "←→"] {
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
    let (index_tx, index_rx) = std::sync::mpsc::channel::<Option<gray_pkg::index::Index>>();
    std::thread::spawn(move || {
        let _ = index_tx.send(fetch_provider_index());
    });
    let mut provider_index: Option<gray_pkg::index::Index> = None;
    let mut first_draw = true;
    let mut before = config.clone();
    let (mut rail, mut keys) = build_rail(config, ollama, None);
    let mut groups = GroupStates::default();
    let mut rows = rail_rows(&rail, &groups);
    let mut model_cache: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut efforts = PaneEfforts::default();
    let mut toggles = PickerToggles::from_config(config);
    let mut focus = Focus::Rail;
    let mut rail_sel = active_row_index(&rail, &rows, config);
    let mut rail_top = 0usize;
    let mut model_sel = 0usize;
    let mut model_top = 0usize;
    let mut filter = String::new();
    let mut filter_moved = false;
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
        let saved_lists = SavedLists::load();
        loop {
            if let Ok(up) = ollama_rx.try_recv()
                && up != ollama
            {
                ollama = up;
                let keep = row_provider(&rail, &rows, rail_sel).map(|p| p.item.id.clone());
                (rail, keys) = build_rail(config, ollama, provider_index.as_ref());
                rows = rail_rows(&rail, &groups);
                rail_sel = keep
                    .and_then(|id| rail.iter().position(|p| p.item.id == id))
                    .and_then(|i| rows.iter().position(|r| *r == RailRow::Provider(i)))
                    .unwrap_or(rail_sel)
                    .min(rows.len().saturating_sub(1));
            }
            if let Ok(index) = index_rx.try_recv() {
                provider_index = index;
                let keep = row_provider(&rail, &rows, rail_sel).map(|p| p.item.id.clone());
                (rail, keys) = build_rail(config, ollama, provider_index.as_ref());
                rows = rail_rows(&rail, &groups);
                rail_sel = keep
                    .and_then(|id| rail.iter().position(|p| p.item.id == id))
                    .and_then(|i| rows.iter().position(|r| *r == RailRow::Provider(i)))
                    .unwrap_or(rail_sel)
                    .min(rows.len().saturating_sub(1));
            }
            while let Ok((list_key, live)) = refresh_rx.try_recv() {
                if !live.is_empty()
                    && let Some(p) = rail.iter().find(|p| p.list_key == list_key)
                {
                    model_cache.insert(list_key.clone(), merge_models(&p.item.base_url, live));
                }
            }

            if std::mem::take(&mut do_remove)
                && let Some(p) = row_provider(&rail, &rows, rail_sel)
            {
                let id = p.item.id.clone();
                let name = super::connect::forget_provider(&p.item, config)?;
                before = config.clone();
                (rail, keys) = build_rail(config, ollama, provider_index.as_ref());
                rows = rail_rows(&rail, &groups);
                rail_sel = rail
                    .iter()
                    .position(|q| q.item.id == id)
                    .and_then(|i| rows.iter().position(|r| *r == RailRow::Provider(i)))
                    .unwrap_or(rail_sel)
                    .min(rows.len().saturating_sub(1));
                place_current = true;
                model_top = 0;
                notice = Some(format!("Removed {name}"));
                removed = Some(name);
            }
            let sel_provider = match rows.get(rail_sel) {
                Some(RailRow::Provider(i)) => Some(*i),
                _ => None,
            };
            let rail_filtered = rail_filter_hits(&rail, &filter);
            rows = rail_rows_for(&rail, &groups, &filter);
            rail_sel = if std::mem::take(&mut filter_moved) && rail_filtered {
                rows.iter()
                    .position(|r| matches!(r, RailRow::Provider(_)))
                    .unwrap_or(0)
            } else {
                sel_provider
                    .and_then(|i| rows.iter().position(|r| *r == RailRow::Provider(i)))
                    .unwrap_or(rail_sel)
            }
            .min(rows.len().saturating_sub(1));
            rail_top = rail_top.min(rail_sel);
            let provider = row_provider(&rail, &rows, rail_sel);
            let adoptable = provider.is_some_and(|p| p.adoptable(&keys));
            let all_models = provider
                .map(|p| models_for(p, &mut model_cache, saved_lists.as_ref(), &saved).to_vec())
                .unwrap_or_default();
            if let Some(p) = provider
                && adoptable
                && (all_models.is_empty() || model_refresh_due(&p.list_key))
            {
                kick(p, p.key(&keys, config), &mut fetching, &refresh_tx);
            }
            let models: Vec<(String, String)> = if filter.is_empty() || rail_filtered {
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

            // Keys already queued (a held arrow, fast typing) are applied
            // before the next paint, so a slow frame can't leave the
            // highlight trailing behind the input.
            let backlog = !first_draw && poll(std::time::Duration::ZERO)?;
            if !backlog {
                terminal.draw(|f| {
                    draw(
                        f,
                        &bg_snapshot,
                        &PaneView {
                            rail: &rail,
                            rows: &rows,
                            groups: &groups,
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
            }
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
            if let Some(up) = crate::keymap::select_move(&key) {
                if shift || focus == Focus::Rail {
                    rail_sel = rail_step(&rows, rail_sel, up);
                    place_current = true;
                    model_top = 0;
                } else if up {
                    model_sel = model_sel.saturating_sub(1);
                } else {
                    model_sel = (model_sel + 1).min(last_model);
                }
                continue;
            }
            match key.code {
                KeyCode::Esc if !filter.is_empty() => {
                    filter.clear();
                    filter_moved = true;
                }
                KeyCode::Esc if focus == Focus::Models => focus = Focus::Rail,
                KeyCode::Esc => return Ok(PanesOutcome::Dismissed),
                KeyCode::Char('c') if ctrl => return Ok(PanesOutcome::Dismissed),
                KeyCode::Char('r') if ctrl => toggles.toggle_reasoning(),
                KeyCode::Delete => confirm_remove = provider.is_some_and(|p| p.removable),
                KeyCode::Char('d') if ctrl => {
                    confirm_remove = provider.is_some_and(|p| p.removable);
                }
                KeyCode::Enter => match focus {
                    Focus::Rail if let Some(RailRow::Group(g)) = rows.get(rail_sel).copied() => {
                        groups.toggle(g);
                        rows = rail_rows(&rail, &groups);
                    }
                    _ if !adoptable => {
                        if let Some(p) = provider {
                            if let Some(name) = &p.install {
                                return Ok(PanesOutcome::Install(name.clone()));
                            }
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
                KeyCode::Up | KeyCode::Down if shift => {
                    rail_sel = rail_step(&rows, rail_sel, key.code == KeyCode::Up);
                    place_current = true;
                    model_top = 0;
                }
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
                KeyCode::Left if focus == Focus::Rail => match rows.get(rail_sel).copied() {
                    Some(RailRow::Group(g)) => {
                        groups.set(g, false);
                        rows = rail_rows(&rail, &groups);
                    }
                    Some(RailRow::Provider(_)) => {
                        if let Some(parent) = enclosing_group(&rows, rail_sel) {
                            rail_sel = parent;
                        }
                    }
                    _ => {}
                },
                KeyCode::Right
                    if focus == Focus::Rail
                        && let Some(RailRow::Group(g)) = rows.get(rail_sel).copied() =>
                {
                    groups.set(g, true);
                    rows = rail_rows(&rail, &groups);
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
                    filter_moved = true;
                    place_current = true;
                }
                KeyCode::Char(c) if !ctrl => {
                    filter.push(c);
                    filter_moved = true;
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
