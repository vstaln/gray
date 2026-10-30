use crate::git;
use crate::graycmds;
use crate::icons::GIcon;
use crate::motion;
use crate::plugins::{self, HubEntry, Installed, PluginMsg};
use crate::provider::{self, MODES, PROVIDERS, ProviderEvent, Turn, TurnRequest};
use crate::sessions::{self, GraySession};
use crate::store::{Event, Item, Role, Store, new_id, now};
use crate::workspace::{self, FileEntry};
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState,
};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, StyledExt, Theme, ThemeMode, h_flex, v_flex,
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
        ToggleViewer,
        SearchThreads
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
        KeyBinding::new("ctrl-shift-f", SearchThreads, None),
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
    Search,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PopKind {
    Mention,
    Slash,
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
    OpenMessage(String, Option<usize>),
    SetNotify(bool),
    SearchThreads,
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
    /// Highlighted palette row (arrow keys / mouse); Enter runs it.
    palette_sel: usize,
    palette_scroll: ScrollHandle,
    /// Scroll the highlighted row into view on the next render.
    palette_reveal: std::cell::Cell<bool>,
    /// Composer popover anchored above the input: @-mention file picker or
    /// slash-command menu. The token is re-read from the caret on every use.
    composer_pop: Option<PopKind>,
    pop_sel: usize,
    pop_scroll: ScrollHandle,
    pop_reveal: std::cell::Cell<bool>,
    /// Model picker popover above the composer's model chip.
    model_open: bool,
    model_input: Entity<InputState>,
    model_query: String,
    model_sel: usize,
    /// Provider filter from the popover's rail (`None` = all providers).
    model_rail: Option<&'static str>,
    model_scroll: ScrollHandle,
    model_reveal: std::cell::Cell<bool>,
    /// When a click outside closed the picker: that same click landing on the
    /// model chip must not reopen it.
    model_closed_at: Option<Instant>,
    /// Threads whose turn finished while off screen: the sidebar shows a
    /// check until the thread is opened.
    done_unseen: HashSet<String>,
    sidebar_open: bool,
    terminal_open: bool,
    settings_open: bool,
    show_archived: bool,
    panel: Option<Panel>,
    turns: HashMap<String, ActiveTurn>,
    terminal_lines: Vec<String>,
    terminal_rx: Vec<Receiver<String>>,
    scroll: ScrollHandle,
    /// Stick the timeline to the bottom as content streams in. Cleared when
    /// the user scrolls away from the bottom, set again when they return.
    follow: bool,
    last_offset: Pixels,
    /// Timeline max scroll offset at the last poll; re-pin only when it grows.
    last_max: Pixels,
    follow_thread: Option<String>,
    /// User overrides of a tool group's fold state (default: open while live).
    group_open: HashMap<String, bool>,
    /// (thread id, item count when it was opened): only items past that count
    /// get the rise-in, so switching threads never replays it.
    rise_base: std::cell::RefCell<Option<(String, usize)>>,
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
    toast_at: Option<Instant>,
    /// Bumps on every toast so a new message replays the card's slide-in.
    toast_seq: u64,
    toast_queue: Vec<(String, bool)>,
    /// Set when a send emptied the welcome slot: the composer slides into its
    /// dock on the next render (monocode `useComposerDockMotion`).
    dock_slide_at: Option<Instant>,
    items_scroll: ScrollHandle,
    scroll_target: Option<(String, usize, u8)>,
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
        let model_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let settings_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &composer,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { shift: false, .. } => {
                    if !this.accept_pop(window, cx) {
                        this.send(window, cx);
                    }
                }
                InputEvent::Change => this.update_composer_pop(cx),
                InputEvent::Blur => {
                    this.composer_pop = None;
                    cx.notify();
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &palette_input,
            window,
            |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.palette_query = state.read(cx).value().to_string();
                    this.palette_sel = 0;
                    this.palette_reveal.set(true);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let items = this.palette_items();
                    let ix = this.palette_sel.min(items.len().saturating_sub(1));
                    if let Some((_, _, cmd)) = items.into_iter().nth(ix) {
                        this.run(cmd, window, cx);
                    }
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &model_input,
            window,
            |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.model_query = state.read(cx).value().to_string();
                    this.model_sel = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.pick_model(this.model_sel, window, cx),
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
            palette_sel: 0,
            palette_scroll: ScrollHandle::new(),
            palette_reveal: std::cell::Cell::new(false),
            composer_pop: None,
            pop_sel: 0,
            pop_scroll: ScrollHandle::new(),
            pop_reveal: std::cell::Cell::new(false),
            model_open: false,
            model_input,
            model_query: String::new(),
            model_sel: 0,
            model_rail: None,
            model_scroll: ScrollHandle::new(),
            model_reveal: std::cell::Cell::new(false),
            model_closed_at: None,
            done_unseen: HashSet::new(),
            sidebar_open: true,
            terminal_open: false,
            settings_open: false,
            show_archived: false,
            panel: None,
            turns: HashMap::new(),
            terminal_lines: Vec::new(),
            terminal_rx: Vec::new(),
            scroll: ScrollHandle::new(),
            follow: true,
            last_offset: px(0.),
            last_max: px(0.),
            follow_thread: None,
            group_open: HashMap::new(),
            rise_base: std::cell::RefCell::new(None),
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
            toast_at: None,
            toast_seq: 0,
            toast_queue: Vec::new(),
            dock_slide_at: None,
            items_scroll: ScrollHandle::new(),
            scroll_target: None,
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
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this
                    .update_in(cx, |app, window, cx| app.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        app
    }
}

impl GrayApp {}

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
        let mut center = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(cx))
            .child(self.render_timeline(window, cx))
            .children(
                self.pending_approval
                    .as_ref()
                    .map(|(_, label)| self.render_approval(label, cx)),
            )
            .when(!self.timeline_empty(), |s| {
                s.child(self.render_composer(window, cx))
            });
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
        let main = v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(div().flex_1().min_h_0().min_w_0().child(main))
            .child(self.render_status_bar(cx));
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
                this.toast_at = None;
                w.clear_notifications(cx);
                this.close_overlays(w, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePalette, w, cx| {
                if this.palette_open {
                    this.close_overlays(w, cx);
                } else {
                    this.open_palette(PaletteMode::Commands, "", w, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &SearchThreads, w, cx| this.run(Cmd::SearchThreads, w, cx)),
            )
            .on_action(cx.listener(|this, _: &FilePicker, w, cx| {
                this.open_palette(PaletteMode::Files, "", w, cx)
            }))
            .on_action(cx.listener(|this, _: &ModelPicker, w, cx| this.open_model_picker(w, cx)))
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
                s.child(self.render_toast(msg, err, cx))
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

mod commands;
use commands::*;
mod style;
pub use style::*;
mod timeline;
use timeline::relative;
use timeline::*;

mod render_views;

mod render_chrome;
mod render_composer;
mod render_palette;
mod render_panels;
mod render_timeline;

mod files;
mod pickers;
mod projects;
mod turns;

#[cfg(test)]
mod tests {
    use super::snippet;

    #[test]
    fn snippet_survives_expanding_lowercase() {
        // 'İ'.to_lowercase() is two chars; indexes must still line up.
        let text = format!("{}ab", "İ".repeat(100));
        let out = snippet(&text, "ab", 80).expect("hit");
        assert!(out.ends_with("ab"), "{out}");
    }

    #[test]
    fn snippet_case_insensitive_window() {
        let text = format!("{} Needle {}", "x".repeat(200), "y".repeat(200));
        let out = snippet(&text, "needle", 30).expect("hit");
        assert!(out.starts_with('…') && out.ends_with('…'), "{out}");
        assert!(out.contains("Needle"), "{out}");
    }
}
