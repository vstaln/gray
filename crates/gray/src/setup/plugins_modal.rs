use super::*;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

struct Row {
    name: String,
    version: String,
    enabled: bool,

    kind: String,

    source: String,

    start: Option<Result<(), String>>,

    stale: Option<String>,
    commands: Vec<String>,
    tools: Vec<String>,
    hooks: Vec<String>,
    capabilities: Vec<String>,

    error: Option<String>,
    build_log: Option<PathBuf>,
}

fn exe_kind(entry: &gray_plugin::lock::LockEntry, cli: bool) -> String {
    let exe = entry
        .cli_argv
        .as_ref()
        .and_then(|a| a.first())
        .or_else(|| entry.argv.first());
    let registered = (entry.cli_argv.is_some() && entry.hash.is_empty()) || cli;
    if registered {
        let local = exe.is_some_and(|e| {
            let p = Path::new(e);
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "release" || n == "debug")
                && p.ancestors()
                    .any(|a| a.file_name().is_some_and(|n| n == "target"))
        });
        return if local { "local build" } else { "executable" }.to_string();
    }
    if !entry.hash.is_empty() || entry.ecosystem == "gray-native" {
        return "index".to_string();
    }
    entry.ecosystem.clone()
}

fn manifest_lists(home: &Path, name: &str) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let path = home.join("plugins").join(format!("{name}-manifest.json"));
    let Ok(raw) = std::fs::read(&path) else {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    };
    let names = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        e.as_str()
                            .map(str::to_string)
                            .or_else(|| e.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    (
        names("commands"),
        names("tools"),
        names("hooks"),
        names("capabilities"),
    )
}

fn last_recorded_error(name: &str) -> Option<String> {
    gray_pkg::errors::list()
        .into_iter()
        .filter(|e| e.item == name)
        .max_by_key(|e| e.ts_secs)
        .map(|e| format!("{}: {}", e.source, e.message))
}

fn build_log_for(name: &str) -> Option<PathBuf> {
    let p = crate::setup::gray_home()
        .ok()?
        .join("logs")
        .join(format!("plugin-build-{name}.log"));
    p.exists().then_some(p)
}

fn load_rows() -> Vec<Row> {
    let Ok(home) = crate::plugin_cli::home() else {
        return Vec::new();
    };
    let results = crate::plugin_status::results();
    crate::plugin_cli::list_managed(&home)
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let on = crate::plugin_cli::enabled(&home, &p.name, &p.entry);
            let start = results.as_ref().and_then(|rs| {
                rs.iter()
                    .find(|s| s.name == p.name)
                    .map(|s| s.error.clone().map_or(Ok(()), Err))
            });
            let exe = p
                .entry
                .argv
                .first()
                .or_else(|| p.entry.cli_argv.as_ref().and_then(|a| a.first()))
                .cloned()
                .unwrap_or_else(|| p.entry.source.clone());
            let (commands, tools, hooks, capabilities) = manifest_lists(&home, &p.name);
            let probe_err = start.as_ref().and_then(|s| s.clone().err());
            let error = probe_err.or_else(|| last_recorded_error(&p.name));
            let build_log = build_log_for(&p.name);
            let kind = exe_kind(&p.entry, p.cli);
            Row {
                error,
                kind,
                source: exe,
                name: p.name,
                version: p.entry.version.clone(),
                enabled: on,
                start,
                stale: None,
                commands,
                tools,
                hooks,
                capabilities,
                build_log,
            }
        })
        .collect()
}

fn status_text(row: &Row) -> String {
    if !row.enabled {
        return "disabled".to_string();
    }
    match &row.start {
        Some(Ok(())) => "loaded".to_string(),
        Some(Err(e)) => format!("failed: {e}"),
        None => "not loaded".to_string(),
    }
}

fn spawn_update(tx: Sender<(String, Result<Vec<String>, String>)>, target: String) {
    std::thread::spawn(move || {
        let out = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("runtime: {e}"))
            .and_then(|rt| {
                rt.block_on(crate::plugin_cli::update_managed(&target))
                    .map_err(|e| format!("{e:#}"))
                    .map(|rows| {
                        rows.iter()
                            .map(|r| format!("{}: {}", r.name, r.detail()))
                            .collect::<Vec<_>>()
                    })
            });
        let _ = tx.send((target, out));
    });
}

fn spawn_stale_check(tx: Sender<Vec<(String, String)>>) {
    std::thread::spawn(move || {
        let anns = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| rt.block_on(crate::plugin_cli::check_managed("all")).ok())
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| match &r.action {
                        gray_pkg::ops::UpdateAction::Stale(why) => {
                            Some((r.name.clone(), why.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let _ = tx.send(anns);
    });
}

fn detail_lines(row: &Row) -> Vec<(String, String)> {
    let join = |v: &[String]| {
        if v.is_empty() {
            "—".to_string()
        } else {
            v.join(" ")
        }
    };
    let mut lines = vec![
        ("version".to_string(), row.version.clone()),
        (
            "enabled".to_string(),
            if row.enabled { "yes" } else { "no" }.to_string(),
        ),
        ("status".to_string(), status_text(row)),
        (
            "source".to_string(),
            format!("{} — {}", row.kind, row.source),
        ),
        ("commands".to_string(), join(&row.commands)),
        (
            "tools".to_string(),
            if row.tools.is_empty() {
                "—".to_string()
            } else {
                row.tools.join(", ")
            },
        ),
        ("hooks".to_string(), row.hooks.join(", ")),
        ("capabilities".to_string(), join(&row.capabilities)),
    ];
    if let Some(stale) = &row.stale {
        lines.push(("update".to_string(), stale.clone()));
    }
    if let Some(err) = &row.error {
        lines.push(("last error".to_string(), err.clone()));
    }
    if let Some(log) = &row.build_log {
        lines.push(("build log".to_string(), log.display().to_string()));
    }
    lines
}

pub(crate) fn run_plugins_manager(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let mut rows = load_rows();
    let mut sel = 0usize;
    let mut query = String::new();
    let mut detail = false;
    let mut changed = false;
    let mut op_msg: Option<String> = None;
    let mut updating: Option<String> = None;
    let (upd_tx, upd_rx) = std::sync::mpsc::channel::<(String, Result<Vec<String>, String>)>();
    let (check_tx, check_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
    spawn_stale_check(check_tx);

    let (_session, mut terminal) = super::open_modal()?;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    (|| -> anyhow::Result<bool> {
        loop {
            while let Ok(anns) = check_rx.try_recv() {
                for (name, why) in anns {
                    if let Some(r) = rows.iter_mut().find(|r| r.name == name) {
                        r.stale = Some(why);
                    }
                }
            }
            while let Ok((target, out)) = upd_rx.try_recv() {
                updating = None;
                crate::plugin_status::drop_boot();
                match out {
                    Ok(lines) => {
                        let summary = if lines.is_empty() {
                            format!("{target}: up to date")
                        } else {
                            changed = true;
                            lines.join("  ")
                        };
                        op_msg = Some(summary);
                    }
                    Err(e) => op_msg = Some(format!("{target}: update failed — {e}")),
                }
                let fresh = load_rows();
                for (i, r) in fresh.into_iter().enumerate() {
                    if let Some(old) = rows.get_mut(i)
                        && old.name == r.name
                    {
                        let stale = old.stale.take();
                        *old = r;
                        old.stale = stale;
                    }
                }
            }
            let q = query.to_lowercase();
            let filtered: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, r)| q.is_empty() || r.name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
            sel = sel.min(filtered.len().saturating_sub(1));

            terminal.draw(|frame| {
                let th = crate::theme::theme();
                let area = frame.area();
                if area.width < 30 || area.height < 10 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = area.width.saturating_sub(4).min(110);
                let modal_h = area.height.saturating_sub(4).min(28);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal);
                frame.render_widget(
                    Block::default().style(Style::default().bg(th.surface_bg)),
                    modal,
                );
                let (ix, iw) = (modal.x + 1, modal.width.saturating_sub(2));
                let w = iw as usize;
                let at = |y: u16| Rect::new(ix, y, iw, 1);
                let top = modal.y + 1;
                let note_y = modal.y + modal.height.saturating_sub(4);
                let keys_y = note_y + 1;

                if detail && let Some(&ri) = filtered.get(sel) {
                    let row = &rows[ri];
                    let head = Line::from(vec![
                        Span::styled(
                            row.name.clone(),
                            Style::default()
                                .fg(th.text_bright)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", status_text(row)),
                            Style::default().fg(th.text_dim),
                        ),
                    ]);
                    frame.render_widget(Paragraph::new(head), at(top));
                    let lines = detail_lines(row);
                    let key_w = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
                    for (i, (k, v)) in lines.iter().enumerate() {
                        let y = top + 2 + i as u16;
                        if y >= note_y {
                            break;
                        }
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(
                                    format!("{k:<key_w$}  "),
                                    Style::default().fg(th.text_muted),
                                ),
                                Span::styled(v.clone(), Style::default().fg(th.text_body)),
                            ])),
                            at(y),
                        );
                    }
                } else {
                    let query_line = if query.is_empty() {
                        "type to filter".to_string()
                    } else {
                        format!("{query}█")
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            query_line,
                            Style::default().fg(if query.is_empty() {
                                th.text_dim
                            } else {
                                th.text_body
                            }),
                        ))),
                        at(top),
                    );
                    let list_y = top + 2;
                    let list_h = (note_y.saturating_sub(list_y)) as usize;
                    let start = sel
                        .saturating_sub(list_h.saturating_sub(1))
                        .min(filtered.len().saturating_sub(list_h));
                    if filtered.is_empty() {
                        let empty_msg = if query.is_empty() {
                            "  no plugins installed — gray plugin install <name>"
                        } else {
                            "  no match"
                        };
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                empty_msg,
                                Style::default().fg(th.text_dim),
                            ))),
                            at(list_y),
                        );
                    }
                    for (vi, &ri) in filtered.iter().enumerate().skip(start).take(list_h) {
                        let row = &rows[ri];
                        let y = list_y + (vi - start) as u16;
                        let is_sel = vi == sel;
                        let mark = if !row.enabled {
                            "○"
                        } else if matches!(&row.start, Some(Err(_))) {
                            "✗"
                        } else {
                            "✓"
                        };
                        let stale = row.stale.as_ref().map(|_| "  [update]").unwrap_or_default();
                        let text = format!(
                            "{} {:<26} {:<10} {:<32} {}{}",
                            mark,
                            row.name,
                            row.version,
                            status_text(row),
                            row.kind,
                            stale,
                        );
                        let text: String = text.chars().take(w).collect();
                        let fill = w.saturating_sub(text.chars().count());
                        let line = if is_sel {
                            Line::from(Span::styled(
                                format!("{text}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(th.on_selection)
                                    .bg(th.accent)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let fg = if row.enabled {
                                th.text_bright
                            } else {
                                th.text_dim
                            };
                            Line::from(vec![
                                Span::styled(text, Style::default().fg(fg).bg(th.surface_bg)),
                                Span::styled(" ".repeat(fill), Style::default().bg(th.surface_bg)),
                            ])
                        };
                        frame.render_widget(Paragraph::new(line), at(y));
                    }
                }

                let note = if let Some(target) = &updating {
                    format!("updating {target} …")
                } else {
                    op_msg.clone().unwrap_or_default()
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        note,
                        Style::default().fg(th.text_dim).bg(th.surface_bg),
                    ))),
                    at(note_y),
                );
                let hint_segs: &[&str] = if detail {
                    &["space toggle", "u update", "U update all", "esc back"]
                } else {
                    &[
                        "↑↓ move",
                        "type to filter",
                        "tab toggle",
                        "ctrl+u update",
                        "enter details",
                        "esc close",
                        "toggles apply next session or /restart",
                    ]
                };
                let mut hint_rows: Vec<String> = vec![String::new()];
                for seg in hint_segs {
                    let last = hint_rows.len() - 1;
                    let joined = if hint_rows[last].is_empty() {
                        seg.to_string()
                    } else {
                        format!("{} · {seg}", hint_rows[last])
                    };
                    if joined.chars().count() <= w || hint_rows[last].is_empty() {
                        hint_rows[last] = joined;
                    } else if hint_rows.len() < 2 {
                        hint_rows.push(seg.to_string());
                    }
                }
                for (i, line) in hint_rows.iter().enumerate() {
                    let line = if line.chars().count() > w {
                        let cut: String = line.chars().take(w).collect();
                        cut[..cut.rfind(' ').unwrap_or(0)]
                            .trim_end_matches(['·', ' '])
                            .to_string()
                    } else {
                        line.clone()
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            line,
                            Style::default().fg(th.text_faint).bg(th.surface_bg),
                        ))),
                        at(keys_y + i as u16),
                    );
                }
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('c') => return Ok(changed),

                    KeyCode::Char('u') if updating.is_none() => {
                        if let Some(&ri) = filtered.get(sel) {
                            let target = rows[ri].name.clone();
                            updating = Some(target.clone());
                            spawn_update(upd_tx.clone(), target);
                        }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    modifiers,
                    ..
                }) => {
                    if detail {
                        match code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace => detail = false,
                            KeyCode::Char(' ') => {
                                if let Some(&ri) = filtered.get(sel) {
                                    let name = rows[ri].name.clone();
                                    let now_on = !rows[ri].enabled;
                                    match crate::plugin_cli::set_managed_enabled(&name, now_on) {
                                        Ok(()) => {
                                            changed = true;
                                            crate::plugin_status::drop_boot();
                                            rows[ri].enabled = now_on;
                                            op_msg = Some(format!(
                                                "{name} {} (applies next session or /restart)",
                                                if now_on { "enabled" } else { "disabled" },
                                            ));
                                        }
                                        Err(e) => {
                                            op_msg = Some(format!("toggle {name} failed: {e}"))
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('u') | KeyCode::Char('U') if updating.is_none() => {
                                let target = if code == KeyCode::Char('U') {
                                    "all".to_string()
                                } else {
                                    match filtered.get(sel) {
                                        Some(&ri) => rows[ri].name.clone(),
                                        None => continue,
                                    }
                                };
                                updating = Some(target.clone());
                                spawn_update(upd_tx.clone(), target);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match code {
                        KeyCode::Esc => {
                            if query.is_empty() {
                                return Ok(changed);
                            }
                            query.clear();
                            sel = 0;
                        }
                        KeyCode::Enter => {
                            if !filtered.is_empty() {
                                detail = true;
                            }
                        }
                        KeyCode::Backspace => {
                            query.pop();
                            sel = 0;
                        }
                        KeyCode::Tab => {
                            if let Some(&ri) = filtered.get(sel) {
                                let name = rows[ri].name.clone();
                                let now_on = !rows[ri].enabled;
                                match crate::plugin_cli::set_managed_enabled(&name, now_on) {
                                    Ok(()) => {
                                        changed = true;
                                        crate::plugin_status::drop_boot();
                                        rows[ri].enabled = now_on;
                                        op_msg = Some(format!(
                                            "{name} {} (applies next session or /restart)",
                                            if now_on { "enabled" } else { "disabled" },
                                        ));
                                    }
                                    Err(e) => op_msg = Some(format!("toggle {name} failed: {e}")),
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Down => {
                            let last = filtered.len().saturating_sub(1);
                            sel = if code == KeyCode::Up {
                                sel.saturating_sub(1)
                            } else {
                                (sel + 1).min(last)
                            };
                        }
                        KeyCode::PageUp | KeyCode::PageDown => {
                            let last = filtered.len().saturating_sub(1);
                            sel = if code == KeyCode::PageUp {
                                sel.saturating_sub(8)
                            } else {
                                (sel + 8).min(last)
                            };
                        }

                        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::ALT) && c != '/' => {
                            query.push(c);
                            sel = 0;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    })()
}
