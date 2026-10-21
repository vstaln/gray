//! `/accounts` — provider account pools.
//!
//! Two credential-ownership models show up here:
//!
//! - **Plugin-owned pools**: a `*-sub` plugin that answers
//!   `<cli> account list --json` (the account contract) is listed with its
//!   accounts, and mutations run the same CLI: `use`/`remove`/`strategy`
//!   plus `add` (a terminal handoff — the provider's own login needs the
//!   real tty). A plugin without the verb exits nonzero and is skipped.
//! - **Vault pools**: `auth.json` v2 plugin entries
//!   (`plugin:<lock>:<provider>:<method>`) hold labeled
//!   `CredentialEnvelope`s the host serves via `CredentialStore`. Pin,
//!   unpin, and remove work through the store directly; adding a second
//!   account goes through `/connect` until the login flow takes a label.
//!
//! The modal is deliberately thin: every row it shows and every mutation
//! it runs goes through the same CLI verbs a user can run by hand.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::auth::CredentialStore;
use crate::setup::open_modal;

/// How long a probe may run before the panel moves on without it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// A provider plugin discovered from the lock with its CLI argv, plus
/// what `account list --json` answered (`None` = probe failed or the
/// plugin doesn't pool).
struct Provider {
    /// Lock name (the `plugin:` auth_ref component).
    plugin: String,
    /// First provider id the manifest declares, for display.
    provider: String,
    argv: Option<Vec<String>>,
    doc: Option<Value>,
    probe_err: Option<String>,
}

/// A flat selectable row.
enum Row {
    /// Provider header — `s` cycles its strategy.
    Header {
        provider: usize,
    },
    Account {
        provider: usize,
        label: String,
    },
    /// "ambient login" row for an empty plugin pool.
    Ambient {
        provider: usize,
    },
    /// The "+ add account" affordance under a pool-capable provider.
    AddHint {
        provider: usize,
    },
    /// Vault (auth.json) account; mutations go through CredentialStore.
    Vault {
        auth_ref: String,
        label: String,
    },
    /// Non-pool provider: display only.
    Plain {
        provider: usize,
    },
}

struct Panel {
    providers: Vec<Provider>,
    rows: Vec<Row>,
    sel: usize,
    changed: bool,
    status: Option<String>,
    /// `Some((provider_idx, buffer))` while the label prompt is open.
    adding: Option<(usize, String)>,
    /// `Some((provider_idx, label))` while a remove awaits a second `d`.
    confirming_remove: Option<(usize, String)>,
}

fn lock_plugins(home: &std::path::Path) -> Vec<(String, gray_plugin::lock::LockEntry)> {
    gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))
        .map(|l| l.plugins.into_iter().filter(|(_, e)| e.enabled).collect())
        .unwrap_or_default()
}

fn plugin_argv_of(entry: &gray_plugin::lock::LockEntry) -> Option<Vec<String>> {
    entry
        .cli_argv
        .clone()
        .filter(|a| !a.is_empty())
        .or_else(|| (!entry.argv.is_empty()).then(|| entry.argv.clone()))
}

/// The first provider id a plugin's cached manifest declares.
fn manifest_provider(home: &std::path::Path, plugin: &str) -> Option<String> {
    let text =
        std::fs::read_to_string(home.join("plugins").join(format!("{plugin}-manifest.json")))
            .ok()?;
    let m: Value = serde_json::from_str(&text).ok()?;
    m["providers"]
        .as_array()?
        .first()?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// `<cli> account list --json` with a hard timeout; `Err` for nonzero
/// exits (a plugin without the verb) or timeouts.
fn probe_accounts(argv: &[String]) -> Result<Value, String> {
    let (prog, base) = argv.split_first().ok_or("empty argv")?;
    let mut child = std::process::Command::new(prog)
        .args(base)
        .args(["account", "list", "--json"])
        .env(
            "GRAY_BIN",
            crate::exe_watch::on_disk_exe().unwrap_or_else(|_| PathBuf::from("gray")),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("probe timed out".into());
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(err
            .lines()
            .next()
            .unwrap_or("account probe failed")
            .to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad account json: {e}"))
}

/// A mutation verb through the plugin CLI; `Ok(stdout)` on success.
fn account_cmd(argv: &[String], args: &[&str]) -> Result<String, String> {
    let (prog, base) = argv.split_first().ok_or("empty argv")?;
    let out = std::process::Command::new(prog)
        .args(base)
        .args(args)
        .env(
            "GRAY_BIN",
            crate::exe_watch::on_disk_exe().unwrap_or_else(|_| PathBuf::from("gray")),
        )
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("spawn: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(err
            .lines()
            .next()
            .unwrap_or("account command failed")
            .to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// auth.json `plugin:*` refs → (auth_ref, lock, provider id).
fn vault_refs() -> Vec<(String, String, String)> {
    crate::setup::catalog::auth_store_path()
        .ok()
        .map(|p| crate::setup::catalog::load_mixed_store(&p))
        .unwrap_or_default()
        .into_keys()
        .filter(|k| k.starts_with("plugin:"))
        .filter_map(|k| {
            let mut parts = k.splitn(4, ':');
            let _ = parts.next();
            let lock = parts.next()?.to_string();
            let provider = parts.next()?.to_string();
            Some((k, lock, provider))
        })
        .collect()
}

impl Panel {
    fn load() -> Self {
        let mut p = Panel {
            providers: Vec::new(),
            rows: Vec::new(),
            sel: 0,
            changed: false,
            status: None,
            adding: None,
            confirming_remove: None,
        };
        p.reload();
        p
    }

    fn reload(&mut self) {
        let home = crate::plugin_cli::home().unwrap_or_default();
        let mut providers: Vec<Provider> = Vec::new();
        for (name, entry) in lock_plugins(&home) {
            let Some(provider) = manifest_provider(&home, &name) else {
                continue;
            };
            let argv = plugin_argv_of(&entry);
            let (doc, probe_err) = match &argv {
                Some(argv) => match probe_accounts(argv) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e)),
                },
                None => (None, None),
            };
            providers.push(Provider {
                plugin: name,
                provider,
                argv,
                doc,
                probe_err,
            });
        }
        // Vault pools for providers without an answering plugin CLI get
        // their own group (e.g. codex): the pool lives in auth.json.
        let served: std::collections::BTreeSet<String> = providers
            .iter()
            .filter(|p| p.doc.is_some())
            .map(|p| p.plugin.clone())
            .collect();
        for (auth_ref, lock, provider_id) in vault_refs() {
            if served.contains(&lock) {
                continue;
            }
            providers.push(Provider {
                plugin: lock,
                provider: provider_id,
                argv: None,
                doc: Some(serde_json::json!({"_vault": auth_ref})),
                probe_err: None,
            });
        }
        self.providers = providers;
        self.rebuild_rows();
        if self.sel >= self.rows.len() {
            self.sel = self.rows.len().saturating_sub(1);
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (i, p) in self.providers.iter().enumerate() {
            self.rows.push(Row::Header { provider: i });
            if let Some(doc) = &p.doc {
                if let Some(auth_ref) = doc.get("_vault").and_then(Value::as_str) {
                    // auth.json pool rows
                    if let Some(store) = crate::setup::catalog::auth_store_path()
                        .ok()
                        .map(CredentialStore::new)
                    {
                        let rows = store.plugin_account_rows(auth_ref).unwrap_or_default();
                        for r in rows {
                            let _ = &r;
                            self.rows.push(Row::Vault {
                                auth_ref: auth_ref.to_string(),
                                label: r.label.clone(),
                            });
                        }
                    }
                } else {
                    let accounts = doc["accounts"].as_array();
                    match accounts {
                        Some(list) if !list.is_empty() => {
                            for a in list {
                                if let Some(label) = a["label"].as_str() {
                                    self.rows.push(Row::Account {
                                        provider: i,
                                        label: label.to_string(),
                                    });
                                }
                            }
                        }
                        _ => self.rows.push(Row::Ambient { provider: i }),
                    }
                    if p.argv.is_some() {
                        self.rows.push(Row::AddHint { provider: i });
                    }
                }
            } else {
                self.rows.push(Row::Plain { provider: i });
            }
        }
    }

    /// The account/pool detail a row renders.
    fn row_text(&self, row: &Row) -> (String, bool) {
        let dim = crate::theme::theme().text_dim;
        let _ = dim;
        match row {
            Row::Header { provider } => {
                let p = &self.providers[*provider];
                let strategy = p
                    .doc
                    .as_ref()
                    .and_then(|d| {
                        if d.get("_vault").is_some() {
                            let auth_ref = d["_vault"].as_str().unwrap_or_default();
                            crate::setup::catalog::auth_store_path()
                                .ok()
                                .map(CredentialStore::new)
                                .and_then(|s| s.plugin_pool_meta(auth_ref).ok().flatten())
                                .map(|(st, _)| st)
                        } else {
                            d["strategy"].as_str().map(str::to_string)
                        }
                    })
                    .unwrap_or_default();
                let mut line = format!("{}  ({})", p.plugin, p.provider);
                if !strategy.is_empty() {
                    line.push_str(&format!("   strategy {strategy}"));
                }
                if let Some(e) = &p.probe_err {
                    line.push_str(&format!("   — {e}"));
                }
                (line, true)
            }
            Row::Account { provider, label } => {
                let p = &self.providers[*provider];
                let doc = p.doc.as_ref();
                let a = doc.and_then(|d| {
                    d["accounts"]
                        .as_array()?
                        .iter()
                        .find(|x| x["label"].as_str() == Some(label))
                });
                let Some(a) = a else {
                    return (format!("    {label}"), false);
                };
                let pin = if a["pinned"].as_bool() == Some(true) {
                    "●"
                } else {
                    " "
                };
                let who = a["email"].as_str().unwrap_or("identity pending");
                let org = a["org"]
                    .as_str()
                    .map(|o| format!(" · {o}"))
                    .unwrap_or_default();
                let uses = a["request_count"].as_u64().unwrap_or(0);
                let state = if a["cooling"].as_bool() == Some(true) {
                    let until = a["cooldown_until"].as_u64().unwrap_or(0);
                    let now = now_secs();
                    let left = until.saturating_sub(now);
                    format!("cooling {}m{:02}s", left / 60, left % 60)
                } else {
                    "ok".to_string()
                };
                let err = a["last_error"]
                    .as_str()
                    .map(|e| format!("  ({e})"))
                    .unwrap_or_default();
                (
                    format!("  {pin} {label:<16} {who}{org}   {uses} uses · {state}{err}"),
                    false,
                )
            }
            Row::Ambient { .. } => (
                "    ambient login serves (no accounts yet)".to_string(),
                false,
            ),
            Row::AddHint { .. } => ("    + add account".to_string(), false),
            Row::Vault { auth_ref, label } => {
                let detail = crate::setup::catalog::auth_store_path()
                    .ok()
                    .map(CredentialStore::new)
                    .and_then(|s| {
                        s.plugin_account_rows(auth_ref)
                            .ok()?
                            .into_iter()
                            .find(|r| r.label == *label)
                    });
                let (pin, who, state) = match &detail {
                    Some(r) => (
                        if r.pinned { "●" } else { " " },
                        r.email.clone().unwrap_or_else(|| "credential".into()),
                        if r.cooling {
                            let left = r.cooldown_until.saturating_sub(now_secs());
                            format!("cooling {}m{:02}s", left / 60, left % 60)
                        } else {
                            "ok".to_string()
                        },
                    ),
                    None => (" ", "credential".to_string(), "?".to_string()),
                };
                (
                    format!("  {pin} {label:<16} {who}   vault · {state}"),
                    false,
                )
            }
            Row::Plain { provider } => {
                let p = &self.providers[*provider];
                (
                    format!("    {} — single login via /connect", p.plugin),
                    false,
                )
            }
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }

    fn move_sel(&mut self, delta: i64) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i64;
        self.sel = ((self.sel as i64 + delta).rem_euclid(n)) as usize;
        self.confirming_remove = None;
    }

    fn run_account(&mut self, args: &[&str]) {
        let Some(
            Row::Account { provider, .. }
            | Row::AddHint { provider }
            | Row::Header { provider }
            | Row::Ambient { provider }
            | Row::Plain { provider },
        ) = self.selected_row()
        else {
            return;
        };
        let argv = match self.providers[*provider].argv.clone() {
            Some(a) => a,
            None => return,
        };
        match account_cmd(&argv, args) {
            Ok(msg) => {
                self.changed = true;
                self.status = Some(msg);
            }
            Err(e) => self.status = Some(e),
        }
        self.reload();
    }

    fn toggle_strategy(&mut self) {
        let idx = match self.selected_row() {
            Some(Row::Header { provider })
            | Some(Row::Account { provider, .. })
            | Some(Row::Ambient { provider })
            | Some(Row::AddHint { provider }) => *provider,
            _ => return,
        };
        let p = &self.providers[idx];
        let cur = p
            .doc
            .as_ref()
            .and_then(|d| d["strategy"].as_str())
            .unwrap_or("fill_first");
        let next = if cur == "fill_first" {
            "least_used"
        } else {
            "fill_first"
        };
        if p.argv.is_some() {
            self.run_account(&["account", "strategy", next]);
        } else if let Some(auth_ref) = p
            .doc
            .as_ref()
            .and_then(|d| d["_vault"].as_str())
            .map(str::to_string)
        {
            let store = crate::setup::catalog::auth_store_path().map(CredentialStore::new);
            self.status = match store {
                Ok(s) => match s.set_plugin_strategy(&auth_ref, next) {
                    Ok(()) => {
                        self.changed = true;
                        Some(format!("strategy: {next}"))
                    }
                    Err(e) => Some(e.to_string()),
                },
                Err(e) => Some(e.to_string()),
            };
            self.reload();
        }
    }

    fn do_use(&mut self, clear: bool) {
        enum Pin {
            Cli(String),
            Vault(String, String),
            None,
        }
        let target = match self.selected_row() {
            Some(Row::Account { label, .. }) => Pin::Cli(label.clone()),
            Some(Row::Vault { auth_ref, label }) => Pin::Vault(auth_ref.clone(), label.clone()),
            _ => Pin::None,
        };
        match target {
            Pin::Cli(label) => {
                self.run_account(&["account", "use", if clear { "--clear" } else { &label }]);
            }
            Pin::Vault(auth_ref, label) => {
                let store = crate::setup::catalog::auth_store_path().map(CredentialStore::new);
                self.status = match store {
                    Ok(s) => match s.select_plugin_account(
                        &auth_ref,
                        if clear { None } else { Some(label.as_str()) },
                    ) {
                        Ok(()) => {
                            self.changed = true;
                            Some(if clear {
                                "pin cleared".into()
                            } else {
                                format!("pinned {label}")
                            })
                        }
                        Err(e) => Some(e.to_string()),
                    },
                    Err(e) => Some(e.to_string()),
                };
                self.reload();
            }
            Pin::None => {}
        }
    }

    fn do_remove(&mut self) {
        let target = match self.selected_row() {
            Some(Row::Account { provider, label }) => Some((*provider, label.clone())),
            _ => None,
        };
        let Some((pi, label)) = target else {
            // Vault rows remove through the store.
            if let Some(Row::Vault { auth_ref, label }) = self.selected_row() {
                let (auth_ref, label) = (auth_ref.clone(), label.clone());
                if self.confirming_remove.as_ref() != Some(&(usize::MAX, label.clone())) {
                    self.confirming_remove = Some((usize::MAX, label.clone()));
                    self.status = Some(format!("press d again to remove {label}"));
                    return;
                }
                let store = crate::setup::catalog::auth_store_path().map(CredentialStore::new);
                self.status = match store {
                    Ok(s) => match s.remove_plugin_account(&auth_ref, &label) {
                        Ok(()) => {
                            self.changed = true;
                            Some(format!("removed {label}"))
                        }
                        Err(e) => Some(e.to_string()),
                    },
                    Err(e) => Some(e.to_string()),
                };
                self.confirming_remove = None;
                self.reload();
            }
            return;
        };
        if self.confirming_remove.as_ref() != Some(&(pi, label.clone())) {
            self.confirming_remove = Some((pi, label.clone()));
            self.status = Some(format!("press d again to remove {label}"));
            return;
        }
        self.confirming_remove = None;
        self.run_account(&["account", "remove", &label]);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hand off the terminal for an interactive `account add` (the provider's
/// own login needs the tty) or run an adopt (`--home`) inline.
enum Outcome {
    Close,
    Handoff { argv: Vec<String>, label: String },
}

fn panel_loop(
    terminal: &mut crate::setup::ModalTerminal,
    panel: &mut Panel,
    bg: &crate::setup::BackgroundSnapshot,
) -> anyhow::Result<Outcome> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let theme = crate::theme::theme();
    let box_bg = theme.surface_bg;
    let accent = theme.accent;
    let text_dim = theme.text_dim;

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            if area.width < 30 || area.height < 8 {
                return;
            }
            crate::setup::ui::render_dimmed_background(frame, bg);
            let modal_w = (area.width.saturating_sub(4))
                .clamp(56, 110)
                .min(area.width);
            let rows = panel.rows.len().max(1) as u16;
            let modal_h = (rows + 7).min(area.height.saturating_sub(2)).max(7);
            let x = area.x + (area.width.saturating_sub(modal_w)) / 2;
            let y = area.y + (area.height.saturating_sub(modal_h)) / 2;
            let rect = Rect::new(x, y, modal_w, modal_h);
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Accounts ")
                .style(Style::default().bg(box_bg));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);

            let mut lines: Vec<Line> = Vec::new();
            if panel.rows.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no provider plugins installed — /connect to add one",
                    Style::default().fg(text_dim),
                )));
            }
            for (i, row) in panel.rows.iter().enumerate() {
                let (text, header) = panel.row_text(row);
                let selected = i == panel.sel;
                let style = if selected {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else if header {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let marker = if selected { "› " } else { "  " };
                lines.push(Line::from(Span::styled(format!("{marker}{text}"), style)));
            }
            lines.push(Line::from(""));
            if let Some((_, buf)) = &panel.adding {
                lines.push(Line::from(Span::styled(
                    format!("new account — label [home]: {buf}▌"),
                    Style::default().fg(accent),
                )));
            }
            if let Some(msg) = &panel.status {
                lines.push(Line::from(Span::styled(
                    msg.clone(),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(Span::styled(
                "enter/u pin · c unpin · a add · d remove · s strategy · r refresh · q close",
                Style::default().fg(text_dim),
            )));
            frame.render_widget(Paragraph::new(lines), inner);
        })?;

        if !poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(KeyEvent {
            code,
            kind,
            modifiers,
            ..
        }) = read()?
        else {
            continue;
        };
        if kind == KeyEventKind::Release {
            continue;
        }

        // Label prompt mode: chars extend the buffer, Enter submits, Esc
        // cancels.
        if let Some((pi, buf)) = &mut panel.adding {
            match code {
                KeyCode::Esc => {
                    panel.adding = None;
                    panel.status = None;
                }
                KeyCode::Enter => {
                    let text = buf.trim().to_string();
                    let argv = panel.providers[*pi].argv.clone();
                    panel.adding = None;
                    let mut it = text.split_whitespace();
                    match (it.next(), it.next()) {
                        (Some(label), home) => {
                            if let Some(argv) = argv {
                                if let Some(home) = home {
                                    match account_cmd(
                                        &argv,
                                        &["account", "add", label, "--home", home],
                                    ) {
                                        Ok(msg) => {
                                            panel.changed = true;
                                            panel.status = Some(msg);
                                        }
                                        Err(e) => panel.status = Some(e),
                                    }
                                    panel.reload();
                                } else {
                                    return Ok(Outcome::Handoff {
                                        argv,
                                        label: label.to_string(),
                                    });
                                }
                            }
                        }
                        _ => panel.status = Some("add needs a label".into()),
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            continue;
        }

        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(Outcome::Close),
            KeyCode::Char('c') if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                return Ok(Outcome::Close);
            }
            KeyCode::Up | KeyCode::Char('k') => panel.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => panel.move_sel(1),
            KeyCode::Enter | KeyCode::Char('u') => panel.do_use(false),
            KeyCode::Char('c') => panel.do_use(true),
            KeyCode::Char('d') => panel.do_remove(),
            KeyCode::Char('s') => panel.toggle_strategy(),
            KeyCode::Char('r') => panel.reload(),
            KeyCode::Char('a') => match panel.selected_row() {
                Some(Row::AddHint { provider })
                | Some(Row::Account { provider, .. })
                | Some(Row::Ambient { provider })
                | Some(Row::Header { provider }) => {
                    if panel.providers[*provider].argv.is_some() {
                        panel.adding = Some((*provider, String::new()));
                        panel.status = None;
                    } else {
                        panel.status =
                            Some("no account CLI for this provider — add via /connect".into());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// The terminal handoff for `account add <label>`: the provider's own
/// login writes to the real tty, then the user presses Enter to come
/// back to the panel.
fn run_handoff(argv: &[String], label: &str) -> anyhow::Result<()> {
    let (prog, base) = argv.split_first().unwrap();
    println!(
        "\nrunning {} account add {label} — complete the login, then press Enter\n",
        prog
    );
    let status = std::process::Command::new(prog)
        .args(base)
        .args(["account", "add", label])
        .env(
            "GRAY_BIN",
            crate::exe_watch::on_disk_exe().unwrap_or_else(|_| PathBuf::from("gray")),
        )
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        println!("account add exited {status}");
    }
    println!("\npress Enter to return to gray");
    let mut line = String::new();
    let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line);
    Ok(())
}

pub(crate) fn run_accounts_modal(
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    let mut panel = Panel::load();
    loop {
        let (session, mut terminal) = open_modal()?;
        let outcome = panel_loop(&mut terminal, &mut panel, &bg.cloned().unwrap_or_default());
        drop(terminal);
        drop(session);
        match outcome? {
            Outcome::Close => return Ok(panel.changed),
            Outcome::Handoff { argv, label } => {
                let _ = run_handoff(&argv, &label);
                panel.changed = true;
                panel.reload();
            }
        }
    }
}

/// Non-TTY `/accounts` — the same data the modal renders, as text.
pub(crate) fn format_text() -> String {
    let mut panel = Panel::load();
    panel.rebuild_rows();
    if panel.rows.is_empty() {
        return "no provider plugins installed — /connect to add one".to_string();
    }
    let mut out = String::new();
    for row in &panel.rows {
        let (text, header) = panel.row_text(row);
        if header {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
            out.push('\n');
        } else {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}
