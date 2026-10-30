use super::*;

impl GrayApp {
    /// Queue an in-app toast; `poll` hands it to the window's notification
    /// layer (it needs the `Window`, which most callers don't have).
    pub(super) fn toast(&mut self, msg: impl Into<String>, error: bool) {
        let msg = msg.into();
        self.toast = Some((msg.clone(), error));
        self.toast_at = Some(Instant::now());
        self.toast_seq += 1;
        self.toast_queue.push((msg, error));
    }

    /// Tell the user a background thread finished or needs approval: a
    /// system notification (plus in-app toast) when the window is unfocused
    /// or the thread isn't the one on screen. Opt-in via the `notify` setting.
    pub(super) fn notify_turn(
        &self,
        tid: &str,
        body: String,
        kind: NotificationType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.store.state.setting("notify", "false") != "true" {
            return false;
        }
        let elsewhere = self.active_thread.as_deref() != Some(tid);
        if window.is_window_active() && !elsewhere {
            return false;
        }
        let title = self
            .store
            .state
            .thread(tid)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Gray".into());
        let id = tid.to_string();
        let this = cx.entity().downgrade();
        let note = Notification::new()
            .with_type(kind)
            .title(format!("Gray · {title}"))
            .message(body)
            .in_app_and_system()
            .on_click(move |_, window, cx| {
                window.activate_window();
                let id = id.clone();
                let _ = this.update(cx, |app, cx| {
                    app.focus_thread(&id, cx);
                });
            });
        window.push_notification(note, cx);
        true
    }

    pub(super) fn focus_thread(&mut self, tid: &str, cx: &mut Context<Self>) {
        let Some(pid) = self.store.state.thread(tid).map(|t| t.project_id.clone()) else {
            return;
        };
        if self.active_thread.as_deref() != Some(tid) {
            self.active_project = Some(pid);
            self.active_thread = Some(tid.to_string());
            self.tabs.clear();
            self.active_tab = None;
            self.refresh_workspace(cx);
        }
        cx.notify();
    }

    pub(super) fn defaults(&self) -> (String, String, String) {
        let provider = self.store.state.setting("default_provider", "gray");
        let model = self.store.state.setting(
            &format!("default_model.{provider}"),
            provider::default_model(&provider),
        );
        let mode = self.store.state.setting("default_mode", "full");
        let mode = if provider == "gray" {
            "full".to_string()
        } else {
            mode
        };
        (provider, model, mode)
    }

    pub(super) fn create_thread(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let pid = self.active_project.clone()?;
        let (provider, model, mode) = self.defaults();
        let id = new_id();
        self.persist(Event::ThreadCreated {
            id: id.clone(),
            project_id: pid,
            title: "New thread".into(),
            provider,
            model,
            mode,
            at: now(),
        });
        self.active_thread = Some(id.clone());
        cx.notify();
        Some(id)
    }

    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer_pop = None;
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(cmd) = text.strip_prefix('/') {
            self.composer
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.slash(cmd.trim(), window, cx);
            return;
        }
        let Some(root) = self.work_root() else {
            self.toast("Add a project first", true);
            cx.notify();
            return;
        };
        let tid = match self.active_thread.clone() {
            Some(t) if self.store.state.thread(&t).is_some() => t,
            _ => match self.create_thread(cx) {
                Some(t) => t,
                None => return,
            },
        };
        if self.turns.contains_key(&tid) {
            self.toast("A turn is already running in this thread", true);
            cx.notify();
            return;
        }
        self.composer
            .update(cx, |s, cx| s.set_value("", window, cx));
        let thread = self.store.state.thread(&tid).cloned();
        let Some(thread) = thread else { return };
        let was_empty = thread.items.is_empty();
        if thread.title == "New thread" {
            let title: String = text.lines().next().unwrap_or("").chars().take(48).collect();
            self.persist(Event::ThreadRenamed {
                id: tid.clone(),
                title,
            });
        }
        if git::is_repo(&root)
            && let Some(tree) = git::checkpoint(&root)
        {
            self.persist(Event::ActivityAdded {
                thread_id: tid.clone(),
                id: new_id(),
                kind: "checkpoint".into(),
                label: "Checkpoint".into(),
                detail: tree,
                at: now(),
            });
        }
        self.persist(Event::MessageAdded {
            thread_id: tid.clone(),
            id: new_id(),
            role: Role::User,
            text: text.clone(),
            at: now(),
        });
        let mut prompt = expand_mentions(&text, &root);
        for a in &self.attachments {
            prompt.push_str(&format!("\n[Attached image: {}]", a.display()));
        }
        self.attachments.clear();
        let turn = provider::start(TurnRequest {
            provider: thread.provider.clone(),
            model: thread.model.clone(),
            mode: if thread.provider == "gray" {
                "full".to_string()
            } else {
                thread.mode.clone()
            },
            prompt,
            cwd: root,
            effort: thread.effort.clone(),
            session: thread
                .session
                .as_ref()
                .filter(|(p, _)| p == &thread.provider)
                .map(|(_, sid)| sid.clone()),
        });
        self.turns.insert(
            tid,
            ActiveTurn {
                turn,
                started: Instant::now(),
                text: String::new(),
            },
        );
        self.follow = true;
        self.scroll.scroll_to_bottom();
        if was_empty {
            self.dock_slide_at = Some(Instant::now());
        }
        cx.notify();
    }

    pub(super) fn slash(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (name, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let arg = arg.trim().to_string();
        let c = match name {
            "new" => Cmd::NewThread,
            "model" => Cmd::Model(arg),
            "provider" => Cmd::Provider(arg),
            "mode" => Cmd::Mode(arg),
            "rename" => Cmd::Rename(arg),
            "branch" => Cmd::NewBranch(arg),
            "checkout" => Cmd::Branch(arg),
            "diff" => Cmd::ToggleDiff,
            "terminal" => Cmd::ToggleTerminal,
            "settings" => {
                if arg.is_empty() {
                    Cmd::Settings
                } else {
                    Cmd::SettingsSection(arg.to_lowercase())
                }
            }
            "theme" => Cmd::ToggleTheme,
            "pin" => Cmd::Pin,
            "archive" => Cmd::Archive,
            "checkpoint" => Cmd::Checkpoint,
            "restore" => Cmd::Restore,
            "discard" => Cmd::DiscardChanges,
            "stop" => Cmd::Stop,
            "settle" => Cmd::Settle,
            "plugins" => Cmd::TogglePlugins,
            "install" => Cmd::PluginInstall(arg),
            "uninstall" => Cmd::PluginRemove(arg),
            "enable" => Cmd::PluginEnable(arg, true),
            "disable" => Cmd::PluginEnable(arg, false),
            "snooze" => Cmd::Snooze(arg.parse().unwrap_or(60)),
            "worktree" => Cmd::Worktree(arg),
            "effort" => Cmd::Effort(arg),
            "background" | "bg" => Cmd::SetBackground(arg),
            "view" | "preview" => Cmd::PreviewFile(arg),
            "sessions" => Cmd::ToggleSessions,
            "skills" => Cmd::ToggleSkills,
            "resume" => Cmd::ResumeSession(arg),
            "gray" => Cmd::Gray(arg),
            "approve" => Cmd::Approve(true),
            "layout" => Cmd::SetLayout(arg),
            "attach" | "image" => Cmd::AttachImage(arg),
            "autoaccept" => Cmd::SetAutoAccept(matches!(arg.as_str(), "on" | "true" | "1")),
            "reject" => Cmd::Approve(false),
            "turndiff" | "changes" => Cmd::TurnDiff,
            "headdiff" => Cmd::HeadDiff,
            _ => {
                self.toast(format!("Unknown command /{name}"), true);
                cx.notify();
                return;
            }
        };
        self.run(c, window, cx);
    }

    pub(super) fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ticks += 1;
        if let Some(path) = std::env::var_os("GRAY_IDE_CMDS")
            && let Ok(text) = std::fs::read_to_string(&path)
            && !text.trim().is_empty()
        {
            let _ = std::fs::write(&path, "");
            self.pending_cmds.extend(
                text.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()),
            );
            cx.notify();
        }
        let mut changed = false;
        let mut finished: Vec<(String, Result<(), String>)> = Vec::new();
        let mut activities: Vec<(String, String, String, String)> = Vec::new();
        let mut sessions: Vec<(String, String)> = Vec::new();
        let mut approvals: Vec<(String, String)> = Vec::new();
        for (tid, active) in self.turns.iter_mut() {
            loop {
                match active.turn.rx.try_recv() {
                    Ok(ProviderEvent::Text(d)) => {
                        active.text.push_str(&d);
                        changed = true;
                    }
                    Ok(ProviderEvent::Replace(t)) => {
                        active.text = t;
                        changed = true;
                    }
                    Ok(ProviderEvent::Activity {
                        kind,
                        label,
                        detail,
                    }) => {
                        activities.push((tid.clone(), kind, label, detail));
                        changed = true;
                    }
                    Ok(ProviderEvent::Session(sid)) => sessions.push((tid.clone(), sid)),
                    Ok(ProviderEvent::Approval { label }) => {
                        approvals.push((tid.clone(), label));
                        changed = true;
                    }
                    Ok(ProviderEvent::Done) => {
                        finished.push((tid.clone(), Ok(())));
                        break;
                    }
                    Ok(ProviderEvent::Error(e)) => {
                        finished.push((tid.clone(), Err(e)));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished.push((tid.clone(), Ok(())));
                        break;
                    }
                }
            }
        }
        for (tid, label) in approvals {
            let auto = self.store.state.setting("auto_accept", "false") == "true";
            if auto {
                if let Some(t) = self.turns.get(&tid)
                    && let Ok(mut p) = t.turn.pending.lock()
                    && let Some(tx) = p.take()
                {
                    let _ = tx.send(true);
                }
                self.persist(Event::ActivityAdded {
                    thread_id: tid,
                    id: new_id(),
                    kind: "approval".into(),
                    label: "Auto-approved".into(),
                    detail: label,
                    at: now(),
                });
            } else {
                self.notify_turn(
                    &tid,
                    format!("Approve: {label}"),
                    NotificationType::Warning,
                    window,
                    cx,
                );
                self.pending_approval = Some((tid, label));
            }
        }
        for (tid, sid) in sessions {
            let provider = self.store.state.thread(&tid).map(|t| t.provider.clone());
            let known = self
                .store
                .state
                .thread(&tid)
                .and_then(|t| t.session.as_ref())
                .map(|(p, s)| Some(p) == provider.as_ref() && s == &sid)
                .unwrap_or(false);
            if let (Some(provider), false) = (provider, known) {
                self.persist(Event::ThreadSession {
                    id: tid,
                    provider,
                    session: sid,
                });
            }
        }
        for (tid, kind, label, detail) in activities {
            self.persist(Event::ActivityAdded {
                thread_id: tid,
                id: new_id(),
                kind,
                label,
                detail,
                at: now(),
            });
        }
        for (tid, result) in finished {
            changed = true;
            // The live group goes back to its default (closed) when the turn ends.
            if let Some(th) = self.store.state.thread(&tid)
                && let Some(Row::Group(r)) = timeline_rows(&th.items).last()
            {
                self.group_open.remove(&group_id(&th.items[r.start]));
            }
            let text = self.turns.remove(&tid).map(|a| a.text).unwrap_or_default();
            if result.is_ok()
                && (self.active_thread.as_deref() != Some(tid.as_str())
                    || !window.is_window_active())
            {
                self.done_unseen.insert(tid.clone());
            }
            let body = match &result {
                Err(e) => format!("Turn failed: {}", e.lines().next().unwrap_or("")),
                Ok(()) if text.trim().is_empty() => "Finished".to_string(),
                Ok(()) => preview(text.trim(), 120),
            };
            let kind = if result.is_ok() {
                NotificationType::Success
            } else {
                NotificationType::Error
            };
            let notified = self.notify_turn(&tid, body, kind, window, cx);
            if !text.trim().is_empty() {
                self.persist(Event::MessageAdded {
                    thread_id: tid.clone(),
                    id: new_id(),
                    role: Role::Assistant,
                    text: text.trim_end().to_string(),
                    at: now(),
                });
            }
            let status = match result {
                Ok(()) => "completed".to_string(),
                Err(e) => {
                    self.persist(Event::ActivityAdded {
                        thread_id: tid.clone(),
                        id: new_id(),
                        kind: "error".into(),
                        label: "Turn failed".into(),
                        detail: e.clone(),
                        at: now(),
                    });
                    let msg = format!("Turn failed: {}", e.lines().next().unwrap_or(""));
                    if notified {
                        // notify_turn already showed an in-app error card.
                        self.toast = Some((msg, true));
                    } else {
                        self.toast(msg, true);
                    }
                    "error".to_string()
                }
            };
            self.persist(Event::TurnFinished {
                thread_id: tid,
                status,
                at: now(),
            });
            self.refresh_workspace(cx);
        }
        while let Ok(cmds) = self.gray_cmd_rx.try_recv() {
            changed = true;
            self.gray_cmds = cmds;
        }
        while let Ok(msg) = self.plugin_rx.try_recv() {
            changed = true;
            match msg {
                PluginMsg::Hub(Ok(h)) => {
                    self.hub = h;
                    self.hub_error = None;
                }
                PluginMsg::Hub(Err(e)) => self.hub_error = Some(e),
                PluginMsg::Installed(i) => self.installed = i,
                PluginMsg::Done(label, r) => {
                    self.plugin_busy = None;
                    match r {
                        Ok(line) => self.toast(format!("{label}: {line}"), false),
                        Err(line) => self.toast(format!("{label} failed: {line}"), true),
                    }
                }
            }
        }
        if !self.terminal_rx.is_empty() {
            let mut i = 0;
            while i < self.terminal_rx.len() {
                let mut closed = false;
                loop {
                    match self.terminal_rx[i].try_recv() {
                        Ok(line) => {
                            self.terminal_lines.push(line);
                            changed = true;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            closed = true;
                            break;
                        }
                    }
                }
                if closed {
                    self.terminal_rx.remove(i);
                    self.refresh_workspace(cx);
                    changed = true;
                } else {
                    i += 1;
                }
            }
            if self.terminal_lines.len() > 2000 {
                let cut = self.terminal_lines.len() - 2000;
                self.terminal_lines.drain(..cut);
            }
        }
        // Focused: the in-app toast card shows it. Unfocused: fall back to the
        // window's notification stack so the user still sees it.
        if !window.is_window_active() {
            for (msg, err) in std::mem::take(&mut self.toast_queue) {
                let kind = if err {
                    NotificationType::Error
                } else {
                    NotificationType::Info
                };
                window.push_notification(Notification::new().with_type(kind).message(msg), cx);
            }
        } else {
            self.toast_queue.clear();
        }
        if self
            .toast_at
            .is_some_and(|at| at.elapsed() > motion::ms(motion::TOAST_MS))
        {
            self.toast = None;
            self.toast_at = None;
            changed = true;
        }
        if self.active_thread != self.follow_thread {
            // A freshly opened thread starts at its latest message.
            self.follow_thread = self.active_thread.clone();
            self.follow = true;
            if let Some(id) = &self.active_thread {
                self.done_unseen.remove(id);
            }
            changed = true;
        } else {
            let off = self.scroll.offset().y;
            let at_bottom = self.scroll.max_offset().y + off <= px(24.);
            let was = self.follow;
            if at_bottom {
                self.follow = true;
            } else if off != self.last_offset {
                // The offset moved without content pinning it: the user scrolled.
                self.follow = false;
            } else if self.follow && self.scroll.max_offset().y != self.last_max {
                // Content grew under us (fold opened, text reflowed): re-pin.
                // Gated on the max changing: with no timeline laid out (empty
                // thread) the offsets are stale and this would fire forever.
                changed = true;
            }
            if was != self.follow {
                cx.notify();
            }
        }
        if changed {
            if self.follow {
                self.scroll.scroll_to_bottom();
            }
            cx.notify();
        } else if !self.turns.is_empty() && self.ticks.is_multiple_of(3) {
            // Keep the elapsed-time label ticking between stream events
            // (and under reduced motion, when no animation frames run).
            cx.notify();
        }
        self.last_offset = self.scroll.offset().y;
        self.last_max = self.scroll.max_offset().y;
        self.apply_scroll_target(cx);
    }

    /// Scroll the timeline so a search hit is at the top. Item bounds only
    /// exist after the thread has been laid out, so retry for a few frames.
    pub(super) fn apply_scroll_target(&mut self, cx: &mut Context<Self>) {
        let Some((tid, ix, tries)) = self.scroll_target.clone() else {
            return;
        };
        if self.active_thread.as_deref() != Some(tid.as_str()) || tries > 30 {
            self.scroll_target = None;
            return;
        }
        if tries < 2 {
            // Bounds still describe the previous layout until it re-renders.
            self.scroll_target = Some((tid, ix, tries + 1));
            cx.notify();
            return;
        }
        // Timeline children are rows (tool runs fold into one), not items.
        let row = self
            .store
            .state
            .thread(&tid)
            .and_then(|th| timeline_rows(&th.items).iter().position(|r| r.contains(ix)))
            .unwrap_or(ix);
        let (Some(item), Some(first)) = (
            self.items_scroll.bounds_for_item(row),
            self.items_scroll.bounds_for_item(0),
        ) else {
            self.scroll_target = Some((tid, ix, tries + 1));
            cx.notify();
            return;
        };
        // Offset from the top of the first item is layout-stable, so this
        // works no matter where the timeline is currently scrolled.
        let max = self.scroll.max_offset().y;
        let y = (item.top() - first.top() - px(16.)).max(px(0.)).min(max);
        self.scroll.set_offset(point(px(0.), -y));
        self.scroll_target = None;
        cx.notify();
    }

    pub(super) fn run_terminal(&mut self, cmd: String, cx: &mut Context<Self>) {
        let cmd = cmd.trim().to_string();
        if cmd.is_empty() {
            return;
        }
        if cmd == "clear" {
            self.terminal_lines.clear();
            cx.notify();
            return;
        }
        let Some(root) = self.work_root() else {
            return;
        };
        self.terminal_lines.push(format!("$ {cmd}"));
        self.terminal_rx.push(workspace::run_shell(&root, &cmd));
        cx.notify();
    }
}
