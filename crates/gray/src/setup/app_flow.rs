//! The app setup flow's headless core: ask for exactly what the app
//! declares, write it privately, prove it with the app's own doctor, then
//! register the app's tool. The REPL modal (task 5b) rides the same core.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use super::channel_picker::{ChannelSource, Destination, RestChannels};
use super::discord_check::{self, BotCheck, CheckError};
use super::registry::{CHECK_DISCORD_BOT, FieldKind, SetupDecl, SetupField};
use super::supervise::start_daemon;
use super::write_config::{Supplied, write_config};

/// Fields the flow must still ask about: everything non-derived the app's
/// config does not already answer. Required first, in declaration order.
pub fn plan_missing<'a>(decl: &'a SetupDecl, home: &Path) -> Vec<&'a SetupField> {
    let path = home.join(decl.config_path);
    decl.fields
        .iter()
        .filter(|f| !matches!(f.kind, FieldKind::Derived))
        .filter(|f| !super::registry::key_present(&path, f.key))
        .collect()
}

/// The initial pass asks Required fields only, in declaration order (token,
/// then home channel). Optional keys come after \u{2014} by then the token
/// works, so an app can discover them (Discord pairing learns the owner's ID)
/// instead of making the operator type them.
pub fn plan_required<'a>(decl: &'a SetupDecl, home: &Path) -> Vec<&'a SetupField> {
    plan_missing(decl, home)
        .into_iter()
        .filter(|f| f.is_required())
        .collect()
}

/// Optional keys still unanswered \u{2014} what the setup report calls "later".
pub fn missing_optional<'a>(decl: &'a SetupDecl, home: &Path) -> Vec<&'a SetupField> {
    plan_missing(decl, home)
        .into_iter()
        .filter(|f| !f.is_required())
        .collect()
}

/// `--field key=value` answers, checked against the declaration. Secrets are
/// flagged so `Supplied` can redact them everywhere else.
pub fn supplied_from_flags(decl: &SetupDecl, fields: &[String]) -> Result<Supplied> {
    let mut out = Supplied::default();
    for raw in fields {
        let (key, value) = raw
            .split_once('=')
            .with_context(|| format!("--field wants key=value, got '{raw}'"))?;
        let field = decl
            .field(key)
            .with_context(|| format!("'{key}' is not a setup field of this app"))?;
        anyhow::ensure!(
            !matches!(field.kind, FieldKind::Derived),
            "'{key}' is derived; gray fills it in"
        );
        out.insert(key, value.to_string(), field.secret);
    }
    Ok(out)
}

/// Required fields neither this run nor the app's existing config answered.
/// A key already written counts as answered: hand-edited configs (the
/// documented path) and a re-run after a partial setup must be able to
/// finish with `--field`-less invocations instead of being demanded again.
/// Non-empty means the flow cannot finish yet — the caller reports these
/// instead of writing a half-config.
pub fn missing_required<'a>(
    decl: &'a SetupDecl,
    supplied: &Supplied,
    config_path: &Path,
) -> Vec<&'a SetupField> {
    decl.fields
        .iter()
        .filter(|f| {
            f.is_required()
                && supplied.get(f.key).is_none()
                && !super::registry::key_present(config_path, f.key)
        })
        .collect()
}

/// One human line per missing field, with the portal URL when the app named
/// one. No values, no secrets.
pub fn describe_missing(missing: &[&SetupField]) -> String {
    missing
        .iter()
        .map(|f| match f.url {
            Some(url) => format!("{} (get it at {url})", f.description),
            None => f.description.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n  ")
}

/// The app's verify command, with the declaration's argv[0] (the app binary)
/// resolved through the plugin registry.
pub fn verify_argv(app: &str, home: &Path, decl: &SetupDecl) -> Result<Vec<String>> {
    let (_bin, args) = decl
        .verify
        .split_first()
        .context("the app's declaration has no verify command")?;
    let mut argv = crate::plugin_cli::command_argv(home, app)?;
    argv.extend(args.iter().map(|a| a.to_string()));
    Ok(argv)
}

/// The app's register command (`<app-bin> register`).
pub fn register_argv(app: &str, home: &Path) -> Result<Vec<String>> {
    let mut argv = crate::plugin_cli::command_argv(home, app)?;
    argv.push("register".to_string());
    Ok(argv)
}

/// Running one of the app's own commands and catching its real output.
pub struct StepOutput {
    pub ok: bool,
    pub output: String,
}

pub fn run_step(argv: &[String]) -> StepOutput {
    let Some((program, args)) = argv.split_first() else {
        return StepOutput {
            ok: false,
            output: "empty command".to_string(),
        };
    };
    match Command::new(program).args(args).output() {
        Ok(out) => StepOutput {
            ok: out.status.success(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        },
        Err(e) => StepOutput {
            ok: false,
            output: format!("could not run {program}: {e}"),
        },
    }
}

/// The token check gray runs before writing anything, injected so tests
/// keep the network out (`discord_check::check_bot_token` in production).
pub type Checker<'a> = &'a dyn Fn(&str) -> Result<BotCheck, CheckError>;

/// What a pasted token turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerdict {
    /// Discord knows it.
    Accepted(BotCheck),
    /// Discord could not be asked (offline, odd answer): saved anyway, with
    /// this warning (Hermes parity).
    Unverified(String),
    /// Ask again; the line says why and how.
    Reask(String),
    /// Three rejections in a row: stop here, nothing is saved.
    GiveUp(String),
}

/// Re-asks Discord's rejections up to three times and a numeric app-ID
/// paste once. Holds counts only, never a token.
#[derive(Debug, Default)]
pub struct TokenGate {
    rejected: u8,
    numeric_warned: bool,
}

pub const TOKEN_TRIES: u8 = 3;
pub const INTENT_RECHECKS: u8 = 5;
pub const OFFLINE_INTENT_NOTE: &str = "Make sure Message Content Intent is on (Bot page \u{2192} Privileged Gateway Intents), or Discord will refuse the bot's connection.";

impl TokenGate {
    /// Clean the paste (curly quotes, non-ASCII, whitespace), then ask
    /// Discord. Returns the cleaned token alongside the verdict: only an
    /// `Accepted` or `Unverified` token may be saved.
    pub fn submit(&mut self, raw: &str, check: Checker) -> (String, TokenVerdict) {
        let token = discord_check::clean_token(raw);
        if token.is_empty() {
            return (
                token,
                TokenVerdict::Reask("this one is required".to_string()),
            );
        }
        if let Some(why) = discord_check::token_shape_error(&token) {
            if !self.numeric_warned {
                self.numeric_warned = true;
                return (token, TokenVerdict::Reask(why.to_string()));
            }
        } else {
            self.numeric_warned = false;
        }
        let rejected = |gate: &mut Self, why: &str| {
            gate.rejected += 1;
            if gate.rejected >= TOKEN_TRIES {
                TokenVerdict::GiveUp(
                    "Discord rejected three tokens in a row; nothing was saved.".to_string(),
                )
            } else {
                TokenVerdict::Reask(why.to_string())
            }
        };
        if discord_check::has_inner_break(&token) {
            let verdict = rejected(
                self,
                "That isn't a bot token (it contains a line break). Copy it again from the Bot page.",
            );
            return (token, verdict);
        }
        let verdict = match check(&token) {
            Ok(bot) => TokenVerdict::Accepted(bot),
            Err(CheckError::Rejected) => rejected(
                self,
                "Discord rejected that token. On the Bot page click Reset Token, copy the new token and paste it here.",
            ),
            Err(CheckError::Status(code)) => TokenVerdict::Unverified(format!(
                "Couldn't verify the token (Discord answered {code}); saving it anyway."
            )),
            Err(CheckError::Unreachable(why)) => TokenVerdict::Unverified(format!(
                "Couldn't reach Discord to verify the token ({why}); saving it anyway."
            )),
        };
        (token, verdict)
    }
}

/// Ask again after the operator flipped the intent toggle. A failed
/// re-check keeps what was already known.
pub fn recheck_intent(token: &str, current: &BotCheck, check: Checker) -> BotCheck {
    check(token).unwrap_or_else(|_| current.clone())
}

/// Discord user IDs from a comma-separated answer, mention syntax stripped.
fn clean_user_ids(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|id| {
            let id = id.trim();
            let id = id
                .strip_prefix("<@")
                .and_then(|i| i.strip_suffix('>'))
                .map(|i| i.trim_start_matches('!'))
                .unwrap_or(id);
            let id = id.strip_prefix("user:").unwrap_or(id);
            id.trim().to_string()
        })
        .filter(|id| !id.is_empty())
        .collect()
}

/// Who is already allowed: the config's `allowed_users`, then this run's.
fn current_allowed(supplied: &Supplied, config_path: &Path) -> Vec<String> {
    let existing = match super::write_config::read_object(config_path).get("allowed_users") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(serde_json::Value::String(raw)) => clean_user_ids(raw),
        _ => Vec::new(),
    };
    let fresh = match (
        supplied.list("allowed_users"),
        supplied.get("allowed_users"),
    ) {
        (Some(items), _) => items.to_vec(),
        (None, Some(raw)) => clean_user_ids(raw),
        (None, None) => Vec::new(),
    };
    discord_check::merge_allowed(&existing, &fresh)
}

fn current_owner(supplied: &Supplied, config_path: &Path) -> Option<String> {
    supplied.get("owner_id").map(str::to_string).or_else(|| {
        super::write_config::read_object(config_path)
            .get("owner_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// Owners Discord named who are neither the owner nor allowlisted yet.
pub fn owners_to_allow(bot: &BotCheck, supplied: &Supplied, config_path: &Path) -> Vec<String> {
    let allowed = current_allowed(supplied, config_path);
    let owner = current_owner(supplied, config_path);
    bot.owners
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| owner.as_ref() != Some(id) && !allowed.contains(id))
        .collect()
}

/// Allowlist the detected owner(s): `owner_id` is filled only when nobody
/// set one, and `allowed_users` starts from what is already allowed and
/// only adds.
pub fn allow_owners(bot: &BotCheck, supplied: &mut Supplied, config_path: &Path) {
    if current_owner(supplied, config_path).is_none()
        && let Some((id, _)) = bot.owners.first()
    {
        supplied.insert("owner_id", id.clone(), false);
    }
    let owners: Vec<String> = bot.owners.iter().map(|(id, _)| id.clone()).collect();
    let merged = discord_check::merge_allowed(&current_allowed(supplied, config_path), &owners);
    supplied.insert_list("allowed_users", merged);
}

/// The app stores `allowed_users` as an array: a comma-separated answer is
/// merged into whatever the config already allows, never replacing it.
pub fn normalize_allowed(supplied: &mut Supplied, config_path: &Path) {
    if supplied.get("allowed_users").is_some() {
        let merged = current_allowed(supplied, config_path);
        supplied.insert_list("allowed_users", merged);
    }
}

pub fn allowlisted_line(bot: &BotCheck) -> String {
    format!(
        "You are allowlisted ({}): owner detected, no Developer Mode needed.",
        discord_check::owner_names(bot)
    )
}

/// The headless Discord onboarding: check the `--field token=` with
/// Discord before anything is written. A rejected token is an error and
/// never saved; offline saves with a warning. `ask` re-checks the intent
/// when a terminal is attached. Returns the lines to show the operator.
pub fn onboard_flags(
    supplied: &mut Supplied,
    config_path: &Path,
    check: Checker,
    mut ask: Option<&mut dyn FnMut(&str) -> String>,
) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    if let Some(raw) = supplied.get("token").map(str::to_string) {
        let (token, verdict) = TokenGate::default().submit(&raw, check);
        match verdict {
            TokenVerdict::Reask(why) | TokenVerdict::GiveUp(why) => {
                anyhow::bail!("{why}\nNothing was saved.")
            }
            TokenVerdict::Unverified(warning) => {
                supplied.insert("token", token, true);
                notes.push(warning);
                notes.push(OFFLINE_INTENT_NOTE.to_string());
            }
            TokenVerdict::Accepted(mut bot) => {
                supplied.insert("token", token.clone(), true);
                notes.push(format!(
                    "Token checked with Discord: this is the bot \"{}\".",
                    bot.bot_name
                ));
                if !bot.message_content
                    && let Some(ask) = ask.as_mut()
                {
                    for _ in 0..INTENT_RECHECKS {
                        for line in discord_check::intent_lines(&bot) {
                            eprintln!("{line}");
                        }
                        let answer =
                            ask("Press Enter once it's saved to re-check, or type 'skip': ");
                        if answer.trim().eq_ignore_ascii_case("skip") {
                            break;
                        }
                        bot = recheck_intent(&token, &bot, check);
                        if bot.message_content {
                            break;
                        }
                    }
                }
                if bot.message_content {
                    notes.push("Message Content Intent is on.".to_string());
                } else {
                    notes.extend(discord_check::intent_lines(&bot));
                }
                notes.extend(discord_check::invite_lines(&bot));
                if !owners_to_allow(&bot, supplied, config_path).is_empty() {
                    allow_owners(&bot, supplied, config_path);
                    notes.push(allowlisted_line(&bot));
                }
            }
        }
    }
    normalize_allowed(supplied, config_path);
    Ok(notes)
}

/// A stdin line reader for the intent re-check, only when a person is there.
fn terminal_ask() -> Option<impl FnMut(&str) -> String> {
    use std::io::IsTerminal;
    (std::io::stdin().is_terminal() && std::io::stderr().is_terminal()).then_some(|text: &str| {
        use std::io::Write;
        eprint!("{text}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        line
    })
}

/// Headless twin of the /gateway flow: write what the flags answer, prove it
/// with the app's doctor, register the app's tool, and say so. Anything
/// missing is reported, never guessed; nothing success-shaped is printed
/// until the doctor agrees.
pub fn run_headless(app: &str, fields: &[String], start: bool) -> Result<()> {
    let (gray_home, user) = (crate::plugin_cli::home()?, super::user_home()?);
    let decl = crate::plugin_cli::setup_decl(app)
        .with_context(|| format!("gray has no setup declaration for '{app}'"))?;
    let mut supplied = supplied_from_flags(decl, fields)?;
    let config_path = user.join(decl.config_path);
    if decl.check == Some(CHECK_DISCORD_BOT) {
        let mut ask = terminal_ask();
        let ask = ask.as_mut().map(|f| f as &mut dyn FnMut(&str) -> String);
        for line in onboard_flags(
            &mut supplied,
            &config_path,
            &discord_check::check_bot_token,
            ask,
        )? {
            println!("{line}");
        }
    }
    let missing = missing_required(decl, &supplied, &config_path);
    if !missing.is_empty() {
        anyhow::bail!("{} still needs:\n  {}", app, describe_missing(&missing));
    }
    println!(
        "{}",
        finish_after_answers(app, decl, &gray_home, &user, &supplied, start)?
    );
    Ok(())
}

/// The REPL flow: one field at a time, secrets masked, the app's own doctor
/// as the referee. `gray gateway setup` rides [`run_headless`]; this is the
/// same core behind a modal.
pub fn run_app_setup_modal(app: &str) -> anyhow::Result<()> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
    use std::time::Duration;

    let (gray_home, user) = (crate::plugin_cli::home()?, super::user_home()?);
    let decl = crate::plugin_cli::setup_decl(app)
        .with_context(|| format!("gray has no setup declaration for '{app}'"))?;
    let fields = plan_required(decl, &user);
    if fields.is_empty() {
        // Nothing to ask: prove the app works or report why it does not.
        // (Before the modal owns the screen, so a line to stderr is fine.)
        let report =
            finish_after_answers(app, decl, &gray_home, &user, &Supplied::default(), true)?;
        eprintln!("{report}");
        return Ok(());
    }

    let box_bg = crate::theme::theme().surface_bg;
    let accent = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;

    enum Phase {
        Filling,
        Picking {
            items: Vec<Destination>,
            sel: usize,
            at_guilds: bool,
        },
        /// Message Content Intent is off: link to the toggle, Enter re-checks.
        Intent {
            bot: BotCheck,
            tries: u8,
        },
        /// The owner Discord named is not allowlisted yet: offer it.
        Allow {
            bot: BotCheck,
        },
        Verifying,
        Failed,
        Done,
    }
    /// After the token (and intent) step: the invite link, then the owner
    /// offer when there is someone to allowlist, else the next field.
    fn after_intent(
        bot: BotCheck,
        notes: &mut Vec<String>,
        supplied: &Supplied,
        config_path: &Path,
        done: bool,
    ) -> Phase {
        notes.extend(discord_check::invite_lines(&bot));
        if !owners_to_allow(&bot, supplied, config_path).is_empty() {
            Phase::Allow { bot }
        } else {
            next_field(done)
        }
    }
    fn next_field(done: bool) -> Phase {
        if done {
            Phase::Verifying
        } else {
            Phase::Filling
        }
    }
    let checks_token = decl.check == Some(CHECK_DISCORD_BOT);
    let config_path = user.join(decl.config_path);
    let mut gate = TokenGate::default();
    let mut notes: Vec<String> = Vec::new();
    let mut supplied = Supplied::default();
    let mut idx = 0usize;
    let mut buf = String::new();
    let mut status: Option<String> = None;
    let mut phase = Phase::Filling;
    let mut report: Option<String> = None;

    let (_session, mut terminal) = super::open_modal()?;

    let outcome = (|| -> anyhow::Result<()> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 30 || area.height < 10 {
                    return;
                }
                let w = (area.width.saturating_sub(4))
                    .clamp(40, 100)
                    .min(area.width);
                // Room for the onboarding notes (the invite link wraps).
                let h = (12 + 2 * notes.len() as u16)
                    .min(area.height.saturating_sub(2))
                    .max(10);
                let rect = ratatui::layout::Rect::new(
                    (area.width.saturating_sub(w)) / 2,
                    (area.height.saturating_sub(h)) / 4,
                    w,
                    h,
                );
                frame.render_widget(Clear, rect);
                frame.render_widget(Block::default().style(Style::default().bg(box_bg)), rect);
                let inner = ratatui::layout::Rect::new(
                    rect.x + 2,
                    rect.y + 1,
                    rect.width.saturating_sub(4),
                    rect.height.saturating_sub(2),
                );
                let mut lines: Vec<Line> = vec![Line::from(Span::styled(
                    format!("Set up {app}"),
                    Style::default()
                        .fg(accent)
                        .add_modifier(Modifier::BOLD)
                        .bg(box_bg),
                ))];
                match &phase {
                    Phase::Filling => {
                        let field = fields[idx];
                        for note in &notes {
                            lines.push(Line::from(Span::styled(
                                note.clone(),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                        lines.push(Line::from(Span::styled(
                            format!("{}/{}", idx + 1, fields.len()),
                            Style::default().fg(text_dim).bg(box_bg),
                        )));
                        lines.push(Line::from(Span::styled(
                            field.description,
                            Style::default().fg(text_dim).bg(box_bg),
                        )));
                        if let Some(url) = field.url {
                            lines.push(Line::from(Span::styled(
                                format!("get it at {url}"),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                        let shown = if field.secret {
                            mask(&buf)
                        } else {
                            buf.clone()
                        };
                        lines.push(Line::from(Span::styled(
                            format!("> {shown}"),
                            Style::default().fg(accent).bg(box_bg),
                        )));
                        if let Some(status) = &status {
                            lines.push(Line::from(Span::styled(
                                status.clone(),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                        if field.picker.is_some() && supplied.get("token").is_some() {
                            lines.push(Line::from(Span::styled(
                                "Tab \u{2014} pick from a server instead of pasting an ID",
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                    }
                    Phase::Picking {
                        items,
                        sel,
                        at_guilds,
                    } => {
                        lines.push(Line::from(Span::styled(
                            if *at_guilds {
                                "pick a server (the bot sees these)"
                            } else {
                                "pick a channel, or the DM"
                            },
                            Style::default().fg(text_dim).bg(box_bg),
                        )));
                        let shown = items.len().min(8);
                        let start = sel.saturating_sub(shown.saturating_sub(1));
                        for (i, item) in items.iter().enumerate().skip(start).take(shown) {
                            let marker = if i == *sel { "\u{25b8} " } else { "  " };
                            lines.push(Line::from(Span::styled(
                                format!("{marker}{}", item.label),
                                Style::default()
                                    .fg(if i == *sel { accent } else { text_dim })
                                    .bg(box_bg),
                            )));
                        }
                        if items.is_empty() {
                            lines.push(Line::from(Span::styled(
                                "(nothing to pick \u{2014} paste an ID instead)",
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                    }
                    Phase::Intent { bot, .. } => {
                        for line in discord_check::intent_lines(bot) {
                            lines.push(Line::from(Span::styled(
                                line,
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                        if let Some(status) = &status {
                            lines.push(Line::from(Span::styled(
                                status.clone(),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                        lines.push(Line::from(Span::styled(
                            "Enter \u{2014} re-check once it's saved \u{00b7} s \u{2014} skip",
                            Style::default().fg(accent).bg(box_bg),
                        )));
                    }
                    Phase::Allow { bot } => {
                        let who = discord_check::owner_names(bot);
                        let question = if bot.owners.len() == 1 {
                            format!("Allow yourself ({who}) to talk to the bot?")
                        } else {
                            format!("Allow your team ({who}) to talk to the bot?")
                        };
                        lines.push(Line::from(Span::styled(
                            question,
                            Style::default().fg(text_dim).bg(box_bg),
                        )));
                        lines.push(Line::from(Span::styled(
                            "Enter/y \u{2014} yes \u{00b7} n \u{2014} no",
                            Style::default().fg(accent).bg(box_bg),
                        )));
                    }
                    Phase::Verifying | Phase::Failed | Phase::Done => {
                        let text = report.clone().unwrap_or_default();
                        for line in text.lines().take(16) {
                            lines.push(Line::from(Span::styled(
                                line.to_string(),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                    }
                }
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match phase {
                    Phase::Filling => match code {
                        KeyCode::Esc => return Ok(()),
                        KeyCode::Enter if checks_token && fields[idx].key == "token" => {
                            // Check with Discord before it can be saved: a
                            // rejected token is re-asked, never written.
                            let field = fields[idx];
                            let (token, verdict) =
                                gate.submit(&buf, &discord_check::check_bot_token);
                            buf.clear();
                            status = None;
                            let done = idx + 1 >= fields.len();
                            match verdict {
                                TokenVerdict::Reask(why) => {
                                    status = Some(why);
                                    continue;
                                }
                                TokenVerdict::GiveUp(why) => {
                                    report = Some(why);
                                    phase = Phase::Failed;
                                    continue;
                                }
                                TokenVerdict::Unverified(warning) => {
                                    supplied.insert(field.key, token, field.secret);
                                    notes.push(warning);
                                    notes.push(OFFLINE_INTENT_NOTE.to_string());
                                    idx += 1;
                                    phase = next_field(done);
                                }
                                TokenVerdict::Accepted(bot) => {
                                    supplied.insert(field.key, token, field.secret);
                                    notes.push(format!(
                                        "Token checked with Discord: this is the bot \"{}\".",
                                        bot.bot_name
                                    ));
                                    idx += 1;
                                    phase = if bot.message_content {
                                        after_intent(bot, &mut notes, &supplied, &config_path, done)
                                    } else {
                                        Phase::Intent { bot, tries: 0 }
                                    };
                                }
                            }
                        }
                        KeyCode::Enter => {
                            let field = fields[idx];
                            let value = buf.trim().to_string();
                            if value.is_empty() && field.is_required() {
                                status = Some("this one is required".to_string());
                                continue;
                            }
                            if !value.is_empty() {
                                supplied.insert(field.key, value, field.secret);
                            }
                            buf.clear();
                            status = None;
                            idx += 1;
                            if idx >= fields.len() {
                                phase = Phase::Verifying;
                            }
                        }
                        KeyCode::Backspace => {
                            buf.pop();
                        }
                        KeyCode::Tab if fields[idx].picker.is_some() => {
                            if let Some(token) = supplied.get("token") {
                                match RestChannels::new(token).and_then(|source| source.guilds()) {
                                    Ok(guilds) if !guilds.is_empty() => {
                                        let items = guilds
                                            .into_iter()
                                            .map(|g| Destination {
                                                id: g.id,
                                                label: g.name,
                                                sort_key: 0,
                                                is_dm: false,
                                            })
                                            .collect();
                                        phase = Phase::Picking {
                                            items,
                                            sel: 0,
                                            at_guilds: true,
                                        };
                                    }
                                    Ok(_) => {
                                        status = Some("the bot is in no servers yet".to_string());
                                    }
                                    Err(e) => {
                                        status = Some(format!("{e:#}"));
                                    }
                                }
                            } else {
                                status = Some("the token comes first".to_string());
                            }
                        }
                        KeyCode::Char(c) => buf.push(c),
                        _ => {}
                    },
                    Phase::Picking { .. } => {
                        let (len, _sel, at_guilds) = match &phase {
                            Phase::Picking {
                                items,
                                sel,
                                at_guilds,
                            } => (items.len(), *sel, *at_guilds),
                            _ => unreachable!("matched Picking above"),
                        };
                        match code {
                            KeyCode::Esc => {
                                phase = Phase::Filling;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Phase::Picking { sel, .. } = &mut phase {
                                    *sel = sel.saturating_sub(1);
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Phase::Picking { sel, .. } = &mut phase {
                                    *sel = (*sel + 1).min(len.saturating_sub(1));
                                }
                            }
                            KeyCode::Enter => {
                                let picked = match &phase {
                                    Phase::Picking { items, sel, .. } => items[*sel].clone(),
                                    _ => unreachable!("matched Picking above"),
                                };
                                if at_guilds {
                                    let token =
                                        supplied.get("token").unwrap_or_default().to_string();
                                    let owner = supplied
                                        .get("owner_id")
                                        .map(str::to_string)
                                        .unwrap_or_default();
                                    match RestChannels::new(&token).and_then(|source| {
                                        super::channel_picker::destinations_for(
                                            &source, &picked.id, &owner,
                                        )
                                    }) {
                                        Ok(list) if !list.is_empty() => {
                                            phase = Phase::Picking {
                                                items: list,
                                                sel: 0,
                                                at_guilds: false,
                                            };
                                        }
                                        Ok(_) => {
                                            status =
                                                Some("that server has no channels".to_string());
                                            phase = Phase::Filling;
                                        }
                                        Err(e) => {
                                            status = Some(format!("{e:#}"));
                                            phase = Phase::Filling;
                                        }
                                    }
                                } else {
                                    supplied.insert("channel_id", picked.id, false);
                                    buf.clear();
                                    idx += 1;
                                    phase = if idx >= fields.len() {
                                        Phase::Verifying
                                    } else {
                                        Phase::Filling
                                    };
                                }
                            }
                            _ => {}
                        }
                    }
                    Phase::Intent { .. } => {
                        let Phase::Intent { bot, tries } = &phase else {
                            unreachable!("matched Intent above")
                        };
                        let (bot, tries) = (bot.clone(), *tries);
                        let done = idx >= fields.len();
                        match code {
                            KeyCode::Esc => return Ok(()),
                            KeyCode::Enter => {
                                let token = supplied.get("token").unwrap_or_default().to_string();
                                let bot =
                                    recheck_intent(&token, &bot, &discord_check::check_bot_token);
                                let tries = tries + 1;
                                if bot.message_content {
                                    status = None;
                                    notes.push("Message Content Intent is on.".to_string());
                                    phase = after_intent(
                                        bot,
                                        &mut notes,
                                        &supplied,
                                        &config_path,
                                        done,
                                    );
                                } else if tries >= INTENT_RECHECKS {
                                    status = None;
                                    notes.extend(discord_check::intent_lines(&bot));
                                    phase = after_intent(
                                        bot,
                                        &mut notes,
                                        &supplied,
                                        &config_path,
                                        done,
                                    );
                                } else {
                                    status = Some(format!(
                                        "still off ({tries}/{INTENT_RECHECKS}): toggle it, Save Changes, then Enter"
                                    ));
                                    phase = Phase::Intent { bot, tries };
                                }
                            }
                            KeyCode::Char('s') => {
                                status = None;
                                notes.extend(discord_check::intent_lines(&bot));
                                phase =
                                    after_intent(bot, &mut notes, &supplied, &config_path, done);
                            }
                            _ => {}
                        }
                    }
                    Phase::Allow { .. } => {
                        let Phase::Allow { bot } = &phase else {
                            unreachable!("matched Allow above")
                        };
                        let bot = bot.clone();
                        let done = idx >= fields.len();
                        match code {
                            KeyCode::Esc => return Ok(()),
                            KeyCode::Enter | KeyCode::Char('y') => {
                                allow_owners(&bot, &mut supplied, &config_path);
                                notes.push(allowlisted_line(&bot));
                                phase = next_field(done);
                            }
                            KeyCode::Char('n') => {
                                notes.push(
                                    "Not allowlisted: DM the bot later and approve the pairing code."
                                        .to_string(),
                                );
                                phase = next_field(done);
                            }
                            _ => {}
                        }
                    }
                    Phase::Failed => match code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                            anyhow::bail!("{}", report.clone().unwrap_or_default())
                        }
                        _ => {}
                    },
                    Phase::Verifying | Phase::Done => match code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => return Ok(()),
                        _ => {}
                    },
                },
                Event::Paste(text) => {
                    if matches!(phase, Phase::Filling) {
                        super::connect::insert_paste(&mut buf, &text);
                    }
                }
                _ => {}
            }
            if matches!(phase, Phase::Verifying) && report.is_none() {
                if checks_token {
                    normalize_allowed(&mut supplied, &config_path);
                }
                // The onboarding notes (invite link first of all) stay on
                // screen whatever the doctor says.
                let lead = if notes.is_empty() {
                    String::new()
                } else {
                    format!("{}\n\n", notes.join("\n"))
                };
                match finish_after_answers(app, decl, &gray_home, &user, &supplied, true) {
                    Ok(line) => {
                        report = Some(format!("{lead}{line}"));
                        phase = Phase::Done;
                    }
                    Err(e) => {
                        report = Some(format!("{lead}{e:#}"));
                        phase = Phase::Failed;
                    }
                }
            }
        }
    })();
    drop(terminal);
    outcome
}

/// Whether the register post-step still has to run. `gray <app> register`
/// refuses to overwrite an existing entry, so on a re-setup (the command is
/// already registered) running it fails and strands the flow before the
/// daemon can start. Register only when the app declares it AND it is not
/// registered yet — making `finish_after_answers` idempotent.
fn needs_registration(decl: &SetupDecl, gray_home: &Path, app: &str) -> bool {
    decl.post_steps.contains(&"register") && !crate::plugin_cli::is_command(gray_home, app)
}

/// Prove the answers, write nothing more, register the app's tool. The
/// doctor's own text is the only success report.
fn finish_after_answers(
    app: &str,
    decl: &SetupDecl,
    gray_home: &Path,
    user_home: &Path,
    supplied: &Supplied,
    start: bool,
) -> anyhow::Result<String> {
    let missing = missing_required(decl, supplied, &user_home.join(decl.config_path));
    if !missing.is_empty() {
        anyhow::bail!("{} still needs:\n  {}", app, describe_missing(&missing));
    }
    write_config(
        &user_home.join(decl.config_path),
        decl,
        supplied,
        gray_home,
        user_home,
    )?;
    let verify = run_step(&verify_argv(app, gray_home, decl)?);
    if !verify.ok {
        anyhow::bail!(
            "the config was written, but {}'s doctor disagrees:\n{}",
            app,
            verify.output.trim()
        );
    }
    if needs_registration(decl, gray_home, app) {
        let register = run_step(&register_argv(app, gray_home)?);
        anyhow::ensure!(
            register.ok,
            "the config works but registering the tool failed:\n{}",
            register.output.trim()
        );
    }
    let config_path = user_home.join(decl.config_path);
    let mut report = format!("{app} is set up.");
    // An ownerless app is finished, not broken: the bot tells their own ID
    // to whoever DMs it, and one command admits them. Say so here so the
    // operator never has to guess what to do with the pairing code.
    if decl.field("owner_id").is_some() && !super::registry::key_present(&config_path, "owner_id") {
        report.push_str(
            "\n\nOwner: nobody admitted yet.\n1. DM the bot anything.\n2. It replies with your own Discord ID and a one-time code.\n3. gray discord pairing approve discord <code>",
        );
    }
    let later = missing_optional(decl, user_home);
    if !later.is_empty() {
        let names: Vec<&str> = later.iter().map(|f| f.key).collect();
        report.push_str(&format!("\n\nSet later: {}", names.join(", ")));
    }
    if start && let Some(service) = decl.service {
        let mut argv = crate::plugin_cli::command_argv(gray_home, app)?;
        argv.extend(service.iter().skip(1).map(|a| a.to_string()));
        let config_dir = user_home
            .join(decl.config_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| user_home.to_path_buf());
        let line = start_daemon(app, &argv, &config_dir)?;
        report.push('\n');
        report.push_str(&line);
    }
    Ok(report)
}

/// Rendered width of masked input: one bullet per character.
fn mask(buf: &str) -> String {
    "\u{2022}".repeat(buf.chars().count())
}

#[cfg(test)]
mod modal_tests {
    use super::mask;

    #[test]
    fn masking_hides_every_character() {
        assert_eq!(mask(""), "");
        assert_eq!(
            mask("sk-abc"),
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
        );
    }
}

#[path = "app_flow_tests.rs"]
#[cfg(test)]
mod tests;
