use super::*;

impl GrayApp {
    pub(super) fn persist(&mut self, e: Event) {
        if let Err(err) = self.store.append(e) {
            self.toast(format!("Couldn't save: {err}"), true);
        }
    }

    pub(super) fn dirty_tabs(&self, cx: &App) -> usize {
        self.tabs
            .iter()
            .filter(|t| t.editor.read(cx).value() != t.saved)
            .count()
    }

    pub(super) fn configure(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        mode: Option<String>,
    ) {
        let Some(tid) = self.active_thread.clone() else {
            if let Some(p) = &provider {
                self.persist(Event::SettingChanged {
                    key: "default_provider".into(),
                    value: p.clone(),
                });
            }
            if let (Some(p), Some(m)) = (&provider, &model) {
                self.persist(Event::SettingChanged {
                    key: format!("default_model.{p}"),
                    value: m.clone(),
                });
            }
            if let Some(m) = &mode {
                self.persist(Event::SettingChanged {
                    key: "default_mode".into(),
                    value: m.clone(),
                });
            }
            return;
        };
        let Some(t) = self.store.state.thread(&tid).cloned() else {
            return;
        };
        let provider_id = provider.clone().unwrap_or(t.provider.clone());
        let mode = mode.or(Some(t.mode.clone())).map(|m| {
            if provider_id == "gray" {
                "full".to_string()
            } else {
                m
            }
        });
        let model = model.unwrap_or_else(|| {
            if provider.is_some() && provider_id != t.provider {
                provider::default_model(&provider_id).to_string()
            } else {
                t.model.clone()
            }
        });
        self.persist(Event::ThreadConfigured {
            id: tid,
            provider: provider_id,
            model,
            mode: mode.unwrap_or_default(),
        });
    }

    pub(super) fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        let opening_terminal = matches!(cmd, Cmd::ToggleTerminal) && !self.terminal_open;
        let opening_file = matches!(cmd, Cmd::OpenFile(_));
        match cmd {
            Cmd::NewThread => {
                self.create_thread(cx);
            }
            Cmd::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Cmd::ToggleTerminal => self.terminal_open = !self.terminal_open,
            Cmd::ToggleDiff => {
                self.panel = if self.panel == Some(Panel::Diff) {
                    None
                } else {
                    Some(Panel::Diff)
                };
                self.refresh_workspace(cx);
            }
            Cmd::ToggleFiles => {
                self.panel = if self.panel == Some(Panel::Files) {
                    None
                } else {
                    Some(Panel::Files)
                };
                self.refresh_workspace(cx);
            }
            Cmd::Settings => {
                self.settings_open = true;
                self.composer.update(cx, |s, cx| s.focus(window, cx));
            }
            Cmd::ToggleTheme => {
                let dark = cx.theme().mode.is_dark();
                let next = if dark {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                self.persist(Event::SettingChanged {
                    key: "theme".into(),
                    value: next.name().into(),
                });
                Theme::change(next, Some(window), cx);
                apply_brand(cx);
            }
            Cmd::Provider(p) => {
                if PROVIDERS.iter().any(|x| x.id == p) {
                    self.configure(Some(p), None, None);
                } else {
                    self.toast(format!("Unknown provider {p}"), true);
                }
            }
            Cmd::Model(spec) => {
                let (p, m) = match spec.split_once(':') {
                    Some((p, m)) => (Some(p.to_string()), m.to_string()),
                    None => (None, spec.clone()),
                };
                if !m.is_empty() {
                    self.configure(p, Some(m), None);
                }
            }
            Cmd::Mode(m) => {
                if MODES.iter().any(|x| x.0 == m) {
                    self.configure(None, None, Some(m));
                }
            }
            Cmd::TurnDiff => {
                let tree = self
                    .active_thread
                    .as_ref()
                    .and_then(|t| self.store.state.thread(t))
                    .and_then(|t| {
                        t.items.iter().rev().find_map(|i| match i {
                            Item::Activity { kind, detail, .. } if kind == "checkpoint" => {
                                Some(detail.clone())
                            }
                            _ => None,
                        })
                    });
                match tree {
                    Some(tree) => {
                        self.diff_base = Some(tree);
                        self.panel = Some(Panel::Diff);
                        self.refresh_workspace(cx);
                    }
                    None => {
                        self.toast("No checkpoint in this thread", true);
                        cx.notify();
                        return;
                    }
                }
            }
            Cmd::HeadDiff => {
                self.diff_base = None;
                self.panel = Some(Panel::Diff);
                self.refresh_workspace(cx);
            }
            Cmd::SettingsSection(section) => {
                self.settings_open = true;
                self.settings_section = section;
                self.composer.update(cx, |s, cx| s.focus(window, cx));
            }
            Cmd::SetLayout(v) => {
                self.persist(Event::SettingChanged {
                    key: "layout".into(),
                    value: v,
                });
            }
            Cmd::SetTerminalPlacement(v) => {
                self.persist(Event::SettingChanged {
                    key: "terminal_placement".into(),
                    value: v,
                });
            }
            Cmd::SetNotify(b) => {
                self.persist(Event::SettingChanged {
                    key: "notify".into(),
                    value: if b { "true" } else { "false" }.into(),
                });
            }
            Cmd::SearchThreads => {
                self.open_palette(PaletteMode::Search, "", window, cx);
                return;
            }
            Cmd::OpenMessage(tid, ix) => {
                if self.active_thread.as_deref() != Some(tid.as_str()) && self.dirty_tabs(cx) > 0 {
                    self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
                    return;
                }
                self.focus_thread(&tid, cx);
                self.scroll_target = ix.map(|ix| (tid, ix, 0));
            }
            Cmd::SetAutoAccept(b) => {
                self.persist(Event::SettingChanged {
                    key: "auto_accept".into(),
                    value: if b { "true" } else { "false" }.into(),
                });
            }
            Cmd::AttachImage(arg) => {
                let rel = PathBuf::from(arg.trim());
                let path = if rel.is_absolute() {
                    rel
                } else {
                    match self.work_root() {
                        Some(r) => r.join(rel),
                        None => rel,
                    }
                };
                if path.is_file() {
                    if !self.attachments.contains(&path) {
                        self.attachments.push(path);
                    }
                } else {
                    self.toast(format!("No image at {}", path.display()), true);
                }
            }
            Cmd::RemoveAttachment(i) => {
                if i < self.attachments.len() {
                    self.attachments.remove(i);
                }
            }
            Cmd::Approve(ok) => {
                if let Some((tid, label)) = self.pending_approval.take() {
                    if let Some(t) = self.turns.get(&tid)
                        && let Ok(mut p) = t.turn.pending.lock()
                        && let Some(tx) = p.take()
                    {
                        let _ = tx.send(ok);
                    }
                    self.persist(Event::ActivityAdded {
                        thread_id: tid,
                        id: new_id(),
                        kind: "approval".into(),
                        label: if ok { "Approved" } else { "Rejected" }.into(),
                        detail: label,
                        at: now(),
                    });
                }
            }
            Cmd::Gray(name) => {
                self.terminal_open = true;
                self.run_terminal(format!("{} cmd run {name}", graycmds::bin()), cx);
            }
            Cmd::DiscardChanges => {
                let before = self.tabs.len();
                self.tabs.retain(|t| t.editor.read(cx).value() == t.saved);
                let dropped = before - self.tabs.len();
                self.active_tab = self.active_tab.filter(|i| *i < self.tabs.len());
                self.toast(
                    if dropped > 0 {
                        format!("Discarded edits in {dropped} file(s)")
                    } else {
                        "Nothing to discard".into()
                    },
                    dropped == 0,
                );
            }
            Cmd::Branch(b) | Cmd::NewBranch(b) if b.is_empty() => {
                self.toast("Branch name required", true)
            }
            Cmd::Branch(b) => {
                if let Some(root) = self.work_root() {
                    match git::checkout(&root, &b, false) {
                        Ok(()) => self.toast(format!("Switched to {b}"), false),
                        Err(e) => self.toast(e, true),
                    }
                    self.refresh_workspace(cx);
                }
            }
            Cmd::NewBranch(b) => {
                if let Some(root) = self.work_root() {
                    match git::checkout(&root, &b, true) {
                        Ok(()) => self.toast(format!("Created {b}"), false),
                        Err(e) => self.toast(e, true),
                    }
                    self.refresh_workspace(cx);
                }
            }
            Cmd::Pin => {
                if let Some(t) = self
                    .active_thread
                    .as_ref()
                    .and_then(|t| self.store.state.thread(t))
                    .cloned()
                {
                    self.persist(Event::ThreadPinned {
                        id: t.id,
                        pinned: !t.pinned,
                    });
                }
            }
            Cmd::Archive => {
                if let Some(t) = self
                    .active_thread
                    .as_ref()
                    .and_then(|t| self.store.state.thread(t))
                    .cloned()
                {
                    self.persist(Event::ThreadArchived {
                        id: t.id,
                        archived: !t.archived,
                    });
                    if !t.archived {
                        self.active_thread = None;
                    }
                }
            }
            Cmd::Delete => {
                if let Some(id) = self.active_thread.take() {
                    if let Some(t) = self.turns.remove(&id) {
                        t.turn.cancel();
                    }
                    self.persist(Event::ThreadDeleted { id });
                }
            }
            Cmd::Rename(title) => {
                if let Some(id) = self.active_thread.clone()
                    && !title.is_empty()
                {
                    self.persist(Event::ThreadRenamed { id, title });
                }
            }
            Cmd::OpenFile(path) => self.open_file(&path, window, cx),
            Cmd::Checkpoint => {
                if let (Some(root), Some(tid)) = (self.work_root(), self.active_thread.clone())
                    && let Some(tree) = git::checkpoint(&root)
                {
                    self.persist(Event::ActivityAdded {
                        thread_id: tid,
                        id: new_id(),
                        kind: "checkpoint".into(),
                        label: "Checkpoint".into(),
                        detail: tree,
                        at: now(),
                    });
                    self.toast("Checkpoint created", false);
                }
            }
            Cmd::Restore => {
                if self.dirty_tabs(cx) > 0 {
                    self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
                    return;
                }
                let tree = self
                    .active_thread
                    .as_ref()
                    .and_then(|t| self.store.state.thread(t))
                    .and_then(|t| {
                        t.items.iter().rev().find_map(|i| match i {
                            Item::Activity { kind, detail, .. } if kind == "checkpoint" => {
                                Some(detail.clone())
                            }
                            _ => None,
                        })
                    });
                match (self.work_root(), tree) {
                    (Some(root), Some(tree)) => {
                        match git::restore(&root, &tree) {
                            Ok(()) => self.toast("Restored checkpoint", false),
                            Err(e) => self.toast(e, true),
                        }
                        self.refresh_workspace(cx);
                        self.reload_tabs(window, cx);
                    }
                    _ => self.toast("No checkpoint in this thread", true),
                }
            }
            Cmd::Stop => {
                if let Some(tid) = &self.active_thread
                    && let Some(t) = self.turns.get(tid)
                {
                    if let Ok(mut p) = t.turn.pending.lock()
                        && let Some(tx) = p.take()
                    {
                        let _ = tx.send(false);
                    }
                    self.pending_approval = None;
                    t.turn.cancel();
                }
            }
            Cmd::AddProject(p) => {
                if self.dirty_tabs(cx) > 0 {
                    self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
                    return;
                }
                self.add_project(Path::new(&p));
                self.tabs.clear();
                self.active_tab = None;
                self.refresh_workspace(cx);
            }
            Cmd::Project(id) => {
                if self.dirty_tabs(cx) > 0 {
                    self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
                    return;
                }
                self.active_project = Some(id.clone());
                self.active_thread = self
                    .store
                    .state
                    .threads_for(&id, false)
                    .first()
                    .map(|t| t.id.clone());
                self.tabs.clear();
                self.active_tab = None;
                self.refresh_workspace(cx);
            }
            Cmd::Thread(id) => {
                if self.dirty_tabs(cx) > 0 {
                    self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
                    return;
                }
                self.active_thread = Some(id);
                self.tabs.clear();
                self.active_tab = None;
                self.refresh_workspace(cx);
            }
            Cmd::ShowArchived => self.show_archived = !self.show_archived,
            Cmd::TogglePlugins => {
                self.panel = if self.panel == Some(Panel::Plugins) {
                    None
                } else {
                    Some(Panel::Plugins)
                };
                if self.panel == Some(Panel::Plugins) {
                    plugins::refresh(self.plugin_tx.clone());
                }
            }
            Cmd::PluginInstall(spec) => {
                if spec.trim().is_empty() {
                    self.toast("Plugin name or path required", true);
                } else {
                    self.plugin_busy = Some(spec.clone());
                    plugins::run(
                        self.plugin_tx.clone(),
                        format!("Install {spec}"),
                        vec!["install".into(), "-y".into(), spec],
                    );
                }
            }
            Cmd::PluginRemove(name) => {
                self.plugin_busy = Some(name.clone());
                plugins::run(
                    self.plugin_tx.clone(),
                    format!("Remove {name}"),
                    vec!["remove".into(), name],
                );
            }
            Cmd::PluginEnable(name, on) => {
                self.plugin_busy = Some(name.clone());
                let verb = if on { "enable" } else { "disable" };
                plugins::run(
                    self.plugin_tx.clone(),
                    format!("{verb} {name}"),
                    vec![verb.into(), name],
                );
            }
            Cmd::Settle => {
                if let Some(t) = self
                    .active_thread
                    .as_ref()
                    .and_then(|t| self.store.state.thread(t))
                    .cloned()
                {
                    self.persist(Event::ThreadSettled {
                        id: t.id,
                        settled: !t.settled,
                    });
                }
            }
            Cmd::Snooze(minutes) => {
                if let Some(id) = self.active_thread.clone() {
                    let until = if minutes <= 0 {
                        0
                    } else {
                        now() + minutes * 60_000
                    };
                    self.persist(Event::ThreadSnoozed { id, until });
                }
            }
            Cmd::ToggleViewer => {
                self.panel = if self.panel == Some(Panel::Viewer) {
                    None
                } else {
                    Some(Panel::Viewer)
                };
            }
            Cmd::PreviewFile(rel) => {
                if rel.is_empty() {
                    self.toast("Usage: /view <file>", true);
                } else {
                    self.preview_file(&rel, window, cx);
                }
            }
            Cmd::ToggleSkills => {
                self.panel = if self.panel == Some(Panel::Skills) {
                    None
                } else {
                    Some(Panel::Skills)
                };
                if self.panel == Some(Panel::Skills) {
                    let agent_dir = crate::data_dir()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(crate::data_dir);
                    self.skills = crate::skills::discover(self.work_root().as_deref(), &agent_dir);
                }
            }
            Cmd::UseSkill(name) => {
                if let Some(sk) = self.skills.iter().find(|sk| sk.name == *name) {
                    let text = if sk.body.is_empty() {
                        format!("Use the {} skill.", sk.name)
                    } else {
                        sk.body.clone()
                    };
                    self.composer
                        .update(cx, |s, cx| s.set_value(text, window, cx));
                }
            }
            Cmd::ToggleSessions => {
                self.panel = if self.panel == Some(Panel::Sessions) {
                    None
                } else {
                    Some(Panel::Sessions)
                };
                self.gray_sessions = sessions::list();
            }
            Cmd::ResumeSession(id) => {
                let Some(tid) = self.create_thread(cx) else {
                    return;
                };
                let model = self
                    .store
                    .state
                    .setting("default_model.gray", provider::default_model("gray"));
                self.persist(Event::ThreadConfigured {
                    id: tid.clone(),
                    provider: "gray".into(),
                    model,
                    mode: "full".into(),
                });
                self.persist(Event::ThreadSession {
                    id: tid.clone(),
                    provider: "gray".into(),
                    session: id.clone(),
                });
                self.persist(Event::ThreadRenamed {
                    id: tid,
                    title: format!("gray/{id}"),
                });
                self.toast(format!("Resuming {id}"), false);
            }
            Cmd::SetBackground(b) => {
                self.persist(Event::SettingChanged {
                    key: "background".into(),
                    value: b.clone(),
                });
                self.toast(
                    if b == "none" {
                        "Background cleared".into()
                    } else {
                        format!("Background: {b}")
                    },
                    false,
                );
            }
            Cmd::Effort(e) => {
                if ["low", "medium", "high"].contains(&e.as_str()) {
                    if let Some(id) = self.active_thread.clone() {
                        self.persist(Event::ThreadEffort { id, effort: e });
                    }
                } else {
                    self.toast("Effort must be low, medium or high", true);
                }
            }
            Cmd::Worktree(name) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    self.toast("Worktree name required", true);
                } else if let Some(root) = self.project_path() {
                    let tid = match self.active_thread.clone() {
                        Some(t) => Some(t),
                        None => self.create_thread(cx),
                    };
                    let base = root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let dir = root
                        .parent()
                        .unwrap_or(&root)
                        .join(format!("{base}-worktrees"))
                        .join(&name);
                    match git::worktree_add(&root, &dir, &name) {
                        Ok(()) if self.dirty_tabs(cx) > 0 => {
                            self.toast(
                                format!("Worktree {name} created; save or /discard to switch"),
                                false,
                            );
                        }
                        Ok(()) => {
                            if let Some(id) = tid {
                                self.persist(Event::ThreadWorktree {
                                    id,
                                    path: dir.display().to_string(),
                                });
                            }
                            self.toast(format!("Worktree {name} ready"), false);
                            self.tabs.clear();
                            self.active_tab = None;
                            self.refresh_workspace(cx);
                        }
                        Err(e) => self.toast(e, true),
                    }
                }
            }
        }
        if opening_terminal {
            self.terminal_input.update(cx, |s, cx| s.focus(window, cx));
        } else if !self.settings_open && !opening_file {
            self.composer.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn open_file(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.path == rel) {
            self.active_tab = Some(i);
            self.panel = Some(Panel::Files);
            self.tabs[i].editor.update(cx, |s, cx| s.focus(window, cx));
            return;
        }
        let Some(root) = self.work_root() else {
            return;
        };
        let content = match std::fs::read_to_string(root.join(rel)) {
            Ok(c) => c,
            Err(e) => {
                self.toast(format!("Can't open {rel}: {e}"), true);
                cx.notify();
                return;
            }
        };
        let lang = workspace::language_for(rel);
        let value = content.clone();
        let editor = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .language(lang)
                .line_number(true);
            s.set_value(value, window, cx);
            s
        });
        editor.update(cx, |s, cx| s.focus(window, cx));
        self.tabs.push(Tab {
            path: rel.to_string(),
            editor,
            saved: content,
        });
        self.active_tab = Some(self.tabs.len() - 1);
        self.panel = Some(Panel::Files);
    }

    pub(super) fn preview_file(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            return;
        };
        let path = root.join(rel);
        if !path.is_file() {
            self.toast(format!("No file: {rel}"), true);
            cx.notify();
            return;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.toast(format!("Can't preview {rel}: {e}"), true);
                cx.notify();
                return;
            }
        };
        let lang = workspace::language_for(rel);
        match &self.viewer {
            Some((p, st)) if p == rel => {
                st.update(cx, |s, cx| s.set_value(content, window, cx));
            }
            _ => {
                let st = cx.new(|cx| {
                    let mut s = EditorState::new(window, cx)
                        .language(lang)
                        .line_number(true);
                    s.set_value(content, window, cx);
                    s
                });
                self.viewer = Some((rel.to_string(), st));
            }
        }
        self.panel = Some(Panel::Viewer);
        cx.notify();
    }

    pub(super) fn reload_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            return;
        };
        for tab in &mut self.tabs {
            if let Ok(content) = std::fs::read_to_string(root.join(&tab.path)) {
                let v = content.clone();
                tab.editor.update(cx, |s, cx| s.set_value(v, window, cx));
                tab.saved = content;
            }
        }
    }

    pub(super) fn save_file(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(i)) = (self.work_root(), self.active_tab) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(i) else {
            return;
        };
        let value = tab.editor.read(cx).value().to_string();
        match std::fs::write(root.join(&tab.path), &value) {
            Ok(()) => {
                tab.saved = value;
                let p = tab.path.clone();
                self.toast(format!("Saved {p}"), false);
            }
            Err(e) => self.toast(e.to_string(), true),
        }
        self.refresh_workspace(cx);
        cx.notify();
    }

    pub(super) fn write_snapshot(&mut self, cx: &App) {
        let Some(path) = self.state_file.clone() else {
            return;
        };
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|t| self.store.state.thread(t));
        let mut messages: Vec<serde_json::Value> = Vec::new();
        let mut activities: Vec<serde_json::Value> = Vec::new();
        let mut groups: Vec<serde_json::Value> = Vec::new();
        if let Some(t) = thread {
            let running = self.turns.contains_key(&t.id);
            for row in timeline_rows(&t.items) {
                if let Row::Group(r) = row {
                    let g = group_info(&t.items, &r, running);
                    groups.push(serde_json::json!({
                        "id": g.id, "steps": g.steps, "live": g.live, "error": g.err,
                        "open": self.group_open.get(&g.id).copied().unwrap_or(g.live),
                        "label": g.label(), "elapsed_ms": g.elapsed_ms,
                    }));
                }
            }
            for i in &t.items {
                match i {
                    Item::Message { role, text, .. } => {
                        messages.push(serde_json::json!({"role": role, "text": text}))
                    }
                    Item::Activity {
                        kind,
                        label,
                        detail,
                        ..
                    } => activities
                        .push(serde_json::json!({"kind": kind, "label": label, "detail": detail})),
                }
            }
            if let Some(a) = self.turns.get(&t.id) {
                messages.push(
                    serde_json::json!({"role": "assistant", "text": a.text, "streaming": true}),
                );
            }
        }
        let threads: Vec<serde_json::Value> = self
            .store
            .state
            .threads
            .iter()
            .filter(|t| Some(&t.project_id) == self.active_project.as_ref())
            .map(|t| {
                serde_json::json!({
                    "id": t.id, "title": t.title, "pinned": t.pinned, "archived": t.archived, "settled": t.settled, "snoozed": t.snoozed_until > now(),
                    "status": if self.turns.contains_key(&t.id) { "running".to_string() } else { t.last_status.clone() },
                })
            })
            .collect();
        let snap = serde_json::json!({
            "ready": true,
            "sidebar_open": self.sidebar_open,
            "terminal_open": self.terminal_open,
            "settings_open": self.settings_open,
            "palette_open": self.palette_open,
            "palette_mode": format!("{:?}", self.palette_mode),
            "palette_query": self.palette_query,
            "palette_items": self.palette_items().iter().take(12).map(|i| i.0.clone()).collect::<Vec<_>>(),
            "palette_sel": self.palette_sel,
            "model_open": self.model_open,
            "model_items": self.model_items().iter().map(|(p, m)| format!("{p}:{m}")).collect::<Vec<_>>(),
            "model_sel": self.model_sel,
            "panel": self.panel.map(|p| format!("{p:?}")),
            "theme": cx.theme().mode.name(),
            "background": self.store.state.setting("background", "none"),
            "viewer": self.viewer.as_ref().map(|(p, _)| p.clone()),
            "gray_cmds": self
                .gray_cmds
                .iter()
                .map(|c| c.0.clone())
                .collect::<Vec<_>>(),
            "gray_sessions": self.gray_sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            "project": self.active_project.as_ref().and_then(|p| self.store.state.project(p)).map(|p| serde_json::json!({"name": p.name, "path": p.path})),
            "projects": self.store.state.projects.len(),
            "thread": thread.map(|t| serde_json::json!({"id": t.id, "title": t.title, "provider": t.provider, "model": t.model, "mode": t.mode, "pinned": t.pinned, "settled": t.settled, "snoozed": t.snoozed_until > now(), "worktree": t.worktree, "effort": t.effort, "session": t.session.as_ref().map(|s| s.1.clone())})),
            "threads": threads,
            "messages": messages,
            "activities": activities,
            "groups": groups,
            "following": self.follow,
            "running": thread.map(|t| self.turns.contains_key(&t.id)).unwrap_or(false),
            "turn_seconds": self.active_thread.as_ref().and_then(|t| self.turns.get(t)).map(|a| a.started.elapsed().as_secs_f64()),
            "branch": self.branch,
            "branches": self.branches,
            "tree": self.tree.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(),
            "tabs": self.tabs.iter().map(|t| t.path.clone()).collect::<Vec<_>>(),
            "active_tab": self.active_tab.and_then(|i| self.tabs.get(i)).map(|t| t.path.clone()),
            "editor_text": self.active_tab.and_then(|i| self.tabs.get(i)).map(|t| t.editor.read(cx).value().to_string()),
            "diff_files": self.diff_files,
            "diff_base": self.diff_base,
            "approval": self.pending_approval.as_ref().map(|(_, l)| l.clone()),
            "settings_section": self.settings_section,
            "layout": self.store.state.setting("layout", "focus"),
            "auto_accept": self.store.state.setting("auto_accept", "false") == "true",
            "notify": self.store.state.setting("notify", "false") == "true",
            "terminal_placement": self.store.state.setting("terminal_placement", "bottom"),
            "pane_sizes": self.pane_sizes,
            "attachments": self.attachments.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "skills": self.skills.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            "terminal": self.terminal_lines.iter().rev().take(40).rev().cloned().collect::<Vec<_>>(),
            "composer": self.composer.read(cx).value().to_string(),
            "composer_pop": format!("{:?}", self.composer_pop),
            "toast": self.toast.as_ref().map(|t| t.0.clone()),
            "hub": self.hub.iter().map(|h| h.name.clone()).collect::<Vec<_>>(),
            "hub_error": self.hub_error,
            "plugins": self.installed.iter().map(|p| serde_json::json!({"name": p.name, "version": p.version, "enabled": p.enabled})).collect::<Vec<_>>(),
            "plugin_busy": self.plugin_busy,
        });
        let s = snap.to_string();
        if s != self.last_snapshot {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &s).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
                }
                let _ = std::fs::rename(&tmp, &path);
            }
            self.last_snapshot = s;
        }
    }
}
