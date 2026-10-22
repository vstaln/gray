use crate::git;
use crate::graycmds;
use crate::motion;
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
use gpui_kit::component::{ResizableState, h_resizable, resizable_panel};
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
    Skills,
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
    ToggleSkills,
    UseSkill(String),
    ResumeSession(String),
    DiscardChanges,
    Gray(String),
    TurnDiff,
    HeadDiff,
    Approve(bool),
    AttachImage(String),
    RemoveAttachment(usize),
    SettingsSection(String),
    SetLayout(String),
    SetTerminalPlacement(String),
    SetAutoAccept(bool),
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
    settings_search: Entity<InputState>,
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
    terminal_rx: Vec<Receiver<String>>,
    scroll: ScrollHandle,
    expanded: HashSet<String>,
    tree: Vec<FileEntry>,
    tabs: Vec<Tab>,
    viewer: Option<(String, Entity<EditorState>)>,
    gray_sessions: Vec<GraySession>,
    gray_cmds: Vec<(String, String)>,
    pending_approval: Option<(String, String)>,
    attachments: Vec<PathBuf>,
    skills: Vec<crate::skills::Skill>,
    settings_section: String,
    settings_query: String,
    pane_sizes: Vec<f32>,
    gray_cmd_rx: Receiver<Vec<(String, String)>>,
    active_tab: Option<usize>,
    diff: Option<Entity<DiffState>>,
    diff_base: Option<String>,
    diff_files: Vec<String>,
    branch: Option<String>,
    branches: Vec<String>,
    toast: Option<(String, bool)>,
    state_file: Option<PathBuf>,
    last_snapshot: String,
    _subs: Vec<Subscription>,
    ticks: u64,
    window_handle: Option<gpui::AnyWindowHandle>,
    pending_cmds: Vec<String>,
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
        let settings_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));

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
            &settings_search,
            window,
            |this, state, _ev: &InputEvent, _window, cx| {
                this.settings_query = state.read(cx).value().to_string();
                cx.notify();
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
        let (gray_cmd_tx, gray_cmd_rx) = channel::<Vec<(String, String)>>();
        std::thread::spawn(move || {
            let _ = gray_cmd_tx.send(graycmds::list());
        });
        let mut app = Self {
            focus: cx.focus_handle(),
            store,
            active_project: None,
            active_thread: None,
            composer,
            palette_input,
            terminal_input,
            settings_search,
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
            terminal_rx: Vec::new(),
            scroll: ScrollHandle::new(),
            expanded: HashSet::new(),
            tree: Vec::new(),
            tabs: Vec::new(),
            viewer: None,
            gray_sessions: Vec::new(),
            gray_cmds: Vec::new(),
            pending_approval: None,
            attachments: Vec::new(),
            skills: Vec::new(),
            settings_section: "preferences".into(),
            settings_query: String::new(),
            pane_sizes: Vec::new(),
            gray_cmd_rx,
            window_handle: None,
            pending_cmds: Vec::new(),
            active_tab: None,
            diff: None,
            diff_base: None,
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
        app.window_handle = Some(window.window_handle());
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
        self.persist(Event::ProjectAdded {
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
            let text = match &self.diff_base {
                Some(base) => git::diff_since(&root, base),
                None => git::diff(&root),
            };
            let files = DiffFile::parse(&text).unwrap_or_default();
            self.diff_files = match &self.diff_base {
                Some(base) => git::files_since(&root, base),
                None => git::status(&root).into_iter().map(|(_, p)| p).collect(),
            };
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
        let mode = if provider == "gray" {
            "full".to_string()
        } else {
            mode
        };
        (provider, model, mode)
    }

    fn create_thread(&mut self, cx: &mut Context<Self>) -> Option<String> {
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

    fn poll(&mut self, cx: &mut Context<Self>) {
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
            let text = self.turns.remove(&tid).map(|a| a.text).unwrap_or_default();
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
                    self.toast(
                        format!("Turn failed: {}", e.lines().next().unwrap_or("")),
                        true,
                    );
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
        if changed {
            self.scroll.scroll_to_bottom();
            cx.notify();
        } else if !self.turns.is_empty() && self.ticks % 3 == 0 {
            // Keep the elapsed-time label ticking between stream events
            // (and under reduced motion, when no animation frames run).
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
        self.terminal_rx.push(workspace::run_shell(&root, &cmd));
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
        self.settings_open = false;
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
            items.push((
                format!("Effort: {e}"),
                k("Ctrl+Shift+E"),
                Cmd::Effort(e.into()),
            ));
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

    fn persist(&mut self, e: Event) {
        if let Err(err) = self.store.append(e) {
            self.toast(format!("Couldn't save: {err}"), true);
        }
    }

    fn dirty_tabs(&self, cx: &App) -> usize {
        self.tabs
            .iter()
            .filter(|t| t.editor.read(cx).value() != t.saved)
            .count()
    }

    fn configure(&mut self, provider: Option<String>, model: Option<String>, mode: Option<String>) {
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

    fn reload_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            "terminal_placement": self.store.state.setting("terminal_placement", "bottom"),
            "pane_sizes": self.pane_sizes,
            "attachments": self.attachments.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "skills": self.skills.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
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
        .hover(|s| s.bg(t.secondary_hover))
        .active(|s| s.bg(t.secondary_active));
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
        .active(|s| s.bg(t.secondary_active))
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
            return motion::enter(
                v_flex()
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
                    ),
                "welcome-in",
                motion::WELCOME_MS,
                -8.,
                motion::ease_out(),
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
                            .child(motion::churner(&t))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .font_family(t.mono_font_family.clone())
                                    .child(format!(
                                        "{} ·",
                                        provider::provider_label(&thread.provider)
                                    ))
                                    .child(motion::shimmer_label("churn-label", "CHURNING", &t))
                                    .child(format!("{:.1}S", a.started.elapsed().as_secs_f32())),
                            ),
                    )
                    .map(|s| {
                        if a.text.is_empty() {
                            s.child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(motion::typing_dots(&t))
                                    .child(motion::breathe(
                                        div()
                                            .text_sm()
                                            .text_color(t.muted_foreground)
                                            .child("Thinking"),
                                        "think-pulse",
                                    )),
                            )
                        } else {
                            s.child(
                                TextView::markdown("streaming", a.text.clone()).selectable(true),
                            )
                        }
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

    fn render_approval(&self, label: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div()
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .px_6()
            .pb_2()
            .child(motion::enter(
                h_flex()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .border_1()
                    .border_color(t.warning)
                    .bg(t.warning.opacity(0.08))
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(t.warning)
                            .child(format!("Approve? {label}")),
                    )
                    .child(
                        Button::new("approve")
                            .primary()
                            .small()
                            .label("Approve")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Approve(true), window, cx)
                            })),
                    )
                    .child(
                        Button::new("reject")
                            .ghost()
                            .small()
                            .label("Reject")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Approve(false), window, cx)
                            })),
                    ),
                SharedString::from(format!("approval-in-{label}")),
                motion::CARD_MS,
                8.,
                motion::ease_out(),
            ))
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
                    .children(
                        self.attachments
                            .iter()
                            .enumerate()
                            .map(|(i, path)| {
                                h_flex()
                                    .id(SharedString::from(format!("attachment-{i}")))
                                    .px_2()
                                    .pt_2()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .h(px(44.))
                                            .rounded_md()
                                            .border_1()
                                            .border_color(t.border)
                                            .overflow_hidden()
                                            .child(
                                                img(path.clone())
                                                    .w(px(44.))
                                                    .h(px(44.))
                                                    .object_fit(ObjectFit::Cover),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(t.muted_foreground)
                                            .child(
                                                path.file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_default(),
                                            ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "attach-remove-{i}"
                                        )))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.run(Cmd::RemoveAttachment(i), window, cx)
                                            }),
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>(),
                    )
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

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut icons = v_flex()
            .id("rail-scroll")
            .flex_1()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .items_center();
        for p in &self.store.state.projects {
            let pid = p.id.clone();
            let active = Some(&p.id) == self.active_project.as_ref();
            icons = icons.child(
                Button::new(SharedString::from(format!("rail-project-{}", p.id)))
                    .ghost()
                    .small()
                    .icon(if active {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    })
                    .tooltip(p.name.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Project(pid.clone()), window, cx)
                    })),
            );
            if active {
                let threads: Vec<_> = self
                    .store
                    .state
                    .threads_for(&p.id, self.show_archived)
                    .into_iter()
                    .filter(|t| self.show_archived || t.snoozed_until <= now())
                    .collect();
                for th in threads {
                    let id = th.id.clone();
                    let sel = Some(&th.id) == self.active_thread.as_ref();
                    let color = if self.turns.contains_key(&th.id) {
                        t.info
                    } else if th.last_status == "error" {
                        t.danger
                    } else if th.last_status == "completed" {
                        t.success
                    } else {
                        t.muted_foreground
                    };
                    icons = icons.child(
                        div()
                            .id(SharedString::from(format!("rail-thread-{}", th.id)))
                            .w(px(10.))
                            .h(px(10.))
                            .rounded_full()
                            .bg(if sel { color } else { color.opacity(0.55) })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::Thread(id.clone()), window, cx)
                            })),
                    );
                }
            }
        }
        let icon_btn = |id: &'static str,
                        icon: IconName,
                        tip: &'static str,
                        cmd: Cmd,
                        cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)))
        };
        v_flex()
            .w(px(52.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .child(icons)
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .items_center()
                    .border_t_1()
                    .border_color(t.sidebar_border)
                    .child(icon_btn(
                        "rail-new",
                        IconName::Plus,
                        "New thread",
                        Cmd::NewThread,
                        cx,
                    ))
                    .child(
                        Button::new("rail-palette")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .tooltip("Commands (Ctrl+K)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_palette(PaletteMode::Commands, "", window, cx)
                            })),
                    )
                    .child(icon_btn(
                        "rail-files",
                        IconName::FileText,
                        "Files (Ctrl+E)",
                        Cmd::ToggleFiles,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-diff",
                        IconName::Copy,
                        "Diff (Ctrl+D)",
                        Cmd::ToggleDiff,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-terminal",
                        IconName::SquareTerminal,
                        "Terminal (Ctrl+J)",
                        Cmd::ToggleTerminal,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-plugins",
                        IconName::Star,
                        "Plugins (Ctrl+Shift+P)",
                        Cmd::TogglePlugins,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-sessions",
                        IconName::Inbox,
                        "Sessions",
                        Cmd::ToggleSessions,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-skills",
                        IconName::BookOpen,
                        "Skills",
                        Cmd::ToggleSkills,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-settings",
                        IconName::Settings,
                        "Settings (Ctrl+,)",
                        Cmd::Settings,
                        cx,
                    )),
            )
    }

    fn render_terminal(&self, side: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut lines = v_flex().font_family(t.mono_font_family.clone()).text_xs();
        for l in self.terminal_lines.iter().rev().take(300).rev() {
            lines = lines.child(div().child(l.clone()));
        }
        let mut outer = v_flex().flex_none().border_color(t.border).bg(t.background);
        outer = if side {
            outer.w(px(300.)).h_full().border_l_1()
        } else {
            outer.h(px(240.)).border_t_1()
        };
        outer
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
                    .when(!self.terminal_rx.is_empty(), |s| {
                        s.child(motion::running_bars(&t))
                    })
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
            Panel::Skills => self.render_skills(cx).into_any_element(),
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
                    .active(|s| s.bg(t.list_active))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)),
                    )
                    .child(div().flex_1().truncate().child(label))
                    .child(div().text_xs().text_color(t.muted_foreground).child(hint)),
            );
        }
        let backdrop = div()
            .absolute()
            .inset_0()
            .bg(hsla(0., 0., 0., 0.35))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(96.))
            .child(motion::enter(
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
                "palette-in",
                motion::POPOVER_MS,
                6.,
                motion::ease_pop(),
            ));
        motion::fade_in(backdrop, "palette-backdrop-in", motion::BACKDROP_MS)
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (dp, _, dmode) = self.defaults();
        let layout = self.store.state.setting("layout", "focus");
        let auto_accept = self.store.state.setting("auto_accept", "false") == "true";
        let term_side = self.store.state.setting("terminal_placement", "bottom") == "right";
        let q = self.settings_query.to_lowercase();
        let vis = |label: &str| q.is_empty() || fuzzy(&label.to_lowercase(), &q);
        let nav = |_id: &'static str,
                   icon: IconName,
                   label: &'static str,
                   section: &'static str,
                   cx: &mut Context<Self>| {
            let sel = self.settings_section == section;
            let sec = section.to_string();
            h_flex()
                .id(SharedString::from(format!("settings-nav-{section}")))
                .px_3()
                .py_1p5()
                .gap_2()
                .rounded_md()
                .text_sm()
                .text_color(if sel {
                    t.foreground
                } else {
                    t.muted_foreground
                })
                .cursor_pointer()
                .hover(|s| s.bg(t.sidebar_accent))
                .when(sel, |s| s.bg(t.sidebar_accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run(Cmd::SettingsSection(sec.clone()), window, cx)
                }))
                .child(Icon::new(icon).xsmall())
                .child(div().flex_1().child(label))
        };
        let mut nav_col = v_flex()
            .w(px(230.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .id("settings-back")
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.foreground))
                    .on_click(cx.listener(|this, _, window, cx| this.close_overlays(window, cx)))
                    .child(Icon::new(IconName::ArrowLeft).xsmall())
                    .child("Back to app"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .child(Input::new(&self.settings_search).small()),
            )
            .child(nav(
                "nav-preferences",
                IconName::Settings,
                "Preferences",
                "preferences",
                cx,
            ))
            .child(nav(
                "nav-appearance",
                IconName::Palette,
                "Appearance",
                "appearance",
                cx,
            ))
            .child(nav(
                "nav-shortcuts",
                IconName::LayoutDashboard,
                "Shortcuts",
                "shortcuts",
                cx,
            ));
        nav_col = nav_col.child(
            div()
                .px_3()
                .pt_4()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(t.muted_foreground)
                .child("PROJECTS"),
        );
        for p in &self.store.state.projects {
            let pid = p.id.clone();
            let sel = Some(&p.id) == self.active_project.as_ref();
            nav_col = nav_col.child(
                h_flex()
                    .id(SharedString::from(format!("settings-project-{}", p.id)))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .rounded_md()
                    .text_sm()
                    .text_color(if sel {
                        t.foreground
                    } else {
                        t.muted_foreground
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(t.sidebar_accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Project(pid.clone()), window, cx)
                    }))
                    .child(Icon::new(IconName::Folder).xsmall())
                    .child(div().flex_1().truncate().child(p.name.clone())),
            );
        }
        nav_col = nav_col.child(div().flex_1()).child(nav(
            "nav-about",
            IconName::Info,
            "About",
            "about",
            cx,
        ));

        let row = |label: &str, desc: &str, control: AnyElement| {
            h_flex()
                .py_3()
                .gap_4()
                .border_b_1()
                .border_color(t.border)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
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
        let opt = |id: SharedString, label: &str, sel: bool| {
            Button::new(id)
                .small()
                .label(label.to_string())
                .selected(sel)
        };

        let mut providers = h_flex().gap_1().flex_wrap();
        for prov in PROVIDERS {
            let id = prov.id.to_string();
            providers = providers.child(
                opt(
                    SharedString::from(format!("settings-provider-{}", prov.id)),
                    prov.label,
                    dp == prov.id,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.persist(Event::SettingChanged {
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
                opt(
                    SharedString::from(format!("settings-mode-{id}")),
                    label,
                    dmode == *id,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.persist(Event::SettingChanged {
                        key: "default_mode".into(),
                        value: id_s.clone(),
                    });
                    cx.notify();
                })),
            );
        }
        let dark = t.mode.is_dark();
        let mut content = v_flex()
            .id("settings-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_8();
        let mut page = v_flex().w(px(720.)).gap_1();
        let (title, subtitle) = match self.settings_section.as_str() {
            "appearance" => ("Appearance", "Theme and background"),
            "shortcuts" => ("Shortcuts", "Keyboard shortcuts"),
            "about" => ("About", "Version and data"),
            _ => ("Preferences", "Customize layout and default behavior"),
        };
        page = page
            .child(div().text_xl().font_semibold().child(title))
            .child(
                div()
                    .pb_4()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child(subtitle),
            );

        match self.settings_section.as_str() {
            "appearance" => {
                page = page
                    .when(vis("Theme"), |s| {
                        s.child(row(
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
                                            }
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Background"), |s| {
                        s.child(row(
                            "Background",
                            "Dithered wallpaper (custom: /background <file>)",
                            h_flex()
                                .gap_1()
                                .children(bg_rows(cx, self))
                                .into_any_element(),
                        ))
                    });
            }
            "shortcuts" => {
                for (keys, desc) in [
                    ("Ctrl K", "Command palette"),
                    ("Ctrl P", "File picker"),
                    ("Ctrl N", "New thread"),
                    ("Ctrl B", "Toggle sidebar"),
                    ("Ctrl E", "Files panel"),
                    ("Ctrl D", "Diff panel"),
                    ("Ctrl J", "Terminal drawer"),
                    ("Ctrl S", "Save editor file"),
                    ("Ctrl ,", "Settings"),
                    ("Ctrl Shift P", "Plugins"),
                    ("Ctrl Shift V", "Viewer"),
                    ("Ctrl Shift L", "Toggle theme"),
                    ("Ctrl Shift E", "Next effort"),
                    ("Enter", "Send message"),
                    ("Shift Enter", "New line in composer"),
                    ("Esc", "Close panel or overlay"),
                ] {
                    page = page.child(row(
                        desc,
                        "",
                        div()
                            .font_family(t.mono_font_family.clone())
                            .text_xs()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(t.border)
                            .child(keys)
                            .into_any_element(),
                    ));
                }
            }
            "about" => {
                page = page
                    .child(row(
                        "Version",
                        "gray-ide",
                        div()
                            .text_sm()
                            .text_color(t.muted_foreground)
                            .child(env!("CARGO_PKG_VERSION"))
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
                    ))
                    .child(row(
                        "Providers",
                        "Supported agent CLIs",
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(
                                PROVIDERS
                                    .iter()
                                    .map(|p| p.label)
                                    .collect::<Vec<_>>()
                                    .join(" \u{00b7} "),
                            )
                            .into_any_element(),
                    ));
            }
            _ => {
                let bar = |w: f32, h: f32, c: Hsla| div().w(px(w)).h(px(h)).rounded_sm().bg(c);
                let mut cards = h_flex().gap_4().pb_4();
                for (id, name, desc, sketch) in [
                    (
                        "focus",
                        "Focus",
                        "Roomy layout with the full sidebar. Made for prompting.",
                        h_flex()
                            .h(px(86.))
                            .w_full()
                            .gap_1()
                            .p_1p5()
                            .rounded_md()
                            .bg(t.muted.opacity(0.25))
                            .child(div().w(px(54.)).h_full().rounded_sm().bg(t.sidebar_accent))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(bar(f32::MAX, 9., t.sidebar_accent))
                                    .child(bar(110., 6., t.muted.opacity(0.6)))
                                    .child(bar(80., 6., t.muted.opacity(0.6)))
                                    .child(bar(140., 6., t.muted.opacity(0.6)))
                                    .child(div().flex_1().rounded_sm().bg(t.sidebar_accent)),
                            )
                            .into_any_element(),
                    ),
                    (
                        "pro",
                        "Pro",
                        "Compact layout with an icon-first rail and minimal chrome.",
                        h_flex()
                            .h(px(86.))
                            .w_full()
                            .gap_1()
                            .p_1p5()
                            .rounded_md()
                            .bg(t.muted.opacity(0.25))
                            .child(div().w(px(10.)).h_full().rounded_sm().bg(t.sidebar_accent))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(bar(f32::MAX, 7., t.sidebar_accent))
                                    .child(bar(120., 6., t.muted.opacity(0.6)))
                                    .child(bar(90., 6., t.muted.opacity(0.6)))
                                    .child(bar(150., 6., t.muted.opacity(0.6)))
                                    .child(div().flex_1().rounded_sm().bg(t.sidebar_accent)),
                            )
                            .into_any_element(),
                    ),
                ] {
                    let sel = layout == id;
                    let id_s = id.to_string();
                    cards = cards.child(
                        v_flex()
                            .id(SharedString::from(format!("layout-card-{id}")))
                            .w(px(320.))
                            .p_3()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(if sel { t.info } else { t.border })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::SetLayout(id_s.clone()), window, cx)
                            }))
                            .child(sketch)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(14.))
                                            .h(px(14.))
                                            .rounded_full()
                                            .border_2()
                                            .border_color(if sel {
                                                t.info
                                            } else {
                                                t.muted_foreground
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(sel, |s| {
                                                s.child(
                                                    div()
                                                        .w(px(7.))
                                                        .h(px(7.))
                                                        .rounded_full()
                                                        .bg(t.info),
                                                )
                                            }),
                                    )
                                    .child(div().text_sm().font_semibold().child(name)),
                            )
                            .child(div().text_xs().text_color(t.muted_foreground).child(desc)),
                    );
                }
                page = page
                    .when(vis("Layout"), |s| {
                        s.child(div().pb_1().text_sm().font_semibold().child("Layout"))
                            .child(cards)
                    })
                    .when(vis("Default provider"), |s| {
                        s.child(row(
                            "Default provider",
                            "Used for new threads",
                            providers.into_any_element(),
                        ))
                    })
                    .when(vis("Default access"), |s| {
                        s.child(row(
                            "Default access",
                            "Runtime permission mode for new threads",
                            modes.into_any_element(),
                        ))
                    })
                    .when(vis("Auto-accept permissions"), |s| {
                        s.child(row(
                            "Auto-accept permissions",
                            "Approve permission requests without asking",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-autoaccept-on")
                                        .small()
                                        .label("On")
                                        .selected(auto_accept)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetAutoAccept(true), window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("settings-autoaccept-off")
                                        .small()
                                        .label("Off")
                                        .selected(!auto_accept)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetAutoAccept(false), window, cx)
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Terminal placement"), |s| {
                        s.child(row(
                            "Terminal placement",
                            "Where the terminal drawer lives",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-term-bottom")
                                        .small()
                                        .icon(IconName::PanelBottom)
                                        .label("Bottom")
                                        .selected(!term_side)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(
                                                Cmd::SetTerminalPlacement("bottom".into()),
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new("settings-term-right")
                                        .small()
                                        .icon(IconName::PanelRight)
                                        .label("Right")
                                        .selected(term_side)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(
                                                Cmd::SetTerminalPlacement("right".into()),
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Language"), |s| {
                        s.child(row(
                            "Language",
                            "Display language",
                            div()
                                .text_sm()
                                .text_color(t.muted_foreground)
                                .child("English")
                                .into_any_element(),
                        ))
                    });
            }
        }
        content = content.child(page);
        motion::fade_in(
            div()
                .id("settings-page")
                .absolute()
                .inset_0()
                .bg(t.background)
                .child(motion::enter(
                    h_flex().size_full().child(nav_col).child(content),
                    "settings-panel-in",
                    motion::MODAL_MS,
                    -8.,
                    motion::ease_pop(),
                )),
            "settings-fade-in",
            motion::BACKDROP_MS,
        )
    }
}

impl GrayApp {
    fn render_skills(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut rows = v_flex().gap_0p5();
        for sk in &self.skills {
            let name = sk.name.clone();
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("skill-{}", sk.name)))
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
                                            .child(sk.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(t.mono_font_family.clone())
                                            .text_color(t.info)
                                            .child(sk.source.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .child(sk.description.clone()),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("use-skill-{}", sk.name)))
                            .ghost()
                            .xsmall()
                            .label("Use")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::UseSkill(name.clone()), window, cx)
                            })),
                    ),
            );
        }
        if self.skills.is_empty() {
            rows = rows.child(
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No skills found. Add SKILL.md files under .gray/skills/ in this project or ~/.gray/skills/."),
            );
        }
        rows
    }

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
        if !self.pending_cmds.is_empty() {
            let lines = std::mem::take(&mut self.pending_cmds);
            for line in lines {
                self.composer
                    .update(cx, |s, cx| s.set_value(line, window, cx));
                self.send(window, cx);
            }
        }
        self.write_snapshot(cx);
        let t = cx.theme().clone();
        let _ = window;
        let mut center = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(cx))
            .child(self.render_timeline(cx))
            .children(
                self.pending_approval
                    .as_ref()
                    .map(|(_, label)| self.render_approval(label, cx)),
            )
            .child(self.render_composer(cx));
        let side_terminal = self.terminal_open
            && self.store.state.setting("terminal_placement", "bottom") == "right";
        if self.terminal_open && !side_terminal {
            center = center.child(self.render_terminal(false, cx));
        }
        let mut center_outer = h_flex().flex_1().min_w_0().h_full().child(center);
        if side_terminal {
            center_outer = center_outer.child(self.render_terminal(true, cx));
        }
        let pro = self.store.state.setting("layout", "focus") == "pro";
        let mut main = h_resizable("main-split").on_resize(cx.listener(
            |this, state: &Entity<ResizableState>, _w, cx| {
                let sizes: Vec<f32> = state
                    .read(cx)
                    .sizes()
                    .iter()
                    .map(|p| f32::from(*p))
                    .collect();
                if sizes != this.pane_sizes {
                    this.pane_sizes = sizes;
                }
            },
        ));
        if pro {
            main = main.child(
                resizable_panel()
                    .visible(self.sidebar_open)
                    .size(px(52.))
                    .size_range(px(52.)..px(52.))
                    .child(self.render_rail(cx)),
            );
        } else {
            main = main.child(
                resizable_panel()
                    .visible(self.sidebar_open)
                    .size(px(232.))
                    .size_range(px(160.)..px(460.))
                    .child(self.render_sidebar(cx)),
            );
        }
        main = main.child(resizable_panel().child(center_outer));
        if let Some(p) = self.panel {
            main = main.child(
                resizable_panel()
                    .size(px(400.))
                    .size_range(px(260.)..px(760.))
                    .child(self.render_panel(p, cx)),
            );
        }
        let main = div().size_full().min_w_0().min_h_0().child(main);
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
        let full = root.join(rel);
        let inside = full
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .map(|(f, r)| f.starts_with(&r))
            .unwrap_or(false);
        if inside && let Ok(content) = std::fs::read_to_string(&full) {
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
