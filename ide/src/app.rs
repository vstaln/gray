use gpui::{
    App, AsyncApp, Context, Entity, FocusHandle, Focusable, IntoElement, MouseButton, Render,
    ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, actions, div, prelude::*,
    px, rgb,
};

use gray::session_store::SessionSummary;
use gray_core::event::{AgentEvent, StopReason};
use gray_core::message::{ContentBlock, Message, Role};

use gray::ide::UiCommand;
use gray_ide::bridge::{IdeCommand, IdeEvent, IdeHandle};

use crate::composer::{Composer, PromptSubmitted};
use crate::theme;

actions!(gray_ide, [CancelTurn, NewSession]);

enum Block {
    User(String),
    Assistant(String),
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        args: Option<String>,
        output: Option<String>,
        is_error: bool,
        running: bool,
        expanded: bool,
    },
    Note(String),
    Output(String),
    Error(String),
}

pub struct GrayIde {
    composer: Entity<Composer>,
    handle: IdeHandle,
    sessions: Vec<SessionSummary>,
    active_session: Option<String>,
    blocks: Vec<Block>,
    running: bool,
    model: String,
    fatal: Option<String>,
    models_open: bool,
    models: Vec<(String, String)>,
    usage: gray::ide::IdeUsage,
    scroll: ScrollHandle,
    _events: Task<()>,
    _composer_sub: Subscription,
}

impl GrayIde {
    pub fn new(
        handle: IdeHandle,
        events: flume::Receiver<IdeEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(Composer::new);
        let composer_sub = cx.subscribe(
            &composer,
            |this: &mut Self, _composer, ev: &PromptSubmitted, _cx| {
                this.handle.submit(ev.text.clone());
            },
        );
        let focus = composer.read(cx).focus_handle(cx);
        window.focus(&focus);

        let events_task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(ev) = events.recv_async().await {
                    if this.update(&mut cx, |this, cx| this.apply(ev, cx)).is_err() {
                        break;
                    }
                }
            }
        });

        Self {
            composer,
            handle,
            sessions: Vec::new(),
            active_session: None,
            blocks: vec![Block::Note("new session — type a prompt below".into())],
            running: false,
            model: "…".into(),
            fatal: None,
            models_open: false,
            models: Vec::new(),
            usage: Default::default(),
            scroll: ScrollHandle::new(),
            _events: events_task,
            _composer_sub: composer_sub,
        }
    }

    fn apply(&mut self, ev: IdeEvent, cx: &mut Context<Self>) {
        // Only keep the tail pinned when the user is already at the bottom —
        // scrolling up during streaming must not get yanked back on every
        // delta.
        let pinned = self.scroll.offset().y >= -self.scroll.max_offset().height - px(24.);
        match ev {
            IdeEvent::Ready { label } | IdeEvent::Label(label) => self.model = label,
            IdeEvent::Fatal(msg) => {
                self.fatal = Some(msg);
                if self.blocks.len() == 1
                    && matches!(&self.blocks[0], Block::Note(n) if n.starts_with("loading"))
                {
                    self.blocks.clear();
                }
                self.running = false;
            }
            IdeEvent::Sessions(list) => self.sessions = list,
            IdeEvent::Sync { session, history } => {
                self.active_session = session;
                self.blocks = messages_to_blocks(&history);
                if self.blocks.is_empty() {
                    self.blocks
                        .push(Block::Note("new session — type a prompt below".into()));
                }
                self.running = false;
            }
            IdeEvent::UserEcho(text) => self.blocks.push(Block::User(text)),
            IdeEvent::Output(text) => match self.blocks.last_mut() {
                Some(Block::Output(buf)) => {
                    buf.push('\n');
                    buf.push_str(&text);
                }
                _ => self.blocks.push(Block::Output(text)),
            },
            IdeEvent::Models(rows) => self.models = rows,
            IdeEvent::Usage(u) => self.usage = u,
            IdeEvent::Agent(ev) => self.apply_agent(ev),
            IdeEvent::TurnDone { error } => {
                self.running = false;
                if let Some(e) = error {
                    self.blocks.push(Block::Error(e));
                }
            }
            IdeEvent::Ui(cmd) => self.apply_ui(cmd, cx),
            IdeEvent::Quit => cx.quit(),
        }
        if pinned {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn apply_agent(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Start => self.running = true,
            AgentEvent::TextDelta { delta } => match self.blocks.last_mut() {
                Some(Block::Assistant(text)) => text.push_str(&delta),
                _ => self.blocks.push(Block::Assistant(delta)),
            },
            AgentEvent::ThinkingDelta { delta } => match self.blocks.last_mut() {
                Some(Block::Thinking(text)) => text.push_str(&delta),
                _ => self.blocks.push(Block::Thinking(delta)),
            },
            AgentEvent::ToolCallStart { id, name } => {
                self.blocks.push(Block::ToolCall {
                    id,
                    name,
                    args: None,
                    output: None,
                    is_error: false,
                    running: true,
                    expanded: false,
                });
            }
            AgentEvent::ToolCallProgress {
                id, args_so_far, ..
            } => {
                if let Some(args) = self.tool_args_mut(&id) {
                    *args = Some(args_so_far);
                }
            }
            AgentEvent::ToolCallEnd { id, args } => {
                if let Some(slot) = self.tool_args_mut(&id) {
                    *slot = Some(pretty_json(&args));
                }
            }
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } => {
                if let Some(Block::ToolCall {
                    output: slot,
                    is_error: err,
                    running,
                    ..
                }) = self
                    .blocks
                    .iter_mut()
                    .rev()
                    .find(|b| matches!(b, Block::ToolCall { id: tid, .. } if *tid == id))
                {
                    *slot = Some(output);
                    *err = is_error;
                    *running = false;
                }
            }
            AgentEvent::StepUsage { .. } | AgentEvent::PrefixChanged { .. } => {}
            AgentEvent::StreamError { message, details } => {
                let text = if details.is_empty() {
                    message
                } else {
                    format!("{message}\n{details}")
                };
                self.blocks.push(Block::Error(text));
            }
            AgentEvent::MessageBoundary => {}
            AgentEvent::Steered { text } => self.blocks.push(Block::User(text)),
            AgentEvent::Retract => self.blocks.push(Block::Note("last step retracted".into())),
            AgentEvent::TurnEnd { stop_reason, usage } => {
                if matches!(stop_reason, StopReason::Cancelled) {
                    self.blocks.push(Block::Note("turn cancelled".into()));
                }
                let _ = usage;
            }
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                ..
            } => self.blocks.push(Block::Note(format!(
                "context compacted {tokens_before} → {tokens_after} tokens"
            ))),
        }
    }

    #[cfg(windows)]
    fn cmd_start(title: &str, target: &str) -> bool {
        use std::os::windows::process::CommandExt as _;
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("start")
            .raw_arg(format!("\"{title}\" \"{target}\""))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn open_in_editor(path: &std::path::Path) -> bool {
        #[cfg(windows)]
        {
            Self::cmd_start("", &path.display().to_string())
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    fn launch_gray_terminal() -> bool {
        let gray = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .map(|d| d.join(if cfg!(windows) { "gray.exe" } else { "gray" }))
            .filter(|p| p.exists())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "gray".to_string());
        #[cfg(windows)]
        {
            Self::cmd_start("gray", &gray)
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .args(["-a", "Terminal", &gray])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            for term in [
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xterm",
                "kgx",
            ] {
                if std::process::Command::new(term)
                    .args(["-e", &gray])
                    .spawn()
                    .is_ok()
                {
                    return true;
                }
            }
            false
        }
    }

    fn tool_args_mut(&mut self, id: &str) -> Option<&mut Option<String>> {
        self.blocks.iter_mut().rev().find_map(|b| match b {
            Block::ToolCall { id: tid, args, .. } if tid == id => Some(args),
            _ => None,
        })
    }

    fn toggle_tool(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(Block::ToolCall { expanded, .. }) = self
            .blocks
            .iter_mut()
            .find(|b| matches!(b, Block::ToolCall { id: tid, .. } if tid == id))
        {
            *expanded = !*expanded;
            cx.notify();
        }
    }

    fn cancel(&mut self, _: &CancelTurn, _window: &mut Window, _cx: &mut Context<Self>) {
        self.handle.cancel();
    }

    fn apply_ui(&mut self, cmd: UiCommand, cx: &mut Context<Self>) {
        match cmd {
            UiCommand::ModelPicker => {
                self.models_open = true;
                self.handle.send(IdeCommand::ListModels);
            }
            UiCommand::ResumePicker => {
                self.blocks
                    .push(Block::Note("pick a session in the left sidebar".into()));
            }
            UiCommand::SetupWizard => {
                self.blocks.push(Block::Note(
                    "no model configured — run `gray` once in a terminal to connect a \
                     provider, or set GRAY_MODEL/GRAY_API_KEY"
                        .into(),
                ));
            }
            UiCommand::SysEdit { path } => match path {
                Some(path) => {
                    let opened = Self::open_in_editor(&path);
                    self.blocks.push(Block::Note(if opened {
                        format!("opened {}", path.display())
                    } else {
                        format!("couldn't open an editor — file is at {}", path.display())
                    }));
                }
                None => self.blocks.push(Block::Note(
                    "can't resolve the system prompt file (GRAY_HOME unset)".into(),
                )),
            },
            UiCommand::ProviderLogin => {
                self.blocks.push(Block::Note(if Self::launch_gray_terminal() {
                    "provider connect opened in a terminal — finish there, then pick the model here".into()
                } else {
                    "couldn't open a terminal — run `gray` yourself to connect a provider".into()
                }));
            }
            UiCommand::Restart => match std::env::current_exe() {
                Ok(exe) => match std::process::Command::new(exe).spawn() {
                    Ok(_) => cx.quit(),
                    Err(e) => {
                        self.blocks
                            .push(Block::Error(format!("restart failed to spawn: {e}")));
                    }
                },
                Err(e) => {
                    self.blocks
                        .push(Block::Error(format!("restart failed: {e}")));
                }
            },
        }
        cx.notify();
    }

    fn new_session(&mut self, _: &NewSession, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.blocks = vec![Block::Note("new session — type a prompt below".into())];
        self.active_session = None;
        self.handle.send(IdeCommand::Submit("/new".into()));
        cx.notify();
    }

    fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.blocks = vec![Block::Note("loading session…".into())];
        self.handle
            .send(IdeCommand::Submit(format!("/resume {id}")));
        cx.notify();
    }

    fn pick_model(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.models_open = false;
        self.handle
            .send(IdeCommand::Submit(format!("/models {id}")));
        cx.notify();
    }

    fn busy(&self) -> bool {
        self.running || gray::ide::turn_in_flight()
    }
}

impl Focusable for GrayIde {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

fn pretty_json(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn messages_to_blocks(messages: &[Message]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut tool_idx: std::collections::HashMap<String, usize> = Default::default();
    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if msg.role == Role::User {
                        blocks.push(Block::User(text.clone()));
                    } else {
                        blocks.push(Block::Assistant(text.clone()));
                    }
                }
                ContentBlock::Thinking { text, .. } => {
                    blocks.push(Block::Thinking(text.clone()));
                }
                ContentBlock::ToolUse { id, name, args, .. } => {
                    tool_idx.insert(id.clone(), blocks.len());
                    blocks.push(Block::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        args: Some(pretty_json(args)),
                        output: None,
                        is_error: false,
                        running: false,
                        expanded: false,
                    });
                }
                ContentBlock::ToolResult {
                    id,
                    content,
                    is_error,
                } => {
                    if let Some(&i) = tool_idx.get(id)
                        && let Block::ToolCall {
                            output,
                            is_error: err,
                            ..
                        } = &mut blocks[i]
                    {
                        *output = Some(content.clone());
                        *err = *is_error;
                    }
                }
                ContentBlock::Image { .. } | ContentBlock::Media { .. } => {
                    blocks.push(Block::Note("[media]".into()));
                }
                _ => {}
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(Block::Note("empty session".into()));
    }
    blocks
}

fn block_view(b: &Block, ix: usize, cx: &mut Context<GrayIde>) -> gpui::AnyElement {
    match b {
        Block::User(text) => div()
            .id(ix)
            .my_1()
            .flex()
            .justify_end()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(theme::ACCENT_DIM))
                    .text_color(rgb(theme::TEXT))
                    .child(SharedString::from(text.clone())),
            )
            .into_any_element(),
        Block::Assistant(text) => div()
            .id(ix)
            .my_1()
            .px_3()
            .py_2()
            .text_color(rgb(theme::TEXT))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        Block::Thinking(text) => div()
            .id(ix)
            .my_1()
            .px_3()
            .py_1()
            .italic()
            .text_color(rgb(theme::MUTED))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        Block::ToolCall {
            id,
            name,
            args,
            output,
            is_error,
            running,
            expanded,
        } => {
            let status = if *running {
                ("running", theme::TOOL)
            } else if *is_error {
                ("error", theme::ERROR)
            } else {
                ("done", theme::OK)
            };
            let id = id.clone();
            let mut el = div()
                .id(ix)
                .my_1()
                .rounded_md()
                .bg(rgb(theme::SURFACE2))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .px_3()
                .py_2()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _w, cx| this.toggle_tool(&id, cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(rgb(theme::TOOL))
                                .child(SharedString::from(format!("⚙ {name}"))),
                        )
                        .child(div().text_color(rgb(status.1)).text_sm().child(status.0)),
                );
            if *expanded {
                if let Some(args) = args {
                    el = el.child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(rgb(theme::MUTED))
                            .child(SharedString::from(args.clone())),
                    );
                }
                if let Some(out) = output {
                    el = el.child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(rgb(if *is_error { theme::ERROR } else { theme::TEXT }))
                            .child(SharedString::from(out.clone())),
                    );
                }
            }
            el.into_any_element()
        }
        Block::Note(text) => div()
            .id(ix)
            .my_1()
            .px_3()
            .py_1()
            .text_sm()
            .text_color(rgb(theme::MUTED))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        Block::Output(text) => div()
            .id(ix)
            .my_1()
            .mx_3()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(theme::SURFACE2))
            .text_sm()
            .text_color(rgb(theme::TEXT))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        Block::Error(text) => div()
            .id(ix)
            .my_1()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::ERROR))
            .text_color(rgb(theme::ERROR))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
    }
}

fn session_row(s: &SessionSummary, cx: &mut Context<GrayIde>) -> gpui::AnyElement {
    let title = s
        .title
        .clone()
        .or_else(|| s.first_user_text.clone())
        .unwrap_or_else(|| s.id.as_str().to_owned());
    let title: String = title.chars().take(34).collect();
    let cwd: SharedString = s
        .cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.cwd.display().to_string())
        .into();
    let id = s.id.as_str().to_owned();
    div()
        .id(SharedString::from(format!("sess-{}", s.id.as_str())))
        .px_3()
        .py_2()
        .cursor_pointer()
        .hover(|d| d.bg(rgb(theme::SURFACE2)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _w, cx| this.open_session(id.clone(), cx)),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(theme::TEXT))
                .child(SharedString::from(title)),
        )
        .child(div().text_xs().text_color(rgb(theme::MUTED)).child(cwd))
        .into_any_element()
}

impl Render for GrayIde {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = div()
            .w(px(240.))
            .h_full()
            .bg(rgb(theme::SURFACE))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(theme::MUTED))
                            .child("sessions"),
                    )
                    .child(
                        div()
                            .id("new-session")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .text_color(rgb(theme::ACCENT))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(theme::SURFACE2)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, w, cx| this.new_session(&NewSession, w, cx)),
                            )
                            .child("+ new"),
                    ),
            )
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children({
                        let mut rows = Vec::with_capacity(self.sessions.len());
                        for s in &self.sessions {
                            rows.push(session_row(s, cx));
                        }
                        rows
                    }),
            );

        let status = if self.running { "working…" } else { "idle" };
        let topbar = div()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(div().text_color(rgb(theme::ACCENT)).child("gray"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(theme::MUTED))
                            .child(SharedString::from(self.model.clone())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(if self.running {
                                theme::TOOL
                            } else {
                                theme::MUTED
                            }))
                            .child(status),
                    )
                    .when(self.running, |d| {
                        d.child(
                            div()
                                .id("cancel")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(theme::ERROR))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgb(theme::SURFACE2)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, w, cx| this.cancel(&CancelTurn, w, cx)),
                                )
                                .child("cancel (esc)"),
                        )
                    }),
            );

        let transcript = div()
            .id("transcript")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_4()
            .py_2()
            .children({
                let mut els = Vec::with_capacity(self.blocks.len());
                for (ix, b) in self.blocks.iter().enumerate() {
                    els.push(block_view(b, ix, cx));
                }
                els
            });

        let composer_bar = div()
            .border_t_1()
            .border_color(rgb(theme::BORDER))
            .px_4()
            .py_3()
            .child(self.composer.clone());

        let mut main = div()
            .relative()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(topbar)
            .child(transcript)
            .child(composer_bar);

        if self.models_open {
            let mut rows = Vec::with_capacity(self.models.len());
            for (id, label) in &self.models {
                let id = id.clone();
                let label = if label.is_empty() {
                    id.clone()
                } else {
                    label.clone()
                };
                rows.push(
                    div()
                        .id(SharedString::from(format!("model-{id}")))
                        .px_3()
                        .py_2()
                        .cursor_pointer()
                        .text_sm()
                        .hover(|d| d.bg(rgb(theme::SURFACE2)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _w, cx| this.pick_model(id.clone(), cx)),
                        )
                        .child(SharedString::from(label)),
                );
            }
            main = main.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(rgb(0x00000066))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _w, cx| {
                            this.models_open = false;
                            cx.notify();
                        }),
                    )
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("model-picker")
                            .w(px(360.))
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .rounded_lg()
                            .bg(rgb(theme::SURFACE))
                            .border_1()
                            .border_color(rgb(theme::BORDER))
                            .on_mouse_down(MouseButton::Left, |_e, _w, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(theme::BORDER))
                                    .text_sm()
                                    .text_color(rgb(theme::MUTED))
                                    .child("models — pick or /models <id>"),
                            )
                            .children(rows),
                    ),
            );
        }

        if let Some(fatal) = &self.fatal {
            main = main.child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(theme::SURFACE2))
                    .border_t_1()
                    .border_color(rgb(theme::ERROR))
                    .text_color(rgb(theme::ERROR))
                    .text_sm()
                    .child(SharedString::from(fatal.clone())),
            );
        }

        div()
            .key_context("GrayIde")
            .size_full()
            .bg(rgb(theme::BG))
            .text_color(rgb(theme::TEXT))
            .flex()
            .flex_row()
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::new_session))
            .child(sidebar)
            .child(main)
    }
}
