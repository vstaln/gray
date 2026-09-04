//! Interactive REPL mode for Gray.
// 2 turn loops (~400 lines) + 3 provider blocks duplicated; extract ensure_provider + run_turn when adding streaming resume.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{
    default_root, JsonlSessionStore, SessionId, SessionMeta,
};

use std::sync::Mutex as StdMutex;


/// Static slash-command table driving both `/help` and the autocomplete panel.
/// True while an agent turn is in flight: `Some(token)` cancels on first
/// Ctrl-C (token consumed), a second press — or any press at the prompt —
/// exits. Single mutex = no TOCTOU between flag and token.
static TURN_STATE: StdMutex<Option<tokio_util::sync::CancellationToken>> = StdMutex::new(None);

/// Installs the single global Ctrl-C policy:
/// - during a turn: cancel the turn (first press), the turn handler reports it
/// - at the prompt: exit cleanly
async fn spawn_ctrl_c_policy() {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let token = TURN_STATE.lock().ok().and_then(|mut g| g.take());
        if let Some(t) = token {
            t.cancel(); // first press mid-turn: cancel, stay alive
        } else {
            // Say something — a bare exit(0) mid-turn looks like a crash.
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = write!(std::io::stdout(), "\x1b[?25h\r\n\x1b[2m(interrupted — bye)\x1b[0m\r\n");
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
    }
}
use crate::{build_agent, load_or_create_system_prompt_at, DEFAULT_SYS_PROMPT};
use crate::config::Config;

pub mod attachments;
pub mod commands;
pub mod format;

pub use commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
pub(crate) use commands::{COMMANDS, completion_matches_dyn};
pub use format::{fmt_event, fmt_usage, format_core_error, THINKING_STYLE};
pub(crate) use format::build_user_message_with_attachments;

pub(crate) struct SessionState {
    pub(crate) store: gray_session::JsonlSessionStore,
    pub(crate) session_id: gray_session::SessionId,
}

/// Command feedback: through the composer when it owns the terminal, else stdout.
/// Raw println! while the composer viewport is live collides with the next draw (ghost input).
fn say(tui: Option<&crate::composer::SharedTui>, msg: &str) {
    if let Some(t) = tui {
        let mut t = t.lock().expect("tui lock");
        // No gap above: command cards skip their trailing gap so this hugs them.
        for line in msg.split('\n') {
            t.push_dim(format!("└ {line}"));
        }
        // Breathing room below command output before the next prompt.
        t.ensure_gap(1);
    } else {
        println!("{msg}");
    }
}

/// Restores the inline viewport after an alternate-screen modal (model/provider/etc).
/// EnterAlternateScreen/LeaveAlternateScreen breaks ratatui's Inline(10) viewport anchor;
/// without this the next Tui::draw renders off-screen and the input box vanishes.
/// Width unchanged → just re-anchor (LeaveAlternateScreen already restored the
/// scrollback; clearing/re-emitting here destroyed it). Width changed → full reflow.
fn restore_viewport(tui: Option<&crate::composer::SharedTui>) {
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        // Breathing room when a modal is dismissed with no output: command
        // cards skip their trailing gap, so without this the next prompt
        // would jam against the card. Idempotent after output with a gap.
        t.ensure_gap(1);
        let cols = crossterm::terminal::size().map(|(c, _)| c).unwrap_or(t.last_width);
        if cols == t.last_width {
            t.reanchor_viewport(cols);
        } else {
            t.pending_resize = None;
            t.reflow_on_resize(cols);
        }
    }
}

async fn with_modal<T>(tui: Option<&crate::composer::SharedTui>, fut: impl std::future::Future<Output = T>) -> T {
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = true;
    }
    let r = fut.await;
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = false;
    }
    restore_viewport(tui);
    r
}

fn with_modal_sync<T>(tui: Option<&crate::composer::SharedTui>, f: impl FnOnce() -> T) -> T {
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = true;
    }
    let r = f();
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = false;
    }
    restore_viewport(tui);
    r
}

/// Expands `/skills:<name> [args]` into a Prompt carrying the skill body
/// (Grok-style: frontmatter stripped, wrapped in a `<skill>` envelope, args
/// appended). Bare `/skills` opens an interactive picker like /resume.
/// With `local` set (Esc mid-turn), the skill is announced but never expanded
/// into an AI prompt — the turn was cancelled, nothing talks to the model.
fn expand_skill_command(cmd: ReplCommand, cwd: &Path, tui: Option<&crate::composer::SharedTui>, local: bool) -> ReplCommand {
    // local: turn was cancelled — run everything as a no-AI no-op
    let to_prompt = |expanded: String| if local { ReplCommand::Empty } else { ReplCommand::Prompt(expanded) };
    let ReplCommand::Skill(payload) = cmd else { return cmd };
    let discovered = crate::skills::discover_skills(cwd);
    let Some(rest) = payload else {
        // Bare /skills — interactive picker (EnterAlternateScreen, like /resume)
        let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
        let picked = match with_modal_sync(tui, || crate::setup::run_skills_modal(cwd, bg.as_ref())) {
            Ok(v) => v,
            Err(e) => {
                say(tui, &format!("skills picker error: {e}"));
                return ReplCommand::Empty;
            }
        };
        let Some((skill, picked_args)) = picked else {
            // Esc — picker cancelled; viewport already restored
            return ReplCommand::Empty;
        };
        // load skill body and optionally append args part from query
        let expanded = match std::fs::read_to_string(&skill.file_path) {
            Ok(content) => {
                let body = gray_tools::skill::strip_frontmatter(&content);
                let mut out = format!(
                    "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
                    skill.name,
                    skill.file_path.display(),
                    body
                );
                if !picked_args.trim().is_empty() {
                    out.push_str(&format!("\n\n**ARGUMENTS:** {}", picked_args.trim()));
                }
                out
            }
            Err(e) => {
                say(tui, &format!("failed to read {}: {e}", skill.file_path.display()));
                return ReplCommand::Empty;
            }
        };
        // surface a dim line like resume does so user sees the pick
        say(tui, &format!("→ /skills:{} {}", skill.name, if picked_args.trim().is_empty() { String::new() } else { picked_args.trim().to_string() }));
        return to_prompt(expanded);
    };
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n.trim(), Some(a.trim().to_string())),
        None => (rest.as_str(), None),
    };
    let Some(skill) = discovered.skills.iter().find(|s| s.name == name) else {
        let names = discovered.skills.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ");
        say(tui, &format!("no skill '{name}' (available: {})", if names.is_empty() { "(none)" } else { &names }));
        return ReplCommand::Empty;
    };
    let expanded = match std::fs::read_to_string(&skill.file_path) {
        Ok(content) => {
            let body = gray_tools::skill::strip_frontmatter(&content);
            let mut out = format!(
                "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
                skill.name,
                skill.file_path.display(),
                body
            );
            if let Some(a) = args.as_deref().filter(|a| !a.is_empty()) {
                out.push_str(&format!("\n\n**ARGUMENTS:** {a}"));
            }
            out
        }
        Err(e) => {
            say(tui, &format!("failed to read {}: {e}", skill.file_path.display()));
            return ReplCommand::Empty;
        }
    };
    to_prompt(expanded)
}

/// Handles the `/agentsmd` command family (alias `/sys`): edit, show, reset.
async fn handle_sys(config: &Config, cwd: &Path, action: SysAction, agent: &mut Option<Agent>, tui: Option<&crate::composer::SharedTui>) {
    let path = match crate::sys_prompt_path() {
        Ok(p) => p,
        Err(e) => {
            say(tui, &format!("{e}"));
            return;
        }
    };
    match action {
        SysAction::Show => {
            match load_or_create_system_prompt_at(&path) {
                Ok(body) => {
                    say(tui, &format!("system prompt: {}\n---\n{body}\n---", path.display()));
                }
                Err(e) => say(tui, &format!("failed to read {}: {e}", path.display())),
            }
        }
        SysAction::Reset => {
            if let Err(e) = std::fs::write(&path, DEFAULT_SYS_PROMPT) {
                say(tui, &format!("failed to reset {}: {e}", path.display()));
                return;
            }
            say(tui, &format!("✓ system prompt restored to default ({})", path.display()));
            reload_agent(agent, config, cwd).await;
        }
        SysAction::Edit => {
            // Make sure the file exists before opening an editor on it.
            let initial = match load_or_create_system_prompt_at(&path) {
                Ok(b) => b,
                Err(e) => {
                    say(tui, &format!("{e}"));
                    return;
                }
            };
            let tui_snap = tui.cloned();
            let editor_paused = if let Some(shared) = &tui_snap {
                let mut t = shared.lock().expect("tui lock");
                t.pending_resize = Some((t.last_width, std::time::Instant::now() + std::time::Duration::from_secs(3600)));
                t.modal_open = true;
                true
            } else { false };
            let mut editor = crate::sys_editor::SysEditor::new(&initial, &path);
            let res = editor.run();
            if editor_paused {
                if let Some(shared) = &tui_snap {
                    let mut t = shared.lock().expect("tui lock");
                    t.pending_resize = None;
                    t.modal_open = false;
                    if let Ok((cols, _)) = crossterm::terminal::size() {
                        t.reflow_on_resize(cols);
                    } else {
                        let _ = t.draw();
                    }
                }
            }
            match res {
                Ok(Some(saved)) => {
                    if let Err(e) = std::fs::write(&path, &saved) {
                        say(tui, &format!("failed to save {}: {e}", path.display()));
                        return;
                    }
                    say(tui, "✓ system prompt saved — applies from your next message");
                    reload_agent(agent, config, cwd).await;
                }
                Ok(None) => {
                    say(tui, "prompt unchanged");
                }
                Err(e) => {
                    say(tui, &format!("editor error: {e}"));
                }
            }
        }
    }
}

/// Rebuilds the agent after a system-prompt change, preserving conversation history.
async fn reload_agent(agent: &mut Option<Agent>, config: &Config, cwd: &Path) {
    let old = agent.take();
    let mut rebuilt = match build_agent(config, cwd, None).await {
        Ok(a) => a,
        Err(e) => {
            println!("{e}");
            *agent = old;
            return;
        }
    };
    if let Some(old) = old {
        rebuilt = rebuilt.with_messages(old.messages().to_vec());
    }
    *agent = Some(rebuilt);
}

/// Handles `/model`: interactive picker (no arg) or direct set (`/model provider/id`).
/// Switching updates the live agent and persists to ~/.gray/config.json.
async fn handle_model(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if let Some(m) = direct {
        config.model = Some(m.clone());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.model = Some(m.clone());
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.set_model(m.clone());
            t.push_action("Model set to", Some(&m));
        } else {
            println!("✓ Model set to {m}");
        }
        if crate::setup::get_user_context_window().is_none()
            && crate::setup::get_cached_model_context(&m).is_none()
        {
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::spawn(async move {
                crate::setup::fetch_live_provider_models(&base, key.as_deref());
            });
        }
        reload_agent(agent, config, cwd).await;
        return;
    }

    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = with_modal(tui, crate::setup::run_model_menu(config, bg.as_ref())).await;
    match result {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                    t.push_action("Model set to", Some(m));
                }
                let _ = t.draw();
            }
            if let Some(m) = config.model.clone() {
                if crate::setup::get_user_context_window().is_none()
                    && crate::setup::get_cached_model_context(&m).is_none()
                {
                    let base = config.base_url.clone();
                    let key = config.api_key.clone();
                    tokio::spawn(async move {
                        crate::setup::fetch_live_provider_models(&base, key.as_deref());
                    });
                }
            }
            reload_agent(agent, config, cwd).await;
        }
        Ok(false) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.textarea.set_text("");
                t.matches.clear();
                t.sel = 0;
                t.history_idx = None;
                t.draft.clear();
                t.attachments.clear();
                t.pending_pastes.clear();
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("└ error: {e}"));
            } else {
                println!("model error: {e}");
            }
        }
    }
}

/// Handles `/thinking` / `/effort`: direct set (`/thinking high`), toggle visibility (bare `/thinking`), or picker.
async fn handle_thinking(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    hide_thinking: &mut bool,
) {
    if let Some(eff) = direct {
        let eff_clean = eff.to_lowercase();
        if eff_clean == "off" || crate::setup::THINKING_LEVELS.iter().any(|(l, _)| *l == eff_clean) {
            config.thinking_effort = Some(eff_clean.clone());
            if let Ok(path) = crate::setup::saved_config_path() {
                let mut saved = crate::setup::load_saved_config_at(&path);
                saved.thinking_effort = Some(eff_clean.clone());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
            *hide_thinking = config.reasoning_hidden();
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.set_thinking_effort(eff_clean.clone());
                t.set_hide_thinking(*hide_thinking);
                t.push_action("Thinking effort set to", Some(&eff_clean));
            } else {
                println!("✓ Thinking effort set to {eff_clean}");
            }
            reload_agent(agent, config, cwd).await;
            return;
        }
        let msg = format!("unknown level '{eff_clean}' — try: off, minimal, low, medium, high, xhigh, max");
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
        } else {
            println!("{msg}");
        }
        return;
    }

    let has_explicit_level = config.thinking_effort.is_some();
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = with_modal(tui, crate::setup::run_effort_menu(config, bg.as_ref())).await;
    match result {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(eff) = &config.thinking_effort {
                    t.set_thinking_effort(eff.clone());
                    *hide_thinking = config.reasoning_hidden();
                    t.set_hide_thinking(*hide_thinking);
                    t.push_action("Thinking effort set to", Some(eff));
                }
                let _ = t.draw();
            }
            reload_agent(agent, config, cwd).await;
        }
        Ok(false) => {
            if !has_explicit_level {
                // First run, Esc: flip the display setting (effort untouched).
                let shown = !config.show_reasoning.unwrap_or(true);
                config.show_reasoning = Some(shown);
                if let Ok(path) = crate::setup::saved_config_path() {
                    let mut saved = crate::setup::load_saved_config_at(&path);
                    saved.show_reasoning = Some(shown);
                    let _ = crate::setup::save_saved_config_at(&path, &saved);
                }
                *hide_thinking = config.reasoning_hidden();
                let msg = if *hide_thinking { "reasoning hidden — /thinking to show" } else { "reasoning shown" };
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.set_hide_thinking(*hide_thinking);
                    t.push_dim(format!("└ {msg}"));
                } else {
                    println!("{msg}");
                }
            } else if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.textarea.set_text("");
                t.matches.clear();
                t.sel = 0;
                t.history_idx = None;
                t.draft.clear();
                t.attachments.clear();
                t.pending_pastes.clear();
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("└ error: {e}"));
            } else {
                println!("effort error: {e}");
            }
        }
    }
}

/// Running token + cost totals for the current process session (reset on `/new`).
#[derive(Debug, Default)]
struct SessionTotals {
    turns: usize,
    input: usize,
    output: usize,
    cost: f64,
}

impl SessionTotals {
    fn add(&mut self, usage: &gray_core::event::Usage, model: &str) {
        self.turns += 1;
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;
        if let Some(c) = crate::setup::turn_cost(usage, model) {
            self.cost += c;
        }
    }

    /// Rebuilds totals from stored session entries. Usage is recorded on the
    /// last message of each turn, so entries carrying usage map 1:1 to turns.
    fn from_entries(entries: &[gray_session::SessionEntry], model: &str) -> Self {
        let mut t = SessionTotals::default();
        for u in entries.iter().filter_map(|e| e.usage) {
            t.add(&u, model);
        }
        t
    }
}

/// `⬡ 12,400 tok · $0.004 ($0.41 session)` — cost parts appear only when the
/// model has a LiteLLM rate; otherwise the footer is tokens-only as before.
fn turn_footer(
    usage: &gray_core::event::Usage,
    model: &str,
    totals: &SessionTotals,
) -> String {
    let base = format!("\u{2b22} {} tok", crate::repl::fmt_usage(usage.total()));
    match crate::setup::turn_cost(usage, model) {
        Some(c) if totals.turns > 1 => format!(
            "{base} · {} ({} session)",
            crate::setup::format_cost(c),
            crate::setup::format_cost(totals.cost)
        ),
        Some(c) => format!("{base} · {}", crate::setup::format_cost(c)),
        None => base,
    }
}

/// Handles `/usage` / `/cost`: session totals plus the active model's rate.
/// TUI renders like `/model` — `✓` action header + dim detail lines.
fn handle_usage(
    totals: &SessionTotals,
    config: &Config,
    tui: Option<&crate::composer::SharedTui>,
) {
    if totals.turns == 0 {
        say(tui, "no turns yet this session — usage appears after the first turn");
        return;
    }
    let model = config.model.as_deref().unwrap_or("no model");
    let header = format!("{model} · {} turn{}", totals.turns, if totals.turns == 1 { "" } else { "s" });
    let body = format!(
        "{} in · {} out · {} total",
        crate::repl::fmt_usage(totals.input),
        crate::repl::fmt_usage(totals.output),
        crate::repl::fmt_usage(totals.input + totals.output),
    );
    let cost_line = match crate::setup::get_model_rate(config.model.as_deref().unwrap_or("")) {
        Some(r) => format!(
            "{} @ ${:.2}/${:.2} per 1M in/out",
            crate::setup::format_cost(totals.cost),
            r.input * 1_000_000.0,
            r.output * 1_000_000.0
        ),
        None => "unpriced (no rate yet — pricing tables lag new models)".to_string(),
    };
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        t.push_action("Session usage", Some(&header));
        t.push_dim(body);
        t.push_dim(cost_line);
    } else {
        println!("✓ Session usage — {header}\n  {body}\n  {cost_line}");
    }
}

async fn handle_context_window(
    config: &mut Config,
    cwd: &Path,
    agent: &Option<Agent>,
    direct: Option<String>,
    tui: Option<&crate::composer::SharedTui>,
) {
    fn emit(msg: String, tui: Option<&crate::composer::SharedTui>, ok: bool) {
        if let Some(shared) = tui {
            if ok {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(format!("└ {msg}"));
                let _ = t.draw();
            } else {
                shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
            }
        } else if ok {
            println!("✓ {msg}");
        } else {
            println!("{msg}");
        }
    }
    fn persist_window(config: &Config) {
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.context_window = config.context_window;
            saved.context_reserve = config.context_reserve;
            saved.context_keep = config.context_keep;
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
    }
    fn collect_parts(
        cwd: &Path,
        agent: &Option<Agent>,
        tui: Option<&crate::composer::SharedTui>,
    ) -> crate::setup::ContextParts {
        let sys = crate::sys_prompt_path()
            .ok()
            .and_then(|p| load_or_create_system_prompt_at(&p).ok())
            .map(|s| crate::setup::estimate_str_tokens(&s))
            .unwrap_or(0);
        let ctx_bytes: usize = crate::system_prompt::discover_context_files(cwd)
            .iter()
            .map(|f| f.content.len())
            .sum::<usize>();
        let skills = crate::skills::discover_skills(cwd).skills;
        let skills_toks =
            crate::setup::estimate_str_tokens(&crate::skills::format_skills_for_prompt(&skills));
        let tools_toks = serde_json::to_string(&gray_tools::Registry::builtin().defs())
            .map(|s| crate::setup::estimate_str_tokens(&s))
            .unwrap_or(0);
        let latest = tui.and_then(|t| t.lock().ok().and_then(|g| g.latest_usage));
        let messages = agent
            .as_ref()
            .map(|a| crate::compact::estimate_context_tokens(a.messages(), latest))
            .unwrap_or(0);
        crate::setup::ContextParts {
            system_prompt: sys,
            project_context: (ctx_bytes as f64 / 4.0).ceil() as usize,
            tools: tools_toks,
            skills: skills_toks,
            messages,
        }
    }
    fn breakdown_text(config: &Config, parts: &crate::setup::ContextParts) -> String {
        let model = config.model.as_deref().unwrap_or("");
        let window = crate::setup::resolve_model_context_length(model);
        let max = crate::setup::model_max_context(model);
        let source = crate::setup::context_source(model);
        let reserve = crate::setup::user_reserve_tokens_for(window);
        let keep = crate::setup::user_keep_for(window);
        let used = parts.used();
        let free = parts.free(window, reserve);
        let pct = |n: usize| if window > 0 { n * 100 / window } else { 0 };
        let f = crate::setup::format_context_length;
        let ic = crate::setup::icon;
        // 10x10 hexagon grid, kinds: 0-4 categories, 5 free, 6 buffer.
        let grid_cells = parts.grid_cells(window, reserve);
        let mut flat: Vec<usize> = Vec::with_capacity(100);
        for (kind, n) in grid_cells.iter().enumerate() {
            flat.extend(std::iter::repeat(kind).take(*n));
        }
        while flat.len() < 100 {
            flat.push(5);
        }
        let cell = |kind: usize| match kind {
            0..=4 => ic("cell"),
            5 => ic("cell_free"),
            _ => ic("cell_buffer"),
        };
        let mut grid = String::new();
        for r in 0..10 {
            for c in 0..10 {
                grid.push_str(cell(flat[r * 10 + c]));
                grid.push(' ');
            }
            grid.push('\n');
        }
        format!(
            "Context Usage\n{} · {}/{} tokens ({}%) — source: {source} / max {} ({})\n{grid}\nEstimated usage by category\n{} System prompt: {} tokens ({}%)\n{} Project context: {} tokens ({}%)\n{} System tools: {} tokens ({}%)\n{} Skills: {} tokens ({}%)\n{} Messages: {} tokens ({}%)\n{} Free space: {} ({}%)\n{} Autocompact buffer: {} tokens ({}%)\n  reserve: {}  keep: {}\n  set: /context 128k | /context reserve 16k | /context keep 20k  |  clear: /context auto",
            if model.is_empty() { "no model" } else { model },
            f(used),
            f(window),
            pct(used),
            max,
            f(max),
            ic("cell"),
            f(parts.system_prompt),
            pct(parts.system_prompt),
            ic("cell"),
            f(parts.project_context),
            pct(parts.project_context),
            ic("cell"),
            f(parts.tools),
            pct(parts.tools),
            ic("cell"),
            f(parts.skills),
            pct(parts.skills),
            ic("cell"),
            f(parts.messages),
            pct(parts.messages),
            ic("cell_free"),
            f(free),
            pct(free),
            ic("cell_buffer"),
            f(reserve),
            pct(reserve),
            f(reserve),
            f(keep),
        )
    }
    let Some(val) = direct else {
        // Bare `/context`: modal in the TUI, static breakdown on pipes.
        if tui.is_some() {
            let model = config.model.clone().unwrap_or_default();
            let breakdown = collect_parts(cwd, agent, tui);
            let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
            let res = with_modal_sync(tui, || {
                crate::setup::run_context_modal(config, &breakdown, &model, bg.as_ref())
            });
            match res {
                Ok(true) => emit(breakdown_text(config, &collect_parts(cwd, agent, tui)), tui, false),
                Ok(false) => {}
                Err(e) => emit(format!("context error: {e}"), tui, false),
            }
        } else {
            emit(breakdown_text(config, &collect_parts(cwd, agent, tui)), tui, false);
        }
        return;
    };
    let lower = val.trim().to_lowercase();
    if lower.is_empty() || lower == "status" || lower == "show" {
        emit(breakdown_text(config, &collect_parts(cwd, agent, tui)), tui, false);
        return;
    }
    if lower == "auto" || lower == "clear" || lower == "reset" || lower == "0" {
        config.context_window = None;
        crate::setup::set_user_context_window(None);
        persist_window(config);
        // re-prime cache if model present
        if let Some(m) = config.model.clone() {
            if crate::setup::get_cached_model_context(&m).is_none() {
                let base = config.base_url.clone();
                let key = config.api_key.clone();
                tokio::spawn(async move {
                    crate::setup::fetch_live_provider_models(&base, key.as_deref());
                });
            }
        }
        let effective = crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
        emit(
            format!(
                "context window cleared → auto ({} / {})",
                effective,
                crate::setup::format_context_length(effective)
            ),
            tui,
            true,
        );
        return;
    }
    // Subcommands: `reserve <n|auto|default>` and `keep <n|auto|default|off`
    let mut parts = lower.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    if head == "reserve" || head == "keep" {
        let is_reserve = head == "reserve";
        if rest.is_empty() || rest == "status" || rest == "show" {
            let window =
                crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
            let cur = if is_reserve {
                crate::setup::user_reserve_tokens_for(window)
            } else {
                crate::setup::user_keep_for(window)
            };
            emit(
                format!("{head}: {} ({})", cur, crate::setup::format_context_length(cur)),
                tui,
                false,
            );
            return;
        }
        if rest == "auto" || rest == "clear" || rest == "reset" || rest == "default" {
            if is_reserve {
                config.context_reserve = None;
                crate::setup::set_user_reserve_tokens(None);
            } else {
                config.context_keep = None;
                crate::setup::set_user_keep_recent_tokens(None);
            }
            persist_window(config);
            emit(format!("{head} cleared → auto"), tui, true);
            return;
        }
        if !is_reserve && (rest == "off" || rest == "0") {
            config.context_keep = Some(0);
            crate::setup::set_user_keep_recent_tokens(Some(0));
            persist_window(config);
            emit("keep set to 0 (summary only)".to_string(), tui, true);
            return;
        }
        match crate::setup::parse_context_window(rest) {
            Some(n) if n > 0 => {
                if is_reserve {
                    config.context_reserve = Some(n);
                    crate::setup::set_user_reserve_tokens(Some(n));
                } else {
                    config.context_keep = Some(n);
                    crate::setup::set_user_keep_recent_tokens(Some(n));
                }
                persist_window(config);
                emit(
                    format!("{head} set to {n} tokens ({})", crate::setup::format_context_length(n)),
                    tui,
                    true,
                );
            }
            _ => emit(
                format!("invalid value '{rest}' — use e.g. 16k, 20000, or 'auto' to clear"),
                tui,
                false,
            ),
        }
        return;
    }
    match crate::setup::parse_context_window(&val) {
        Some(n) if n > 0 => {
            let model = config.model.as_deref().unwrap_or("");
            let max = crate::setup::model_max_context(model);
            let (final_n, clamped) = if n > max { (max, true) } else { (n, false) };
            config.context_window = Some(final_n);
            crate::setup::set_user_context_window(Some(final_n));
            persist_window(config);
            let msg = if clamped {
                format!(
                    "context window clamped to model max {} ({}) — requested {}",
                    final_n,
                    crate::setup::format_context_length(final_n),
                    crate::setup::format_context_length(n)
                )
            } else {
                format!(
                    "context window set to {} tokens ({}) — overrides auto-fetched value",
                    final_n,
                    crate::setup::format_context_length(final_n)
                )
            };
            emit(msg, tui, true);
        }
        _ => {
            emit(
                format!("invalid value '{val}' — use e.g. 128000, 128k, 1m, or 'auto' to clear"),
                tui,
                false,
            );
        }
    }
}

/// Handles the `/compact` / `/compress` command family.
async fn handle_compact(
    config: &Config,
    cwd: &Path,
    custom_instructions: Option<String>,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if agent.is_none() {
        reload_agent(agent, config, cwd).await;
    }
    let Some(ag) = agent.as_mut() else {
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim("└ error: agent could not be initialized".to_string());
        } else {
            println!("error: agent could not be initialized");
        }
        return;
    };

    let messages = ag.messages().to_vec();
    if messages.is_empty() {
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim("└ nothing to compact (conversation is empty)".to_string());
        } else {
            println!("nothing to compact (conversation is empty)");
        }
        return;
    }

    let msg_count = messages.len();

    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_status(Some("Compacting conversation context"));
    }

    let compact_res =
        crate::compact::compact_with_keep(ag, custom_instructions.as_deref(), crate::setup::user_keep_recent_tokens()).await;

    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_status(None);
    }

    match compact_res {
        Ok(true) => {
            // Record to session storage if active (helper already set_messages)
            if let Some(state) = session_state {
                for msg in ag.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }

            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!(
                    "└ compressed context ({} turns -> structured summary)",
                    msg_count
                ));
            } else {
                println!("compressed context ({} turns -> structured summary)", msg_count);
            }
        }
        Ok(false) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim("└ nothing to compact (conversation is empty)".to_string());
            } else {
                println!("nothing to compact (conversation is empty)");
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("└ compaction failed: {e}"));
            } else {
                println!("compaction failed: {e}");
            }
        }
    }
}

async fn handle_cron(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    let trimmed = raw.trim();
    // Strip leading "/cron" and trim
    let args_str = trimmed.strip_prefix("/cron").unwrap_or("").trim();
    if args_str.is_empty() || args_str == "list" {
        let jobs = gray_cron::list_jobs();
        let msg = if jobs.is_empty() {
            "no cron jobs — create one with: /cron create --schedule \"every 30m\" --prompt \"...\"".to_string()
        } else {
            let mut out = format!("{:<10} {:<20} {:<16} {}\n", "ID", "NAME", "SCHEDULE", "NEXT RUN");
            out.push_str(&"-".repeat(70));
            out.push('\n');
            for j in jobs {
                let next = j.next_run.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_else(|| "-".to_string());
                out.push_str(&format!("{:<10} {:<20} {:<16} {}\n", j.id, j.name, j.schedule, next));
            }
            out
        };
        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(msg); } else { println!("{msg}"); }
        return;
    }
    if args_str.starts_with("add ") {
        let input = args_str.strip_prefix("add ").unwrap().trim().trim_matches(|c| c == '"' || c == '\'');
        if input.is_empty() {
            let msg = "usage: /cron add \"check inbox every 30m\"  — schedule auto-extracted";
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
            return;
        }
        match gray_cron::schedule::split_human_input(input) {
            Some((sched, prompt)) => {
                let name = format!("job-{}", &prompt.chars().take(12).collect::<String>());
                match gray_cron::create_job(name.clone(), sched.clone(), prompt.clone()) {
                    Ok(job) => {
                        let msg = format!("created cron job {} (\"{}\") — schedule: {} — next: {}", job.id, job.name, job.schedule, job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
                        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                        if let Some(shared) = tui {
                            let jobs = gray_cron::list_jobs();
                            if let Some(j) = jobs.iter().filter(|x| x.enabled && x.next_run.is_some()).min_by_key(|x| x.next_run) {
                                if let Ok(mut t) = shared.try_lock() { t.set_next_cron(Some(j.name.clone()), j.next_run); }
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("failed: {e}");
                        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                    }
                }
            }
            None => {
                let msg = format!("could not parse schedule from '{input}' — try 'check inbox every 30m' or 'remind me in 10m'");
                if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
            }
        }
        return;
    }
    if args_str.starts_with("create") {
        // Very simple parse: --schedule <val> --prompt <val> [--name <val>]
        // Supports quoted values via trimming quotes
        fn extract_flag(s: &str, flag: &str) -> Option<String> {
            let pat = format!("{flag} ");
            let start = s.find(&pat)? + pat.len();
            let rest = &s[start..].trim_start();
            if rest.starts_with('"') || rest.starts_with('\'') {
                let q = rest.chars().next().unwrap();
                let end = rest[1..].find(q)? + 1;
                Some(rest[1..end].to_string())
            } else {
                let end = rest.find(" --").unwrap_or(rest.len());
                Some(rest[..end].trim().to_string())
            }
        }
        let schedule = extract_flag(args_str, "--schedule");
        let prompt = extract_flag(args_str, "--prompt");
        let name = extract_flag(args_str, "--name");
        match (schedule, prompt) {
            (Some(s), Some(p)) => {
                let n = name.unwrap_or_else(|| format!("job-{}", &p.chars().take(12).collect::<String>()));
                match gray_cron::parse_schedule(&s) {
                    Ok(_) => match gray_cron::create_job(n.clone(), s.clone(), p.clone()) {
                        Ok(job) => {
                            let msg = format!("created cron job {} (\"{}\") — schedule: {} — next: {}", job.id, job.name, job.schedule, job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
                            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                            if let Some(shared) = tui {
                                let jobs = gray_cron::list_jobs();
                                if let Some(j) = jobs.iter().filter(|x| x.enabled && x.next_run.is_some()).min_by_key(|x| x.next_run) {
                                    if let Ok(mut t) = shared.try_lock() { t.set_next_cron(Some(j.name.clone()), j.next_run); }
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("failed: {e}");
                            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                        }
                    },
                    Err(e) => {
                        let msg = format!("invalid schedule: {e}");
                        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                    }
                }
            }
            _ => {
                // Try human shorthand: "/cron create check inbox every 30m"
                let raw = args_str.strip_prefix("create").unwrap().trim().trim_matches(|c| c == '"' || c == '\'');
                if !raw.is_empty() && !raw.starts_with("--") {
                    if let Some((sched, prompt)) = gray_cron::schedule::split_human_input(raw) {
                        let name = format!("job-{}", &prompt.chars().take(12).collect::<String>());
                        match gray_cron::create_job(name.clone(), sched.clone(), prompt.clone()) {
                            Ok(job) => {
                                let msg = format!("created cron job {} (\"{}\") — schedule: {} — next: {}", job.id, job.name, job.schedule, job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string()));
                                if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                                if let Some(shared) = tui {
                                    let jobs = gray_cron::list_jobs();
                                    if let Some(j) = jobs.iter().filter(|x| x.enabled && x.next_run.is_some()).min_by_key(|x| x.next_run) {
                                        if let Ok(mut t) = shared.try_lock() { t.set_next_cron(Some(j.name.clone()), j.next_run); }
                                    }
                                }
                                return;
                            }
                            Err(e) => {
                                let msg = format!("failed: {e}");
                                if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                                return;
                            }
                        }
                    }
                }
                let msg = "usage: /cron create --schedule \"every 30m\" --prompt \"...\" [--name myjob]  or /cron add \"check inbox every 30m\"  or /cron create \"check inbox every 30m\"";
                if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
            }
        }
        return;
    }
    if let Some(id) = args_str.strip_prefix("remove ").map(|s| s.trim()) {
        if id.is_empty() {
            let msg = "usage: /cron remove <id|name>";
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        } else {
            match gray_cron::remove_job(id) {
                Ok(true) => {
                    let msg = format!("removed {id}");
                    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                    if let Some(shared) = tui {
                        let jobs = gray_cron::list_jobs();
                        if let Some(j) = jobs.iter().filter(|x| x.enabled && x.next_run.is_some()).min_by_key(|x| x.next_run) {
                            if let Ok(mut t) = shared.try_lock() { t.set_next_cron(Some(j.name.clone()), j.next_run); }
                        } else if let Ok(mut t) = shared.try_lock() { t.set_next_cron(None, None); }
                    }
                }
                Ok(false) => {
                    let msg = format!("no job found for '{id}'");
                    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                }
                Err(e) => {
                    let msg = format!("error: {e}");
                    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
                }
            }
        }
        return;
    }
    if let Some(id) = args_str.strip_prefix("show ").map(|s| s.trim()) {
        if let Some(j) = gray_cron::find_job(id) {
            let msg = serde_json::to_string_pretty(&j).unwrap_or_else(|_| format!("{j:?}"));
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(msg); } else { println!("{msg}"); }
        } else {
            let msg = format!("no job found for '{id}'");
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        }
        return;
    }
    // Fallback help
    let msg = "cron: /cron list | /cron add \"check inbox every 30m\" | /cron create --schedule \"every 10m\" --prompt \"...\" | /cron remove <id> | /cron show <id>  (also \"in 10m\", \"0 9 * * *\")";
    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
}

static PROXY_HANDLE: StdMutex<Option<tokio::task::JoinHandle<()>>> = StdMutex::new(None);

/// What `/gateway ...` should do. Parsed by [`parse_gateway_args`].
#[derive(Debug, PartialEq)]
enum GatewayAction {
    Status,
    Connect(gray_gateway::config::Platform, String),
    Disconnect(gray_gateway::config::Platform),
    Enable(gray_gateway::config::Platform),
    Run,
    Stop,
    Autostart(bool),
    Install,
    Uninstall,
    Help,
}

/// Parses `/gateway [sub] [args]` — bare or unknown subcommands default to
/// Status/Help so a mistyped command never silently starts or stops anything.
fn parse_gateway_args(raw: &str) -> GatewayAction {
    let mut toks = raw.split_whitespace().skip(1); // drop "/gateway"
    match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
        None | Some("status") => GatewayAction::Status,
        Some("run") => GatewayAction::Run,
        Some("stop") => GatewayAction::Stop,
        Some("autostart") => match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("on") | Some("true") | Some("enable") => GatewayAction::Autostart(true),
            Some("off") | Some("false") | Some("disable") => GatewayAction::Autostart(false),
            _ => GatewayAction::Help,
        },
        Some("install") => GatewayAction::Install,
        Some("uninstall") => GatewayAction::Uninstall,
        Some("help") => GatewayAction::Help,
        Some("connect") => match (toks.next(), toks.next()) {
            (Some(p), Some(tok)) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Connect(plat, tok.to_string()),
                Err(_) => GatewayAction::Help,
            },
            _ => GatewayAction::Help,
        },
        Some("disconnect") => match toks.next() {
            Some(p) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Disconnect(plat),
                Err(_) => GatewayAction::Help,
            },
            None => GatewayAction::Help,
        },
        Some("enable") => match toks.next() {
            Some(p) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Enable(plat),
                Err(_) => GatewayAction::Help,
            },
            None => GatewayAction::Help,
        },
        Some(_) => GatewayAction::Help,
    }
}

/// One human line per known platform: enabled/disabled. `running` is the
/// in-process daemon state (systemd status is reported separately by callers).
fn gateway_status_lines(cfg: &gray_gateway::config::GatewayConfig, running: bool) -> Vec<String> {
    use gray_gateway::config::Platform;
    let mut lines = vec![format!(
        "gateway {} — config: {}",
        if running { "connected (in-process)" } else { "not running" },
        gray_gateway::config::gray_gateway_path().map(|p| p.display().to_string()).unwrap_or_default(),
    )];
    for plat in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        let state = match cfg.platforms.get(&plat) {
            Some(pc) if pc.enabled => "enabled",
            Some(pc) if pc.token.as_ref().is_some_and(|t| !t.is_empty()) => "disabled (token saved)",
            _ => "disabled",
        };
        lines.push(format!("  {}: {state}", plat.label()));
    }
    lines.push(format!("  autostart: {}", if cfg.autostart { "on" } else { "off" }));
    lines.push("usage: /gateway connect <platform> <token> | enable <platform> | disconnect <platform> | run | stop | autostart on|off | install | uninstall | status".to_string());
    lines
}

/// Starts the gateway daemon in-process (shared by /gateway run and launch
/// autostart). Returns false when already running.
fn start_gateway_in_background(tui: Option<&crate::composer::SharedTui>) -> bool {
    let already = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
    if already {
        return false;
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tui_arc = tui.cloned();
    let h = tokio::spawn(async move {
        let res = gray_gateway::daemon::run_gateway_shutdown(rx).await;
        if let Err(e) = &res {
            log::warn!("gateway exited: {e}");
        }
        GATEWAY_HANDLE.lock().ok().and_then(|mut g| g.take());
        if let Some(shared) = tui_arc {
            if let Ok(mut t) = shared.lock() {
                match res {
                    Ok(()) => t.push_dim("└ gateway stopped".to_string()),
                    Err(e) => t.push_dim(format!("└ gateway exited: {e}")),
                }
                let _ = t.draw();
            }
        }
    });
    *GATEWAY_HANDLE.lock().unwrap() = Some((h, tx));
    true
}

/// Enables `plat` in `cfg` with `token` (mutates in place; caller saves).
fn apply_connect(cfg: &mut gray_gateway::config::GatewayConfig, plat: gray_gateway::config::Platform, token: &str) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = true;
    pc.token = Some(token.to_string());
}

/// Disables `plat` but keeps its token so re-enabling doesn't ask again.
fn apply_disconnect(cfg: &mut gray_gateway::config::GatewayConfig, plat: gray_gateway::config::Platform) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = false;
}

/// Re-enables `plat` with its saved token. Returns false when no token is
/// stored (caller should ask for `/gateway connect <platform> <token>`).
fn apply_enable(cfg: &mut gray_gateway::config::GatewayConfig, plat: gray_gateway::config::Platform) -> bool {
    let Some(pc) = cfg.platforms.get_mut(&plat) else { return false; };
    if pc.token.as_ref().is_some_and(|t| !t.is_empty()) {
        pc.enabled = true;
        true
    } else {
        false
    }
}

type GatewayHandle = (tokio::task::JoinHandle<()>, tokio::sync::oneshot::Sender<()>);
static GATEWAY_HANDLE: StdMutex<Option<GatewayHandle>> = StdMutex::new(None);

async fn handle_gateway(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    match parse_gateway_args(raw) {
        GatewayAction::Status => {
            let cfg = gray_gateway::config::load_gateway_config();
            let running = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
            for line in gateway_status_lines(&cfg, running) {
                say(tui, &line);
            }
            // systemd state, best-effort (mirrors gray_gateway::systemd::status)
            if let Ok(out) = std::process::Command::new("systemctl")
                .args(["--user", "is-active", "gray-gateway.service"])
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                say(tui, &format!("systemd unit: {s}"));
            }
        }
        GatewayAction::Connect(plat, token) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_connect(&mut cfg, plat, &token);
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(tui, &format!("{} connected — token saved to ~/.gray/gateway.yaml (start with /gateway run or /gateway install)", plat.label())),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Disconnect(plat) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_disconnect(&mut cfg, plat);
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(tui, &format!("{} disabled", plat.label())),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Enable(plat) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            if apply_enable(&mut cfg, plat) {
                match gray_gateway::config::save_gateway_config(&cfg) {
                    Ok(()) => say(tui, &format!("{} enabled (saved token)", plat.label())),
                    Err(e) => say(tui, &format!("gateway config error: {e}")),
                }
            } else {
                say(tui, &format!("no saved token for {} — use /gateway connect {plat} <token>", plat.label()));
            }
        }
        GatewayAction::Run => {
            if start_gateway_in_background(tui) {
                say(tui, "gateway starting — platforms connect in background (~45s timeout each)");
            } else {
                say(tui, "gateway already running");
            }
        }
        GatewayAction::Autostart(on) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            cfg.autostart = on;
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(tui, &format!("gateway autostart {}", if on { "on — starts with gray" } else { "off" })),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Stop => {
            let mut g = GATEWAY_HANDLE.lock().ok();
            if let Some((h, tx)) = g.as_mut().and_then(|g| g.take()) {
                let _ = tx.send(());
                h.abort();
                say(tui, "gateway stopped");
            } else {
                say(tui, "gateway not running in this session (if installed as a service: /gateway uninstall)");
            }
        }
        GatewayAction::Install => {
            match with_modal_sync(tui, gray_gateway::systemd::install) {
                Ok(()) => say(tui, "gateway installed as systemd user service"),
                Err(e) => say(tui, &format!("gateway install failed: {e}")),
            }
        }
        GatewayAction::Uninstall => {
            match with_modal_sync(tui, gray_gateway::systemd::uninstall) {
                Ok(()) => say(tui, "gateway systemd service removed"),
                Err(e) => say(tui, &format!("gateway uninstall failed: {e}")),
            }
        }
        GatewayAction::Help => {
            for line in gateway_status_lines(&gray_gateway::config::load_gateway_config(), false) {
                say(tui, &line);
            }
        }
    }
}

async fn handle_proxy(raw: &str, config: &Config, tui: Option<&crate::composer::SharedTui>) {
    let lower = raw.to_ascii_lowercase();
    let trimmed = raw.trim().to_ascii_lowercase();
    // parse optional port and provider
    let mut port: u16 = 8645;
    let mut provider: Option<String> = None;
    for tok in raw.split_whitespace().skip(1) {
        if let Ok(p) = tok.parse::<u16>() {
            port = p;
        } else if tok.starts_with("--provider=") {
            provider = Some(tok.trim_start_matches("--provider=").to_string());
        } else if tok == "--provider" {
            // next token is provider – handled via provider variable on next iteration not needed for minimal
        } else if matches!(tok.to_ascii_lowercase().as_str(), "xai" | "codex" | "openai" | "openrouter" | "grok") {
            provider = Some(tok.to_string());
        } else if tok.starts_with("--port=") {
            if let Ok(p) = tok.trim_start_matches("--port=").parse::<u16>() {
                port = p;
            }
        }
    }
    // also support --port 8645 form
    let parts: Vec<&str> = raw.split_whitespace().collect();
    for i in 0..parts.len() {
        if parts[i] == "--port" && i + 1 < parts.len() {
            if let Ok(p) = parts[i + 1].parse::<u16>() {
                port = p;
            }
        }
        if parts[i] == "--provider" && i + 1 < parts.len() {
            provider = Some(parts[i + 1].to_string());
        }
    }

    if lower.contains("stop") {
        let mut g = PROXY_HANDLE.lock().ok();
        if let Some(h) = g.as_mut().and_then(|g| g.take()) {
            h.abort();
            let msg = "proxy stopped";
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        } else {
            let msg = "proxy not running";
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        }
        return;
    }

    // — interactive picker like /model: bare /proxy or /proxy start without provider opens modal
    let is_start = lower.contains("start");
    let is_bare = trimmed == "/proxy" || trimmed == "/portal";
    let needs_picker = is_bare || (is_start && provider.is_none() && !lower.contains("--provider"));
    if needs_picker {
        // show picker
        let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
        let picked = match with_modal(tui, crate::setup::run_proxy_menu(config, bg.as_ref())).await {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("proxy picker error: {e}");
                if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { eprintln!("{msg}"); }
                return;
            }
        };
        let Some(picked_provider) = picked else {
            // cancelled — viewport already restored
            return;
        };
        provider = Some(picked_provider);
        // if already running, restart with new provider
        if PROXY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false) {
            if let Some(h) = PROXY_HANDLE.lock().ok().and_then(|mut g| g.take()) {
                h.abort();
            }
        }
        // fall through to start with picked provider
    } else if is_start && PROXY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false) {
        let msg = "proxy already running";
        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        return;
    }

    if is_start || needs_picker {
        let adapter: std::sync::Arc<dyn crate::proxy::UpstreamAdapter> = if let Some(p) = provider.as_deref() {
            match crate::proxy::get_adapter(p) {
                Ok(a) => a,
                Err(e) => {
                    let msg = format!("proxy: {e}");
                    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { eprintln!("{msg}"); }
                    return;
                }
            }
        } else {
            crate::proxy::default_adapter(config)
        };
        if !adapter.is_authenticated() {
            let msg = format!("Not logged into {}. Run /connect first.", adapter.display());
            if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { eprintln!("{msg}"); }
            return;
        }
        let host = "127.0.0.1".to_string();
        let display = adapter.display().to_string();
        let h = tokio::spawn(async move {
            let _ = crate::proxy::run_server(adapter, &host, port).await;
        });
        *PROXY_HANDLE.lock().unwrap() = Some(h);
        let msg = format!("proxy: http://127.0.0.1:{port}/v1 → {display} ✓");
        if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(format!("└ {msg}")); } else { println!("{msg}"); }
        return;
    }
    // status (default) — explicit /proxy status
    let mut out = String::from("proxy status:\n");
    for name in ["openrouter", "xai", "codex"] {
        if let Ok(a) = crate::proxy::get_adapter(name) {
            if a.is_authenticated() {
                out.push_str(&format!("  [{:8}] {} — ready\n", name, a.display()));
            } else {
                out.push_str(&format!("  [{:8}] {} — not logged in\n", name, a.display()));
            }
        }
    }
    let running = PROXY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
    if running {
        out.push_str("  (running on 127.0.0.1:8645)\n");
    } else {
        out.push_str("  (not running — run /proxy to start)\n");
    }
    if let Some(shared) = tui { shared.lock().expect("tui lock").push_dim(out.trim_end().to_string()); } else { println!("{out}"); }
}

fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state {
        println!("\x1b[2mTo resume: gray resume {}\x1b[0m", state.session_id.as_str());
        let _ = std::io::stdout().flush();
    }
}

async fn handle_resume(
    config: &Config,
    cwd: &Path,
    args: ResumeArgs,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    totals: &mut SessionTotals,
    tui: Option<&crate::composer::SharedTui>,
) {
    let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
    let target_id: Option<SessionId> = if let Some(raw) = args.target.as_deref() {
        if let Some(root) = default_root() {
            let store = JsonlSessionStore::new(root);
            if let Some(id) = crate::resume::resolve_prefix(&store, raw, args.all).await {
                Some(id)
            } else {
                let sid = SessionId::new(raw);
                match store.load(&sid).await {
                    Ok(_) => Some(sid),
                    Err(e) => {
                        let msg = format!("no session matching '{raw}': {e}");
                        if let Some(shared) = &tui {
                            shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
                        } else {
                            println!("{msg}");
                        }
                        return;
                    }
                }
            }
        } else {
            None
        }
    } else if args.last {
        let Some(root) = default_root() else { return; };
        let store = JsonlSessionStore::new(root);
        let summaries = store.list().await;
        let cwd_now = std::env::current_dir().ok();
        let filt = if args.all { None } else { cwd_now.as_deref() };
        match crate::resume::latest_summary(&summaries, filt) {
            Some(s) => Some(s.id.clone()),
            None => {
                let msg = if args.all { "no saved sessions" } else { "no saved sessions in this directory (try /resume --all or --all)" };
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
                } else {
                    println!("{msg}");
                }
                return;
            }
        }
    } else {
        let result = with_modal(tui, crate::resume::run_resume_picker(args.all, bg.as_ref())).await;
        match result {
            Ok(Some(id)) => Some(id),
            Ok(None) => return,
            Err(e) => {
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").push_dim(format!("└ resume picker error: {e}"));
                } else {
                    println!("resume picker error: {e}");
                }
                return;
            }
        }
    };

    let Some(sid) = target_id else { return; };
    let Some(root) = default_root() else { return; };
    let store = JsonlSessionStore::new(root);
    match store.load(&sid).await {
        Ok((meta, entries)) => {
            let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
            let n = history.len();
            let model = if meta.model.is_empty() {
                config.model.as_deref().unwrap_or("")
            } else {
                meta.model.as_str()
            };
            match build_agent(config, cwd, Some(sid.as_str())).await {
                Ok(built) => {
                    *agent = Some(built.with_messages(history));
                    *session_state = Some(SessionState { session_id: sid.clone(), store });
                    *totals = SessionTotals::from_entries(&entries, model);
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.replay_session_history(&entries, cwd);
                        t.ensure_gap(1);
                        t.push_dim(format!("\u{2b22} Resumed session {} ({n} messages)", sid.as_str()));
                        t.ensure_gap(1);
                    } else {
                        println!("\x1b[2m\u{2b22} Resumed session {} ({n} messages)\x1b[0m", sid.as_str());
                    }
                }
                Err(e) => {
                    let msg = format!("could not resume (no provider): {e}");
                    if let Some(shared) = &tui {
                        shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
                    } else {
                        println!("{msg}");
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("could not resume session {}: {e}", sid.as_str());
            if let Some(shared) = &tui {
                shared.lock().expect("tui lock").push_dim(format!("└ {msg}"));
            } else {
                println!("{msg}");
            }
        }
    }
}

/// Appends whatever messages reached memory this turn (success, cancel, or
/// error) to the session store, so the JSONL transcript never diverges from
/// in-memory history.
async fn persist_turn_messages(
    session_state: &mut Option<SessionState>,
    agent: &Agent,
    config: &Config,
    cwd: &Path,
    initial_count: usize,
    latest_usage: Option<gray_core::event::Usage>,
) {
    if session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let session_id = SessionId::generate();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        );
        if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
        *session_state = Some(SessionState { store, session_id });
    }
    if let Some(state) = session_state
        && agent.messages().len() > initial_count
    {
        let new_messages = &agent.messages()[initial_count..];
        for (i, msg) in new_messages.iter().enumerate() {
            let usage = if i == new_messages.len() - 1 {
                latest_usage
            } else {
                None
            };
            if let Err(e) = state.store.append_with_usage(&state.session_id, msg, usage).await {
                log::warn!(target: "gray_session", "session append failed: {e}");
            }
        }
    }
}

async fn ensure_session_state(
    session_state: &mut Option<SessionState>,
    config: &Config,
    cwd: &Path,
) {
    if session_state.is_none() {
        if let Some(root) = default_root() {
            let store = JsonlSessionStore::new(root);
            let session_id = SessionId::generate();
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let meta = SessionMeta::new(
                session_id.clone(),
                timestamp,
                cwd.to_path_buf(),
                config.model.clone().unwrap_or_else(|| "unset".into()),
            );
            if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
            *session_state = Some(SessionState { store, session_id });
        }
    }
}

fn dispatch_agent_event(
    ev: &AgentEvent,
    tui_stream: Option<&crate::composer::SharedTui>,
    interactive: bool,
    current_tool_name: &mut Option<String>,
    current_tool_args: &mut Option<serde_json::Value>,
    turn_usage: &mut Option<gray_core::event::Usage>,
    cwd: &Path,
    model: &str,
    totals: &mut SessionTotals,
) {
    if let Some(shared) = tui_stream {
        if let Ok(mut t) = shared.lock() {
            match ev {
                AgentEvent::ThinkingDelta { delta } => t.stream_thinking(delta),
                AgentEvent::TextDelta { delta } => t.stream_text(delta),
                AgentEvent::ToolCallStart { name, .. } => {
                    t.flush_markdown();
                    t.end_thinking();
                    *current_tool_name = Some(name.clone());
                    *current_tool_args = None;
                }
                AgentEvent::ToolCallEnd { args, .. } => {
                    t.end_thinking();
                    *current_tool_args = Some(args.clone());
                }
                AgentEvent::ToolResult { output, is_error, .. } => {
                    let name = current_tool_name.take().unwrap_or_default();
                    let args = current_tool_args.take();
                    if name != "request_user_input" {
                        let lines = crate::tool_fmt::format_tool_result_lines_with_context(
                            &name,
                            args.as_ref(),
                            output,
                            *is_error,
                            Some(cwd),
                        );
                        let header = args
                            .as_ref()
                            .map(|a| crate::tool_fmt::format_tool_call_header(&name, a, Some(cwd)))
                            .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                        t.push_tool_box(header, lines);
                    }
                }
                AgentEvent::StepUsage { usage } => {
                    t.set_usage(*usage);
                }
                AgentEvent::TurnEnd { usage, .. } => {
                    *turn_usage = Some(*usage);
                    t.end_thinking();
                    t.set_usage(*usage);
                    if usage.total() > 0 {
                        totals.add(usage, model);
                        t.push_usage(turn_footer(usage, model, totals));
                    }
                }
                _ => {}
            }
            let _ = std::io::stdout().flush();
            return;
        }
    }
    if !interactive {
        match ev {
            AgentEvent::TextDelta { delta } => print!("{delta}"),
            AgentEvent::ThinkingDelta { delta } => print!("{THINKING_STYLE}{delta}\x1b[0m"),
            AgentEvent::ToolCallStart { name, .. } => {
                *current_tool_name = Some(name.clone());
                *current_tool_args = None;
            }
            AgentEvent::ToolCallEnd { args, .. } => {
                let name = current_tool_name.as_deref().unwrap_or("tool");
                *current_tool_args = Some(args.clone());
                if name != "request_user_input" {
                    println!(
                        "\n{}",
                        crate::tool_fmt::format_tool_call_header_plain(name, args, Some(cwd))
                    );
                }
            }
            AgentEvent::ToolResult { output, is_error, .. } => {
                let name = current_tool_name.take().unwrap_or_default();
                let args = current_tool_args.take();
                let res = crate::tool_fmt::format_tool_result_plain_with_context(
                    &name,
                    args.as_ref(),
                    output,
                    *is_error,
                    Some(cwd),
                );
                if !res.is_empty() {
                    print!("{res}");
                }
            }
            AgentEvent::TurnEnd { usage, .. } => {
                *turn_usage = Some(*usage);
                if usage.total() > 0 {
                    totals.add(usage, model);
                    println!("\n\x1b[2m{}\x1b[0m", turn_footer(usage, model, totals));
                }
            }
            _ => {}
        }
        let _ = std::io::stdout().flush();
    }
}

async fn maybe_threshold_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    latest: Option<gray_core::event::Usage>,
    initial_count: &mut usize,
) {
    let window = crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
    let tokens = crate::compact::estimate_context_tokens(agent.messages(), latest);
    if !crate::compact::should_compact(tokens, window, &crate::compact::compaction_settings_for(window)) {
        return;
    }
    say(
        tui,
        &format!(
            "auto-compacting {}/{} tokens...",
            crate::setup::format_context_length(tokens),
            crate::setup::format_context_length(window)
        ),
    );
    match crate::compact::auto_compact_if_needed(agent, config, latest, "threshold").await {
        Ok(true) => {
            ensure_session_state(session_state, config, cwd).await;
            if let Some(state) = session_state {
                for msg in agent.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }
            *initial_count = agent.messages().len();
        }
        Ok(false) => {},
        Err(e) => log::warn!(target: "gray_compact", "threshold auto-compact failed: {e}"),
    }
}

async fn maybe_overflow_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    latest: Option<gray_core::event::Usage>,
    initial_count: &mut usize,
    err: &CoreError,
) -> bool {
    if !crate::compact::is_context_overflow_error(err) {
        return false;
    }
    // Mid-turn notice (not after a card): keep the separating blank say() used to add.
    if let Some(t) = tui {
        t.lock().expect("tui lock").ensure_gap(1);
    }
    say(tui, "context overflow — compacting...");
    match crate::compact::auto_compact_if_needed(agent, config, latest, "overflow").await {
        Ok(true) => {
            ensure_session_state(session_state, config, cwd).await;
            if let Some(state) = session_state {
                for msg in agent.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }
            *initial_count = agent.messages().len();
            true
        }
        Ok(false) => {
            log::warn!(target: "gray_compact", "overflow auto-compact returned false (nothing to compact)");
            false
        }
        Err(e) => {
            log::warn!(target: "gray_compact", "overflow auto-compact failed: {e}");
            false
        }
    }
}

/// Runs Gray in interactive REPL mode.
pub async fn run_repl_mode(
    config: &mut Config,
    resume_last: bool,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    let cwd = std::env::current_dir()?;

    // Interactive terminals get the ratatui composer; piped input falls back
    // to plain cooked reads (scripts, tests).
    let interactive = std::io::stdin().is_terminal();
    use std::io::IsTerminal;

    // context window: user override > provider live > disk > litellm/models.dev > guess fallback
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    // auto-fetch provider context window in background if not yet cached and no user override
    if crate::setup::get_user_context_window().is_none() {
        tokio::spawn(crate::setup::fetch_litellm_context_windows());
        tokio::spawn(crate::setup::fetch_models_dev_context());
        tokio::spawn(crate::setup::fetch_openrouter_rates());
        if let Some(m) = config.model.clone() {
            if crate::setup::get_cached_model_context(&m).is_none() {
                let base = config.base_url.clone();
                let key = config.api_key.clone();
                tokio::spawn(async move {
                    crate::setup::fetch_live_provider_models(&base, key.as_deref());
                });
            }
        }
    }

    // boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());

    let mut unconfigured = config.model.is_none();
    if unconfigured {
        let ready = crate::setup::run_onboarding(config).await?;
        if !ready {
            print!("\r\x1b[2mrunning without a provider — send a message to set one up (or /provider)\x1b[0m\r\n");
        }
        print!("\r\n");
        // onboarding may have set model/base_url — re-sync context window override and prime cache
        crate::setup::set_user_context_window(config.context_window);
        crate::setup::set_user_reserve_tokens(config.context_reserve);
        crate::setup::set_user_keep_recent_tokens(config.context_keep);
        if crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            tokio::spawn(crate::setup::fetch_models_dev_context());
            if let Some(m) = config.model.clone() {
                if crate::setup::get_cached_model_context(&m).is_none() {
                    let base = config.base_url.clone();
                    let key = config.api_key.clone();
                    tokio::spawn(async move {
                        crate::setup::fetch_live_provider_models(&base, key.as_deref());
                    });
                }
            }
        }
    } else if !interactive {
        crate::tui::print_logo();
        print!("\r\n");
        print!(
            "\r\x1b[1mgray\x1b[0m\x1b[2m {} \u{b7} Run /help for commands\x1b[0m\r\n",
            env!("CARGO_PKG_VERSION")
        );
    }

    // The agent is built lazily so the REPL opens even with no model/key configured;
    // we surface a friendly hint on first use instead of refusing to start.
    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;
    let mut session_totals = SessionTotals::default();
    let mut pending_history: Vec<Message> = Vec::new();
    let mut resumed_session_info: Option<(SessionId, Vec<gray_session::SessionEntry>)> = None;

    // `--session <id>`: reopen that exact session.
    if let Some(id) = session_id
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(id);
        match store.load(&sid).await {
            Ok((meta, entries)) => {
                if config.model.is_none() && !meta.model.is_empty() {
                    config.model = Some(meta.model.clone());
                }
                let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
                pending_history = history.clone();
                if let Ok(built) = build_agent(config, &cwd, Some(sid.as_str())).await {
                    agent = Some(built.with_messages(history));
                }
                session_state = Some(SessionState {
                    session_id: sid.clone(),
                    store,
                });
                session_totals = SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
                resumed_session_info = Some((sid, entries));
            }
            Err(e) => {
                println!("could not resume session {id}: {e}");
            }
        }
    }

    // `-c`: reopen the most recent session instead of starting blank.
    if resume_last
        && session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let summaries = store.list().await;
        let cwd_now = std::env::current_dir().ok();
        if let Some(latest) = crate::resume::latest_summary(&summaries, cwd_now.as_deref()).or_else(|| crate::resume::latest_summary(&summaries, None)) {
            match store.load(&latest.id).await {
                Ok((meta, entries)) => {
                    if config.model.is_none() && !meta.model.is_empty() {
                        config.model = Some(meta.model.clone());
                    }
                    let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
                    pending_history = history.clone();
                    if let Ok(built) = build_agent(config, &cwd, Some(latest.id.as_str())).await {
                        agent = Some(built.with_messages(history));
                    }
                    session_state = Some(SessionState {
                        session_id: latest.id.clone(),
                        store,
                    });
                    session_totals = SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
                    resumed_session_info = Some((latest.id.clone(), entries));
                }
                Err(e) => println!("could not resume: {e}"),
            }
        }
    }

    // Interactive terminals get the ratatui composer; piped input falls back
    // to plain cooked reads (scripts, tests).
    // The composer owns the bottom pane for the whole session. A tiny ticker
    // task refreshes the elapsed-seconds status while turns run.
    let tui = interactive.then(|| {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let shared = std::sync::Arc::new(std::sync::Mutex::new({
                let mut t = crate::composer::Tui::new().expect("composer init");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                }
                if let Some(eff) = &config.thinking_effort {
                    t.set_thinking_effort(eff.clone());
                }
                t.set_hide_thinking(config.reasoning_hidden());
                t.set_cwd(cwd.display().to_string());
                if let Some((ref sid, ref entries)) = resumed_session_info {
                    t.replay_session_history(entries, &cwd);
                    t.ensure_gap(1);
                    t.push_dim(format!("\u{2b22} Resumed session {} ({} messages)", sid.as_str(), entries.len()));
                    t.ensure_gap(1);
                }
                t
            }));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let ticker_stop = stop.clone();
        let ticker_tui = shared.clone();
        tokio::spawn(async move {
            loop {
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                if let Ok(mut t) = ticker_tui.try_lock() {
                    // Stop may have been set while we were waiting to
                    // acquire the lock (blocked on read_line). Don't
                    // repaint the viewport after the main thread has
                    // already started shutdown.
                    if ticker_stop.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    t.tick_status();
                }
            }
        });
        (shared, stop)
    });

    // request_user_input bridge: TUI overlay when interactive, hermes-style
    // stdin prompts when piped.
    let question_bridge: gray_core::questions::QuestionBridge = if interactive {
        let shared = tui.as_ref().map(|(s, _)| s.clone()).expect("interactive implies tui");
        gray_core::questions::QuestionBridge(std::sync::Arc::new(crate::composer::ComposerQuestionAsker { tui: shared }))
    } else {
        gray_core::questions::QuestionBridge(std::sync::Arc::new(gray_tools::StdinQuestionAsker))
    };

    // Cron background — stolen from hermes Scheduler tick (Step 3)
    // Scans every 60s via Scheduler::scan_due_jobs (grace + fast-forward).
    // Footer clock ticks every second via tick_status when next_cron is set.
    {
        let cron_tui = tui.clone();
        tokio::spawn(async move {
            use gray_cron::Scheduler;
            let scheduler = Scheduler::from_active();
            // helper to push next due to footer clock
            let update_footer = |tui_opt: &Option<(crate::composer::SharedTui, std::sync::Arc<std::sync::atomic::AtomicBool>)>| {
                if let Some((shared, _)) = tui_opt {
                    let jobs = gray_cron::list_jobs();
                    let next = jobs
                        .iter()
                        .filter(|j| j.enabled && j.next_run.is_some())
                        .min_by_key(|j| j.next_run)
                        .cloned();
                    if let Ok(mut t) = shared.try_lock() {
                        if let Some(j) = next {
                            t.set_next_cron(Some(j.name.clone()), j.next_run);
                        } else {
                            t.set_next_cron(None, None);
                        }
                    }
                }
            };
            update_footer(&cron_tui);
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.tick().await;
            loop {
                interval.tick().await;
                let due = match scheduler.scan_due_jobs() {
                    Ok(d) => d,
                    Err(_) => {
                        update_footer(&cron_tui);
                        continue;
                    }
                };
                for job in due {
                    let _ = gray_cron::store::update_job_run(&job.id, chrono::Utc::now());
                    if let Some((shared, _)) = cron_tui.as_ref() {
                        if let Ok(mut t) = shared.try_lock() {
                            t.push_dim(format!("⏰ cron '{}' due: {}", job.name, job.prompt));
                        }
                    }
                }
                update_footer(&cron_tui);
            }
        });
    }

    // pi's hideThinkingBlock — toggled with /thinking, session-only.
    // Reasoning is ON by default — user wants to see thinking (high effort).
    // Bare /thinking toggles visibility; picker sets level persisted to config.
    let mut hide_thinking = config.reasoning_hidden();
    // Wire default: if no effort saved yet, enable reasoning so ThinkingDelta
    // actually streams on openrouter/zen etc. Persist once so future sessions
    // keep it without relying on this default branch.
    if config.thinking_effort.is_none() {
        config.thinking_effort = Some("high".to_string());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            if saved.thinking_effort.is_none() {
                saved.thinking_effort = Some("high".to_string());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
        }
    }
    // Gateway autostart (default on): boot the in-process daemon when any
    // platform is enabled. Silent when nothing is configured.
    if let Some((shared, _)) = tui.as_ref() {
        let cfg = gray_gateway::config::load_gateway_config();
        if cfg.autostart && cfg.platforms.values().any(|p| p.enabled) && start_gateway_in_background(Some(shared)) {
            say(Some(shared), "gateway autostarted — /gateway stop to stop, /gateway autostart off to disable");
        }
    }
    let mut pending_command: Option<ReplCommand> = None;
    let mut pending_images: Vec<std::path::PathBuf> = Vec::new();

    loop {
        if pending_command.is_none()
            && let Some((shared, _)) = tui.as_ref()
            && let Ok(mut t) = shared.try_lock()
            && !t.pending_question_answers.is_empty()
        {
            let texts = std::mem::take(&mut t.pending_question_answers);
            pending_command = Some(ReplCommand::Prompt(texts.join("\n\n")));
        }
        let cmd = if let Some(c) = pending_command.take() {
            c
        } else {
            let (line_text, images) = if interactive {
                let (shared, stop) = tui.as_ref().expect("interactive implies tui");
                let (txt, imgs) = {
                    let mut t = shared.lock().expect("tui lock");
                    let pair = match t.read_line()? {
                        Some(v) => v,
                        None => {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            t.shutdown();
                            print_exit_hint(&session_state);
                            break;
                        }
                    };
                    // Quit will shut down immediately after this block;
                    // set the stop flag now while we still hold the lock so
                    // the ticker's try_lock gap can't slip a draw in between.
                    if matches!(parse_command(&pair.0), ReplCommand::Quit) {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    pair
                };
                (txt, imgs)
            } else {
                print!("\u{203a} ");
                std::io::stdout().flush()?;
                let mut buf = String::new();
                if std::io::stdin().read_line(&mut buf)? == 0 {
                    break;
                }
                (buf.trim().to_string(), Vec::new())
            };
            pending_images = images;
            expand_skill_command(parse_command(&line_text), cwd.as_path(), tui.as_ref().map(|(s, _)| s), false)
        };
        // Clear pending images for non-prompt commands (keep for Prompt/Empty+images)
        if !matches!(&cmd, ReplCommand::Prompt(_) | ReplCommand::Empty) {
            pending_images.clear();
        }

        match cmd {
            ReplCommand::Empty => {
                if !pending_images.is_empty() {
                    // image(s) without text: treat as prompt with images
                    let images = std::mem::take(&mut pending_images);
                    let prompt_text = String::new();
                    // fall through to Prompt handling by constructing message immediately
                    // reuse Prompt logic inline
                    if agent.is_none() {
                        if unconfigured {
                            let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                            let result = with_modal(tui.as_ref().map(|(s, _)| s), crate::setup::run_provider_menu(config, bg.as_ref())).await;
                            match result {
                                Ok(true) => {
                                    unconfigured = false;
                                    if let Some((shared, _)) = &tui {
                                        let mut t = shared.lock().expect("tui lock");
                                        if let Some(m) = &config.model {
                                            t.set_model(m.clone());
                                        }
                                        let model_str = config.model.as_deref().unwrap_or("default");
                                        let prov_name = crate::setup::load_catalog()
                                            .ok()
                                            .and_then(|c| {
                                                c.values()
                                                    .find(|p| p.base_url == config.base_url)
                                                    .map(|p| p.name.clone())
                                            })
                                            .unwrap_or_else(|| "provider".to_string());
                                        t.push_dim(format!(
                                            "└ connected to {prov_name} · {model_str}"
                                        ));
                                        let _ = t.draw();
                                    }
                                }
                                Ok(false) => {
                                    if let Some((shared, _)) = &tui {
                                        let _ = shared.lock().expect("tui lock").draw();
                                    }
                                    continue;
                                }
                                Err(e) => {
                                    if let Some((shared, _)) = &tui {
                                        shared
                                            .lock()
                                            .expect("tui lock")
                                            .push_dim(format!("└ provider error: {e}"));
                                    } else {
                                        println!("provider error: {e}");
                                    }
                                    continue;
                                }
                            }
                        }
                        let sid = session_state.as_ref().map(|s| s.session_id.as_str().to_string());
                        let built = build_agent(config, &cwd, sid.as_deref()).await;
                        match built {
                            Ok(built) => {
                                if !pending_history.is_empty() {
                                    agent = Some(built.with_messages(std::mem::take(&mut pending_history)));
                                } else {
                                    agent = Some(built);
                                }
                            }
                            Err(e) => {
                                println!("{e}");
                                continue;
                            }
                        }
                    }
                    let agent = agent.as_mut().expect("agent built above");
                    let cancel = tokio_util::sync::CancellationToken::new();
                    *TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());
                    let ctx = ToolContext { cwd: cwd.clone(), cancel: cancel.clone(), questions: Some(question_bridge.clone()) };
                    let user_msg = build_user_message_with_attachments(&prompt_text, &images);
                    let user_msg_for_retry = user_msg.clone();
                    let mut initial_count = agent.messages().len();
                    {
                        let latest = tui
                            .as_ref()
                            .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage));
                        maybe_threshold_compact(
                            agent,
                            config,
                            &mut session_state,
                            &cwd,
                            tui.as_ref().map(|(s, _)| s),
                            latest,
                            &mut initial_count,
                        )
                        .await;
                    }
                    let (shared, _) = if interactive { (Some(tui.as_ref().expect("interactive implies tui")), ()) } else { (None, ()) };
                    let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
                    if let Some(s) = &tui_stream { s.lock().expect("tui lock").begin_turn("Working"); }
                    let watch_cancel = cancel.clone();
                    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let watcher_stopped = watch_stop.clone();
                    let watcher_tui = tui_stream.clone();
                    let _key_watcher = tokio::task::spawn_blocking(move || {
                        use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
                        loop {
                            if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) { return; }
                            match poll(std::time::Duration::from_millis(50)) { Ok(true) => {} _ => continue, }
                            let Ok(event) = read() else { continue; };
                            if let Event::Resize(cols, _) = event {
                                if let Some(shared) = watcher_tui.as_ref() {
                                    if let Ok(mut t) = shared.try_lock() {
                                        t.pending_resize = Some((cols, std::time::Instant::now() + std::time::Duration::from_millis(75)));
                                    }
                                }
                                continue;
                            }
                            if let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event {
                                if kind == KeyEventKind::Release { continue; }
                                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) { watch_cancel.cancel(); return; }
                                // request_user_input overlay owns the keyboard while active
                                if let Some(shared) = watcher_tui.as_ref()
                                    && let Ok(mut t) = shared.try_lock()
                                    && t.active_question.is_some()
                                {
                                    crate::composer::handle_question_key(&mut t, code, modifiers);
                                    continue;
                                }
                                if code == KeyCode::Esc { watch_cancel.cancel(); return; }
                            }
                        }
                    });
                    let mut current_tool_name: Option<String> = None;
                    let mut current_tool_args: Option<serde_json::Value> = None;
                    let mut turn_usage: Option<gray_core::event::Usage> = None;
                    let mut run_result = {
                        let mut on_event = |ev: &gray_core::event::AgentEvent| {
                            dispatch_agent_event(ev, tui_stream.as_ref(), interactive, &mut current_tool_name, &mut current_tool_args, &mut turn_usage, &cwd, config.model.as_deref().unwrap_or(""), &mut session_totals);
                        };
                        let mut run_future = Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
                        tokio::select! { res = &mut run_future => res, _ = cancel.cancelled() => Err(gray_core::error::CoreError::Cancelled), }
                    };
                    // overflow recovery (one retry only)
                    if let Err(ref e) = run_result {
                        let latest = tui
                            .as_ref()
                            .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage))
                            .or(turn_usage);
                        if maybe_overflow_compact(
                            agent,
                            config,
                            &mut session_state,
                            &cwd,
                            tui.as_ref().map(|(s, _)| s),
                            latest,
                            &mut initial_count,
                            e,
                        )
                        .await
                        {
                                current_tool_name = None;
                                current_tool_args = None;
                                let ctx2 = gray_core::agent::ToolContext {
                                    cwd: cwd.clone(),
                                    cancel: cancel.clone(),
                                    questions: Some(question_bridge.clone()),
                                };
                                let mut on_event2 = |ev: &gray_core::event::AgentEvent| {
                                    dispatch_agent_event(ev, tui_stream.as_ref(), interactive, &mut current_tool_name, &mut current_tool_args, &mut turn_usage, &cwd, config.model.as_deref().unwrap_or(""), &mut session_totals);
                                };
                                let mut run_future2 = Box::pin(agent.run_streaming(user_msg_for_retry.clone(), ctx2, &mut on_event2));
                                let retry_res = tokio::select! { res = &mut run_future2 => res, _ = cancel.cancelled() => Err(gray_core::error::CoreError::Cancelled), };
                                run_result = retry_res;
                        }
                    }
                    TURN_STATE.lock().expect("turn state lock").take();
                    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    match run_result {
                        Ok(_) => {
                            persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                        }
                        Err(gray_core::error::CoreError::Cancelled) => {
                            persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                            if interactive {
                                if let Some((shared, _)) = &tui {
                                    let mut t = shared.lock().expect("tui lock");
                                    t.end_thinking();
                                    t.stream("(interrupted)\n");
                                }
                            } else {
                                println!("(interrupted)");
                            }
                        }
                        Err(e) => {
                            persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                            let msg = format_core_error(&e, &config.base_url);
                            if interactive {
                                if let Some((shared, _)) = &tui {
                                    let mut t = shared.lock().expect("tui lock");
                                    t.end_thinking();
                                    t.stream(&format!("{msg}\n"));
                                }
                            } else {
                                eprintln!("{msg}");
                            }
                        }
                    }
                    if let Some(s) = &tui_stream {
                        s.lock().expect("tui lock").end_turn();
                    }
                    continue;
                }
                pending_images.clear();
                continue;
            }
            ReplCommand::Quit => {
                if let Some((shared, stop)) = &tui {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    let mut t = shared.lock().expect("tui lock");
                    t.shutdown();
                    print_exit_hint(&session_state);
                } else {
                    print_exit_hint(&session_state);
                }
                break;
            }
            ReplCommand::Sys(action) => {
                handle_sys(config, &cwd, action, &mut agent, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Model(direct) => {
                handle_model(config, &cwd, direct, &mut agent, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Help => {
                if let Some((shared, _)) = &tui {
                    let mut out = String::new();
                    for (name, desc) in COMMANDS {
                        out.push_str(&format!("  /{name:<10} {desc}\n"));
                    }
                    shared.lock().expect("tui lock").push_dim(out.trim_end().to_string());
                } else {
                    println!("{}", crate::rule("commands"));
                    for (name, desc) in COMMANDS {
                        println!("  /{name:<8} {desc}");
                    }
                }
                continue;
            }
            ReplCommand::Resume(args) => {
                handle_resume(config, &cwd, args, &mut agent, &mut session_state, &mut session_totals, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::New(initial_prompt) => {
                pending_history.clear();
                session_totals = SessionTotals::default();
                session_state = None;
                let mut short_id = String::new();
                let mut new_sid: Option<SessionId> = None;
                if let Some(root) = default_root() {
                    let store = JsonlSessionStore::new(root);
                    let session_id = SessionId::generate();
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let meta = SessionMeta::new(
                        session_id.clone(),
                        timestamp,
                        cwd.to_path_buf(),
                        config.model.clone().unwrap_or_else(|| "unset".into()),
                    );
                    if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
                    short_id = session_id.as_str().split('-').next().unwrap_or("new").to_string();
                    new_sid = Some(session_id.clone());
                    session_state = Some(SessionState { store, session_id });
                }
                // Build with the new session id so the prompt-cache shard
                // survives future resumes of this session.
                agent = build_agent(config, &cwd, new_sid.as_ref().map(|s| s.as_str())).await.ok();

                if let Some((shared, _)) = &tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.reset_usage();
                    let detail = if !short_id.is_empty() {
                        Some(format!("({short_id})"))
                    } else {
                        None
                    };
                    t.push_action("New conversation started", detail.as_deref());
                } else {
                    if !short_id.is_empty() {
                        println!("✓ New conversation started ({short_id})");
                    } else {
                        println!("✓ New conversation started");
                    }
                }

                if let Some(prompt_text) = initial_prompt {
                    if let Some((shared, _)) = &tui {
                        shared.lock().expect("tui lock").push_user_prompt(&prompt_text, &[], !prompt_text.starts_with('/'));
                    } else {
                        println!("❯ {prompt_text}");
                    }
                    pending_command = Some(ReplCommand::Prompt(prompt_text));
                }
                continue;
            }
            ReplCommand::Compact(instructions) => {
                handle_compact(
                    config,
                    &cwd,
                    instructions,
                    &mut agent,
                    &mut session_state,
                    tui.as_ref().map(|(s, _)| s),
                ).await;
                continue;
            }
            ReplCommand::Thinking(level) => {
                handle_thinking(config, &cwd, level, &mut agent, tui.as_ref().map(|(s, _)| s), &mut hide_thinking).await;
                continue;
            }
            ReplCommand::ContextWindow(val) => {
                handle_context_window(config, &cwd, &agent, val, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Usage => {
                handle_usage(&session_totals, config, tui.as_ref().map(|(s, _)| s));
                continue;
            }
            ReplCommand::Provider => {
                let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                let result = with_modal(tui.as_ref().map(|(s, _)| s), crate::setup::run_provider_menu(config, bg.as_ref())).await;
                match result {
                    Ok(true) => {
                        unconfigured = false;
                        if let Some((shared, _)) = &tui {
                            let mut t = shared.lock().expect("tui lock");
                            if let Some(m) = &config.model {
                                t.set_model(m.clone());
                            }
                            let model_str = config.model.as_deref().unwrap_or("default");
                            let catalog = crate::setup::load_catalog().ok();
                            let prov_name = catalog
                                .as_ref()
                                .and_then(|c| c.values().find(|p| p.base_url == config.base_url))
                                .map(|p| p.name.as_str())
                                .unwrap_or("provider");
                            t.push_dim(format!("└ connected to {prov_name} · {model_str}"));
                            let _ = t.draw();
                        }
                        reload_agent(&mut agent, config, &cwd).await;
                    }
                    Ok(false) => {
                        if let Some((shared, _)) = &tui {
                            let mut t = shared.lock().expect("tui lock");
                            t.textarea.set_text("");
                            t.matches.clear();
                            t.sel = 0;
                            t.history_idx = None;
                            t.draft.clear();
                            t.attachments.clear();
                            t.pending_pastes.clear();
                            let _ = t.draw();
                        }
                    }
                    Err(e) => {
                        if let Some((shared, _)) = &tui {
                            shared.lock().expect("tui lock").push_dim(format!("└ error: {e}"));
                        } else {
                            println!("provider error: {e}");
                        }
                    }
                }
                continue;
            }
            ReplCommand::Cron(raw) => {
                handle_cron(&raw, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Skill(_) => {
                // fully expanded into Prompt/Empty by expand_skill_command; defensive no-op
                continue;
            }
            ReplCommand::Proxy(raw) => {
                handle_proxy(&raw, config, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Gateway(raw) => {
                let t = tui.as_ref().map(|(s, _)| s);
                // bare /gateway in the TUI opens the interactive picker; the
                // modal returns an equivalent command string to execute
                let cmd = if raw.trim() == "/gateway" && t.is_some() {
                    let bg = t.map(|s| s.lock().expect("tui lock").snapshot());
                    let running = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
                    match with_modal_sync(t, || crate::setup::run_gateway_modal(bg.as_ref(), running)) {
                        Ok(Some(c)) => c,
                        _ => String::new(),
                    }
                } else {
                    raw.clone()
                };
                if !cmd.is_empty() {
                    handle_gateway(&cmd, t).await;
                }
                continue;
            }
            ReplCommand::Unknown(cmd) => {
                say(tui.as_ref().map(|(s, _)| s), &format!("unknown command '{cmd}' — type /help for available commands"));
                continue;
            }
            ReplCommand::Prompt(prompt_text) => {
                if agent.is_none() {
                    if unconfigured {
                        let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                        let result = with_modal(tui.as_ref().map(|(s, _)| s), crate::setup::run_provider_menu(config, bg.as_ref())).await;
                        match result {
                            Ok(true) => {
                                unconfigured = false;
                                if let Some((shared, _)) = &tui {
                                    let mut t = shared.lock().expect("tui lock");
                                    if let Some(m) = &config.model {
                                        t.set_model(m.clone());
                                    }
                                    let model_str = config.model.as_deref().unwrap_or("default");
                                    let prov_name = crate::setup::load_catalog()
                                        .ok()
                                        .and_then(|c| {
                                            c.values()
                                                .find(|p| p.base_url == config.base_url)
                                                .map(|p| p.name.clone())
                                        })
                                        .unwrap_or_else(|| "provider".to_string());
                                    t.push_dim(format!(
                                        "└ connected to {prov_name} · {model_str}"
                                    ));
                                    let _ = t.draw();
                                }
                            }
                            Ok(false) => {
                                if let Some((shared, _)) = &tui {
                                    let _ = shared.lock().expect("tui lock").draw();
                                }
                                continue;
                            }
                            Err(e) => {
                                if let Some((shared, _)) = &tui {
                                    shared
                                        .lock()
                                        .expect("tui lock")
                                        .push_dim(format!("└ provider error: {e}"));
                                } else {
                                    println!("provider error: {e}");
                                }
                                continue;
                            }
                        }
                    }
                    let sid = session_state.as_ref().map(|s| s.session_id.as_str().to_string());
                    let built = build_agent(config, &cwd, sid.as_deref()).await;
                    match built {
                        Ok(built) => {
                            if !pending_history.is_empty() {
                                agent = Some(built.with_messages(std::mem::take(&mut pending_history)));
                            } else {
                                agent = Some(built);
                            }
                        }
                        Err(e) => {
                            println!("{e}");
                            continue;
                        }
                    }
                }
                let agent = agent.as_mut().expect("agent built above");
                let cancel = tokio_util::sync::CancellationToken::new();
                *TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());
                let ctx = ToolContext {
                    cwd: cwd.clone(),
                    cancel: cancel.clone(),
                    questions: Some(question_bridge.clone()),
                };
                let images = std::mem::take(&mut pending_images);
                let user_msg = build_user_message_with_attachments(&prompt_text, &images);
                let user_msg_for_retry = user_msg.clone();
                let mut initial_count = agent.messages().len();
                {
                    let latest = tui
                        .as_ref()
                        .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage));
                    maybe_threshold_compact(
                        agent,
                        config,
                        &mut session_state,
                        &cwd,
                        tui.as_ref().map(|(s, _)| s),
                        latest,
                        &mut initial_count,
                    )
                    .await;
                }

                let (shared, _) = if interactive {
                    (Some(tui.as_ref().expect("interactive implies tui")), ())
                } else { (None, ()) };

                // status row on; events stream straight into the composer
                let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
                if let Some(s) = &tui_stream {
                    s.lock().expect("tui lock").begin_turn("Working");
                }

                // Raw mode swallows ^C (no SIGINT), so a watcher must read
                // key events during the turn and translate Ctrl-C into cancel.
                let watch_cancel = cancel.clone();
                let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let watcher_stopped = watch_stop.clone();
                let watcher_tui = tui_stream.clone();
                let cwd_for_watcher = cwd.clone();
                    let _key_watcher = tokio::task::spawn_blocking(move || {
                    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
                    loop {
                        if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        match poll(std::time::Duration::from_millis(50)) {
                            Ok(true) => {}
                            _ => continue,
                        }
                        let Ok(event) = read() else { continue; };
                        match event {
                            Event::Resize(cols, _) => {
                                if let Some(shared) = watcher_tui.as_ref() {
                                    if let Ok(mut t) = shared.try_lock() {
                                        t.pending_resize = Some((cols, std::time::Instant::now() + std::time::Duration::from_millis(75)));
                                    }
                                }
                            }
                            Event::Key(KeyEvent { code, modifiers, kind, .. }) => {
                                if kind == KeyEventKind::Release {
                                    continue;
                                }
                                // Ctrl+C always cancels turn
                                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                                    watch_cancel.cancel();
                                    return;
                                }
                                // request_user_input overlay owns the keyboard while active
                                if let Some(shared) = watcher_tui.as_ref()
                                    && let Ok(mut t) = shared.try_lock()
                                    && t.active_question.is_some()
                                {
                                    crate::composer::handle_question_key(&mut t, code, modifiers);
                                    continue;
                                }
                                // Esc: dismiss popup if open; else if the draft is a
                                // slash command, cancel the turn and run it locally
                                // (echoed as a sent message, never fed to the AI);
                                // else plain cancel.
                                if code == KeyCode::Esc {
                                    if let Some(shared) = watcher_tui.as_ref()
                                        && let Ok(mut t) = shared.try_lock()
                                    {
                                        if !t.matches.is_empty() {
                                            t.matches.clear();
                                            t.sel = 0;
                                            let _ = t.draw();
                                            continue;
                                        }
                                        let mut text = t.textarea.text().to_string();
                                        for (ph, full) in &t.pending_pastes { text = text.replace(ph, full); }
                                        let text = text.trim().to_string();
                                        if text.starts_with('/') && !text.contains('\n') {
                                            t.push_user_prompt(&text, &[], false);
                                            t.local_command = Some(text);
                                            t.textarea.set_text("");
                                            t.attachments.clear();
                                            t.pending_pastes.clear();
                                            t.history_idx = None;
                                            t.sel = 0;
                                            t.matches.clear();
                                            let _ = t.draw();
                                            watch_cancel.cancel();
                                            return;
                                        }
                                    }
                                    watch_cancel.cancel();
                                    return;
                                }
                                // When a turn is running, allow typing and queue on Enter
                                let Some(shared) = watcher_tui.as_ref() else { continue; };
                                let Ok(mut t) = shared.try_lock() else { continue; };
                                if !t.is_task_running {
                                    continue;
                                }
                                if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('v') | KeyCode::Char('V')) {
                                    t.try_attach_clipboard_image();
                                    // sync matches after clipboard attach may insert placeholder
                                    let cur_text = t.textarea.text().to_string();
                                    t.matches = crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                                    if t.sel >= t.matches.len() { t.sel = t.matches.len().saturating_sub(1); }
                                    let _ = t.draw();
                                    continue;
                                }
                                // Helper to sync matches after text change
                                let sync_matches = |t: &mut crate::composer::Tui| {
                                    let cur_text = t.textarea.text().to_string();
                                    t.matches = crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                                    if t.sel >= t.matches.len() { t.sel = t.matches.len().saturating_sub(1); }
                                };
                                // Ctrl editing keys (must check before generic Char handling)
                                if modifiers.contains(KeyModifiers::CONTROL) {
                                    match code {
                                        KeyCode::Char('p') => {
                                            if !t.matches.is_empty() {
                                                t.sel = t.sel.saturating_sub(1);
                                                let _ = t.draw();
                                            } else {
                                                // treat as Up history? ignore during task running
                                                let _ = t.draw();
                                            }
                                            continue;
                                        }
                                        KeyCode::Char('n') => {
                                            if !t.matches.is_empty() {
                                                t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                                let _ = t.draw();
                                            }
                                            continue;
                                        }
                                        KeyCode::Char('u') => { t.textarea.set_text(""); t.history_idx = None; sync_matches(&mut t); let _ = t.draw(); continue; }
                                        KeyCode::Char('a') => { t.textarea.set_cursor(0); let _ = t.draw(); continue; }
                                        KeyCode::Char('e') => { t.textarea.move_to_end(); let _ = t.draw(); continue; }
                                        KeyCode::Char('k') => { let cur = t.textarea.cursor(); t.textarea.replace_range(cur..usize::MAX, ""); sync_matches(&mut t); let _ = t.draw(); continue; }
                                        KeyCode::Char('w') | KeyCode::Backspace => {
                                            // when popup open, dismiss? mimic input.rs popup swallows word deletes
                                            if !t.matches.is_empty() {
                                                t.textarea.delete_word_backward();
                                                t.sync_attachments();
                                                sync_matches(&mut t);
                                                let _ = t.draw();
                                                continue;
                                            }
                                            t.textarea.delete_word_backward();
                                            t.sync_attachments();
                                            sync_matches(&mut t);
                                            let _ = t.draw();
                                            continue;
                                        }
                                        KeyCode::Delete => {
                                            t.textarea.delete_word_forward();
                                            t.sync_attachments();
                                            sync_matches(&mut t);
                                            let _ = t.draw();
                                            continue;
                                        }
                                        KeyCode::Left => { if !t.matches.is_empty() { t.sel = t.sel.saturating_sub(1); let _ = t.draw(); } else { t.textarea.move_word_left(); let _ = t.draw(); } continue; }
                                        KeyCode::Right => { if !t.matches.is_empty() { t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1)); let _ = t.draw(); } else { t.textarea.move_word_right(); let _ = t.draw(); } continue; }
                                        _ => {}
                                    }
                                }
                                // Alt editing keys
                                if modifiers.contains(KeyModifiers::ALT) {
                                    match code {
                                        KeyCode::Backspace => { t.textarea.delete_word_backward(); t.sync_attachments(); sync_matches(&mut t); let _ = t.draw(); continue; }
                                        KeyCode::Delete => { t.textarea.delete_word_forward(); t.sync_attachments(); sync_matches(&mut t); let _ = t.draw(); continue; }
                                        KeyCode::Char('d') => { t.textarea.delete_word_forward(); sync_matches(&mut t); let _ = t.draw(); continue; }
                                        KeyCode::Char('b') | KeyCode::Left => { t.textarea.move_word_left(); let _ = t.draw(); continue; }
                                        KeyCode::Char('f') | KeyCode::Right => { t.textarea.move_word_right(); let _ = t.draw(); continue; }
                                        _ => {}
                                    }
                                }
                                // Popup navigation when >1 match (a lone match leaves
                                // Up/Down free for history recall / cursor movement)
                                if t.matches.len() > 1 {
                                    match code {
                                        KeyCode::Up => { t.sel = t.sel.saturating_sub(1); let _ = t.draw(); continue; }
                                        KeyCode::Down => { t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1)); let _ = t.draw(); continue; }
                                        KeyCode::Tab => {
                                            if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                                t.textarea.set_text(&format!("/{name} "));
                                                t.textarea.move_to_end();
                                                sync_matches(&mut t);
                                            }
                                            let _ = t.draw();
                                            continue;
                                        }
                                        KeyCode::Enter => {
                                            let cur_text = t.textarea.text().to_string();
                                            if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                                if cur_text != format!("/{name}") && cur_text != format!("/{name} ") {
                                                    t.textarea.set_text(&format!("/{name} "));
                                                    t.textarea.move_to_end();
                                                    sync_matches(&mut t);
                                                    let _ = t.draw();
                                                    continue;
                                                }
                                            }
                                            // if already completed, fall through to queue logic below
                                        }
                                        KeyCode::Esc => {
                                            t.matches.clear();
                                            t.sel = 0;
                                            let _ = t.draw();
                                            continue;
                                        }
                                        _ => {}
                                    }
                                }
                                match code {
                                    KeyCode::Left => {
                                        t.textarea.move_left();
                                        let _ = t.draw();
                                    }
                                    KeyCode::Right => {
                                        t.textarea.move_right();
                                        let _ = t.draw();
                                    }
                                    KeyCode::Up => {
                                        if t.matches.len() > 1 {
                                            t.sel = t.sel.saturating_sub(1);
                                            let _ = t.draw();
                                        } else {
                                            t.textarea.move_up();
                                            let _ = t.draw();
                                        }
                                    }
                                    KeyCode::Down => {
                                        if t.matches.len() > 1 {
                                            t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                            let _ = t.draw();
                                        } else {
                                            t.textarea.move_down();
                                            let _ = t.draw();
                                        }
                                    }
                                    KeyCode::Tab => {
                                        if !t.matches.is_empty() {
                                            if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                                t.textarea.set_text(&format!("/{name} "));
                                                t.textarea.move_to_end();
                                                sync_matches(&mut t);
                                            }
                                            let _ = t.draw();
                                        }
                                    }
                                    KeyCode::Enter => {
                                        let is_newline = modifiers.contains(KeyModifiers::SHIFT) || modifiers.contains(KeyModifiers::ALT);
                                        if is_newline {
                                            t.textarea.insert_str("\n");
                                            sync_matches(&mut t);
                                            let _ = t.draw();
                                            continue;
                                        }
                                        // if popup open and selection not yet applied, complete first
                                        if !t.matches.is_empty() {
                                            if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                                let cur_text = t.textarea.text().to_string();
                                                if cur_text != format!("/{name}") && cur_text != format!("/{name} ") {
                                                    t.textarea.set_text(&format!("/{name} "));
                                                    t.textarea.move_to_end();
                                                    sync_matches(&mut t);
                                                    let _ = t.draw();
                                                    continue;
                                                }
                                            }
                                        }
                                        let mut text = t.textarea.text().to_string();
                                        for (ph, full) in &t.pending_pastes { text = text.replace(ph, full); }
                                        text = text.trim().to_string();
                                        let attached_with_ph: Vec<(String, std::path::PathBuf)> = std::mem::take(&mut t.attachments);
                                        let attached: Vec<std::path::PathBuf> = attached_with_ph.into_iter().map(|(_, p)| p).collect();
                                        // clear pending pastes already handled
                                        if text.is_empty() && attached.is_empty() { continue; }
                                        // queue it — fleeting, not transcript (becomes real prompt when dequeued)
                                        t.queued_inputs.push_back((text.clone(), attached.clone()));
                                        t.textarea.set_text("");
                                        t.pending_pastes.clear();
                                        t.matches.clear();
                                        t.sel = 0;
                                        let _ = t.draw();
                                    }
                                    KeyCode::Char(c) => {
                                        t.textarea.insert_str(&c.to_string());
                                        sync_matches(&mut t);
                                        let _ = t.draw();
                                    }
                                    KeyCode::Backspace => {
                                        if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                                            t.textarea.delete_word_backward();
                                        } else {
                                            t.textarea.delete_backward(1);
                                        }
                                        t.sync_attachments();
                                        sync_matches(&mut t);
                                        let _ = t.draw();
                                    }
                                    KeyCode::Delete => {
                                        if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                                            t.textarea.delete_word_forward();
                                        } else {
                                            t.textarea.delete_forward(1);
                                        }
                                        t.sync_attachments();
                                        sync_matches(&mut t);
                                        let _ = t.draw();
                                    }
                                    KeyCode::Esc => {
                                        if !t.matches.is_empty() {
                                            t.matches.clear();
                                            t.sel = 0;
                                            let _ = t.draw();
                                        } else {
                                            t.textarea.set_text("");
                                            t.attachments.clear();
                                            t.pending_pastes.clear();
                                            t.history_idx = None;
                                            t.sel = 0;
                                            t.matches.clear();
                                            sync_matches(&mut t);
                                            let _ = t.draw();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Event::Paste(data) => {
                                let Some(shared) = watcher_tui.as_ref() else { continue; };
                                let Ok(mut t) = shared.try_lock() else { continue; };
                                if !t.is_task_running { continue; }
                                t.handle_paste(data);
                                let cur_text = t.textarea.text().to_string();
                                t.matches = crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                                if t.sel >= t.matches.len() { t.sel = t.matches.len().saturating_sub(1); }
                                let _ = t.draw();
                            }
                            _ => {}
                        }
                    }
                });

                let mut current_tool_name: Option<String> = None;
                let mut current_tool_args: Option<serde_json::Value> = None;
                let mut turn_usage: Option<gray_core::event::Usage> = None;
                let mut run_result = {
                    let mut on_event = |ev: &AgentEvent| {
                        dispatch_agent_event(ev, tui_stream.as_ref(), interactive, &mut current_tool_name, &mut current_tool_args, &mut turn_usage, &cwd, config.model.as_deref().unwrap_or(""), &mut session_totals);
                    };
                    let mut run_future =
                        Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
                    tokio::select! {
                        res = &mut run_future => res,
                        _ = cancel.cancelled() => Err(CoreError::Cancelled),
                    }
                };
                // overflow recovery (one retry only)
                if let Err(ref e) = run_result {
                    let latest = tui
                        .as_ref()
                        .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage))
                        .or(turn_usage);
                    if maybe_overflow_compact(
                        agent,
                        config,
                        &mut session_state,
                        &cwd,
                        tui.as_ref().map(|(s, _)| s),
                        latest,
                        &mut initial_count,
                        e,
                    )
                    .await
                    {
                            current_tool_name = None;
                            current_tool_args = None;
                            let ctx2 = ToolContext {
                                cwd: cwd.clone(),
                                cancel: cancel.clone(),
                                questions: Some(question_bridge.clone()),
                            };
                            let mut on_event2 = |ev: &AgentEvent| {
                                dispatch_agent_event(ev, tui_stream.as_ref(), interactive, &mut current_tool_name, &mut current_tool_args, &mut turn_usage, &cwd, config.model.as_deref().unwrap_or(""), &mut session_totals);
                            };
                            let mut run_future2 =
                                Box::pin(agent.run_streaming(user_msg_for_retry.clone(), ctx2, &mut on_event2));
                            let retry_res = tokio::select! {
                                res = &mut run_future2 => res,
                                _ = cancel.cancelled() => Err(CoreError::Cancelled),
                            };
                            run_result = retry_res;
                    }
                }
                TURN_STATE.lock().expect("turn state lock").take();
                // signal the watcher to exit; it dies within one 100ms tick.
                // (Never .await it here without the flag — deadlock.)
                watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);

                match run_result {
                    Ok(_) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                    }
                    Err(CoreError::Cancelled) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                        if interactive {
                            if let Some((shared, _)) = &tui {
                                let mut t = shared.lock().expect("tui lock");
                                t.end_thinking();
                                t.stream("(interrupted)\n");
                            }
                        } else {
                            println!("(interrupted)");
                        }
                    }
                    Err(e) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                        let msg = format_core_error(&e, &config.base_url);
                        if interactive {
                            if let Some((shared, _)) = &tui {
                                let mut t = shared.lock().expect("tui lock");
                                t.end_thinking();
                                t.stream(&format!("{msg}\n"));
                            }
                        } else {
                            eprintln!("{msg}");
                        }
                    }
                }
                if let Some(s) = &tui_stream {
                    s.lock().expect("tui lock").end_turn();
                }
                // if we queued input while working, start it immediately
                if interactive {
                    if let Some((shared, _)) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        if let Some(text) = t.local_command.take() {
                            // Esc mid-turn: command already echoed; run locally, never to the AI
                            drop(t);
                            pending_command = Some(expand_skill_command(parse_command(&text), &cwd, Some(shared), true));
                        } else if let Some((qtext, qimages)) = t.queued_inputs.pop_front() {
                            t.push_user_prompt(&qtext, &qimages, !qtext.starts_with('/'));
                            drop(t);
                            pending_command = Some(expand_skill_command(parse_command(&qtext), &cwd, Some(shared), false));
                            pending_images = qimages;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_core_error;
    use gray_core::error::CoreError;

    #[test]
    fn format_core_error_includes_provider_hint_and_cf_ray() {
        let base = "https://opencode.ai/zen/go/v1";
        let detail = "status 422: boom, cf-ray: abc123-sjc";
        let e = CoreError::Provider(detail.to_string());
        let out = format_core_error(&e, base);
        assert!(
            out.contains("Provider: https://opencode.ai/zen/go/v1 — try /model"),
            "missing provider hint, got: {out}"
        );
        assert!(out.contains("cf-ray: abc123-sjc"), "missing cf-ray, got: {out}");
    }

    #[test]
    fn format_core_error_plain_no_cf_ray_noise() {
        let base = "https://opencode.ai/zen/go/v1";
        let detail = "status 422: boom";
        let e = CoreError::Provider(detail.to_string());
        let out = format_core_error(&e, base);
        assert!(
            out.contains("Provider: https://opencode.ai/zen/go/v1 — try /model"),
            "missing provider hint, got: {out}"
        );
        assert!(!out.contains("cf-ray"), "should not contain cf-ray, got: {out}");
    }

    #[test]
    fn format_core_error_bounds_detail_and_marks_retryable() {
        let base = "https://opencode.ai/zen/go/v1";
        let long = format!("status 429: {}", "x".repeat(2000));
        let out = format_core_error(&CoreError::Provider(long), base);
        assert!(out.contains("(retryable)"), "rate arm must say retryable: {out}");
        assert!(out.chars().count() < 1200, "detail must be capped, got {} chars", out.chars().count());
        let auth = format_core_error(&CoreError::Provider("401 unauthorized nope".into()), base);
        assert!(auth.contains("(not retryable)"), "auth arm must say not retryable: {auth}");
    }

    #[test]
    fn gateway_args_parse_all_actions() {
        use super::GatewayAction as G;
        use gray_gateway::config::Platform;
        assert!(matches!(super::parse_gateway_args("/gateway"), G::Status));
        assert!(matches!(super::parse_gateway_args("/gateway status"), G::Status));
        assert!(matches!(super::parse_gateway_args("/gateway run"), G::Run));
        assert!(matches!(super::parse_gateway_args("/gateway stop"), G::Stop));
        assert!(matches!(super::parse_gateway_args("/gateway install"), G::Install));
        assert!(matches!(super::parse_gateway_args("/gateway uninstall"), G::Uninstall));
        assert!(matches!(super::parse_gateway_args("/gateway bogus"), G::Help));
        match super::parse_gateway_args("/gateway connect discord abc123") {
            G::Connect(Platform::Discord, tok) => assert_eq!(tok, "abc123"),
            other => panic!("expected connect discord, got {other:?}"),
        }
        match super::parse_gateway_args("/gateway connect TELEGRAM  123:XYZ extra") {
            G::Connect(Platform::Telegram, tok) => assert_eq!(tok, "123:XYZ"), // token = first arg after platform
            other => panic!("expected connect telegram, got {other:?}"),
        }
        assert!(matches!(
            super::parse_gateway_args("/gateway connect slack"), // no token
            G::Help
        ));
        match super::parse_gateway_args("/gateway disconnect slack") {
            G::Disconnect(Platform::Slack) => {}
            other => panic!("expected disconnect slack, got {other:?}"),
        }
        match super::parse_gateway_args("/gateway enable Telegram") {
            G::Enable(Platform::Telegram) => {}
            other => panic!("expected enable telegram, got {other:?}"),
        }
        assert!(matches!(super::parse_gateway_args("/gateway autostart on"), G::Autostart(true)));
        assert!(matches!(super::parse_gateway_args("/gateway autostart OFF"), G::Autostart(false)));
        assert!(matches!(super::parse_gateway_args("/gateway autostart"), G::Help));
        assert!(matches!(super::parse_gateway_args("/gateway autostart maybe"), G::Help));
        // default-on: fresh config autostarts
        assert!(gray_gateway::config::GatewayConfig::default().autostart);
    }

    #[test]
    fn gateway_connect_disconnect_roundtrip() {
        let mut cfg = gray_gateway::config::GatewayConfig::default();
        super::apply_connect(&mut cfg, gray_gateway::config::Platform::Telegram, "tok-1");
        let pc = cfg.platforms.get(&gray_gateway::config::Platform::Telegram).unwrap();
        assert!(pc.enabled && pc.token.as_deref() == Some("tok-1"));
        super::apply_disconnect(&mut cfg, gray_gateway::config::Platform::Telegram);
        let pc = cfg.platforms.get(&gray_gateway::config::Platform::Telegram).unwrap();
        assert!(!pc.enabled && pc.token.as_deref() == Some("tok-1")); // token kept
        assert!(super::apply_enable(&mut cfg, gray_gateway::config::Platform::Telegram));
        let pc = cfg.platforms.get(&gray_gateway::config::Platform::Telegram).unwrap();
        assert!(pc.enabled && pc.token.as_deref() == Some("tok-1"));
        assert!(!super::apply_enable(&mut cfg, gray_gateway::config::Platform::Slack)); // no token
    }

    #[test]
    fn gateway_status_lines_hide_tokens() {
        let mut cfg = gray_gateway::config::GatewayConfig::default();
        super::apply_connect(&mut cfg, gray_gateway::config::Platform::Discord, "secret-token");
        let lines = super::gateway_status_lines(&cfg, true);
        let joined = lines.join("\n");
        assert!(joined.contains("Discord"), "should list platform: {joined}");
        assert!(joined.contains("connected"), "should show in-process running state: {joined}");
        assert!(!joined.contains("secret-token"), "token must never render: {joined}");
        let lines = super::gateway_status_lines(&cfg, false);
        assert!(lines.join("\n").contains("not running"));
    }

    #[test]
    fn totals_rebuild_from_stored_entries() {
        let v: serde_json::Value = serde_json::json!({
            "test-persist-model": {
                "max_input_tokens": 100000,
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000002,
            },
        });
        crate::setup::parse_litellm_context_json(&v);
        let entry = |id: u64, text: &str, usage: Option<gray_core::event::Usage>| {
            gray_session::SessionEntry {
                entry_id: id,
                parent_id: if id == 1 { None } else { Some(id - 1) },
                timestamp: 0,
                message: gray_core::message::Message::user(text),
                usage,
            }
        };
        let entries = vec![
            entry(1, "hi", Some(gray_core::event::Usage::new(1000, 500))),
            entry(2, "yo", None),
            entry(3, "again", Some(gray_core::event::Usage::new(2000, 1000))),
        ];
        let t = super::SessionTotals::from_entries(&entries, "test-persist-model");
        assert_eq!(t.turns, 2);
        assert_eq!(t.input, 3000);
        assert_eq!(t.output, 1500);
        let want = 3000.0 * 0.000001 + 1500.0 * 0.000002;
        assert!((t.cost - want).abs() < 1e-12, "got {}, want {want}", t.cost);
    }
}
