use crate::git;
use crate::plugins::{self, HubEntry, Installed, PluginMsg};
use crate::provider::{self, MODES, PROVIDERS, ProviderEvent, Turn, TurnRequest};
use crate::sessions::{self, GraySession};
use crate::store::{Event, Item, Role, Store, new_id, now};
use crate::workspace::{self, FileEntry};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, Theme, ThemeMode, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;
use std::time::Instant;

actions!(
    gray,
    [
        ToggleSidebar,
        ToggleTerminal,
        ToggleDiff,
        ToggleFiles,
        TogglePalette,
        FilePicker,
        NewThread,
        ModelPicker,
        ModePicker,
        BranchPicker,
        OpenSettings,
        ToggleTheme,
        Dismiss,
        SaveFile,
        StopTurn,
        FocusComposer,
        EffortPicker,
        TogglePluginsPanel,
        ToggleViewer
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-j", ToggleTerminal, None),
        KeyBinding::new("ctrl-d", ToggleDiff, None),
        KeyBinding::new("ctrl-e", ToggleFiles, None),
        KeyBinding::new("ctrl-k", TogglePalette, None),
        KeyBinding::new("ctrl-p", FilePicker, None),
        KeyBinding::new("ctrl-n", NewThread, None),
        KeyBinding::new("ctrl-shift-m", ModelPicker, None),
        KeyBinding::new("ctrl-shift-a", ModePicker, None),
        KeyBinding::new("ctrl-shift-g", BranchPicker, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-shift-l", ToggleTheme, None),
        KeyBinding::new("escape", Dismiss, Some("GrayApp")),
        KeyBinding::new("ctrl-s", SaveFile, None),
        KeyBinding::new("ctrl-shift-x", StopTurn, None),
        KeyBinding::new("ctrl-l", FocusComposer, None),
        KeyBinding::new("ctrl-shift-e", EffortPicker, None),
        KeyBinding::new("ctrl-shift-p", TogglePluginsPanel, None),
        KeyBinding::new("ctrl-shift-v", ToggleViewer, None),
    ]);
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Panel {
    Files,
    Diff,
    Plugins,
    Viewer,
    Sessions,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PaletteMode {
    Commands,
    Files,
}

#[derive(Clone, Debug)]
enum Cmd {
    NewThread,
    ToggleSidebar,
    ToggleTerminal,
    ToggleDiff,
    ToggleFiles,
    Settings,
    ToggleTheme,
    Provider(String),
    Model(String),
    Mode(String),
    Branch(String),
    NewBranch(String),
    Pin,
    Archive,
    Delete,
    Rename(String),
    OpenFile(String),
    Checkpoint,
    Restore,
    Stop,
    AddProject(String),
    Project(String),
    Thread(String),
    ShowArchived,
    TogglePlugins,
    PluginInstall(String),
    PluginRemove(String),
    PluginEnable(String, bool),
    Settle,
    Snooze(i64),
    Worktree(String),
    Effort(String),
    SetBackground(String),
    ToggleViewer,
    PreviewFile(String),
    ToggleSessions,
    ResumeSession(String),
}

struct ActiveTurn {
    turn: Turn,
    text: String,
    started: Instant,
}

struct Tab {
    path: String,
    editor: Entity<EditorState>,
    saved: String,
}

pub struct GrayApp {
    focus: FocusHandle,
    store: Store,
    active_project: Option<String>,
    active_thread: Option<String>,
    composer: Entity<TextareaState>,
    palette_input: Entity<InputState>,
    terminal_input: Entity<InputState>,
    palette_open: bool,
    palette_mode: PaletteMode,
    palette_query: String,
    sidebar_open: bool,
    terminal_open: bool,
    settings_open: bool,
    show_archived: bool,
    panel: Option<Panel>,
    turns: HashMap<String, ActiveTurn>,
    terminal_lines: Vec<String>,
    terminal_rx: Option<Receiver<String>>,
    scroll: ScrollHandle,
    expanded: HashSet<String>,
    tree: Vec<FileEntry>,
    tabs: Vec<Tab>,
    viewer: Option<(String, Entity<EditorState>)>,
    gray_sessions: Vec<GraySession>,
    active_tab: Option<usize>,
    diff: Option<Entity<DiffState>>,
    diff_files: Vec<String>,
    branch: Option<String>,
    branches: Vec<String>,
    toast: Option<(String, bool)>,
    state_file: Option<PathBuf>,
    last_snapshot: String,
    _subs: Vec<Subscription>,
    ticks: u64,
    hub: Vec<HubEntry>,
    hub_error: Option<String>,
    installed: Vec<Installed>,
    plugin_busy: Option<String>,
    plugin_tx: Sender<PluginMsg>,
    plugin_rx: Receiver<PluginMsg>,
}

impl GrayApp {
    pub fn new(
        store: Store,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .submit_on_enter(true)
                .placeholder("Ask Gray anything, @ to mention files, / for commands")
        });
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type a command or search"));
        let terminal_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Run a shell command"));
        let mut subs = Vec::new();
        subs.push(
            cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = ev {
                    this.send(window, cx);
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &palette_input,
            window,
            |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.palette_query = state.read(cx).value().to_string();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    if let Some((_, _, cmd)) = this.palette_items().into_iter().next() {
                        this.run(cmd, window, cx);
                    }
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &terminal_input,
            window,
            |this, state, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let cmd = state.read(cx).value().to_string();
                    state.update(cx, |s, cx| s.set_value("", window, cx));
                    this.run_terminal(cmd, cx);
                }
            },
        ));
        let (plugin_tx, plugin_rx) = channel();
        let mut app = Self {
            focus: cx.focus_handle(),
            store,
            active_project: None,
            active_thread: None,
            composer,
            palette_input,
            terminal_input,
            palette_open: false,
            palette_mode: PaletteMode::Commands,
            palette_query: String::new(),
            sidebar_open: true,
            terminal_open: false,
            settings_open: false,
            show_archived: false,
            panel: None,
            turns: HashMap::new(),
            terminal_lines: Vec::new(),
            terminal_rx: None,
            scroll: ScrollHandle::new(),
            expanded: HashSet::new(),
            tree: Vec::new(),
            tabs: Vec::new(),
            viewer: None,
            gray_sessions: Vec::new(),
            active_tab: None,
            diff: None,
            diff_files: Vec::new(),
            branch: None,
            branches: Vec::new(),
            toast: None,
            state_file: std::env::var_os("GRAY_IDE_STATE").map(PathBuf::from),
            last_snapshot: String::new(),
            _subs: subs,
            ticks: 0,
            hub: Vec::new(),
            hub_error: None,
            installed: Vec::new(),
            plugin_busy: None,
            plugin_tx,
            plugin_rx,
        };
        if let Some(path) = initial {
            app.add_project(&path);
        }
        if app.active_project.is_none() {
            app.active_project = app.store.state.projects.first().map(|p| p.id.clone());
        }
        if let Some(pid) = app.active_project.clone() {
            app.active_thread = app
                .store
                .state
                .threads_for(&pid, false)
                .first()
                .map(|t| t.id.clone());
        }
        let mode = if app.store.state.setting("theme", "dark") == "light" {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        apply_brand(cx);
        app.refresh_workspace(cx);
        app.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |app, cx| app.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        app
    }

    fn work_root(&self) -> Option<PathBuf> {
        if let Some(w) = self
            .active_thread
            .as_ref()
            .and_then(|t| self.store.state.thread(t))
            .and_then(|t| t.worktree.clone())
        {
            return Some(PathBuf::from(w));
        }
        self.project_path()
    }

    fn project_path(&self) -> Option<PathBuf> {
        self.active_project
            .as_ref()
            .and_then(|id| self.store.state.project(id))
            .map(|p| PathBuf::from(&p.path))
    }

    fn add_project(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let p = path.display().to_string();
        if let Some(existing) = self.store.state.projects.iter().find(|x| x.path == p) {
            self.active_project = Some(existing.id.clone());
            return;
        }
        let id = new_id();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.clone());
        self.store.append(Event::ProjectAdded {
            id: id.clone(),
            name,
            path: p,
            at: now(),
        });
        self.active_project = Some(id);
        self.active_thread = None;
    }

    fn refresh_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            self.tree.clear();
            self.branch = None;
            self.branches.clear();
            self.diff = None;
            self.diff_files.clear();
            return;
        };
        self.tree = workspace::tree(&root, &self.expanded);
        if git::is_repo(&root) {
            self.branch = git::branch(&root);
            self.branches = git::branches(&root);
            let text = git::diff(&root);
            let files = DiffFile::parse(&text).unwrap_or_default();
            self.diff_files = git::status(&root).into_iter().map(|(_, p)| p).collect();
            self.diff = if files.is_empty() {
                None
            } else {
                Some(cx.new(|cx| DiffState::new(files, cx)))
            };
        } else {
            self.branch = None;
            self.branches.clear();
            self.diff = None;
            self.diff_files.clear();
        }
    }

    fn toast(&mut self, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), error));
    }

    fn defaults(&self) -> (String, String, String) {
        let provider = self.store.state.setting("default_provider", "gray");
        let model = self.store.state.setting(
            &format!("default_model.{provider}"),
            provider::default_model(&provider),
        );
        let mode = self.store.state.setting("default_mode", "full");
        (provider, model, mode)
    }

    fn create_thread(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let pid = self.active_project.clone()?;
        let (provider, model, mode) = self.defaults();
        let id = new_id();
        self.store.append(Event::ThreadCreated {
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

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        if thread.title == "New thread" {
            let title: String = text.lines().next().unwrap_or("").chars().take(48).collect();
            self.store.append(Event::ThreadRenamed {
                id: tid.clone(),
                title,
            });
        }
        if git::is_repo(&root)
            && let Some(tree) = git::checkpoint(&root)
        {
            self.store.append(Event::ActivityAdded {
                thread_id: tid.clone(),
                id: new_id(),
                kind: "checkpoint".into(),
                label: "Checkpoint".into(),
                detail: tree,
                at: now(),
            });
        }
        self.store.append(Event::MessageAdded {
            thread_id: tid.clone(),
            id: new_id(),
            role: Role::User,
            text: text.clone(),
            at: now(),
        });
        let prompt = expand_mentions(&text, &root);
        let turn = provider::start(TurnRequest {
            provider: thread.provider.clone(),
            model: thread.model.clone(),
            mode: thread.mode.clone(),
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
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn slash(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
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
            "settings" => Cmd::Settings,
            "theme" => Cmd::ToggleTheme,
            "pin" => Cmd::Pin,
            "archive" => Cmd::Archive,
            "checkpoint" => Cmd::Checkpoint,
            "restore" => Cmd::Restore,
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
            "resume" => Cmd::ResumeSession(arg),
            _ => {
                self.toast(format!("Unknown command /{name}"), true);
                cx.notify();
                return;
            }
        };
        self.run(c, window, cx);
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        self.ticks += 1;
        let mut changed = false;
        let mut finished: Vec<(String, Result<(), String>)> = Vec::new();
        let mut activities: Vec<(String, String, String, String)> = Vec::new();
        let mut sessions: Vec<(String, String)> = Vec::new();
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
                self.store.append(Event::ThreadSession {
                    id: tid,
                    provider,
                    session: sid,
                });
            }
        }
        for (tid, kind, label, detail) in activities {
            self.store.append(Event::ActivityAdded {
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
            let text = self.turns.remove(&tid).map(|a| a.text).unwrap_or_default();
            if !text.trim().is_empty() {
                self.store.append(Event::MessageAdded {
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
                    self.store.append(Event::ActivityAdded {
                        thread_id: tid.clone(),
                        id: new_id(),
                        kind: "error".into(),
                        label: "Turn failed".into(),
                        detail: e.clone(),
                        at: now(),
                    });
                    self.toast(
                        format!("Turn failed: {}", e.lines().next().unwrap_or("")),
                        true,
                    );
                    "error".to_string()
                }
            };
            self.store.append(Event::TurnFinished {
                thread_id: tid,
                status,
                at: now(),
            });
            self.refresh_workspace(cx);
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
        if let Some(rx) = &self.terminal_rx {
            loop {
                match rx.try_recv() {
                    Ok(line) => {
                        self.terminal_lines.push(line);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.terminal_rx = None;
                        self.refresh_workspace(cx);
                        changed = true;
                        break;
                    }
                }
            }
            if self.terminal_lines.len() > 2000 {
                let cut = self.terminal_lines.len() - 2000;
                self.terminal_lines.drain(..cut);
            }
        }
        if changed {
            self.scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn run_terminal(&mut self, cmd: String, cx: &mut Context<Self>) {
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
        self.terminal_rx = Some(workspace::run_shell(&root, &cmd));
        cx.notify();
    }

    fn open_palette(
        &mut self,
        mode: PaletteMode,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_open = true;
        self.palette_mode = mode;
        self.palette_query = query.to_string();
        let q = query.to_string();
        self.palette_input.update(cx, |s, cx| {
            s.set_value(q, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        self.settings_open = false;
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn palette_items(&self) -> Vec<(String, String, Cmd)> {
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
        let k = |s: &str| s.to_string();
        items.push((k("New thread"), k("Ctrl+N"), Cmd::NewThread));
        items.push((k("Toggle sidebar"), k("Ctrl+B"), Cmd::ToggleSidebar));
        items.push((k("Toggle terminal"), k("Ctrl+J"), Cmd::ToggleTerminal));
        items.push((k("Toggle diff panel"), k("Ctrl+D"), Cmd::ToggleDiff));
        items.push((k("Toggle files panel"), k("Ctrl+E"), Cmd::ToggleFiles));
        items.push((k("Open settings"), k("Ctrl+,"), Cmd::Settings));
        items.push((k("Toggle theme"), k("Ctrl+Shift+L"), Cmd::ToggleTheme));
        items.push((k("Create checkpoint"), k("Git"), Cmd::Checkpoint));
        items.push((k("Plugins"), k("Ctrl+Shift+P"), Cmd::TogglePlugins));
        items.push((k("Sessions"), k("Gray"), Cmd::ToggleSessions));
        for gs in self.gray_sessions.iter().take(10) {
            items.push((
                format!("Resume session: {}", gs.id),
                k("Gray"),
                Cmd::ResumeSession(gs.id.clone()),
            ));
        }
        items.push((k("Code viewer"), k("Ctrl+Shift+V"), Cmd::ToggleViewer));
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
            items.push((
                format!("Effort: {e}"),
                k("Ctrl+Shift+E"),
                Cmd::Effort(e.into()),
            ));
        }
        items.push((k("Restore last checkpoint"), k("Git"), Cmd::Restore));
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
            items.push((k("Stop turn"), k("Ctrl+Shift+X"), Cmd::Stop));
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
        for (id, label) in MODES {
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

    fn configure(&mut self, provider: Option<String>, model: Option<String>, mode: Option<String>) {
        let Some(tid) = self.active_thread.clone() else {
            if let Some(p) = &provider {
                self.store.append(Event::SettingChanged {
                    key: "default_provider".into(),
                    value: p.clone(),
                });
            }
            if let (Some(p), Some(m)) = (&provider, &model) {
                self.store.append(Event::SettingChanged {
                    key: format!("default_model.{p}"),
                    value: m.clone(),
                });
            }
            if let Some(m) = &mode {
                self.store.append(Event::SettingChanged {
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
        let model = model.unwrap_or_else(|| {
            if provider.is_some() && provider_id != t.provider {
                provider::default_model(&provider_id).to_string()
            } else {
                t.model.clone()
            }
        });
        self.store.append(Event::ThreadConfigured {
            id: tid,
            provider: provider_id,
            model,
            mode: mode.unwrap_or(t.mode),
        });
    }

    fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
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
            Cmd::Settings => self.settings_open = true,
            Cmd::ToggleTheme => {
                let dark = cx.theme().mode.is_dark();
                let next = if dark {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                self.store.append(Event::SettingChanged {
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
                    self.store.append(Event::ThreadPinned {
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
                    self.store.append(Event::ThreadArchived {
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
                    self.store.append(Event::ThreadDeleted { id });
                }
            }
            Cmd::Rename(title) => {
                if let Some(id) = self.active_thread.clone()
                    && !title.is_empty()
                {
                    self.store.append(Event::ThreadRenamed { id, title });
                }
            }
            Cmd::OpenFile(path) => self.open_file(&path, window, cx),
            Cmd::Checkpoint => {
                if let (Some(root), Some(tid)) = (self.work_root(), self.active_thread.clone())
                    && let Some(tree) = git::checkpoint(&root)
                {
                    self.store.append(Event::ActivityAdded {
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
                    t.turn.cancel();
                }
            }
            Cmd::AddProject(p) => {
                self.add_project(Path::new(&p));
                self.tabs.clear();
                self.active_tab = None;
                self.refresh_workspace(cx);
            }
            Cmd::Project(id) => {
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
                    self.store.append(Event::ThreadSettled {
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
                    self.store.append(Event::ThreadSnoozed { id, until });
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
                self.store.append(Event::ThreadConfigured {
                    id: tid.clone(),
                    provider: "gray".into(),
                    model,
                    mode: "full".into(),
                });
                self.store.append(Event::ThreadSession {
                    id: tid.clone(),
                    provider: "gray".into(),
                    session: id.clone(),
                });
                self.store.append(Event::ThreadRenamed {
                    id: tid,
                    title: format!("gray/{id}"),
                });
                self.toast(format!("Resuming {id}"), false);
            }
            Cmd::SetBackground(b) => {
                self.store.append(Event::SettingChanged {
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
                        self.store.append(Event::ThreadEffort { id, effort: e });
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
                        Ok(()) => {
                            if let Some(id) = tid {
                                self.store.append(Event::ThreadWorktree {
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

    fn open_file(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.path == rel) {
            self.active_tab = Some(i);
            self.panel = Some(Panel::Files);
            self.tabs[i].editor.update(cx, |s, cx| s.focus(window, cx));
            return;
        }
        let Some(root) = self.work_root() else {
            return;
        };
        let content = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
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

    fn preview_file(&mut self, rel: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            return;
        };
        let path = root.join(rel);
        if !path.is_file() {
            self.toast(format!("No file: {rel}"), true);
            cx.notify();
            return;
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
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

    fn reload_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            return;
        };
        for tab in &mut self.tabs {
            let content = std::fs::read_to_string(root.join(&tab.path)).unwrap_or_default();
            let v = content.clone();
            tab.editor.update(cx, |s, cx| s.set_value(v, window, cx));
            tab.saved = content;
        }
    }

    fn save_file(&mut self, cx: &mut Context<Self>) {
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

    fn write_snapshot(&mut self, cx: &App) {
        let Some(path) = self.state_file.clone() else {
            return;
        };
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|t| self.store.state.thread(t));
        let mut messages: Vec<serde_json::Value> = Vec::new();
        let mut activities: Vec<serde_json::Value> = Vec::new();
        if let Some(t) = thread {
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
            "panel": self.panel.map(|p| format!("{p:?}")),
            "theme": cx.theme().mode.name(),
            "background": self.store.state.setting("background", "none"),
            "viewer": self.viewer.as_ref().map(|(p, _)| p.clone()),
            "gray_sessions": self.gray_sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            "project": self.active_project.as_ref().and_then(|p| self.store.state.project(p)).map(|p| serde_json::json!({"name": p.name, "path": p.path})),
            "projects": self.store.state.projects.len(),
            "thread": thread.map(|t| serde_json::json!({"id": t.id, "title": t.title, "provider": t.provider, "model": t.model, "mode": t.mode, "pinned": t.pinned, "settled": t.settled, "snoozed": t.snoozed_until > now(), "worktree": t.worktree, "effort": t.effort, "session": t.session.as_ref().map(|s| s.1.clone())})),
            "threads": threads,
            "messages": messages,
            "activities": activities,
            "running": thread.map(|t| self.turns.contains_key(&t.id)).unwrap_or(false),
            "turn_seconds": self.active_thread.as_ref().and_then(|t| self.turns.get(t)).map(|a| a.started.elapsed().as_secs_f64()),
            "branch": self.branch,
            "branches": self.branches,
            "tree": self.tree.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(),
            "tabs": self.tabs.iter().map(|t| t.path.clone()).collect::<Vec<_>>(),
            "active_tab": self.active_tab.and_then(|i| self.tabs.get(i)).map(|t| t.path.clone()),
            "editor_text": self.active_tab.and_then(|i| self.tabs.get(i)).map(|t| t.editor.read(cx).value().to_string()),
            "diff_files": self.diff_files,
            "terminal": self.terminal_lines.iter().rev().take(40).rev().cloned().collect::<Vec<_>>(),
            "composer": self.composer.read(cx).value().to_string(),
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
                let _ = std::fs::rename(&tmp, &path);
            }
            self.last_snapshot = s;
        }
    }
}

fn fuzzy(hay: &str, needle: &str) -> bool {
    if hay.contains(needle) {
        return true;
    }
    let mut it = hay.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| it.any(|h| h == c))
}

fn churner(ticks: u64, t: &Theme) -> impl IntoElement {
    h_flex().gap_0p5().children((0..8).map(|i| {
        let on = ((ticks / 2) + i) % 8 < 4;
        div().size(px(4.)).bg(if on { t.primary } else { t.border })
    }))
}

fn provider_brand(provider: &str) -> Option<&'static str> {
    match provider {
        "gray" => Some("icons/gray-logo.svg"),
        "claude" => Some("icons/claude.svg"),
        "codex" => Some("icons/openai.svg"),
        _ => None,
    }
}

fn provider_chip(
    id: impl Into<ElementId>,
    provider: &str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let mut c = h_flex()
        .id(id)
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs()
        .text_color(t.muted_foreground)
        .cursor_pointer()
        .hover(|s| s.bg(t.secondary_hover));
    c = match provider_brand(provider) {
        Some(path) => c.child(
            svg()
                .path(path.to_string())
                .size_3()
                .text_color(t.muted_foreground),
        ),
        None => c.child(Icon::new(IconName::Bot).xsmall()),
    };
    c.child(label.into())
}

fn chip(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    h_flex()
        .id(id)
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs()
        .text_color(t.muted_foreground)
        .cursor_pointer()
        .hover(|s| s.bg(t.secondary_hover))
        .child(Icon::new(icon).xsmall())
        .child(label.into())
}

impl GrayApp {
    fn render_plugins(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let label = |s: &str| {
            div()
                .text_xs()
                .font_family(t.mono_font_family.clone())
                .text_color(t.muted_foreground)
                .child(s.to_uppercase())
        };
        let badge = |s: &str| {
            div()
                .px_1p5()
                .py_0p5()
                .border_1()
                .border_color(t.primary.opacity(0.4))
                .bg(t.primary.opacity(0.1))
                .text_color(t.primary)
                .text_xs()
                .font_family(t.mono_font_family.clone())
                .child(s.to_uppercase())
        };
        let mut col = v_flex()
            .id("plugins-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3();
        col = col.child(label(&format!("Installed · {}", self.installed.len())));
        if self.installed.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No plugins installed"),
            );
        }
        for p in &self.installed {
            let (n1, n2, on) = (p.name.clone(), p.name.clone(), p.enabled);
            col = col.child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .child(
                        div()
                            .font_family(t.mono_font_family.clone())
                            .text_sm()
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(p.version.clone()),
                    )
                    .when(!on, |s| s.child(badge("disabled")))
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("plugin-toggle-{}", p.name)))
                            .xsmall()
                            .ghost()
                            .label(if on { "Disable" } else { "Enable" })
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.run(Cmd::PluginEnable(n1.clone(), !on), w, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("plugin-remove-{}", p.name)))
                            .xsmall()
                            .ghost()
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.run(Cmd::PluginRemove(n2.clone()), w, cx)
                            })),
                    ),
            );
        }
        col = col.child(
            div()
                .mt_2()
                .child(label(&format!("Hub · {}", self.hub.len()))),
        );
        if let Some(e) = &self.hub_error {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(t.danger)
                    .child(format!("Could not load {}: {e}", plugins::hub_url())),
            );
        } else if self.hub.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("Loading catalog"),
            );
        }
        for h in &self.hub {
            let installed = self.installed.iter().any(|p| p.name == h.name);
            let busy = self.plugin_busy.as_deref() == Some(h.name.as_str());
            let name = h.name.clone();
            col = col.child(
                v_flex()
                    .gap_2()
                    .p_4()
                    .border_1()
                    .border_color(t.border)
                    .bg(hsla(0., 0., 0., 1.))
                    .hover(|s| s.border_color(t.primary))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(t.mono_font_family.clone())
                                    .font_semibold()
                                    .child(h.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(format!("v{}", h.version)),
                            )
                            .when(h.official, |s| s.child(badge("official")))
                            .when(h.verified, |s| s.child(badge("verified")))
                            .when(h.kind != "plugin", |s| s.child(badge(&h.kind)))
                            .child(div().flex_1())
                            .child(if installed {
                                Button::new(SharedString::from(format!("hub-installed-{}", h.name)))
                                    .xsmall()
                                    .ghost()
                                    .label("Installed")
                                    .icon(IconName::Check)
                            } else {
                                Button::new(SharedString::from(format!("hub-install-{}", h.name)))
                                    .xsmall()
                                    .primary()
                                    .label(if busy { "Installing" } else { "Install" })
                                    .loading(busy)
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.run(Cmd::PluginInstall(name.clone()), w, cx)
                                    }))
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.muted_foreground)
                            .child(h.description.clone()),
                    )
                    .when(!h.author.is_empty(), |s| {
                        s.child(label(&format!("by {}", h.author)))
                    }),
            );
        }
        col
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut list = v_flex()
            .id("sidebar-scroll")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .gap_0p5();
        for p in &self.store.state.projects {
            let pid = p.id.clone();
            let active_project = Some(&p.id) == self.active_project.as_ref();
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("project-{}", p.id)))
                    .mt_2()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded_md()
                    .text_xs()
                    .font_semibold()
                    .text_color(if active_project {
                        t.sidebar_foreground
                    } else {
                        t.muted_foreground
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(t.sidebar_accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Project(pid.clone()), window, cx)
                    }))
                    .child(
                        Icon::new(if active_project {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        })
                        .xsmall(),
                    )
                    .child(div().flex_1().truncate().child(p.name.clone())),
            );
            if !active_project {
                continue;
            }
            let threads: Vec<_> = self
                .store
                .state
                .threads_for(&p.id, self.show_archived)
                .into_iter()
                .filter(|t| self.show_archived || t.snoozed_until <= now())
                .collect();
            if threads.is_empty() {
                list = list.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child("No threads yet"),
                );
            }
            for th in threads {
                let id = th.id.clone();
                let active = Some(&th.id) == self.active_thread.as_ref();
                let running = self.turns.contains_key(&th.id);
                let status_color = if running {
                    t.info
                } else if th.last_status == "error" {
                    t.danger
                } else if th.last_status == "completed" {
                    t.success
                } else {
                    t.muted_foreground
                };
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("thread-{}", th.id)))
                        .pl_5()
                        .pr_2()
                        .py_1p5()
                        .gap_2()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .when(active, |s| {
                            s.bg(t.sidebar_accent)
                                .text_color(t.sidebar_accent_foreground)
                        })
                        .hover(|s| s.bg(t.sidebar_accent))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run(Cmd::Thread(id.clone()), window, cx)
                        }))
                        .child(div().size_1p5().rounded_full().bg(status_color))
                        .child(div().flex_1().truncate().child(th.title.clone()))
                        .when(th.pinned, |s| {
                            s.child(Icon::new(IconName::Star).xsmall().text_color(t.warning))
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.muted_foreground)
                                .child(relative(th.updated_at)),
                        ),
                );
            }
        }
        v_flex()
            .w(px(272.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .child(
                h_flex()
                    .h(px(48.))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(t.sidebar_border)
                    .child(
                        div()
                            .size_6()
                            .rounded_md()
                            .bg(t.foreground)
                            .text_color(t.background)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(
                                svg()
                                    .path("icons/gray-logo.svg")
                                    .size_4()
                                    .text_color(t.background),
                            ),
                    )
                    .child(div().font_semibold().child("Gray"))
                    .child(
                        div()
                            .text_xs()
                            .px_1()
                            .rounded_sm()
                            .bg(t.secondary)
                            .text_color(t.muted_foreground)
                            .child("IDE"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("new-thread")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("New thread (Ctrl+N)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::NewThread, window, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .id("sidebar-search")
                    .mx_2()
                    .mt_2()
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_palette(PaletteMode::Commands, "", window, cx)
                    }))
                    .child(Icon::new(IconName::Search).xsmall())
                    .child(div().flex_1().child("Search or run a command"))
                    .child("Ctrl K"),
            )
            .child(list)
            .child(
                h_flex()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(t.sidebar_border)
                    .child(
                        Button::new("open-settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .label("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Settings, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("open-plugins")
                            .ghost()
                            .small()
                            .icon(IconName::Star)
                            .tooltip("Plugins (Ctrl+Shift+P)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::TogglePlugins, window, cx)
                            })),
                    )
                    .child(
                        Button::new("toggle-archived")
                            .ghost()
                            .small()
                            .icon(IconName::Inbox)
                            .tooltip("Show archived")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ShowArchived, window, cx)
                            })),
                    )
                    .child(
                        Button::new("toggle-theme")
                            .ghost()
                            .small()
                            .icon(if t.mode.is_dark() {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .tooltip("Toggle theme")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ToggleTheme, window, cx)
                            })),
                    ),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let project = self
            .active_project
            .as_ref()
            .and_then(|id| self.store.state.project(id));
        let title = thread
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "New thread".into());
        h_flex()
            .h(px(48.))
            .px_3()
            .gap_2()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .when(!self.sidebar_open, |s| {
                s.child(
                    Button::new("show-sidebar")
                        .ghost()
                        .small()
                        .icon(IconName::PanelLeft)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleSidebar, window, cx)
                        })),
                )
            })
            .child(div().font_semibold().truncate().child(title))
            .when_some(thread.and_then(|t| t.worktree.clone()), |s, w| {
                let name = Path::new(&w)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                s.child(
                    div()
                        .text_xs()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border)
                        .text_color(t.muted_foreground)
                        .child(format!("worktree {name}")),
                )
            })
            .when_some(project, |s, p| {
                s.child(
                    div()
                        .text_xs()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(t.secondary)
                        .text_color(t.muted_foreground)
                        .child(p.name.clone()),
                )
            })
            .when_some(self.branch.clone(), |s, b| {
                s.child(
                    chip("header-branch", IconName::Github, b, cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_palette(PaletteMode::Commands, "Branch:", window, cx)
                        },
                    )),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("toggle-files")
                    .ghost()
                    .small()
                    .icon(IconName::FileText)
                    .tooltip("Files (Ctrl+E)")
                    .selected(self.panel == Some(Panel::Files))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(Cmd::ToggleFiles, window, cx)),
                    ),
            )
            .child(
                Button::new("toggle-diff")
                    .ghost()
                    .small()
                    .icon(IconName::Copy)
                    .label(if self.diff_files.is_empty() {
                        "Diff".to_string()
                    } else {
                        format!("Diff {}", self.diff_files.len())
                    })
                    .tooltip("Diff (Ctrl+D)")
                    .selected(self.panel == Some(Panel::Diff))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(Cmd::ToggleDiff, window, cx)),
                    ),
            )
            .child(
                Button::new("toggle-terminal")
                    .ghost()
                    .small()
                    .icon(IconName::SquareTerminal)
                    .tooltip("Terminal (Ctrl+J)")
                    .selected(self.terminal_open)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleTerminal, window, cx)
                        }),
                    ),
            )
            .when(self.sidebar_open, |s| {
                s.child(
                    Button::new("hide-sidebar")
                        .ghost()
                        .small()
                        .icon(IconName::PanelLeft)
                        .tooltip("Sidebar (Ctrl+B)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleSidebar, window, cx)
                        })),
                )
            })
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let empty = thread.map(|th| th.items.is_empty()).unwrap_or(true)
            && thread
                .map(|th| !self.turns.contains_key(&th.id))
                .unwrap_or(true);
        if empty {
            let project = self
                .active_project
                .as_ref()
                .and_then(|id| self.store.state.project(id))
                .map(|p| p.name.clone());
            let hint = |k: &str, l: &str| {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(t.border)
                            .child(k.to_string()),
                    )
                    .child(l.to_string())
            };
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path("icons/gray-logo.svg")
                        .size_16()
                        .text_color(t.foreground),
                )
                .child(
                    div()
                        .text_3xl()
                        .font_family("Plus Jakarta Sans")
                        .font_bold()
                        .child("Gray"),
                )
                .child(div().text_color(t.muted_foreground).child(match project {
                    Some(p) => format!("What should we build in {p}?"),
                    None => "Open a project to get started".to_string(),
                }))
                .child(
                    v_flex()
                        .mt_4()
                        .gap_1p5()
                        .child(hint("Ctrl K", "Command palette"))
                        .child(hint("Ctrl P", "Open file"))
                        .child(hint("Ctrl Shift M", "Switch model"))
                        .child(hint("Ctrl J", "Terminal"))
                        .child(hint("Ctrl D", "Diff")),
                )
                .into_any_element();
        }
        let thread = thread.unwrap();
        let mut col = v_flex()
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .px_6()
            .py_6()
            .gap_4();
        for item in &thread.items {
            col = col.child(render_item(item, &t));
        }
        if let Some(a) = self.turns.get(&thread.id) {
            col = col.child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(churner(self.ticks, &t))
                            .child(div().font_family(t.mono_font_family.clone()).child(format!(
                                "{} · CHURNING {:.1}S",
                                provider::provider_label(&thread.provider),
                                a.started.elapsed().as_secs_f32()
                            ))),
                    )
                    .when(!a.text.is_empty(), |s| {
                        s.child(TextView::markdown("streaming", a.text.clone()).selectable(true))
                    }),
            );
        }
        div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(col)
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let (dp, dm, dmode) = self.defaults();
        let provider = thread.map(|t| t.provider.clone()).unwrap_or(dp);
        let model = thread.map(|t| t.model.clone()).unwrap_or(dm);
        let mode = thread.map(|t| t.mode.clone()).unwrap_or(dmode);
        let effort = thread
            .map(|t| t.effort.clone())
            .unwrap_or_else(|| "medium".into());
        let running = thread
            .map(|t| self.turns.contains_key(&t.id))
            .unwrap_or(false);
        div()
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .px_6()
            .pb_4()
            .child(
                v_flex()
                    .rounded_xl()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .shadow_sm()
                    .child(
                        div()
                            .px_2()
                            .pt_2()
                            .child(Textarea::new(&self.composer).appearance(false)),
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .pb_2()
                            .gap_1()
                            .child(
                                provider_chip(
                                    "composer-model",
                                    &provider,
                                    format!(
                                        "{} · {} · {}",
                                        provider::provider_label(&provider),
                                        model,
                                        effort
                                    ),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.open_palette(
                                            PaletteMode::Commands,
                                            "Model:",
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .child(
                                chip(
                                    "composer-mode",
                                    IconName::TriangleAlert,
                                    provider::mode_label(&mode),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.open_palette(
                                            PaletteMode::Commands,
                                            "Mode:",
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .when_some(self.branch.clone(), |s, b| {
                                s.child(chip("composer-branch", IconName::Github, b, cx).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.open_palette(
                                            PaletteMode::Commands,
                                            "Branch:",
                                            window,
                                            cx,
                                        )
                                    }),
                                ))
                            })
                            .child(div().flex_1())
                            .child(if running {
                                Button::new("stop")
                                    .small()
                                    .danger()
                                    .icon(IconName::Square)
                                    .tooltip("Stop (Ctrl+Shift+X)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run(Cmd::Stop, window, cx)
                                    }))
                            } else {
                                Button::new("send")
                                    .small()
                                    .primary()
                                    .icon(IconName::ArrowUp)
                                    .tooltip("Send (Enter)")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                                    )
                            }),
                    ),
            )
    }

    fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut lines = v_flex().font_family(t.mono_font_family.clone()).text_xs();
        for l in self.terminal_lines.iter().rev().take(300).rev() {
            lines = lines.child(div().child(l.clone()));
        }
        v_flex()
            .h(px(240.))
            .flex_none()
            .border_t_1()
            .border_color(t.border)
            .bg(t.background)
            .child(
                h_flex()
                    .h(px(32.))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .border_b_1()
                    .border_color(t.border)
                    .child(Icon::new(IconName::SquareTerminal).xsmall())
                    .child("Terminal")
                    .child(div().flex_1())
                    .child(
                        Button::new("close-terminal")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ToggleTerminal, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("terminal-output")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .child(lines),
            )
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .child(Input::new(&self.terminal_input).prefix(div().text_xs().child("$"))),
            )
    }

    fn render_panel(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let header = h_flex()
            .h(px(48.))
            .px_2()
            .gap_1()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .child(
                Button::new("panel-files")
                    .ghost()
                    .small()
                    .label("Files")
                    .selected(panel == Panel::Files)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Files);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-sessions")
                    .ghost()
                    .small()
                    .label("Sessions")
                    .selected(panel == Panel::Sessions)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Sessions);
                        this.gray_sessions = sessions::list();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-viewer")
                    .ghost()
                    .small()
                    .label("Viewer")
                    .selected(panel == Panel::Viewer)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Viewer);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-plugins")
                    .ghost()
                    .small()
                    .label("Plugins")
                    .selected(panel == Panel::Plugins)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Plugins);
                        plugins::refresh(this.plugin_tx.clone());
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-diff")
                    .ghost()
                    .small()
                    .label("Diff")
                    .selected(panel == Panel::Diff)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Diff);
                        this.refresh_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("panel-refresh")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-close")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = None;
                        cx.notify();
                    })),
            );
        let body: AnyElement = match panel {
            Panel::Sessions => self.render_sessions(cx).into_any_element(),
            Panel::Viewer => self.render_viewer(cx).into_any_element(),
            Panel::Plugins => self.render_plugins(cx).into_any_element(),
            Panel::Diff => match &self.diff {
                Some(state) => Diff::new(state).flex_1().min_h_0().into_any_element(),
                None => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No changes")
                    .into_any_element(),
            },
            Panel::Files => {
                let mut tree = v_flex()
                    .id("file-tree")
                    .w(px(220.))
                    .h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .py_1()
                    .border_r_1()
                    .border_color(t.border);
                for e in &self.tree {
                    let rel = e.rel.clone();
                    let is_dir = e.is_dir;
                    let open = self.expanded.contains(&e.rel);
                    let active = self
                        .active_tab
                        .and_then(|i| self.tabs.get(i))
                        .map(|t| t.path == e.rel)
                        .unwrap_or(false);
                    tree = tree.child(
                        h_flex()
                            .id(SharedString::from(format!("file-{}", e.rel)))
                            .pl(px(8. + e.depth as f32 * 12.))
                            .pr_2()
                            .py_0p5()
                            .gap_1p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(active, |s| s.bg(t.list_active))
                            .hover(|s| s.bg(t.list_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if is_dir {
                                    if !this.expanded.remove(&rel) {
                                        this.expanded.insert(rel.clone());
                                    }
                                    this.refresh_workspace(cx);
                                } else {
                                    this.preview_file(&rel, window, cx);
                                }
                                cx.notify();
                            }))
                            .child(
                                Icon::new(if is_dir {
                                    if open {
                                        IconName::FolderOpen
                                    } else {
                                        IconName::Folder
                                    }
                                } else {
                                    IconName::File
                                })
                                .xsmall(),
                            )
                            .child(div().truncate().child(e.name.clone())),
                    );
                }
                let mut tabs = h_flex()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(t.border)
                    .overflow_x_hidden();
                for (i, tab) in self.tabs.iter().enumerate() {
                    let dirty = tab.editor.read(cx).value().as_ref() != tab.saved.as_str();
                    let name = Path::new(&tab.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let active = Some(i) == self.active_tab;
                    tabs = tabs.child(
                        h_flex()
                            .id(SharedString::from(format!("tab-{}", tab.path)))
                            .h_full()
                            .px_3()
                            .gap_1()
                            .text_xs()
                            .border_r_1()
                            .border_color(t.border)
                            .cursor_pointer()
                            .when(active, |s| {
                                s.bg(t.tab_active).text_color(t.tab_active_foreground)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_tab = Some(i);
                                cx.notify();
                            }))
                            .child(name)
                            .when(dirty, |s| {
                                s.child(div().size_1p5().rounded_full().bg(t.warning))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("tab-close-{i}")))
                                    .ml_1()
                                    .text_color(t.muted_foreground)
                                    .hover(|s| s.text_color(t.foreground))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if i < this.tabs.len() {
                                            this.tabs.remove(i);
                                        }
                                        this.active_tab = if this.tabs.is_empty() {
                                            None
                                        } else {
                                            Some(i.min(this.tabs.len() - 1))
                                        };
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    );
                }
                let editor: AnyElement = match self.active_tab.and_then(|i| self.tabs.get(i)) {
                    Some(tab) => div()
                        .flex_1()
                        .min_h_0()
                        .child(Editor::new(&tab.editor).h_full().bordered(false))
                        .into_any_element(),
                    None => v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(t.muted_foreground)
                        .child("Select a file or press Ctrl+P")
                        .into_any_element(),
                };
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(tree)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(tabs)
                            .child(editor),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .w(px(if panel == Panel::Files { 640. } else { 560. }))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(t.border)
            .bg(t.background)
            .child(header)
            .child(body)
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut list = v_flex()
            .id("palette-list")
            .max_h(px(380.))
            .overflow_y_scroll()
            .p_1();
        for (i, (label, hint, cmd)) in self.palette_items().into_iter().take(60).enumerate() {
            list = list.child(
                h_flex()
                    .id(("palette-item", i))
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(i == 0, |s| s.bg(t.list_active))
                    .hover(|s| s.bg(t.list_hover))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)),
                    )
                    .child(div().flex_1().truncate().child(label))
                    .child(div().text_xs().text_color(t.muted_foreground).child(hint)),
            );
        }
        div()
            .absolute()
            .inset_0()
            .bg(hsla(0., 0., 0., 0.35))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(96.))
            .child(
                v_flex()
                    .w(px(600.))
                    .h_auto()
                    .max_h(px(460.))
                    .rounded_xl()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .shadow_lg()
                    .child(
                        div().p_2().border_b_1().border_color(t.border).child(
                            Input::new(&self.palette_input)
                                .appearance(false)
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(list),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (dp, _, dmode) = self.defaults();
        let mut providers = h_flex().gap_1().flex_wrap();
        for p in PROVIDERS {
            let id = p.id.to_string();
            providers = providers.child(
                Button::new(SharedString::from(format!("settings-provider-{}", p.id)))
                    .small()
                    .label(p.label)
                    .selected(dp == p.id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.append(Event::SettingChanged {
                            key: "default_provider".into(),
                            value: id.clone(),
                        });
                        cx.notify();
                    })),
            );
        }
        let mut modes = h_flex().gap_1();
        for (id, label) in MODES {
            let id_s = id.to_string();
            modes = modes.child(
                Button::new(SharedString::from(format!("settings-mode-{id}")))
                    .small()
                    .label(*label)
                    .selected(dmode == *id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.append(Event::SettingChanged {
                            key: "default_mode".into(),
                            value: id_s.clone(),
                        });
                        cx.notify();
                    })),
            );
        }
        let row = |label: &str, desc: &str, control: AnyElement| {
            h_flex()
                .py_3()
                .gap_4()
                .border_b_1()
                .border_color(t.border)
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().text_sm().font_medium().child(label.to_string()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.muted_foreground)
                                .child(desc.to_string()),
                        ),
                )
                .child(control)
        };
        let dark = t.mode.is_dark();
        div()
            .absolute()
            .inset_0()
            .bg(hsla(0., 0., 0., 0.35))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(640.))
                    .rounded_xl()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .shadow_lg()
                    .p_5()
                    .child(
                        h_flex()
                            .pb_2()
                            .child(div().text_lg().font_semibold().child("Settings"))
                            .child(div().flex_1())
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_overlays(window, cx)
                                    })),
                            ),
                    )
                    .child(row(
                        "Theme",
                        "Light or dark appearance",
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("settings-light")
                                    .small()
                                    .icon(IconName::Sun)
                                    .label("Light")
                                    .selected(!dark)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if dark {
                                            this.run(Cmd::ToggleTheme, window, cx);
                                            this.settings_open = true;
                                        }
                                    })),
                            )
                            .child(
                                Button::new("settings-dark")
                                    .small()
                                    .icon(IconName::Moon)
                                    .label("Dark")
                                    .selected(dark)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !dark {
                                            this.run(Cmd::ToggleTheme, window, cx);
                                            this.settings_open = true;
                                        }
                                    })),
                            )
                            .into_any_element(),
                    ))
                    .child(row(
                        "Default provider",
                        "Used for new threads",
                        providers.into_any_element(),
                    ))
                    .child(row(
                        "Default access",
                        "Runtime permission mode for new threads",
                        modes.into_any_element(),
                    ))
                    .child(row(
                        "Background",
                        "Dithered wallpaper behind the chat (custom path: /background <file>)",
                        h_flex()
                            .gap_1()
                            .children(bg_rows(cx, self))
                            .into_any_element(),
                    ))
                    .child(row(
                        "Data directory",
                        "Event log and settings",
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(crate::data_dir().display().to_string())
                            .into_any_element(),
                    )),
            )
    }
}

impl GrayApp {
    fn render_sessions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut rows = v_flex().gap_0p5();
        for gs in &self.gray_sessions {
            let id = gs.id.clone();
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("gsession-{}", gs.id)))
                    .px_3()
                    .py_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .font_family(t.mono_font_family.clone())
                                            .text_xs()
                                            .child(gs.id.clone()),
                                    )
                                    .when(gs.live, |s| {
                                        s.child(
                                            div()
                                                .text_xs()
                                                .font_family(t.mono_font_family.clone())
                                                .text_color(t.success)
                                                .child("LIVE"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .child(if gs.preview.is_empty() {
                                        format!("{} · {}", gs.model, relative(gs.timestamp))
                                    } else {
                                        format!(
                                            "{} · {} · {}",
                                            gs.preview,
                                            gs.model,
                                            relative(gs.timestamp)
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .child(gs.cwd.clone()),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("resume-{}", gs.id)))
                            .small()
                            .label("Resume")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::ResumeSession(id.clone()), window, cx);
                                this.panel = None;
                            })),
                    ),
            );
        }
        v_flex()
            .flex_1()
            .h_full()
            .child(rows)
            .when(self.gray_sessions.is_empty(), |s| {
                s.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .text_color(t.muted_foreground)
                        .child("No Gray sessions found")
                        .child(
                            div()
                                .text_xs()
                                .child("Run gray in a terminal, or send a turn here"),
                        ),
                )
            })
    }

    fn render_viewer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        match &self.viewer {
            Some((rel, st)) => {
                let lang = workspace::language_for(rel);
                let mut crumbs = h_flex().gap_1().items_center().text_xs();
                crumbs = crumbs.child(Icon::new(IconName::File).xsmall());
                let parts: Vec<&str> = rel.split('/').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        crumbs = crumbs.child(
                            Icon::new(IconName::ChevronRight)
                                .xsmall()
                                .text_color(t.muted_foreground),
                        );
                    }
                    let part = part.to_string();
                    crumbs = crumbs.child(
                        div()
                            .when(i < parts.len() - 1, |s| s.text_color(t.muted_foreground))
                            .child(part),
                    );
                }
                let body = v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(st).h_full().bordered(false).readonly(true));
                v_flex()
                    .flex_1()
                    .h_full()
                    .child(
                        h_flex()
                            .h(px(32.))
                            .flex_none()
                            .px_3()
                            .items_center()
                            .border_b_1()
                            .border_color(t.border)
                            .child(crumbs)
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(format!("{lang} · read-only")),
                            )
                            .child(
                                Button::new("viewer-copy")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some((_, st)) = &this.viewer {
                                            let v = st.read(cx).value().to_string();
                                            cx.write_to_clipboard(ClipboardItem::new_string(v));
                                            this.toast("Copied", false);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("viewer-open-editor")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .on_click(cx.listener({
                                        let rel = rel.clone();
                                        move |this, _, window, cx| {
                                            this.open_file(&rel, window, cx);
                                        }
                                    })),
                            ),
                    )
                    .child(body)
                    .into_any_element()
            }
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.muted_foreground)
                .child("Click a file in the tree to preview it")
                .into_any_element(),
        }
    }
}

fn render_item(item: &Item, t: &Theme) -> AnyElement {
    match item {
        Item::Message {
            id,
            role: Role::User,
            text,
            ..
        } => h_flex()
            .justify_end()
            .child(
                div()
                    .id(SharedString::from(format!("msg-{id}")))
                    .max_w(px(620.))
                    .px_4()
                    .py_2()
                    .rounded_2xl()
                    .bg(t.secondary)
                    .text_sm()
                    .child(text.clone()),
            )
            .into_any_element(),
        Item::Message { id, text, .. } => div()
            .text_sm()
            .child(
                TextView::markdown(SharedString::from(format!("msg-{id}")), text.clone())
                    .selectable(true),
            )
            .into_any_element(),
        Item::Activity {
            kind,
            label,
            detail,
            ..
        } => {
            let (icon, color) = match kind.as_str() {
                "error" => (IconName::CircleX, t.danger),
                "edit" => (IconName::FileText, t.success),
                "command" => (IconName::SquareTerminal, t.muted_foreground),
                "checkpoint" => (IconName::Undo2, t.muted_foreground),
                "thinking" => (IconName::Info, t.muted_foreground),
                _ => (IconName::Check, t.muted_foreground),
            };
            let short: String = detail
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            let short = if kind == "checkpoint" {
                short.chars().take(10).collect()
            } else {
                short
            };
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(color)
                .child(icon)
                .child(div().font_family(t.mono_font_family.clone()).child(format!(
                    "{} · {}",
                    kind.to_uppercase(),
                    label
                )))
                .child(div().truncate().text_color(t.muted_foreground).child(short))
                .into_any_element()
        }
    }
}

fn background_layer(bg: &str) -> Div {
    div().absolute().inset_0().overflow_hidden().child(
        match bg {
            "andromeda" => img("bg/andromeda.png"),
            "bluemarble" => img("bg/bluemarble.png"),
            path => img(PathBuf::from(path)),
        }
        .w_full()
        .h_full()
        .opacity(0.3)
        .grayscale(true),
    )
}

fn bg_rows(cx: &mut Context<GrayApp>, app: &GrayApp) -> Vec<AnyElement> {
    let cur = app.store.state.setting("background", "none");
    [
        ("none", "None"),
        ("andromeda", "Andromeda"),
        ("bluemarble", "Blue Marble"),
    ]
    .into_iter()
    .map(|(id, label)| {
        let id = id.to_string();
        Button::new(SharedString::from(format!("bg-{}", id)))
            .small()
            .label(label)
            .selected(cur == id)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run(Cmd::SetBackground(id.clone()), window, cx);
                this.settings_open = true;
            }))
            .into_any_element()
    })
    .collect()
}

fn relative(ms: i64) -> String {
    let secs = ((now() - ms) / 1000).max(0);
    if secs < 60 {
        "now".into()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

impl Render for GrayApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.write_snapshot(cx);
        let t = cx.theme().clone();
        let _ = window;
        let mut center = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(cx))
            .child(self.render_timeline(cx))
            .child(self.render_composer(cx));
        if self.terminal_open {
            center = center.child(self.render_terminal(cx));
        }
        let mut main = h_flex().size_full().min_h_0();
        if self.sidebar_open {
            main = main.child(self.render_sidebar(cx));
        }
        main = main.child(center);
        if let Some(p) = self.panel {
            main = main.child(self.render_panel(p, cx));
        }
        let bg = self.store.state.setting("background", "none");
        div()
            .id("gray-root")
            .key_context("GrayApp")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font_family.clone())
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, w, cx| this.run(Cmd::ToggleSidebar, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleTerminal, w, cx| this.run(Cmd::ToggleTerminal, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleDiff, w, cx| this.run(Cmd::ToggleDiff, w, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleFiles, w, cx| this.run(Cmd::ToggleFiles, w, cx)),
            )
            .on_action(cx.listener(|this, _: &NewThread, w, cx| this.run(Cmd::NewThread, w, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, w, cx| this.run(Cmd::Settings, w, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, w, cx| this.run(Cmd::ToggleTheme, w, cx)),
            )
            .on_action(cx.listener(|this, _: &StopTurn, w, cx| this.run(Cmd::Stop, w, cx)))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.save_file(cx)))
            .on_action(cx.listener(|this, _: &FocusComposer, w, cx| this.close_overlays(w, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, w, cx| {
                this.toast = None;
                this.close_overlays(w, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePalette, w, cx| {
                if this.palette_open {
                    this.close_overlays(w, cx);
                } else {
                    this.open_palette(PaletteMode::Commands, "", w, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FilePicker, w, cx| {
                this.open_palette(PaletteMode::Files, "", w, cx)
            }))
            .on_action(cx.listener(|this, _: &ModelPicker, w, cx| {
                this.open_palette(PaletteMode::Commands, "Model:", w, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePluginsPanel, w, cx| {
                this.run(Cmd::TogglePlugins, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleViewer, w, cx| this.run(Cmd::ToggleViewer, w, cx)),
            )
            .on_action(cx.listener(|this, _: &EffortPicker, w, cx| {
                this.open_palette(PaletteMode::Commands, "Effort:", w, cx)
            }))
            .on_action(cx.listener(|this, _: &ModePicker, w, cx| {
                this.open_palette(PaletteMode::Commands, "Mode:", w, cx)
            }))
            .on_action(cx.listener(|this, _: &BranchPicker, w, cx| {
                this.open_palette(PaletteMode::Commands, "Branch:", w, cx)
            }))
            .child(main)
            .when(bg != "none", |s| s.child(background_layer(&bg)))
            .when(self.palette_open, |s| s.child(self.render_palette(cx)))
            .when(self.settings_open, |s| s.child(self.render_settings(cx)))
            .when_some(self.toast.clone(), |s, (msg, err)| {
                s.child(
                    div()
                        .id("toast")
                        .absolute()
                        .bottom_4()
                        .right_4()
                        .max_w(px(420.))
                        .px_4()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(if err { t.danger } else { t.border })
                        .bg(t.popover)
                        .shadow_lg()
                        .text_sm()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toast = None;
                            cx.notify();
                        }))
                        .child(msg),
                )
            })
    }
}

fn expand_mentions(text: &str, root: &Path) -> String {
    let mut out = text.to_string();
    let mut seen = HashSet::new();
    for word in text.split_whitespace() {
        let Some(rel) = word.strip_prefix('@') else {
            continue;
        };
        let rel = rel.trim_end_matches([',', '.', ';', ':', ')']);
        if rel.is_empty() || rel.contains("..") || !seen.insert(rel.to_string()) {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(root.join(rel)) {
            let lang = workspace::language_for(rel);
            out.push_str(&format!(
                "\n\n<file path=\"{rel}\">\n```{lang}\n{content}\n```\n</file>"
            ));
        }
    }
    out
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

pub fn apply_brand(cx: &mut App) {
    let dark = cx.theme().mode.is_dark();
    let t = Theme::global_mut(cx);
    t.font_family = "Plus Jakarta Sans".into();
    t.mono_font_family = "Departure Mono".into();
    let accent = hex(0xd4a373);
    t.primary = accent;
    t.primary_hover = hex(0xdfb891);
    t.primary_active = hex(0xe8cdb0);
    t.ring = accent;
    t.caret = accent;
    t.button_primary = accent;
    t.button_primary_hover = hex(0xdfb891);
    t.button_primary_active = hex(0xe8cdb0);
    t.danger = hex(0xb7513f);
    t.warning = accent;
    t.radius = px(4.);
    t.radius_lg = px(6.);
    if !dark {
        t.primary_foreground = hex(0x050506);
        t.button_primary_foreground = hex(0x050506);
        t.tokens = gpui_kit::component::ThemeTokens::from(&t.colors);
        return;
    }
    t.background = hex(0x050506);
    t.foreground = hex(0xeceae7);
    t.muted = hex(0x131316);
    t.muted_foreground = hex(0x8b8a86);
    t.border = hex(0x1c1c20);
    t.input = hex(0x2a2a30);
    t.popover = hex(0x0a0a0b);
    t.popover_foreground = hex(0xeceae7);
    t.secondary = hex(0x131316);
    t.secondary_hover = hex(0x1c1c20);
    t.secondary_active = hex(0x2a2a30);
    t.secondary_foreground = hex(0xeceae7);
    t.accent = hex(0x131316);
    t.accent_foreground = hex(0xeceae7);
    t.sidebar = hex(0x0a0a0b);
    t.sidebar_foreground = hex(0xeceae7);
    t.sidebar_border = hex(0x1c1c20);
    t.sidebar_accent = hex(0x131316);
    t.sidebar_accent_foreground = hex(0xeceae7);
    t.list_hover = hex(0x0e0e10);
    t.list_active = hex(0x1c1c20);
    t.list_active_border = accent;
    t.tab_active = hex(0x0e0e10);
    t.tab_active_foreground = hex(0xeceae7);
    t.selection = hex(0x3a3a42);
    t.primary_foreground = hex(0x050506);
    t.button_primary_foreground = hex(0x050506);
    t.tokens = gpui_kit::component::ThemeTokens::from(&t.colors);
}
