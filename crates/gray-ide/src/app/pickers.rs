use super::*;

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

    pub(super) fn current_model(&self) -> (String, String) {
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let (dp, dm, _) = self.defaults();
        (
            thread.map(|t| t.provider.clone()).unwrap_or(dp),
            thread.map(|t| t.model.clone()).unwrap_or(dm),
        )
    }

    /// Models shown in the picker, filtered by the rail and the search box.
    pub(super) fn model_items(&self) -> Vec<(&'static str, &'static str)> {
        let q = self.model_query.trim().to_lowercase();
        let mut out = Vec::new();
        for p in PROVIDERS {
            if self.model_rail.is_some_and(|r| r != p.id) {
                continue;
            }
            for m in p.models {
                let hay = format!("{} {}", p.label, m).to_lowercase();
                if q.is_empty() || hay.contains(&q) || fuzzy(&hay, &q) {
                    out.push((p.id, *m));
                }
            }
        }
        out
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
        let (cp, cm) = self.current_model();
        self.model_sel = self
            .model_items()
            .iter()
            .position(|(p, m)| *p == cp && *m == cm)
            .unwrap_or(0);
        self.model_reveal.set(true);
        self.model_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn pick_model(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((p, m)) = self.model_items().get(ix).copied() else {
            return;
        };
        self.run(Cmd::Model(format!("{p}:{m}")), window, cx);
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

    /// Project files ranked for the mention query: substring hits first,
    /// then fuzzy matches, each in walk order.
    pub(super) fn mention_items(&self, query: &str) -> Vec<String> {
        let Some(root) = self.work_root() else {
            return Vec::new();
        };
        let q = query.to_lowercase();
        let mut scored: Vec<(u8, String)> = workspace::all_files(&root, 5000)
            .into_iter()
            .filter_map(|f| {
                if f.to_lowercase().contains(&q) {
                    Some((0, f))
                } else if fuzzy(&f.to_lowercase(), &q) {
                    Some((1, f))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by_key(|(r, _)| *r);
        scored.truncate(50);
        scored.into_iter().map(|(_, f)| f).collect()
    }

    pub(super) fn slash_items(&self, query: &str) -> Vec<SlashCmd> {
        let q = query.to_lowercase();
        let mut scored: Vec<(u8, SlashCmd)> = SLASH_COMMANDS
            .iter()
            .filter_map(|c| {
                if c.name.starts_with(&q) {
                    Some((0, *c))
                } else if c.name.contains(&q) || c.hint.to_lowercase().contains(&q) {
                    Some((1, *c))
                } else if fuzzy(c.name, &q) {
                    Some((2, *c))
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
                let cmd = *cmd;
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
                    self.slash(cmd.name, window, cx);
                }
            }
        }
        self.composer_pop = None;
        cx.notify();
        true
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
                for f in workspace::all_files(&root, 5000) {
                    if q.is_empty() || fuzzy(&f.to_lowercase(), &q) {
                        items.push((f.clone(), "Open file".into(), Cmd::OpenFile(f)));
                    }
                    if items.len() >= 50 {
                        break;
                    }
                }
            }
            return items;
        }
        if self.palette_mode == PaletteMode::Search {
            return self.search_items(&q);
        }
        let k = |s: &str| s.to_string();
        items.push((k("New thread"), k("Thread"), Cmd::NewThread));
        items.push((k("Search threads"), k("Threads"), Cmd::SearchThreads));
        items.push((k("Toggle sidebar"), k("View"), Cmd::ToggleSidebar));
        items.push((k("Toggle terminal"), k("View"), Cmd::ToggleTerminal));
        items.push((k("Toggle diff panel"), k("View"), Cmd::ToggleDiff));
        items.push((k("Toggle files panel"), k("View"), Cmd::ToggleFiles));
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
        for p in PROVIDERS {
            items.push((
                format!("Provider: {}", p.label),
                k("Provider"),
                Cmd::Provider(p.id.into()),
            ));
            for m in p.models {
                items.push((
                    format!("Model: {} {}", p.label, m),
                    k("Model"),
                    Cmd::Model(format!("{}:{}", p.id, m)),
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
