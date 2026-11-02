use super::*;

/// Owned slash-menu row: local `SlashCmd`s and dynamic gray commands share
/// one shape so the popover can mix them.
#[derive(Clone)]
pub(super) struct SlashRow {
    pub(super) name: String,
    pub(super) hint: String,
    pub(super) group: String,
    pub(super) arg: bool,
}

pub(super) const FAV_TAB: &str = "\u{2605}favorites";

/// Catalog names often repeat their provider ("Gemini 3.8 Flash
/// (Antigravity)" under "Antigravity subscription") — drop that suffix.
pub(super) fn model_display_name(provider_label: &str, name: &str) -> String {
    let first = provider_label
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    let mut n = name.trim().to_string();
    if !first.is_empty()
        && let Some(open) = n.rfind(" (")
        && n.ends_with(')')
        && n[open + 2..n.len() - 1].to_lowercase() == first
    {
        n.truncate(open);
    }
    n
}

impl GrayApp {
    pub(super) fn open_palette(
        &mut self,
        mode: PaletteMode,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_open = true;
        self.model_open = false;
        self.settings_open = false;
        self.palette_mode = mode;
        self.palette_query = query.to_string();
        self.palette_sel = 0;
        self.palette_reveal.set(true);
        let q = query.to_string();
        self.palette_input.update(cx, |s, cx| {
            s.set_value(q, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Current (provider, model) resolved through the catalog: a thread on
    /// the legacy "gray" provider displays as the CLI's active connection.
    pub(super) fn current_model(&self) -> (String, String) {
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let (dp, dm, _) = self.defaults();
        let (p, m) = (
            thread.map(|t| t.provider.clone()).unwrap_or(dp),
            thread.map(|t| t.model.clone()).unwrap_or(dm),
        );
        if let Some(st) = self
            .active_thread
            .as_ref()
            .and_then(|t| self.engines.get(t))
            .and_then(|e| e.status.as_ref())
        {
            let sp = st.get("provider_id").and_then(|x| x.as_str()).unwrap_or("");
            let sm = st.get("model").and_then(|x| x.as_str()).unwrap_or("");
            if !sm.is_empty() {
                return (
                    if sp.is_empty() { p } else { sp.to_string() },
                    sm.to_string(),
                );
            }
        }
        self.resolve_model(&p, &m)
    }

    /// Provider list for the rail and palette: the gray CLI's catalog (or a
    /// single fallback "gray" entry), then the installed external CLIs with
    /// the models they reported. The fake
    /// provider only appears when the e2e harness enables it.
    pub(super) fn providers(&self) -> Vec<provider::ProviderEntry> {
        let mut out = match &self.model_catalog {
            Some(c) if !c.is_empty() => c.clone(),
            _ => vec![provider::fallback_gray_provider()],
        };
        if provider::fake_enabled() {
            out.push(provider::fake_provider());
        }
        out.extend(self.cli_providers.iter().cloned());
        out
    }

    pub(super) fn provider_label(&self, id: &str) -> String {
        self.providers()
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.label.clone())
            .unwrap_or_else(|| provider::static_label(id))
    }

    pub(super) fn default_model(&self, provider: &str) -> String {
        self.providers()
            .iter()
            .find(|p| p.id == provider)
            .and_then(|p| {
                p.models
                    .iter()
                    .find(|m| m.current)
                    .or_else(|| p.models.first())
            })
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "default".into())
    }

    /// Resolve the legacy "gray" provider to the CLI's active connection so
    /// display and checkmarks line up with the catalog.
    pub(super) fn resolve_model(&self, provider: &str, model: &str) -> (String, String) {
        if provider == "gray"
            && let Some(ap) = self
                .model_catalog
                .as_ref()
                .and_then(|c| c.iter().find(|p| p.active))
        {
            let m = if model.is_empty() || model == "default" {
                ap.models
                    .iter()
                    .find(|m| m.current)
                    .or_else(|| ap.models.first())
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| model.to_string())
            } else {
                model.to_string()
            };
            return (ap.id.clone(), m);
        }
        (provider.to_string(), model.to_string())
    }

    /// Kick a fresh `gray --dump-models` load when the catalog is not already
    /// loading — cheap enough to repeat on every picker open.
    pub(super) fn refresh_model_catalog(&mut self) {
        if self.model_catalog_loading
            || self
                .model_catalog_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        self.model_catalog_loading = true;
        let tx = self.model_catalog_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider::load_model_catalog());
        });
        // Re-ask the external CLIs too: they may have been installed,
        // logged in or updated since.
        let tx = self.cli_providers_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider::load_cli_providers());
        });
    }

    /// Models shown in the picker, filtered by the rail and the search box.
    /// Rows carry the CLI's group tags (composite/recent/recommended/other) so
    /// the list can draw the same dividers the /models modal does.
    /// Picker tab: one provider at a time, or the favorites list — never the
    /// whole catalog at once (that was ~1000 rows of noise). Search filters
    /// within the open tab, like MonoCode's picker.
    pub(super) fn model_items(&self) -> Vec<ModelEntry> {
        let q = self.model_query.trim().to_lowercase();
        let tab = self.model_tab();
        let favs = self.model_favorites();
        let mut out = Vec::new();
        let providers = self.providers();
        let push = |p: &provider::ProviderEntry, m: &ModelEntry, out: &mut Vec<ModelEntry>| {
            let label =
                model_display_name(&p.label, if m.name.is_empty() { &m.id } else { &m.name });
            let hay = format!("{} {} {}", p.label, m.id, label).to_lowercase();
            if q.is_empty() || hay.contains(&q) || fuzzy(&hay, &q) {
                let mut row = m.clone();
                row.name = label;
                row.provider = p.id.clone();
                out.push(row);
            }
        };
        if tab == FAV_TAB {
            for key in &favs {
                // Provider ids and model ids can both contain ':', so match
                // on the full "provider:" prefix rather than splitting.
                if let Some((p, m)) = providers.iter().find_map(|p| {
                    let mid = key.strip_prefix(p.id.as_str())?.strip_prefix(':')?;
                    p.models.iter().find(|m| m.id == mid).map(|m| (p, m))
                }) {
                    push(p, m, &mut out);
                }
            }
            return out;
        }
        if let Some(p) = providers.iter().find(|p| p.id == tab) {
            for m in &p.models {
                push(p, m, &mut out);
            }
        }
        out
    }

    /// Human name for a model id ("claude-opus-5-5" → "Claude Opus 5.5"),
    /// falling back to the id when the catalog doesn't know it.
    pub(super) fn model_name(&self, provider: &str, model: &str) -> String {
        self.providers()
            .iter()
            .find(|p| p.id == provider)
            .and_then(|p| {
                p.models.iter().find(|m| m.id == model).map(|m| {
                    model_display_name(&p.label, if m.name.is_empty() { &m.id } else { &m.name })
                })
            })
            .unwrap_or_else(|| model.to_string())
    }

    /// The open picker tab: the explicit choice, else the current model's
    /// provider (falling back to the first provider).
    pub(super) fn model_tab(&self) -> String {
        if let Some(r) = &self.model_rail {
            return r.clone();
        }
        let (cp, _) = self.current_model();
        let ps = self.providers();
        if ps.iter().any(|p| p.id == cp) {
            cp
        } else {
            ps.first().map(|p| p.id.clone()).unwrap_or_default()
        }
    }

    /// Starred models as "provider:model" keys, in the order they were added.
    pub(super) fn model_favorites(&self) -> Vec<String> {
        self.store
            .state
            .setting("model_favorites", "")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    pub(super) fn toggle_model_favorite(&mut self, key: String, cx: &mut Context<Self>) {
        let mut favs = self.model_favorites();
        if let Some(i) = favs.iter().position(|k| *k == key) {
            favs.remove(i);
        } else {
            favs.push(key);
        }
        self.persist(Event::SettingChanged {
            key: "model_favorites".into(),
            value: favs.join("\n"),
        });
        cx.notify();
    }

    pub(super) fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_open {
            self.close_overlays(window, cx);
            return;
        }
        self.model_closed_at = None;
        self.palette_open = false;
        self.settings_open = false;
        self.model_open = true;
        self.model_rail = None;
        self.model_query.clear();
        self.refresh_model_catalog();
        let (cp, cm) = self.current_model();
        self.model_sel = self
            .model_items()
            .iter()
            .position(|m| m.provider == cp && m.id == cm)
            .unwrap_or(0);
        self.model_reveal.set(true);
        self.model_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Effort a pick should apply for this row: the stepped value when the
    /// user changed it in the picker, else the thread effort clamped to the
    /// model's levels (default "medium", same as a new thread).
    pub(super) fn model_effort_for(&self, m: &ModelEntry) -> String {
        if m.efforts.is_empty() {
            return String::new();
        }
        let key = format!("{}:{}", m.provider, m.id);
        if let Some(e) = self.model_efforts.get(&key) {
            return e.clone();
        }
        let (cp, cm) = self.current_model();
        if m.provider == cp
            && m.id == cm
            && let Some(st) = self
                .active_thread
                .as_ref()
                .and_then(|t| self.engines.get(t))
                .and_then(|e| e.status.as_ref())
            && let Some(e) = st.get("effort").and_then(|x| x.as_str())
            && m.efforts.iter().any(|x| x == e)
        {
            return e.to_string();
        }
        let cur = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id))
            .map(|t| t.effort.clone())
            .unwrap_or_else(|| "medium".into());
        clamp_effort(&m.efforts, &cur)
    }

    pub(super) fn pick_model(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.model_items().get(ix).cloned() else {
            return;
        };
        self.run(Cmd::Model(format!("{}:{}", m.provider, m.id)), window, cx);
        let eff = self.model_effort_for(&m);
        if !eff.is_empty() {
            self.run(Cmd::Effort(eff), window, cx);
        }
        self.close_overlays(window, cx);
    }

    /// Token under the composer's caret that can drive a popover:
    /// `@path` anywhere after whitespace, `/cmd` only at a line start.
    /// Returns (kind, token start byte offset, query).
    pub(super) fn composer_token(&self, cx: &App) -> Option<(PopKind, usize, String)> {
        let st = self.composer.read(cx);
        let v = st.value().to_string();
        let cur = st.cursor().min(v.len());
        if !v.is_char_boundary(cur) {
            return None;
        }
        let before = &v[..cur];
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let tok = &before[start..];
        match tok.as_bytes().first() {
            Some(b'@') => Some((PopKind::Mention, start, tok[1..].to_string())),
            Some(b'/') if start == 0 || before.as_bytes()[start - 1] == b'\n' => {
                Some((PopKind::Slash, start, tok[1..].to_string()))
            }
            _ => None,
        }
    }

    pub(super) fn update_composer_pop(&mut self, cx: &mut Context<Self>) {
        let next = self
            .composer_token(cx)
            .map(|(k, _, _)| k)
            .filter(|k| *k != PopKind::Mention || self.work_root().is_some());
        self.composer_pop = next;
        self.pop_sel = 0;
        self.pop_reveal.set(true);
        cx.notify();
    }

    /// Project files ranked for the mention query (see `path_score`).
    pub(super) fn mention_items(&self, query: &str) -> Vec<String> {
        let Some(root) = self.work_root() else {
            return Vec::new();
        };
        rank_paths(workspace::all_files(&root, 5000), &query.to_lowercase(), 50)
    }

    /// `/`-menu rows: IDE-local commands (minus names gray owns) plus the
    /// engine's real command list — builtins from `list_commands`, or the
    /// static builtin names until its answer lands. Gray rows always type
    /// `/name ` so nothing destructive fires from a menu click.
    pub(super) fn slash_items(&self, query: &str) -> Vec<SlashRow> {
        let q = query.to_lowercase();
        let tid = self.active_thread.as_deref();
        let gray_thread = tid
            .and_then(|t| self.store.state.thread(t))
            .map(|t| provider::is_gray_provider(&t.provider))
            .unwrap_or(true);
        let engine_cmds: Vec<engine::GrayCmd> = if gray_thread {
            tid.and_then(|t| self.engines.get(t))
                .map(|e| e.commands.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut all: Vec<SlashRow> = Vec::new();
        for c in SLASH_COMMANDS {
            if gray_thread && self.is_gray_command(tid, c.name) {
                continue;
            }
            all.push(SlashRow {
                name: c.name.into(),
                hint: c.hint.into(),
                group: c.group.into(),
                arg: c.arg,
            });
        }
        if engine_cmds.is_empty() {
            for n in engine::GRAY_BUILTINS {
                all.push(SlashRow {
                    name: (*n).into(),
                    hint: "gray command".into(),
                    group: "Gray".into(),
                    arg: true,
                });
            }
        } else {
            for c in &engine_cmds {
                all.push(SlashRow {
                    name: c.name.clone(),
                    hint: if c.description.is_empty() {
                        "gray command".into()
                    } else {
                        c.description.clone()
                    },
                    group: if c.source.is_empty() || c.source == "builtin" {
                        "Gray".into()
                    } else {
                        format!("Plugin · {}", c.source)
                    },
                    arg: true,
                });
            }
        }
        let mut scored: Vec<(u8, SlashRow)> = all
            .into_iter()
            .filter_map(|c| {
                if c.name.starts_with(&q) {
                    Some((0, c))
                } else if c.name.contains(&q) || c.hint.to_lowercase().contains(&q) {
                    Some((1, c))
                } else if fuzzy(&c.name, &q) {
                    Some((2, c))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by_key(|(r, _)| *r);
        scored.truncate(50);
        scored.into_iter().map(|(_, c)| c).collect()
    }

    pub(super) fn pop_len(&self, cx: &App) -> usize {
        match self.composer_token(cx) {
            Some((PopKind::Mention, _, q)) => self.mention_items(&q).len(),
            Some((PopKind::Slash, _, q)) => self.slash_items(&q).len(),
            None => 0,
        }
    }

    pub(super) fn pop_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.pop_len(cx);
        if count > 0 {
            let n = count as isize;
            self.pop_sel = (self.pop_sel.min(count - 1) as isize + delta).rem_euclid(n) as usize;
            self.pop_reveal.set(true);
        }
        cx.notify();
        cx.stop_propagation();
    }

    /// Enter while a composer popover is open takes the highlighted row.
    /// Returns false when nothing is selectable so Enter still sends.
    pub(super) fn accept_pop(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.composer_pop.is_none() {
            return false;
        }
        let Some((kind, start, query)) = self.composer_token(cx) else {
            self.composer_pop = None;
            return false;
        };
        let cur = self.composer.read(cx).cursor();
        match kind {
            PopKind::Mention => {
                let items = self.mention_items(&query);
                let Some(path) = items.get(self.pop_sel.min(items.len().saturating_sub(1))) else {
                    return false;
                };
                let text = format!("@{path} ");
                self.composer.update(cx, |s, cx| {
                    s.set_selected_range(start..cur, cx);
                    s.replace(text, window, cx);
                });
            }
            PopKind::Slash => {
                let items = self.slash_items(&query);
                let Some(cmd) = items.get(self.pop_sel.min(items.len().saturating_sub(1))) else {
                    return false;
                };
                let cmd = cmd.clone();
                self.composer.update(cx, |s, cx| {
                    s.set_selected_range(start..cur, cx);
                    s.replace(
                        if cmd.arg {
                            format!("/{} ", cmd.name)
                        } else {
                            String::new()
                        },
                        window,
                        cx,
                    );
                });
                if !cmd.arg {
                    self.dispatch_slash(&cmd.name, window, cx);
                }
            }
        }
        self.composer_pop = None;
        cx.notify();
        true
    }

    /// Ctrl+Shift+H: kick a find-in-files worker when the query changed.
    /// rg first (respects .gitignore), plain grep as the fallback.
    pub(super) fn grep_spawn(&mut self) {
        if self.palette_mode != PaletteMode::Grep {
            return;
        }
        let q = self.palette_query.trim().to_string();
        if q == self.grep_q {
            return;
        }
        self.grep_q = q.clone();
        self.grep_rx = None;
        if q.chars().count() < 2 {
            self.grep_items.clear();
            return;
        }
        let Some(root) = self.work_root() else {
            self.grep_items.clear();
            return;
        };
        let (tx, rx) = channel();
        self.grep_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send((q.clone(), grep_run(&root, &q)));
        });
    }

    pub(super) fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        self.settings_open = false;
        self.model_open = false;
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub(super) fn palette_items(&self) -> Vec<(String, String, Cmd)> {
        let q = self.palette_query.trim().to_lowercase();
        let mut items: Vec<(String, String, Cmd)> = Vec::new();
        if self.palette_mode == PaletteMode::Files {
            if let Some(root) = self.work_root() {
                // The row is the path; a per-row "Open file" hint was noise.
                for f in rank_paths(workspace::all_files(&root, 5000), &q, 50) {
                    items.push((f.clone(), String::new(), Cmd::OpenFile(f)));
                }
            }
            return items;
        }
        if self.palette_mode == PaletteMode::Search {
            return self.search_items(&q);
        }
        if self.palette_mode == PaletteMode::Grep {
            return self.grep_items.clone();
        }
        if self.palette_mode == PaletteMode::GoToLine {
            let raw = self.palette_query.trim();
            let mut parts = raw.split(':');
            let line = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|l| *l > 0);
            let Some(line) = line else {
                return Vec::new();
            };
            let col = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|c| *c > 0)
                .unwrap_or(1);
            return vec![(
                format!("Go to line {line}"),
                format!("Column {col}"),
                Cmd::GoToLine(line, col),
            )];
        }
        let k = |s: &str| s.to_string();
        items.push((k("New thread"), k("Thread"), Cmd::NewThread));
        items.push((k("Search threads"), k("Threads"), Cmd::SearchThreads));
        items.push((k("Toggle sidebar"), k("View"), Cmd::ToggleSidebar));
        items.push((k("Toggle terminal"), k("View"), Cmd::ToggleTerminal));
        items.push((k("Toggle diff panel"), k("View"), Cmd::ToggleDiff));
        items.push((k("Toggle files panel"), k("View"), Cmd::ToggleFiles));
        items.push((k("Split pane right"), k("View"), Cmd::SplitPane));
        items.push((
            k("Split pane left"),
            k("View"),
            Cmd::SplitTo(panes::SplitDir::Left),
        ));
        items.push((
            k("Split pane up"),
            k("View"),
            Cmd::SplitTo(panes::SplitDir::Up),
        ));
        items.push((
            k("Split pane down"),
            k("View"),
            Cmd::SplitTo(panes::SplitDir::Down),
        ));
        if self.panes.len() > 1 {
            items.push((k("Close pane"), k("View"), Cmd::ClosePane));
            items.push((k("Toggle zoom"), k("View"), Cmd::ToggleZoom));
            for d in panes::SplitDir::ALL {
                let w = d.word();
                items.push((format!("Focus pane {w}"), k("View"), Cmd::FocusPaneDir(d)));
                items.push((format!("Swap pane {w}"), k("View"), Cmd::SwapPaneDir(d)));
            }
        }
        items.push((k("Open settings"), k("Settings"), Cmd::Settings));
        items.push((k("Toggle theme"), k("Theme"), Cmd::ToggleTheme));
        items.push((k("Create checkpoint"), k("Git"), Cmd::Checkpoint));
        items.push((k("Plugins"), k("View"), Cmd::TogglePlugins));
        items.push((k("Sessions"), k("Gray"), Cmd::ToggleSessions));
        items.push((k("Skills"), k("Gray"), Cmd::ToggleSkills));
        for sk in &self.skills {
            items.push((
                format!("Use skill: {}", sk.name),
                sk.source.clone(),
                Cmd::UseSkill(sk.name.clone()),
            ));
        }
        for gs in self.gray_sessions.iter().take(10) {
            items.push((
                format!("Resume session: {}", gs.id),
                k("Gray"),
                Cmd::ResumeSession(gs.id.clone()),
            ));
        }
        items.push((k("Code viewer"), k("View"), Cmd::ToggleViewer));
        for b in [
            ("none", "None"),
            ("andromeda", "Andromeda"),
            ("bluemarble", "Blue Marble"),
        ] {
            items.push((
                format!("Background: {}", b.1),
                k("Theme"),
                Cmd::SetBackground(b.0.into()),
            ));
        }
        if let Some(root) = self.work_root() {
            for f in workspace::all_files(&root, 5000) {
                let ext = f.rsplit('.').next().unwrap_or("").to_lowercase();
                if !matches!(
                    ext.as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
                ) {
                    continue;
                }
                items.push((
                    format!("Attach: {f}"),
                    k("Image"),
                    Cmd::AttachImage(f.clone()),
                ));
                if items.len() > 80 {
                    break;
                }
            }
        }
        for (n, d) in &self.gray_cmds {
            items.push((format!("gray: {n}"), d.clone(), Cmd::Gray(n.clone())));
        }
        for h in &self.hub {
            items.push((
                format!("Install plugin: {}", h.name),
                k("Plugin"),
                Cmd::PluginInstall(h.name.clone()),
            ));
        }
        items.push((k("Settle or unsettle thread"), k("Thread"), Cmd::Settle));
        items.push((k("Snooze thread for 1 hour"), k("Thread"), Cmd::Snooze(60)));
        items.push((k("Unsnooze thread"), k("Thread"), Cmd::Snooze(0)));
        for e in ["low", "medium", "high"] {
            items.push((format!("Effort: {e}"), k("Effort"), Cmd::Effort(e.into())));
        }
        items.push((k("Restore last checkpoint"), k("Git"), Cmd::Restore));
        items.push((k("Diff since last turn"), k("Git"), Cmd::TurnDiff));
        for sec in ["Preferences", "Appearance", "Shortcuts", "About"] {
            items.push((
                format!("Settings: {sec}"),
                k("Settings"),
                Cmd::SettingsSection(sec.to_lowercase()),
            ));
        }
        for (id, label) in [("focus", "Focus"), ("pro", "Pro")] {
            items.push((
                format!("Layout: {label}"),
                k("Layout"),
                Cmd::SetLayout(id.into()),
            ));
        }
        for (id, label) in [("bottom", "Bottom"), ("right", "Right")] {
            items.push((
                format!("Terminal: {label}"),
                k("Layout"),
                Cmd::SetTerminalPlacement(id.into()),
            ));
        }
        items.push((
            k("Notify when a turn finishes: On"),
            k("Settings"),
            Cmd::SetNotify(true),
        ));
        items.push((
            k("Notify when a turn finishes: Off"),
            k("Settings"),
            Cmd::SetNotify(false),
        ));
        items.push((
            k("Auto-accept permissions: On"),
            k("Settings"),
            Cmd::SetAutoAccept(true),
        ));
        items.push((
            k("Auto-accept permissions: Off"),
            k("Settings"),
            Cmd::SetAutoAccept(false),
        ));
        if self.pending_approval.is_some() {
            items.push((k("Approve pending action"), k("Action"), Cmd::Approve(true)));
            items.push((k("Reject pending action"), k("Action"), Cmd::Approve(false)));
        }
        items.push((k("Diff vs HEAD"), k("Git"), Cmd::HeadDiff));
        items.push((
            k(if self.show_archived {
                "Hide archived threads"
            } else {
                "Show archived threads"
            }),
            k("Threads"),
            Cmd::ShowArchived,
        ));
        if self.active_thread.is_some() {
            items.push((k("Pin or unpin thread"), k("Thread"), Cmd::Pin));
            items.push((k("Archive or unarchive thread"), k("Thread"), Cmd::Archive));
            items.push((k("Delete thread"), k("Thread"), Cmd::Delete));
            items.push((k("Stop turn"), k("Turn"), Cmd::Stop));
        }
        for p in self.providers() {
            items.push((
                format!("Provider: {}", p.label),
                k("Provider"),
                Cmd::Provider(p.id.clone()),
            ));
            for m in &p.models {
                let label = if m.name.is_empty() { &m.id } else { &m.name };
                items.push((
                    format!("Model: {} {}", p.label, label),
                    k("Model"),
                    Cmd::Model(format!("{}:{}", p.id, m.id)),
                ));
            }
        }
        let active_provider = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id))
            .map(|t| t.provider.clone());
        for (id, label) in MODES {
            if active_provider.as_deref() == Some("gray") && *id != "full" {
                continue;
            }
            items.push((
                format!("Mode: {label}"),
                k("Access"),
                Cmd::Mode((*id).into()),
            ));
        }
        for b in &self.branches {
            items.push((
                format!("Branch: {b}"),
                k("Checkout"),
                Cmd::Branch(b.clone()),
            ));
        }
        for p in &self.store.state.projects {
            items.push((
                format!("Project: {}", p.name),
                p.path.clone(),
                Cmd::Project(p.id.clone()),
            ));
        }
        if let Some(pid) = &self.active_project {
            for t in self.store.state.threads_for(pid, false) {
                items.push((
                    format!("Thread: {}", t.title),
                    k("Switch"),
                    Cmd::Thread(t.id.clone()),
                ));
            }
        }
        let exact: Vec<(String, String, Cmd)> = items
            .iter()
            .filter(|(l, _, _)| l.to_lowercase().contains(&q))
            .cloned()
            .collect();
        let mut out: Vec<(String, String, Cmd)> = if !exact.is_empty() {
            exact
        } else {
            items
                .into_iter()
                .filter(|(l, _, _)| fuzzy(&l.to_lowercase(), &q))
                .collect()
        };
        let raw = self.palette_query.trim();
        let has_exact =
            !out.is_empty() && out.iter().any(|(l, _, _)| l.to_lowercase().contains(&q));
        if !raw.is_empty() && !has_exact {
            if self.active_thread.is_some() {
                out.push((
                    format!("Rename thread: {raw}"),
                    k("Thread"),
                    Cmd::Rename(raw.into()),
                ));
            }
            out.push((
                format!("Create branch: {raw}"),
                k("Git"),
                Cmd::NewBranch(raw.into()),
            ));
            out.push((
                format!("Create worktree: {raw}"),
                k("Git"),
                Cmd::Worktree(raw.into()),
            ));
        }
        // Stable: keeps match order inside a group, one header per group.
        out.sort_by_key(|(_, _, c)| cmd_group(c).0);
        if !raw.is_empty() && Path::new(raw).is_dir() {
            out.insert(
                0,
                (
                    format!("Add project: {raw}"),
                    k("Project"),
                    Cmd::AddProject(raw.into()),
                ),
            );
        }
        out
    }

    /// Cross-thread search (ctrl-shift-f): thread titles first, then
    /// message text with a snippet around the hit. Empty query lists recent
    /// threads across every project.
    pub(super) fn search_items(&self, q: &str) -> Vec<(String, String, Cmd)> {
        let mut threads: Vec<&crate::store::Thread> = self.store.state.threads.iter().collect();
        threads.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        let project = |pid: &str| {
            self.store
                .state
                .project(pid)
                .map(|p| p.name.clone())
                .unwrap_or_default()
        };
        let mut titles = Vec::new();
        let mut messages = Vec::new();
        for t in &threads {
            let when = relative(t.updated_at);
            let archived = if t.archived { " · archived" } else { "" };
            if q.is_empty() || fuzzy(&t.title.to_lowercase(), q) {
                titles.push((
                    t.title.clone(),
                    format!("{} · {when}{archived}", project(&t.project_id)),
                    Cmd::OpenMessage(t.id.clone(), None),
                ));
            }
            if q.chars().count() < 2 || messages.len() >= 60 {
                continue;
            }
            for (ix, item) in t.items.iter().enumerate() {
                let Item::Message { text, .. } = item else {
                    continue;
                };
                if let Some(snip) = snippet(text, q, 80) {
                    messages.push((
                        snip,
                        format!("{} · {when}", t.title),
                        Cmd::OpenMessage(t.id.clone(), Some(ix)),
                    ));
                    if messages.len() >= 60 {
                        break;
                    }
                }
            }
        }
        titles.truncate(if q.is_empty() { 40 } else { 20 });
        titles.extend(messages);
        titles
    }
}

/// One find-in-files pass: `path:line:col:text` rows capped at 60, with the
/// match text as the label and `path:line` as the hint.
fn grep_run(root: &Path, q: &str) -> Vec<(String, String, Cmd)> {
    // `rg` on PATH isn't always ripgrep: keep its rows only when some
    // actually parse, otherwise fall through to plain grep.
    if let Ok(out) = std::process::Command::new("rg")
        .args([
            "--vimgrep",
            "--no-heading",
            "--color",
            "never",
            "-i",
            "-m",
            "3",
            "--",
            q,
            ".",
        ])
        .current_dir(root)
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let items: Vec<_> = text
            .lines()
            .take(60)
            .filter_map(|line| {
                let mut it = line.splitn(4, ':');
                let (p, l, c, t) = (it.next()?, it.next()?, it.next()?, it.next()?);
                let p = p.strip_prefix("./").unwrap_or(p).to_string();
                let (l, c) = (l.parse::<usize>().ok()?, c.parse::<usize>().ok()?);
                Some((
                    t.trim().chars().take(90).collect::<String>(),
                    format!("{p}:{l}"),
                    Cmd::OpenFileAt(p, l, c),
                ))
            })
            .collect();
        if !items.is_empty() {
            return items;
        }
    }
    if let Ok(out) = std::process::Command::new("grep")
        .args([
            "-RInIi",
            "--exclude-dir=.git",
            "--exclude-dir=target",
            "--exclude-dir=node_modules",
            "-m3",
            "-e",
            q,
            ".",
        ])
        .current_dir(root)
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        return text
            .lines()
            .take(60)
            .filter_map(|line| {
                let mut it = line.splitn(3, ':');
                let (p, l, t) = (it.next()?, it.next()?, it.next()?);
                let p = p.strip_prefix("./").unwrap_or(p).to_string();
                let l = l.parse::<usize>().ok()?;
                Some((
                    t.trim().chars().take(90).collect::<String>(),
                    format!("{p}:{l}"),
                    Cmd::OpenFileAt(p, l, 1),
                ))
            })
            .collect();
    }
    Vec::new()
}

/// Pick `cur` when the model supports it, else the nearest level downward
/// (then upward) — the same clamp the CLI applies to reasoning effort.
fn clamp_effort(levels: &[String], cur: &str) -> String {
    const ORDER: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if levels.iter().any(|l| l == cur) {
        return cur.to_string();
    }
    let want_idx = ORDER.iter().position(|l| *l == cur).unwrap_or(4);
    for cand in ORDER.iter().skip(want_idx) {
        if let Some(l) = levels.iter().find(|l| l.as_str() == *cand) {
            return l.clone();
        }
    }
    for cand in ORDER.iter().take(want_idx).rev() {
        if let Some(l) = levels.iter().find(|l| l.as_str() == *cand) {
            return l.clone();
        }
    }
    levels.first().cloned().unwrap_or_default()
}
