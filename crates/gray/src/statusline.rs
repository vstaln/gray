use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

pub const DEFAULT_LEFT: &[&str] = &["model", "effort"];
pub const DEFAULT_RIGHT: &[&str] = &["context", "cache", "timer", "work", "status"];
pub const DEFAULT_SEPARATOR: &str = " ";

pub const DEFAULT_RIGHT_SEPARATOR: &str = "  ";
const DEFAULT_INTERVAL_MS: u64 = 5_000;
const MIN_INTERVAL_MS: u64 = 500;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusLineConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
}

impl StatusLineConfig {
    pub fn left(&self) -> Vec<String> {
        self.left
            .clone()
            .unwrap_or_else(|| DEFAULT_LEFT.iter().map(|s| s.to_string()).collect())
    }
    pub fn right(&self) -> Vec<String> {
        self.right
            .clone()
            .unwrap_or_else(|| DEFAULT_RIGHT.iter().map(|s| s.to_string()).collect())
    }
    pub fn separator(&self) -> String {
        self.separator
            .clone()
            .unwrap_or_else(|| DEFAULT_SEPARATOR.to_string())
    }

    pub fn right_separator(&self) -> String {
        self.separator
            .clone()
            .unwrap_or_else(|| DEFAULT_RIGHT_SEPARATOR.to_string())
    }
    pub fn interval(&self) -> Duration {
        Duration::from_millis(
            self.interval_ms
                .unwrap_or(DEFAULT_INTERVAL_MS)
                .max(MIN_INTERVAL_MS),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub color: Color,

    pub parts: Option<Vec<(String, Color)>>,
}

impl Seg {
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color,
            parts: None,
        }
    }
}

pub fn compose(
    spec: &[String],
    known: &dyn Fn(&str) -> Option<Seg>,
    sep: &str,
    sep_color: Color,
    text_color: Color,
) -> Vec<Seg> {
    let mut parts: Vec<Seg> = Vec::new();
    for item in spec {
        if let Some(seg) = resolve_item(item, known, text_color)
            && !seg.text.trim().is_empty()
        {
            parts.push(seg);
        }
    }
    let mut out = Vec::with_capacity(parts.len() * 2);
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 && !sep.is_empty() {
            out.push(Seg::new(sep, sep_color));
        }
        out.push(p);
    }
    out
}

pub fn clip(segs: Vec<Seg>, max_w: usize) -> Vec<Seg> {
    use crate::text_width::{chars_width, display_width, fit_char_count};
    if segs.iter().map(|s| display_width(&s.text)).sum::<usize>() <= max_w {
        return segs;
    }
    fn cut(text: &str, left: &mut usize) -> String {
        let chars: Vec<char> = text.chars().collect();
        let n = fit_char_count(&chars, *left);
        *left -= chars_width(&chars[..n]);
        chars[..n].iter().collect()
    }
    let mut left = max_w.saturating_sub(1);
    let mut out: Vec<Seg> = Vec::new();
    for mut s in segs {
        if left == 0 {
            break;
        }
        if let Some(parts) = s.parts.take() {
            let parts: Vec<(String, Color)> = parts
                .into_iter()
                .map(|(t, c)| (cut(&t, &mut left), c))
                .filter(|(t, _)| !t.is_empty())
                .collect();
            s.text = parts.iter().map(|(t, _)| t.as_str()).collect();
            s.parts = Some(parts);
        } else {
            s.text = cut(&s.text, &mut left);
        }
        out.push(s);
    }
    let color = out.last().map_or(Color::Reset, |s| s.color);
    out.push(Seg::new("\u{2026}", color));
    out
}

fn resolve_item(item: &str, known: &dyn Fn(&str) -> Option<Seg>, text_color: Color) -> Option<Seg> {
    if item.contains('{') {
        return Some(Seg::new(render_template(item, known), text_color));
    }
    if let Some(seg) = known(item) {
        return Some(seg);
    }
    Some(Seg::new(item, text_color))
}

pub fn render_template(tpl: &str, known: &dyn Fn(&str) -> Option<Seg>) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('{') {
            out.push('{');
            rest = stripped;
            continue;
        }
        match after.find('}') {
            Some(j) => {
                let name = after[..j].trim();
                if let Some(seg) = known(name) {
                    out.push_str(&seg.text);
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

static CONFIG: RwLock<Option<StatusLineConfig>> = RwLock::new(None);
static PLUGIN_STATUS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
static COMMAND_OUT: Mutex<String> = Mutex::new(String::new());
static FACTS: Mutex<Option<serde_json::Value>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

fn bump() {
    VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

pub fn config() -> StatusLineConfig {
    CONFIG
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

pub fn set_config(cfg: Option<StatusLineConfig>) {
    if let Ok(mut g) = CONFIG.write() {
        *g = cfg;
    }
    if let Ok(mut o) = COMMAND_OUT.lock() {
        o.clear();
    }
    bump();
}

pub fn init_from_saved_config() -> bool {
    let cfg = crate::setup::saved_config_path()
        .ok()
        .and_then(|p| crate::setup::load_saved_config_at(&p).status_line);
    let custom = cfg.is_some();
    let wants_runner = cfg
        .as_ref()
        .and_then(|c| c.command.as_deref())
        .is_some_and(|c| !c.trim().is_empty());
    set_config(cfg);
    if wants_runner {
        spawn_command_runner();
    }
    custom
}

pub fn set_status(key: &str, text: Option<&str>) {
    let Ok(mut m) = PLUGIN_STATUS.lock() else {
        return;
    };
    let line = text.map(one_line).filter(|t| !t.is_empty());
    let changed = match line {
        Some(t) => m.insert(key.to_string(), t.clone()) != Some(t),
        None => m.remove(key).is_some(),
    };
    if changed {
        bump();
    }
}

pub fn status(key: &str) -> Option<String> {
    PLUGIN_STATUS.lock().ok()?.get(key).cloned()
}

pub fn all_statuses(sep: &str) -> String {
    PLUGIN_STATUS
        .lock()
        .map(|m| m.values().cloned().collect::<Vec<_>>().join(sep))
        .unwrap_or_default()
}

pub fn command_output() -> String {
    COMMAND_OUT.lock().map(|o| o.clone()).unwrap_or_default()
}

pub fn publish_facts(facts: serde_json::Value) {
    if let Ok(mut f) = FACTS.lock() {
        *f = Some(facts);
    }
}

fn one_line(s: &str) -> String {
    let s = crate::tui::strip_ansi(s);
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn spawn_command_runner() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("gray-statusline".into())
            .spawn(|| {
                loop {
                    let cfg = config();
                    let Some(cmd) = cfg.command.clone().filter(|c| !c.trim().is_empty()) else {
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    };
                    let facts = FACTS
                        .lock()
                        .ok()
                        .and_then(|f| f.clone())
                        .unwrap_or(serde_json::Value::Null);
                    let out = run_command(&cmd, &facts).unwrap_or_default();
                    let line = one_line(&out);
                    if let Ok(mut o) = COMMAND_OUT.lock()
                        && *o != line
                    {
                        *o = line;
                        bump();
                    }
                    std::thread::sleep(cfg.interval());
                }
            });
    });
}

pub fn run_command(cmd: &str, facts: &serde_json::Value) -> Option<String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let cmd = expand_tilde(cmd);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(facts.to_string().as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

fn expand_tilde(cmd: &str) -> String {
    match (cmd.strip_prefix("~/"), gray_core::paths::user_home()) {
        (Some(rest), Some(home)) => format!("{}/{rest}", home.display()),
        _ => cmd.to_string(),
    }
}

type BranchCache = Option<(PathBuf, Instant, String)>;
static BRANCH: Mutex<BranchCache> = Mutex::new(None);

pub fn git_branch(cwd: &Path) -> String {
    const TTL: Duration = Duration::from_secs(2);
    if let Ok(g) = BRANCH.lock()
        && let Some((dir, at, b)) = g.as_ref()
        && dir == cwd
        && at.elapsed() < TTL
    {
        return b.clone();
    }
    let b = read_branch(cwd).unwrap_or_default();
    if let Ok(mut g) = BRANCH.lock() {
        *g = Some((cwd.to_path_buf(), Instant::now(), b.clone()));
    }
    b
}

pub fn read_branch(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let dotgit = d.join(".git");
        let git_dir = if dotgit.is_dir() {
            Some(dotgit)
        } else if dotgit.is_file() {
            let body = std::fs::read_to_string(&dotgit).ok()?;
            let p = body.trim().strip_prefix("gitdir:")?.trim().to_string();
            let p = PathBuf::from(p);
            Some(if p.is_absolute() { p } else { d.join(p) })
        } else {
            None
        };
        if let Some(gd) = git_dir {
            let head = std::fs::read_to_string(gd.join("HEAD")).ok()?;
            let head = head.trim();
            return Some(match head.strip_prefix("ref: refs/heads/") {
                Some(b) => b.to_string(),
                None => head.chars().take(7).collect(),
            });
        }
        dir = d.parent();
    }
    None
}

pub fn short_cwd(cwd: &str) -> String {
    if let Some(home) = gray_core::paths::user_home() {
        let home = home.display().to_string();
        if let Some(rest) = cwd.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    cwd.to_string()
}
