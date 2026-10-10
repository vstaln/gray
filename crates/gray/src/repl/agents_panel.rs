//! Interactive agents panel — the keyboard-driven sibling of the
//! subagents widget. A plugin's `agent_picker` command outcome opens it;
//! rows and every action ride that plugin's own CLI (`entries` for the
//! list, `view`/`open`/`stop`/`chat` for the verbs), so the panel works
//! for any plugin speaking that surface. Enter on a finished run hands
//! its child session to `/resume` — the subagent then chats as its own
//! session, the same model opencode and Claude Code use.

use serde_json::Value;
use std::path::PathBuf;

/// What the dispatch arm does after the modal closes.
pub(crate) enum PanelAction {
    /// `/resume` this session — a finished run's own child session.
    /// The name rides along so the receipt can say whose session it is.
    Resume { session: String, name: String },
    /// Print text to scrollback (open card, view tail, receipts).
    Say(String),
}

#[derive(Debug, Clone)]
struct Row {
    name: String,
    task: String,
    icon: String,
    status: String,
    stats: String,
    activity: String,
    session: String,
    target: String,
    managed: bool,
    running: bool,
    resumable: bool,
}

/// `<plugin> <verb>` argv resolved from the install lock: `cli_argv`
/// first (the command shim), then the sidecar `argv`.
fn plugin_argv(plugin: &str) -> Option<Vec<String>> {
    let home = crate::plugin_cli::home().ok()?;
    let lock: Value =
        serde_json::from_slice(&std::fs::read(home.join("plugins").join("lock.json")).ok()?)
            .ok()?;
    let entry = &lock["plugins"][plugin];
    for key in ["cli_argv", "argv"] {
        if let Some(argv) = entry[key].as_array() {
            let argv: Vec<String> = argv
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if !argv.is_empty() {
                return Some(argv);
            }
        }
    }
    None
}

/// Raw stdout of one plugin CLI call; stderr tail on failure.
fn spawn_raw(argv: &[String], args: &[&str]) -> Option<String> {
    let (prog, base) = argv.split_first()?;
    let out = std::process::Command::new(prog)
        .args(base)
        .args(args)
        .env(
            "GRAY_BIN",
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gray")),
        )
        .output()
        .ok()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = err
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("plugin command failed");
        return Some(msg.to_string());
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Most verbs answer `{"content": "…"}`; unwrap that, else the raw text.
fn spawn_text(argv: &[String], args: &[&str]) -> Option<String> {
    let raw = spawn_raw(argv, args)?;
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    Some(
        v["content"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(raw)
            .trim_end()
            .to_string(),
    )
}

/// Read-only slash-command view used while a turn owns the agent. Bare
/// `/subagents` opens the picker below; explicit read verbs print here.
pub(crate) fn run_subagents_view(args: &[String]) -> Option<String> {
    let argv = plugin_argv("subagents")?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    spawn_text(&argv, &refs)
}

fn fetch_rows(argv: &[String], all: bool) -> Vec<Row> {
    let mut args = vec!["entries"];
    if all {
        args.push("--all");
    }
    let Some(raw) = spawn_raw(argv, &args) else {
        return Vec::new();
    };
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let Some(entries) = v["entries"].as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|e| Row {
            name: e["name"].as_str().unwrap_or("agent").into(),
            task: e["task"].as_str().unwrap_or("").into(),
            icon: e["icon"].as_str().unwrap_or("·").into(),
            status: e["status"].as_str().unwrap_or("").into(),
            stats: e["stats"].as_str().unwrap_or("").into(),
            activity: e["activity"].as_str().unwrap_or("").into(),
            session: e["session"].as_str().unwrap_or("").into(),
            target: e["target"].as_str().unwrap_or("").into(),
            managed: e["managed"].as_bool().unwrap_or(false),
            running: e["running"].as_bool().unwrap_or(false),
            resumable: e["resumable"].as_bool().unwrap_or(false),
        })
        .collect()
}

/// The keyboard-navigable overlay: ↑↓/j/k move, Enter opens (finished
/// runs resume their child session), c chats into the run, x stops, v
/// prints the transcript tail, a widens to every session, r refreshes.
/// Synchronous like the other pickers — plugin calls are local CLI
/// spawns, fast enough to run between key events.
pub(crate) fn run_agents_picker(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<PanelAction>> {
    run_agents_panel(plugin, bg, false)
}

/// The same picker as a turn-safe viewer: it can browse and print transcript
/// tails, but every verb that mutates or steals the session is disabled.
pub(crate) fn run_agents_viewer(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<PanelAction>> {
    run_agents_panel(plugin, bg, true)
}

fn run_agents_panel(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
    read_only: bool,
) -> anyhow::Result<Option<PanelAction>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let Some(argv) = plugin_argv(plugin) else {
        return Ok(Some(PanelAction::Say(format!(
            "⬢ Agents\n  ⎿ {plugin}: no CLI in the plugin lock — install it first"
        ))));
    };

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(crate::setup::BackgroundSnapshot::default_initial);
    let (_session, mut terminal) = crate::setup::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;
    let on_selection = crate::theme::theme().on_selection;

    let mut all = false;
    let mut rows = fetch_rows(&argv, all);
    let mut sel: usize = 0;
    let mut scroll_top: usize = 0;
    let mut status = String::new();
    let mut preview_key = String::new();
    let mut preview = String::new();
    let mut chat_buf: Option<String> = None;
    let mut result: Option<PanelAction> = None;

    let out: anyhow::Result<()> = (|| {
        loop {
            if rows.is_empty() {
                sel = 0;
            } else if sel >= rows.len() {
                sel = rows.len() - 1;
            }
            // Lazy preview: only respawn `view` when the selection moved
            // or a refresh landed.
            let cur_key = rows
                .get(sel)
                .map(|r| format!("{}|{}", r.target, r.status))
                .unwrap_or_default();
            if cur_key != preview_key && !rows.is_empty() {
                let row = &rows[sel];
                preview = spawn_text(&argv, &["view", &row.target, "--lines", "60"])
                    .unwrap_or_else(|| "(no transcript)".into());
                preview_key = cur_key;
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 40 || area.height < 10 {
                    return;
                }
                crate::setup::render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 96.min(area.width.saturating_sub(2)).max(42).min(area.width);
                let modal_h = 26
                    .min(area.height.saturating_sub(2))
                    .max(12)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 4;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);
                frame.render_widget(
                    Block::default().style(Style::default().bg(box_bg)),
                    modal_rect,
                );

                let pad_x = 2u16;
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    modal_w.saturating_sub(pad_x * 2),
                    modal_h.saturating_sub(2),
                );
                let mut put = |y: u16, line: Line| {
                    frame.render_widget(
                        Paragraph::new(line),
                        Rect::new(inner.x, inner.y + y, inner.width, 1),
                    );
                };

                let title = if read_only {
                    if all {
                        "Agents — all sessions · read-only"
                    } else {
                        "Agents — read-only"
                    }
                } else if all {
                    "Agents — all sessions"
                } else {
                    "Agents"
                };
                let esc_str = "esc";
                let pad_len = (inner.width as usize)
                    .saturating_sub(title.chars().count() + esc_str.chars().count() + 8);
                put(
                    0,
                    Line::from(vec![
                        Span::styled(
                            title,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                        Span::styled("a: all/cwd  ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                    ]),
                );

                // List band: up to 8 rows, then a preview band, then the
                // status/input line, then the keys hint.
                let list_h = rows.len().clamp(1, 8);
                let list_y = 2u16;
                let visible = list_h;
                if sel < scroll_top {
                    scroll_top = sel;
                } else if sel >= scroll_top + visible {
                    scroll_top = sel + 1 - visible;
                }
                if rows.is_empty() {
                    put(
                        list_y,
                        Line::from(Span::styled(
                            "  no runs yet — gray subagents run 'task' to start one",
                            Style::default().fg(text_dim).bg(box_bg),
                        )),
                    );
                } else {
                    for r in 0..visible {
                        let idx = scroll_top + r;
                        if idx >= rows.len() {
                            break;
                        }
                        let row = &rows[idx];
                        let is_sel = idx == sel;
                        let task_w = (inner.width as usize).saturating_sub(
                            3 + row.icon.chars().count()
                                + 1
                                + row.name.chars().count()
                                + 3
                                + row.stats.chars().count()
                                + 3
                                + row.activity.chars().count().min(20),
                        );
                        let task: String = row.task.chars().take(task_w.max(8)).collect();
                        let mut content = format!(
                            " {} {} {} — {} · {}",
                            if is_sel { "▸" } else { " " },
                            row.icon,
                            row.name,
                            task,
                            row.stats
                        );
                        if row.running && !row.activity.is_empty() {
                            let act: String = row.activity.chars().take(20).collect();
                            content.push_str(&format!(" · {act}"));
                        }
                        if !row.running {
                            content.push_str(&format!(" · {}", row.status));
                        }
                        let fill = (inner.width as usize).saturating_sub(content.chars().count());
                        let row_str = format!("{}{}", content, " ".repeat(fill));
                        let line = if is_sel {
                            Line::from(Span::styled(
                                row_str,
                                Style::default()
                                    .fg(on_selection)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            Line::from(Span::styled(
                                row_str,
                                Style::default().fg(Color::White).bg(box_bg),
                            ))
                        };
                        put(list_y + r as u16, line);
                    }
                }

                // Preview band: the selected run's transcript tail, in
                // the widget's own `view` voice.
                let prev_y = list_y + list_h as u16 + 1;
                let status_h = 2u16;
                let keys_h = 1u16;
                let prev_h = inner
                    .height
                    .saturating_sub(prev_y + status_h + keys_h + 1)
                    .min(12);
                if prev_h >= 2 {
                    let title = rows
                        .get(sel)
                        .map(|r| {
                            if r.session.is_empty() {
                                format!(" ─ {} ", r.name)
                            } else {
                                format!(" ─ {} · {} ", r.name, r.session)
                            }
                        })
                        .unwrap_or_else(|| " ─ ".to_string());
                    put(
                        prev_y - 1,
                        Line::from(Span::styled(
                            format!(
                                "{title:─<w$}",
                                w = inner.width as usize + title.chars().count()
                            ),
                            Style::default().fg(text_dim).bg(box_bg),
                        )),
                    );
                    for (i, line) in preview.lines().take(prev_h as usize).enumerate() {
                        let clipped: String = line.chars().take(inner.width as usize).collect();
                        put(
                            prev_y + i as u16,
                            Line::from(Span::styled(
                                format!(" {clipped}"),
                                Style::default().fg(Color::White).bg(box_bg),
                            )),
                        );
                    }
                }

                // Status / chat-input band, then the keys hint.
                let status_y = inner.height.saturating_sub(2);
                if let Some(buf) = &chat_buf {
                    let prompt = format!(
                        " chat {}: {}_",
                        rows.get(sel).map(|r| r.name.as_str()).unwrap_or(""),
                        buf
                    );
                    let clipped: String = prompt.chars().take(inner.width as usize).collect();
                    put(
                        status_y,
                        Line::from(Span::styled(
                            clipped,
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        )),
                    );
                } else {
                    // Idle: the band spells out what the selected row can
                    // do — the affordance, not just the key names.
                    let hint = rows.get(sel).map_or("", |r| {
                        if read_only {
                            " read-only while a turn is running · ⏎/v transcript"
                        } else if r.running {
                            " c queues a follow-up · v transcript · x stops it"
                        } else if r.resumable {
                            " ⏎ steps into its session · c continues it · v transcript"
                        } else if r.managed {
                            " v transcript · c starts a new turn"
                        } else {
                            " v transcript"
                        }
                    });
                    let text = if status.is_empty() {
                        hint
                    } else {
                        status.as_str()
                    };
                    let clipped: String = text.chars().take(inner.width as usize).collect();
                    if !clipped.is_empty() {
                        put(
                            status_y,
                            Line::from(Span::styled(
                                format!(" {clipped}"),
                                Style::default().fg(text_dim).bg(box_bg),
                            )),
                        );
                    }
                }
                put(
                    inner.height.saturating_sub(1),
                    Line::from(Span::styled(
                        if read_only {
                            " ↑↓ move · ⏎/v view · a all · r refresh · esc close"
                        } else {
                            " ↑↓ move · ⏎ open · c chat · x stop · v view · a all · r refresh"
                        },
                        Style::default().fg(text_dim).bg(box_bg),
                    )),
                );
            })?;

            if !poll(Duration::from_millis(250))? {
                continue;
            }
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = read()?
            else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            use crossterm::event::KeyModifiers;
            if chat_buf.is_some() {
                match code {
                    KeyCode::Esc => chat_buf = None,
                    KeyCode::Enter => {
                        let msg = chat_buf.take().unwrap_or_default();
                        if msg.trim().is_empty() {
                            status = "chat cancelled".into();
                        } else if let Some(row) = rows.get(sel) {
                            status = spawn_text(&argv, &["chat", &row.target, &msg])
                                .unwrap_or_else(|| "chat failed".into());
                            rows = fetch_rows(&argv, all);
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(b) = chat_buf.as_mut() {
                            b.pop();
                        }
                    }
                    KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(b) = chat_buf.as_mut() {
                            b.push(c)
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match code {
                KeyCode::Esc | KeyCode::Char('q') => break,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    sel = sel.checked_sub(1).unwrap_or(rows.len().saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !rows.is_empty() {
                        sel = (sel + 1) % rows.len();
                    }
                }
                KeyCode::Enter => {
                    let Some(row) = rows.get(sel) else {
                        continue;
                    };
                    if read_only {
                        let text = spawn_text(&argv, &["view", &row.target, "--lines", "200"])
                            .unwrap_or_else(|| "(no transcript)".into());
                        result = Some(PanelAction::Say(text));
                        break;
                    }
                    if !row.running && row.resumable {
                        result = Some(PanelAction::Resume {
                            session: row.session.clone(),
                            name: row.name.clone(),
                        });
                        break;
                    }
                    // Running or session-less: the open card says what to
                    // do next (chat/stop/view, or transcript tail).
                    let text = spawn_text(&argv, &["open", &row.target])
                        .unwrap_or_else(|| "open failed".into());
                    result = Some(PanelAction::Say(text));
                    break;
                }
                KeyCode::Char('v') => {
                    if let Some(row) = rows.get(sel) {
                        let text = spawn_text(&argv, &["view", &row.target, "--lines", "200"])
                            .unwrap_or_else(|| "(no transcript)".into());
                        result = Some(PanelAction::Say(text));
                        break;
                    }
                }
                KeyCode::Char('x') => {
                    if read_only {
                        status = "read-only while a turn is running".into();
                    } else if let Some(row) = rows.get(sel).cloned() {
                        if row.running {
                            status = spawn_text(&argv, &["stop", &row.target])
                                .unwrap_or_else(|| "stop failed".into());
                        } else {
                            status = format!("{} already {}", row.name, row.status);
                        }
                        rows = fetch_rows(&argv, all);
                    }
                }
                KeyCode::Char('c') => {
                    if read_only {
                        status = "read-only while a turn is running".into();
                    } else if let Some(row) = rows.get(sel) {
                        if row.managed {
                            chat_buf = Some(String::new());
                        } else {
                            status =
                                format!("{} is a raw worker — nothing to chat through", row.name);
                        }
                    }
                }
                KeyCode::Char('a') => {
                    all = !all;
                    rows = fetch_rows(&argv, all);
                }
                KeyCode::Char('r') => {
                    rows = fetch_rows(&argv, all);
                    status = "refreshed".into();
                }
                _ => {}
            }
        }
        Ok(())
    })();
    out?;
    Ok(result)
}
