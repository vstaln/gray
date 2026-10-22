use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex as StdMutex};

use clap::Parser as _;
use gray_core::agent::Agent;
use gray_core::event::AgentEvent;
use gray_core::message::Message;

use crate::config::Config;
use crate::repl::commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
use crate::repl::dispatch::{self, Flow};
use crate::repl::handlers::expand_skill_command;
use crate::repl::prompt_turn::run_prompt_turn;
use crate::repl::{CONTINUE_PROMPT, SessionState, SessionTotals, TuiOpt, say, say_lines};

type OutputSink = Box<dyn Fn(&str) + Send + Sync>;
type EventTap = Box<dyn Fn(&AgentEvent) + Send + Sync>;
type SteerSource = Box<dyn Fn() -> Option<String> + Send + Sync>;

type SharedSink = Arc<dyn Fn(&str) + Send + Sync>;
type SharedTap = Arc<dyn Fn(&AgentEvent) + Send + Sync>;
type SharedSteer = Arc<dyn Fn() -> Option<String> + Send + Sync>;

static OUTPUT_SINK: StdMutex<Option<SharedSink>> = StdMutex::new(None);
static EVENT_TAP: StdMutex<Option<SharedTap>> = StdMutex::new(None);
static STEER_SOURCE: StdMutex<Option<SharedSteer>> = StdMutex::new(None);

pub fn set_output_sink(sink: Option<OutputSink>) {
    *OUTPUT_SINK.lock().unwrap_or_else(|e| e.into_inner()) =
        sink.map(|s| Arc::from(s) as SharedSink);
}

pub(crate) fn sink_active() -> bool {
    OUTPUT_SINK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

pub(crate) fn emit_output(text: &str) -> bool {
    let sink = OUTPUT_SINK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(sink) = sink {
        sink(&crate::tui::strip_ansi(text));
        true
    } else {
        false
    }
}

pub fn set_event_tap(tap: Option<EventTap>) {
    *EVENT_TAP.lock().unwrap_or_else(|e| e.into_inner()) = tap.map(|t| Arc::from(t) as SharedTap);
}

pub(crate) fn tap_active() -> bool {
    EVENT_TAP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

pub(crate) fn tap_agent_event(ev: &AgentEvent) {
    let tap = EVENT_TAP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(tap) = tap {
        tap(ev);
    }
}

pub fn set_steer_source(source: Option<SteerSource>) {
    *STEER_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) =
        source.map(|s| Arc::from(s) as SharedSteer);
}

pub(crate) fn steer_source() -> Option<String> {
    let src = STEER_SOURCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    src.and_then(|src| src())
}

/// Whether `line` would run locally during a turn (usage/jobs/help/etc.) in
/// the TUI instead of being steered into the model. Hosts check this before
/// deciding whether mid-turn input goes to the steer channel or the command
/// queue.
pub fn is_turn_local_command(line: &str) -> bool {
    crate::repl::key_watcher::is_turn_local_command(line, &[])
}

/// Consume a pending host wake request (background-job notification, plugin
/// host/background, …). Hosts poll this to decide when to run
/// [`IdeSession::poll_idle`].
pub fn take_wake() -> bool {
    let requested = crate::host::wake_requested();
    if requested {
        crate::host::clear_wake();
    }
    requested
}

pub fn cancel_turn() {
    if let Some(token) = super::repl::TURN_STATE
        .lock()
        .ok()
        .and_then(|mut g| g.take())
    {
        token.cancel();
    }
}

pub fn turn_in_flight() -> bool {
    super::repl::TURN_IN_FLIGHT.load(Relaxed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    ModelPicker,
    ResumePicker,
    SetupWizard,
    SysEdit { path: Option<PathBuf> },
    ProviderLogin,
    Restart,
}

fn interactive_command(cmd: &ReplCommand, sys_prompt_path: Option<PathBuf>) -> Option<UiCommand> {
    match cmd {
        ReplCommand::Model(None) | ReplCommand::ModelPanes | ReplCommand::ModelFocus(_) => {
            Some(UiCommand::ModelPicker)
        }
        ReplCommand::Resume(ResumeArgs {
            target: None,
            last: false,
            all: _,
            dismiss: false,
        }) => Some(UiCommand::ResumePicker),
        ReplCommand::Sys(SysAction::Edit) => Some(UiCommand::SysEdit {
            path: sys_prompt_path,
        }),
        ReplCommand::ProviderLogin(_) => Some(UiCommand::ProviderLogin),
        ReplCommand::Restart => Some(UiCommand::Restart),
        _ => None,
    }
}

#[derive(Debug, Default, Clone)]
pub struct IdeUsage {
    pub turns: usize,
    pub input: usize,
    pub output: usize,
    pub cost: f64,
    pub total_duration_ms: u64,
    pub cache_read: usize,
    pub cache_write: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Done,
    Quit,
    NeedsUi(UiCommand),
}

pub struct IdeSession {
    pub config: Config,
    cwd: PathBuf,
    tui: TuiOpt,
    agent: Option<Agent>,
    session_state: Option<SessionState>,
    session_totals: SessionTotals,
    pending_command: Option<ReplCommand>,
    pending_history: Arc<Vec<Message>>,
    pending_images: Vec<PathBuf>,
    unconfigured: bool,
    resumable: bool,
    hide_thinking: bool,
}

impl IdeSession {
    pub fn new(config: Config, cwd: PathBuf) -> Self {
        gray_plugin::builder::set_autostart(true);
        let unconfigured = config.model.is_none();
        let hide_thinking = config.reasoning_hidden();
        Self {
            config,
            cwd,
            tui: None,
            agent: None,
            session_state: None,
            session_totals: SessionTotals::default(),
            pending_command: None,
            pending_history: Arc::new(Vec::new()),
            pending_images: Vec::new(),
            unconfigured,
            resumable: false,
            hide_thinking,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string())
    }

    /// Bumps whenever the agent's history is rewritten (undo, compaction) —
    /// plain appends don't move it. Hosts pair this with `session_id` to
    /// decide when to resync the transcript.
    pub fn history_rev(&self) -> u64 {
        self.agent
            .as_ref()
            .map(|a| a.history_revision())
            .unwrap_or(0)
    }

    pub fn history(&self) -> &[Message] {
        if let Some(agent) = self.agent.as_ref() {
            agent.messages()
        } else {
            &self.pending_history
        }
    }

    pub fn model_label(&self) -> String {
        let provider = self.config.provider_id.clone();
        match self.config.model.as_deref() {
            Some(m) => format!("{provider} · {m}"),
            None => format!("{provider} · unset"),
        }
    }

    pub fn parse(line: &str) -> ReplCommand {
        parse_command(line)
    }

    pub fn is_interactive(cmd: &ReplCommand) -> bool {
        interactive_command(cmd, crate::sys_prompt_path().ok()).is_some()
    }

    pub async fn submit(&mut self, line: &str) -> anyhow::Result<SubmitOutcome> {
        self.drain_notices();
        let mut input = Some(line.to_string());
        loop {
            let cmd = match self.pending_command.take() {
                Some(c) => c,
                None => match input.take() {
                    Some(l) => expand_skill_command(parse_command(&l), &self.cwd, None, false),
                    None => {
                        self.drain_notices();
                        return Ok(SubmitOutcome::Done);
                    }
                },
            };
            if !matches!(cmd, ReplCommand::Prompt(_) | ReplCommand::Empty) {
                self.pending_images.clear();
            }
            match cmd {
                ReplCommand::Empty => {
                    if !self.pending_images.is_empty() {
                        self.run_turn(String::new()).await?;
                    } else if self.resumable && self.agent.is_some() {
                        if let Some(msg) = crate::turn_caps::check_caps(
                            &self.config,
                            self.session_totals.turns,
                            self.session_totals.cost,
                        ) {
                            say(None, &msg);
                            return Ok(SubmitOutcome::Done);
                        }
                        say(
                            None,
                            &format!("{} {CONTINUE_PROMPT}", crate::tui::arrow_glyph()),
                        );
                        self.run_turn(CONTINUE_PROMPT.to_string()).await?;
                    }
                }
                ReplCommand::Prompt(text) => {
                    if self.unconfigured && self.agent.is_none() {
                        self.refresh_connection();
                        if self.unconfigured {
                            return Ok(SubmitOutcome::NeedsUi(UiCommand::SetupWizard));
                        }
                    }
                    if let Some(msg) = crate::turn_caps::check_caps(
                        &self.config,
                        self.session_totals.turns,
                        self.session_totals.cost,
                    ) {
                        say(None, &msg);
                        return Ok(SubmitOutcome::Done);
                    }
                    self.run_turn(text).await?;
                }
                other => {
                    if let Some(ui) = interactive_command(&other, crate::sys_prompt_path().ok()) {
                        if let UiCommand::SysEdit { path: Some(p) } = &ui {
                            match crate::load_or_create_system_prompt_at(p) {
                                Ok(_) => {
                                    // Rebuild on the next prompt like
                                    // handlers::reload_agent — keep the history
                                    // or the conversation context is lost.
                                    if let Some(old) = self.agent.take() {
                                        self.pending_history = Arc::new(old.messages().to_vec());
                                    }
                                }
                                Err(e) => {
                                    say(None, &format!("{e}"));
                                    return Ok(SubmitOutcome::Done);
                                }
                            }
                        }
                        return Ok(SubmitOutcome::NeedsUi(ui));
                    }
                    if matches!(
                        other,
                        ReplCommand::New(_) | ReplCommand::Resume(_) | ReplCommand::Compact(_)
                    ) {
                        self.resumable = false;
                    }
                    if self.dispatch(other).await? == Flow::Break {
                        return Ok(SubmitOutcome::Quit);
                    }
                }
            }
        }
    }

    fn refresh_connection(&mut self) {
        let cli = crate::Cli::parse_from(["gray-ide"]);
        let Ok(fresh) = crate::Config::resolve_with(&cli, |k| std::env::var(k).ok()) else {
            return;
        };
        let connected =
            fresh.model.is_some() && (fresh.api_key.is_some() || fresh.uses_plugin_credentials());
        if !connected {
            return;
        }
        let c = &mut self.config;
        c.provider_id = fresh.provider_id;
        c.base_url = fresh.base_url;
        c.credential_source = fresh.credential_source;
        c.auth_ref = fresh.auth_ref;
        c.api_key = fresh.api_key;
        if c.model.is_none() {
            c.model = fresh.model;
        }
        if c.thinking_effort.is_none() {
            c.thinking_effort = fresh.thinking_effort;
        }
        if c.model_parts.is_empty() {
            c.model_parts = fresh.model_parts;
        }
        self.unconfigured = false;
        crate::config::export_provider_env(&self.config);
    }

    pub fn configured(&self) -> bool {
        !self.unconfigured
    }

    pub fn drain_notices(&self) {
        for lines in crate::host::take_host_say() {
            say_lines(None, lines);
        }
    }

    /// The repl loop's idle drain: publish the live session + background
    /// source, then surface cron deliveries, background-job notices and
    /// inbound session sends — running the resulting prompt as a turn.
    /// Returns `Ok(true)` when a turn ran.
    pub async fn poll_idle(&mut self) -> anyhow::Result<bool> {
        self.drain_notices();
        let sid = self
            .session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string());
        crate::host::set_live_session(sid.as_deref());
        crate::host::set_background_source(self.agent.as_ref().map(|a| {
            (
                a.executor_handle(),
                crate::repl::jobs::session_ctx(&self.cwd, sid.as_deref()),
            )
        }));
        let Some((cards, prompt, sends)) =
            crate::repl::notice::idle_wake(self.agent.as_ref(), sid.as_deref(), &self.cwd)
        else {
            return Ok(false);
        };
        for card in cards {
            match card {
                crate::repl::notice::WakeCard::Job(j) => say(
                    None,
                    &format!(
                        "background job {} finished ({}) after {} \u{b7} log {}",
                        j.id, j.outcome, j.elapsed, j.log
                    ),
                ),
                crate::repl::notice::WakeCard::Cron(c) => say(
                    None,
                    &format!(
                        "cron: {}",
                        crate::repl::notice::format_delivery_plain(
                            &c.name, &c.body, c.reminder, c.failed
                        )
                    ),
                ),
                crate::repl::notice::WakeCard::Text(text) => say(None, &text),
            }
        }
        if let Some(msg) = crate::turn_caps::check_caps(
            &self.config,
            self.session_totals.turns,
            self.session_totals.cost,
        ) {
            say(None, &msg);
            crate::session_send::hold_back(&sends);
            return Ok(false);
        }
        if self.unconfigured && self.agent.is_none() {
            // No terminal for the connect modal — try picking up credentials
            // saved since launch, else hold the sends for later.
            self.refresh_connection();
            if self.unconfigured {
                crate::session_send::hold_back(&sends);
                return Ok(false);
            }
        }
        for m in &sends {
            say(None, &crate::session_send::prompt_for(&m.from, &m.text));
        }
        self.pending_images
            .extend(sends.iter().flat_map(|m| m.attachments.iter().cloned()));
        crate::session_send::mark_delivered(&sends);
        let turn = self.run_turn(prompt).await;
        if turn.is_err() || self.agent.is_none() {
            crate::session_send::hold_back(&sends);
        }
        turn.map(|_| true)
    }

    async fn run_turn(&mut self, text: String) -> anyhow::Result<()> {
        run_prompt_turn(
            text,
            &mut self.pending_images,
            &mut self.agent,
            &mut self.config,
            &self.cwd,
            &self.tui,
            false,
            &mut self.session_state,
            &mut self.session_totals,
            &mut self.pending_command,
            &mut self.pending_history,
            &mut self.unconfigured,
            &mut self.resumable,
        )
        .await
    }

    async fn dispatch(&mut self, cmd: ReplCommand) -> anyhow::Result<Flow> {
        dispatch::dispatch_command(
            cmd,
            &mut self.agent,
            &mut self.config,
            &self.cwd,
            &self.tui,
            &mut self.session_state,
            &mut self.session_totals,
            &mut self.pending_command,
            &mut self.pending_history,
            &mut self.unconfigured,
            &mut self.hide_thinking,
        )
        .await
    }

    pub fn totals(&self) -> IdeUsage {
        let t = &self.session_totals;
        IdeUsage {
            turns: t.turns,
            input: t.input,
            output: t.output,
            cost: t.cost,
            total_duration_ms: t.total_duration_ms,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
        }
    }

    pub fn catalog_models(&self) -> Vec<(String, String)> {
        let (_, list_key, _) = crate::setup::picker_scope(&self.config);
        let mut ids = crate::setup::saved_models_for(&list_key, &self.config.base_url);
        let (_, known) = crate::setup::provider_models_for_config(&self.config);
        for row in known {
            if !ids.iter().any(|(id, _)| *id == row.0) {
                ids.push(row);
            }
        }
        ids
    }

}
