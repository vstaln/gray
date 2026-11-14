use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::providers::InstalledProvider;
use crate::text_width::fit_chars;

use super::catalog::{AuthEntry, PluginAuthPool};

const LIST_TIMEOUT: Duration = Duration::from_secs(4);
const USAGE_TIMEOUT: Duration = Duration::from_secs(12);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RED: Color = Color::Rgb(232, 112, 112);
const CELLS: usize = 10;

pub(crate) fn cli_argvs() -> HashMap<String, Vec<String>> {
    let Ok(home) = super::catalog::gray_home() else {
        return HashMap::new();
    };
    let Ok(lock) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(&home)) else {
        return HashMap::new();
    };
    lock.plugins
        .into_iter()
        .filter(|(_, e)| e.enabled)
        .filter_map(|(name, e)| {
            let argv = e
                .cli_argv
                .filter(|a| !a.is_empty())
                .or_else(|| Some(e.argv).filter(|a| !a.is_empty()))?;
            Some((name, argv))
        })
        .collect()
}

pub(crate) fn gray_bin() -> std::ffi::OsString {
    std::env::current_exe()
        .map(Into::into)
        .unwrap_or_else(|_| "gray".into())
}

fn first_err_line(stderr: &[u8], fallback: &str) -> String {
    let err = String::from_utf8_lossy(stderr);
    err.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("error: ").to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn run(argv: &[String], args: &[&str], timeout: Duration) -> Result<String, String> {
    let (prog, base) = argv.split_first().ok_or("empty argv")?;
    let mut child = std::process::Command::new(prog)
        .args(base)
        .args(args)
        .env("GRAY_BIN", gray_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !out.status.success() {
        return Err(first_err_line(&out.stderr, "command failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_json(argv: &[String], args: &[&str], timeout: Duration) -> Result<Value, String> {
    let out = run(argv, args, timeout)?;
    serde_json::from_str(&out).map_err(|e| format!("bad account json: {e}"))
}

pub(crate) struct UsageEntry {
    label: Option<String>,
    card: Option<Value>,
    error: Option<String>,
}

fn fetch_usage(argv: &[String]) -> Result<Vec<UsageEntry>, String> {
    let doc = run_json(argv, &["account", "usage", "--json"], USAGE_TIMEOUT)?;
    let list = doc["accounts"]
        .as_array()
        .ok_or("usage reply has no accounts list")?;
    Ok(list
        .iter()
        .map(|a| UsageEntry {
            label: a["label"].as_str().map(str::to_string),
            card: a["usage"].is_object().then(|| a["usage"].clone()),
            error: a["error"].as_str().map(str::to_string),
        })
        .collect())
}

fn auth_error(e: &str) -> bool {
    let e = e.to_lowercase();
    ["401", "403", "unauthorized", "expired", "login", "auth"]
        .iter()
        .any(|w| e.contains(w))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn iso_epoch(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once(['T', ' '])?;
    let mut ymd = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let cut = rest.find(['Z', '+', '-', '.']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(cut);
    let mut hms = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (hh, mi) = (hms.next()??, hms.next()??);
    let ss = hms.next().unwrap_or(Some(0))?;
    let zone = zone.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone.as_bytes().first().copied() {
        None | Some(b'Z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let (oh, om) = zone[1..].split_once(':')?;
            let secs = oh.parse::<i64>().ok()? * 3_600 + om.parse::<i64>().ok()? * 60;
            if sign == b'+' { secs } else { -secs }
        }
        _ => return None,
    };
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3_600 + mi * 60 + ss - offset)
}

fn countdown(resets_at: &str, now: i64) -> Option<String> {
    let left = iso_epoch(resets_at)? - now;
    if left <= 0 {
        return None;
    }
    Some(if left >= 86_400 {
        format!("{}d", left / 86_400)
    } else if left >= 3_600 {
        format!("{}h{:02}", left / 3_600, left % 3_600 / 60)
    } else {
        format!("{}m", left / 60)
    })
}

struct Gauge {
    label: String,
    pct: f64,
    reset: Option<String>,
}

fn gauge(w: &Value, now: i64) -> Option<Gauge> {
    Some(Gauge {
        label: w["label"]
            .as_str()
            .or_else(|| w["id"].as_str())?
            .to_string(),
        pct: w["used_percent"].as_f64()?,
        reset: w["resets_at"].as_str().and_then(|at| countdown(at, now)),
    })
}

fn gauges(card: &Value) -> Vec<Gauge> {
    let now = now_secs();
    card["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| gauge(w, now))
        .collect()
}

pub(crate) fn pct_color(pct: f64) -> Color {
    let t = crate::theme::theme();
    if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        t.accent
    } else {
        t.text_muted
    }
}

enum Msg {
    List(Result<Value, String>),
    Usage(Result<Vec<UsageEntry>, String>),
}

struct PluginState {
    argv: Vec<String>,
    generation: u64,
    list: Option<Result<Value, String>>,
    usage: Option<Result<Vec<UsageEntry>, String>>,
    usage_busy: bool,
}

pub(crate) enum Tag {
    Pct(f64),
    Expired,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Source {
    Cli(String),
    Vault(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Login(Option<String>),
    Add,
}

pub(crate) struct Strip {
    pub(crate) source: Source,
    pub(crate) items: Vec<Item>,
}

impl Strip {
    pub(crate) fn plugin(&self) -> Option<&str> {
        match &self.source {
            Source::Cli(p) => Some(p),
            Source::Vault(_) => None,
        }
    }
}

enum LoginUsage<'a> {
    Pending,
    Card(&'a Value),
    Failed(&'a str),
    Unknown,
}

pub(crate) struct Logins {
    plugins: HashMap<String, PluginState>,
    vault: BTreeMap<String, PluginAuthPool>,
    tx: Sender<(String, u64, Msg)>,
    rx: Receiver<(String, u64, Msg)>,
}

impl Logins {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            plugins: HashMap::new(),
            vault: BTreeMap::new(),
            tx,
            rx,
        }
    }

    pub(crate) fn start<'a>(&mut self, installed: impl IntoIterator<Item = &'a InstalledProvider>) {
        let argvs = cli_argvs();
        let mut fresh = Vec::new();
        for i in installed {
            if self.plugins.contains_key(&i.plugin) {
                continue;
            }
            let Some(argv) = argvs.get(&i.plugin) else {
                continue;
            };
            self.plugins.insert(
                i.plugin.clone(),
                PluginState {
                    argv: argv.clone(),
                    generation: 0,
                    list: None,
                    usage: None,
                    usage_busy: false,
                },
            );
            fresh.push(i.plugin.clone());
        }
        for plugin in fresh {
            self.refresh(&plugin);
        }
        self.load_vault();
    }

    fn load_vault(&mut self) {
        let Ok(home) = super::catalog::gray_home() else {
            return;
        };
        let store = crate::auth::CredentialStore::new(home.join("auth.json"));
        self.vault = store
            .load()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| match v {
                AuthEntry::PluginPool(pool) => Some((k, pool)),
                _ => None,
            })
            .collect();
    }

    pub(crate) fn refresh(&mut self, plugin: &str) {
        let Some(st) = self.plugins.get_mut(plugin) else {
            return;
        };
        st.generation += 1;
        st.usage_busy = true;
        let (tx, generation, argv, plugin) = (
            self.tx.clone(),
            st.generation,
            st.argv.clone(),
            plugin.to_string(),
        );
        std::thread::spawn(move || {
            let list = run_json(&argv, &["account", "list", "--json"], LIST_TIMEOUT);
            let ok = list.is_ok();
            let _ = tx.send((plugin.clone(), generation, Msg::List(list)));
            if ok {
                let _ = tx.send((plugin, generation, Msg::Usage(fetch_usage(&argv))));
            }
        });
    }

    pub(crate) fn pump(&mut self) {
        while let Ok((plugin, generation, msg)) = self.rx.try_recv() {
            let Some(st) = self.plugins.get_mut(&plugin) else {
                continue;
            };
            if st.generation != generation {
                continue;
            }
            match msg {
                Msg::List(list) => {
                    if list.is_err() {
                        st.usage_busy = false;
                    }
                    st.list = Some(list);
                }
                Msg::Usage(usage) => {
                    st.usage_busy = false;
                    st.usage = Some(usage);
                }
            }
        }
    }

    fn doc(&self, plugin: &str) -> Option<&Value> {
        self.plugins.get(plugin)?.list.as_ref()?.as_ref().ok()
    }

    pub(crate) fn strip(&self, installed: &InstalledProvider) -> Option<Strip> {
        if let Some(doc) = self.doc(&installed.plugin) {
            let mut items: Vec<Item> = doc["accounts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["label"].as_str())
                .map(|l| Item::Login(Some(l.to_string())))
                .collect();
            if items.is_empty() && doc["ambient"] == true {
                items.push(Item::Login(None));
            }
            items.push(Item::Add);
            return Some(Strip {
                source: Source::Cli(installed.plugin.clone()),
                items,
            });
        }
        let pending = self
            .plugins
            .get(&installed.plugin)
            .is_some_and(|s| s.list.is_none());
        let auth_ref = installed.auth_ref();
        let pool = self.vault.get(&auth_ref).filter(|_| !pending)?;
        Some(Strip {
            items: pool
                .accounts
                .iter()
                .map(|a| Item::Login(Some(a.label.clone())))
                .collect(),
            source: Source::Vault(auth_ref),
        })
    }

    fn serving(doc: &Value) -> Option<String> {
        match doc["serving"]["kind"].as_str() {
            Some("account") => return doc["serving"]["label"].as_str().map(str::to_string),
            Some(_) => return None,
            None => {}
        }
        let accounts = doc["accounts"].as_array()?;
        accounts
            .iter()
            .find(|a| a["pinned"] == true)
            .or_else(|| accounts.first())
            .and_then(|a| a["label"].as_str())
            .map(str::to_string)
    }

    fn login_usage(&self, plugin: &str, label: Option<&str>) -> LoginUsage<'_> {
        let Some(st) = self.plugins.get(plugin) else {
            return LoginUsage::Unknown;
        };
        match &st.usage {
            None if st.usage_busy => LoginUsage::Pending,
            None => LoginUsage::Unknown,
            Some(Err(e)) => LoginUsage::Failed(e),
            Some(Ok(entries)) => match entries.iter().find(|e| e.label.as_deref() == label) {
                Some(UsageEntry {
                    card: Some(card), ..
                }) => LoginUsage::Card(card),
                Some(UsageEntry { error: Some(e), .. }) => LoginUsage::Failed(e),
                _ => LoginUsage::Unknown,
            },
        }
    }

    pub(crate) fn rail_tag(&self, installed: &InstalledProvider) -> Option<Tag> {
        let doc = self.doc(&installed.plugin)?;
        let label = Self::serving(doc);
        if label.is_none() && doc["ambient"] != true {
            return None;
        }
        match self.login_usage(&installed.plugin, label.as_deref()) {
            LoginUsage::Card(card) => gauges(card)
                .iter()
                .map(|g| g.pct)
                .reduce(f64::max)
                .map(Tag::Pct),
            LoginUsage::Failed(e) if auth_error(e) => Some(Tag::Expired),
            _ => None,
        }
    }

    fn strategy(&self, source: &Source) -> String {
        let s = match source {
            Source::Cli(p) => self
                .doc(p)
                .and_then(|d| d["strategy"].as_str())
                .unwrap_or("")
                .to_string(),
            Source::Vault(r) => self
                .vault
                .get(r)
                .map(|p| p.strategy.clone())
                .unwrap_or_default(),
        };
        if s.is_empty() {
            "fill_first".to_string()
        } else {
            s
        }
    }

    pub(crate) fn signed_out(&self, plugin: &str, label: &str) -> bool {
        matches!(self.login_usage(plugin, Some(label)), LoginUsage::Failed(e) if e.contains("signed out"))
    }

    pub(crate) fn has_cli(&self, plugin: &str) -> Option<Vec<String>> {
        self.plugins.get(plugin).map(|s| s.argv.clone())
    }

    pub(crate) fn lines(
        &self,
        name: &str,
        strip: &Strip,
        sel: Option<usize>,
        adding: Option<&str>,
        w: usize,
    ) -> Vec<Line<'static>> {
        let t = crate::theme::theme();
        let fg = |c: Color| Style::default().fg(c);
        let logins = strip
            .items
            .iter()
            .filter(|i| matches!(i, Item::Login(_)))
            .count();
        let mut meta = String::from("  ·  ");
        if logins > 1 {
            meta.push_str(&format!("{} · ", self.strategy(&strip.source)));
        }
        meta.push_str(&format!(
            "{logins} login{}",
            if logins == 1 { "" } else { "s" }
        ));
        let mut out = vec![Line::from(vec![
            Span::raw(" "),
            Span::styled(
                fit_chars(name, w.saturating_sub(meta.chars().count() + 1)),
                fg(t.text_bright).add_modifier(Modifier::BOLD),
            ),
            Span::styled(meta, fg(t.text_dim)),
        ])];
        for (idx, item) in strip.items.iter().enumerate() {
            let lit = sel == Some(idx);
            let bar = if lit {
                Style::default().bg(t.accent)
            } else {
                Style::default()
            };
            let pick = |c: Color| if lit { fg(t.on_selection) } else { fg(c) };
            match item {
                Item::Add => {
                    let line = match adding {
                        Some(buf) => Line::from(vec![
                            Span::styled("   label  ", pick(t.text_dim)),
                            Span::styled(format!("{buf}▌"), pick(t.text_bright)),
                        ]),
                        None => Line::from(Span::styled("   + add login", pick(t.accent))),
                    };
                    out.push(line.style(bar));
                }
                Item::Login(label) => {
                    out.extend(self.login_block(strip, label.as_deref(), lit, w));
                }
            }
        }
        out
    }

    fn login_block(
        &self,
        strip: &Strip,
        label: Option<&str>,
        lit: bool,
        w: usize,
    ) -> Vec<Line<'static>> {
        let t = crate::theme::theme();
        let fg = |c: Color| Style::default().fg(c);
        let pick = |c: Color| if lit { fg(t.on_selection) } else { fg(c) };
        let bar = if lit {
            Style::default().bg(t.accent)
        } else {
            Style::default()
        };
        let (pinned, email, org, usage) = match &strip.source {
            Source::Cli(plugin) => {
                let acct = label.and_then(|l| {
                    self.doc(plugin)?["accounts"]
                        .as_array()?
                        .iter()
                        .find(|a| a["label"].as_str() == Some(l))
                        .cloned()
                });
                let usage = self.login_usage(plugin, label);
                let card_email = match &usage {
                    LoginUsage::Card(c) => c["email"].as_str().map(str::to_string),
                    _ => None,
                };
                let a = acct.unwrap_or(Value::Null);
                (
                    a["pinned"] == true,
                    a["email"].as_str().map(str::to_string).or(card_email),
                    a["org"].as_str().map(str::to_string),
                    Some(usage),
                )
            }
            Source::Vault(r) => {
                let pool = self.vault.get(r);
                let acct =
                    pool.and_then(|p| p.accounts.iter().find(|a| Some(a.label.as_str()) == label));
                (
                    pool.is_some_and(|p| p.selected.as_deref() == label && label.is_some()),
                    acct.and_then(|a| a.email.clone()),
                    None,
                    None,
                )
            }
        };
        let mut head = vec![Span::styled(
            if pinned { " ● " } else { "   " },
            pick(t.accent),
        )];
        let mut used = 3;
        match label.filter(|l| !l.is_empty()) {
            Some(l) => {
                let l = fit_chars(l, 16);
                used += l.chars().count() + 2;
                head.push(Span::styled(
                    l,
                    pick(t.text_body).add_modifier(Modifier::BOLD),
                ));
                head.push(Span::raw("  "));
            }
            None if label.is_none() => {
                let who = email.clone().unwrap_or_else(|| "signed in".to_string());
                head.push(Span::styled(
                    fit_chars(&who, w.saturating_sub(used)),
                    pick(t.text_body),
                ));
                used = w;
            }
            None => {}
        }
        if used < w {
            let who = match (&email, &org) {
                (Some(e), Some(o)) => format!("{e} · {o}"),
                (Some(e), None) => e.clone(),
                (None, Some(o)) => o.clone(),
                (None, None) => String::new(),
            };
            head.push(Span::styled(
                fit_chars(&who, w.saturating_sub(used)),
                pick(t.text_dim),
            ));
        }
        let mut out = vec![Line::from(head).style(bar)];
        let dim = |s: &str, c: Color| Line::from(Span::styled(format!("     {s}"), fg(c)));
        match usage {
            Some(LoginUsage::Pending) => out.push(dim("…", t.text_faint)),
            Some(LoginUsage::Failed(e)) if e.contains("signed out") => {
                out.push(dim("signed out · enter to log in", RED))
            }
            Some(LoginUsage::Failed(e)) if auth_error(e) => out.push(dim("login expired", RED)),
            Some(LoginUsage::Failed(_)) => out.push(dim("usage unavailable", t.text_faint)),
            Some(LoginUsage::Card(card)) => out.extend(gauge_lines(&gauges(card), w)),
            Some(LoginUsage::Unknown) | None => {}
        }
        out
    }

    pub(crate) fn act(&mut self, strip: &Strip, verb: Verb<'_>) -> String {
        match &strip.source {
            Source::Cli(plugin) => {
                let Some(argv) = self.has_cli(plugin) else {
                    return String::new();
                };
                let next;
                let args: Vec<&str> = match verb {
                    Verb::Pin(l) => vec!["account", "use", l],
                    Verb::Unpin => vec!["account", "use", "--clear"],
                    Verb::Remove(l) => vec!["account", "remove", l],
                    Verb::Strategy => {
                        next = flip(&self.strategy(&strip.source));
                        vec!["account", "strategy", next]
                    }
                };
                let msg = run(&argv, &args, CMD_TIMEOUT).unwrap_or_else(|e| e);
                self.refresh(plugin);
                msg.lines().next().unwrap_or_default().to_string()
            }
            Source::Vault(auth_ref) => {
                let next = flip(&self.strategy(&strip.source));
                let res = vault_mutate(auth_ref, |pool| {
                    match verb {
                        Verb::Pin(l) => {
                            anyhow::ensure!(
                                pool.accounts.iter().any(|a| a.label == l),
                                "no login {l:?}"
                            );
                            pool.selected = Some(l.to_string());
                        }
                        Verb::Unpin => pool.selected = None,
                        Verb::Remove(l) => {
                            pool.accounts.retain(|a| a.label != l);
                            if pool.selected.as_deref() == Some(l) {
                                pool.selected = None;
                            }
                        }
                        Verb::Strategy => pool.strategy = next.to_string(),
                    }
                    Ok(())
                });
                self.load_vault();
                match (res, verb) {
                    (Err(e), _) => e.to_string(),
                    (Ok(()), Verb::Pin(l)) => format!("pinned {l}"),
                    (Ok(()), Verb::Unpin) => "pin cleared".to_string(),
                    (Ok(()), Verb::Remove(l)) => format!("removed {l}"),
                    (Ok(()), Verb::Strategy) => format!("strategy: {next}"),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Verb<'a> {
    Pin(&'a str),
    Unpin,
    Remove(&'a str),
    Strategy,
}

fn flip(cur: &str) -> &'static str {
    if cur == "fill_first" {
        "least_used"
    } else {
        "fill_first"
    }
}

fn vault_mutate(
    auth_ref: &str,
    f: impl FnOnce(&mut PluginAuthPool) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let home = super::catalog::gray_home()?;
    let store = crate::auth::CredentialStore::new(home.join("auth.json"));
    let _lock = store.lock()?;
    let mut map = store.load()?;
    let Some(AuthEntry::PluginPool(pool)) = map.get_mut(auth_ref) else {
        anyhow::bail!("no credential pool for {auth_ref}");
    };
    f(pool)?;
    store.replace_locked(&map)
}

fn gauge_lines(gs: &[Gauge], w: usize) -> Vec<Line<'static>> {
    let t = crate::theme::theme();
    let fg = |c: Color| Style::default().fg(c);
    let fixed = 5 + 2 + CELLS + 2 + 4;
    let reset_w = 2 + 10 + 5;
    let max_label = gs
        .iter()
        .map(|g| g.label.chars().count())
        .max()
        .unwrap_or(0);
    let with_reset = w >= fixed + reset_w + 6;
    let lw = max_label.min(w.saturating_sub(fixed + if with_reset { reset_w } else { 0 }));
    gs.iter()
        .map(|g| {
            let filled =
                ((g.pct.clamp(0.0, 100.0) * CELLS as f64 / 100.0).round() as usize).min(CELLS);
            let color = pct_color(g.pct);
            let label = fit_chars(&g.label, lw);
            let pad = lw.saturating_sub(label.chars().count());
            let mut spans = vec![
                Span::styled(format!("     {label}{}  ", " ".repeat(pad)), fg(t.text_dim)),
                Span::styled("━".repeat(filled), fg(color)),
                Span::styled("─".repeat(CELLS - filled), fg(t.text_faint)),
                Span::styled(format!("  {:>3.0}%", g.pct), fg(color)),
            ];
            if with_reset && let Some(c) = &g.reset {
                let left = 5 + lw + 2 + CELLS + 2 + 4;
                let gap = w.saturating_sub(left + 15);
                spans.push(Span::styled(
                    format!("{}resets in {c:>5}", " ".repeat(gap)),
                    fg(t.text_faint),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

pub(crate) fn account_argv() -> Option<Vec<String>> {
    cli_argvs().remove("account")
}

pub(crate) fn portal_cmd(args: &[&str]) -> Result<String, String> {
    let argv = account_argv().ok_or("the account plugin is not installed")?;
    run(&argv, args, CMD_TIMEOUT)
}
