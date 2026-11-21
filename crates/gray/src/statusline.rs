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
    bump();
}

pub fn init_from_saved_config() -> bool {
    let cfg = crate::setup::saved_config_path()
        .ok()
        .and_then(|p| crate::setup::load_saved_config_at(&p).status_line);
    let custom = cfg.is_some();
    set_config(cfg);
    custom
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

pub fn set_status(key: &str, text: &str) {
    let key = key.trim().to_string();
    if key.is_empty() {
        return;
    }
    let changed = match PLUGIN_STATUS.lock() {
        Ok(mut map) => {
            let same = map.get(&key).is_some_and(|t| t == text);
            if !same {
                if text.is_empty() {
                    map.remove(&key);
                } else {
                    map.insert(key, text.to_string());
                }
            }
            !same
        }
        Err(_) => false,
    };
    if changed {
        bump();
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
    gray_core::paths::user_home()
        .and_then(|home| gray_core::paths::tilde(Path::new(cwd), &home))
        .unwrap_or_else(|| cwd.to_string())
}
