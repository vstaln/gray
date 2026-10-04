//! The app setup flow's headless core: ask for exactly what the app
//! declares, write it privately, prove it with the app's own doctor, then
//! register the app's tool. The REPL modal (task 5b) rides the same core.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use super::channel_picker::{ChannelSource, Destination, RestChannels};
use super::registry::{FieldKind, SetupDecl, SetupField};
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

/// Headless twin of the /gateway flow: write what the flags answer, prove it
/// with the app's doctor, register the app's tool, and say so. Anything
/// missing is reported, never guessed; nothing success-shaped is printed
/// until the doctor agrees.
pub fn run_headless(app: &str, fields: &[String], start: bool) -> Result<()> {
    let (gray_home, user) = (crate::plugin_cli::home()?, super::user_home()?);
    let decl = crate::plugin_cli::setup_decl(app)
        .with_context(|| format!("gray has no setup declaration for '{app}'"))?;
    let supplied = supplied_from_flags(decl, fields)?;
    let missing = missing_required(decl, &supplied, &user.join(decl.config_path));
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
    use ratatui::widgets::{Block, Clear, Paragraph};
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
        Verifying,
        Failed,
        Done,
    }
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
                let rect = ratatui::layout::Rect::new(
                    (area.width.saturating_sub(w)) / 2,
                    area.height / 4,
                    w,
                    12.min(area.height.saturating_sub(2).max(10)),
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
                    Phase::Verifying | Phase::Failed | Phase::Done => {
                        let text = report.clone().unwrap_or_default();
                        for line in text.lines().take(6) {
                            lines.push(Line::from(Span::styled(
                                line.to_string(),
                                Style::default().fg(text_dim).bg(box_bg),
                            )));
                        }
                    }
                }
                frame.render_widget(Paragraph::new(lines), inner);
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
                match finish_after_answers(app, decl, &gray_home, &user, &supplied, true) {
                    Ok(line) => {
                        report = Some(line);
                        phase = Phase::Done;
                    }
                    Err(e) => {
                        report = Some(format!("{e:#}"));
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
