use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::setup::SavedConfig;

static CACHE: Mutex<Option<(Option<SystemTime>, SavedConfig)>> = Mutex::new(None);

fn saved() -> SavedConfig {
    let Ok(path) = crate::setup::saved_config_path() else {
        return SavedConfig::default();
    };
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s)) = cache.as_ref()
        && *at == mtime
        && mtime.is_some()
    {
        return s.clone();
    }
    let s = crate::setup::load_saved_config_at(&path);
    *cache = Some((mtime, s.clone()));
    s
}

fn update(f: impl FnOnce(&mut SavedConfig)) {
    let Ok(path) = crate::setup::saved_config_path() else {
        return;
    };
    let _lock = crate::setup::lock_saved_config_at(&path).ok();
    let mut s = crate::setup::load_saved_config_at(&path);
    f(&mut s);
    let _ = crate::setup::save_saved_config_at(&path, &s);
}

fn flag(field: BoolField, default: bool) -> bool {
    field(&mut saved()).unwrap_or(default)
}

fn number(field: NumField) -> Option<u32> {
    field(&mut saved()).filter(|n| *n > 0)
}

pub(crate) fn auto_continue() -> bool {
    flag(|s| &mut s.auto_continue, true)
}

pub(crate) fn away_recap() -> bool {
    flag(|s| &mut s.away_recap, true)
}

pub(crate) fn cleanup_period_days() -> Option<u32> {
    number(|s| &mut s.cleanup_period_days)
}

pub(crate) fn notify_on_finish() -> bool {
    flag(|s| &mut s.notify_on_finish, true)
}

pub(crate) fn terminal_title() -> bool {
    flag(|s| &mut s.terminal_title, true)
}

pub(crate) fn terminal_progress() -> bool {
    flag(|s| &mut s.terminal_progress, true)
}

pub(crate) fn issue_links() -> bool {
    flag(|s| &mut s.issue_links, true)
}

pub(crate) fn vim_mode() -> bool {
    flag(|s| &mut s.vim_mode, false)
}

pub(crate) fn prompt_suggestions() -> bool {
    flag(|s| &mut s.prompt_suggestions, false)
}

pub(crate) fn commit_attribution() -> bool {
    flag(|s| &mut s.commit_attribution, false)
}

pub(crate) fn break_reminder_minutes() -> Option<u32> {
    number(|s| &mut s.break_reminder_minutes)
}

static TURN_GEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn turn_started() {
    TURN_GEN.fetch_add(1, Ordering::Relaxed);
}

const CONTINUE_AFTER_LIMIT: &str =
    "Your usage limit has reset. Continue the task where you left off.";

const CONTINUE_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoContinue {
    Off,
    Armed { fire_at: Instant },
}

static AUTO_CONTINUE: Mutex<AutoContinue> = Mutex::new(AutoContinue::Off);

fn auto_continue_slot() -> std::sync::MutexGuard<'static, AutoContinue> {
    AUTO_CONTINUE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn arm_auto_continue(session: &str, wait: Duration) -> String {
    let gen_at_arm = TURN_GEN.load(Ordering::Relaxed);
    let session = session.to_string();
    let wait = wait + CONTINUE_GRACE;
    let fire_at = Instant::now() + wait;
    *auto_continue_slot() = AutoContinue::Armed { fire_at };
    tokio::spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(fire_at)).await;
        {
            let mut slot = auto_continue_slot();
            match *slot {
                AutoContinue::Armed { fire_at: armed } if armed == fire_at => {
                    *slot = AutoContinue::Off;
                }
                _ => return,
            }
        }
        if TURN_GEN.load(Ordering::Relaxed) != gen_at_arm {
            return;
        }
        let Ok(home) = crate::setup::gray_home() else {
            return;
        };
        let Some(dir) = crate::session_mailbox::session_inbox(&home, &session) else {
            return;
        };
        let body = serde_json::json!({
            "card": "usage limit reset · continuing",
            "prompt": CONTINUE_AFTER_LIMIT,
        })
        .to_string();
        if crate::session_mailbox::write_entry(&dir, &body).is_ok() {
            crate::host::request_wake();
        }
    });
    let at = chrono::Local::now() + chrono::Duration::from_std(wait).unwrap_or_default();
    format!(
        "\u{b7} resuming at {} (in {}) \u{b7} esc to cancel",
        at.format("%H:%M"),
        span(wait.as_secs())
    )
}

fn span(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, secs % 3600 / 60)
    } else {
        format!("{}m", secs.div_ceil(60))
    }
}

pub(crate) fn auto_continue_secs() -> Option<u64> {
    match *auto_continue_slot() {
        AutoContinue::Off => None,
        AutoContinue::Armed { fire_at } => {
            Some(fire_at.saturating_duration_since(Instant::now()).as_secs())
        }
    }
}

pub(crate) fn auto_continue_status() -> Option<String> {
    let secs = auto_continue_secs()?;
    let left = if secs >= 3600 {
        let (m, s) = (secs % 3600 / 60, secs % 60);
        format!("{}h {m:02}:{s:02}", secs / 3600)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    };
    Some(format!("resuming in {left} \u{b7} esc to cancel"))
}

pub(crate) fn cancel_auto_continue() {
    let mut slot = auto_continue_slot();
    if matches!(*slot, AutoContinue::Armed { .. }) {
        *slot = AutoContinue::Off;
    }
}

const PRUNE_STAMP: &str = ".last-prune";

pub(crate) fn spawn_session_cleanup() {
    let Some(days) = cleanup_period_days() else {
        return;
    };
    let Some(root) = crate::session_store::default_root() else {
        return;
    };
    let stamp = root.join(PRUNE_STAMP);
    let fresh = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(86_400));
    if fresh {
        return;
    }
    tokio::spawn(async move {
        let _ = std::fs::write(&stamp, b"");
        let store = crate::session_store::JsonlSessionStore::new(root);
        let cutoff_ms = chrono::Utc::now()
            .timestamp_millis()
            .saturating_sub(i64::from(days) * 86_400_000)
            .max(0) as u64;
        match store.prune_before(cutoff_ms).await {
            Ok(removed) => log::info!(
                "session cleanup: removed {} older than {days}d",
                removed.len()
            ),
            Err(e) => log::warn!("session cleanup: {e}"),
        }
    });
}

type BoolField = fn(&mut SavedConfig) -> &mut Option<bool>;
type NumField = fn(&mut SavedConfig) -> &mut Option<u32>;

enum Kind {
    Bool(BoolField, bool),
    Steps(NumField, &'static [u32], &'static str, &'static str),
}

struct CoreRow {
    label: &'static str,
    kind: Kind,
}

const CORE: &[CoreRow] = &[
    CoreRow {
        label: "Continue after usage limit resets",
        kind: Kind::Bool(|s| &mut s.auto_continue, false),
    },
    CoreRow {
        label: "Recap when you come back after 5+ min",
        kind: Kind::Bool(|s| &mut s.away_recap, true),
    },
    CoreRow {
        label: "Notify when a turn finishes while you're away",
        kind: Kind::Bool(|s| &mut s.notify_on_finish, true),
    },
    CoreRow {
        label: "Show status in the terminal title",
        kind: Kind::Bool(|s| &mut s.terminal_title, true),
    },
    CoreRow {
        label: "Progress bar in the terminal tab",
        kind: Kind::Bool(|s| &mut s.terminal_progress, true),
    },
    CoreRow {
        label: "Link #123 to the repo's issues and PRs",
        kind: Kind::Bool(|s| &mut s.issue_links, true),
    },
    CoreRow {
        label: "Vim keys in the prompt",
        kind: Kind::Bool(|s| &mut s.vim_mode, false),
    },
    CoreRow {
        label: "Suggest the next prompt",
        kind: Kind::Bool(|s| &mut s.prompt_suggestions, false),
    },
    CoreRow {
        label: "Add a Co-Authored-By line to commits",
        kind: Kind::Bool(|s| &mut s.commit_attribution, false),
    },
    CoreRow {
        label: "Break reminder after",
        kind: Kind::Steps(
            |s| &mut s.break_reminder_minutes,
            &[30, 60, 90, 120],
            "never",
            "min",
        ),
    },
    CoreRow {
        label: "Delete sessions idle longer than",
        kind: Kind::Steps(
            |s| &mut s.cleanup_period_days,
            &[7, 30, 90, 365],
            "never",
            "days",
        ),
    },
    CoreRow {
        label: "Auto-update (next start)",
        kind: Kind::Bool(|s| &mut s.auto_update, true),
    },
    CoreRow {
        label: "Automatic memory (next start)",
        kind: Kind::Bool(|s| &mut s.memory_auto, true),
    },
    CoreRow {
        label: "Background compaction (next start)",
        kind: Kind::Bool(|s| &mut s.background_compaction, true),
    },
    CoreRow {
        label: "Play the intro",
        kind: Kind::Bool(|s| &mut s.intro, true),
    },
];

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.to_string()
}

fn core_value(kind: &Kind, s: &mut SavedConfig) -> String {
    match kind {
        Kind::Bool(f, d) => on_off(f(s).unwrap_or(*d)),
        Kind::Steps(f, _, zero, unit) => match f(s).filter(|n| *n > 0) {
            Some(n) => format!("{n} {unit}"),
            None => zero.to_string(),
        },
    }
}

fn core_step(kind: &Kind, s: &mut SavedConfig) {
    match kind {
        Kind::Bool(f, d) => {
            let slot = f(s);
            *slot = Some(!slot.unwrap_or(*d));
        }
        Kind::Steps(f, steps, _, _) => {
            let slot = f(s);
            let cur = slot.filter(|n| *n > 0);
            *slot = match cur.and_then(|c| steps.iter().position(|v| *v == c)) {
                None => steps.first().copied(),
                Some(i) => steps.get(i + 1).copied(),
            };
        }
    }
}

struct PluginRow {
    plugin: String,
    title: String,
    key: String,
    label: String,
    choices: Vec<Value>,
    default: Value,
}

pub(crate) fn plugin_settings_path(home: &std::path::Path, plugin: &str) -> std::path::PathBuf {
    home.join("plugin-settings").join(format!("{plugin}.json"))
}

fn plugin_values(home: &std::path::Path, plugin: &str) -> serde_json::Map<String, Value> {
    std::fs::read(plugin_settings_path(home, plugin))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn plugin_rows(home: &std::path::Path, cwd: &std::path::Path) -> Vec<PluginRow> {
    use gray_plugin::lock::{LockFile, effective_enabled, lock_path, project_lock_path};
    let user = LockFile::load(&lock_path(home)).unwrap_or_default();
    let project = LockFile::load(&project_lock_path(cwd)).unwrap_or_default();
    let mut rows = Vec::new();
    for name in user.plugins.keys() {
        if !effective_enabled(&user, &project, name) {
            continue;
        }
        let Some(manifest) =
            std::fs::read(home.join("plugins").join(format!("{name}-manifest.json")))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        let Some(decls) = manifest.get("settings").and_then(Value::as_array) else {
            continue;
        };
        let title = manifest
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        for d in decls {
            let field = |k: &str| d.get(k).and_then(Value::as_str).map(str::to_string);
            let (Some(key), Some(label)) = (field("key"), field("label")) else {
                continue;
            };
            let choices = match field("type").as_deref() {
                Some("bool") => vec![Value::Bool(false), Value::Bool(true)],
                Some("choice") => d
                    .get("choices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => continue,
            };
            let Some(first) = choices.first().cloned() else {
                continue;
            };
            let default = d
                .get("default")
                .filter(|v| choices.contains(v))
                .cloned()
                .unwrap_or(first);
            rows.push(PluginRow {
                plugin: name.clone(),
                title: title.clone(),
                key,
                label,
                choices,
                default,
            });
        }
    }
    rows
}

fn show_json(v: &Value) -> String {
    match v {
        Value::Bool(b) => on_off(*b),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn step_plugin(home: &std::path::Path, row: &PluginRow) {
    let mut values = plugin_values(home, &row.plugin);
    let cur = values.get(&row.key).cloned().unwrap_or(row.default.clone());
    let i = row.choices.iter().position(|c| *c == cur).unwrap_or(0);
    let next = row.choices[(i + 1) % row.choices.len()].clone();
    values.insert(row.key.clone(), next);
    let dir = home.join("plugin-settings");
    let _ = std::fs::create_dir_all(&dir);
    let body = serde_json::to_string_pretty(&Value::Object(values)).unwrap_or_default();
    let _ = crate::util::write_private(&dir, &format!("{}.json", row.plugin), &body);
}

enum Act {
    Core(usize),
    Plugin(usize),
}

struct Item {
    label: String,
    value: String,
    gap: bool,
    act: Act,
}

pub(crate) fn run_settings_picker(tui: Option<&crate::composer::SharedTui>, cwd: &std::path::Path) {
    let bg = crate::setup::BackgroundSnapshot::default_initial();
    let home = crate::setup::gray_home().ok();
    let mut selected = 0;
    loop {
        let mut current = saved();
        let mut items: Vec<Item> = CORE
            .iter()
            .enumerate()
            .map(|(i, r)| Item {
                label: r.label.to_string(),
                value: core_value(&r.kind, &mut current),
                gap: false,
                act: Act::Core(i),
            })
            .collect();
        let prows = home
            .as_deref()
            .map(|h| plugin_rows(h, cwd))
            .unwrap_or_default();
        let mut last_plugin = None;
        for (i, r) in prows.iter().enumerate() {
            let values = home
                .as_deref()
                .map(|h| plugin_values(h, &r.plugin))
                .unwrap_or_default();
            items.push(Item {
                label: format!("{} · {}", r.title, r.label),
                value: show_json(values.get(&r.key).unwrap_or(&r.default)),
                gap: last_plugin != Some(&r.plugin),
                act: Act::Plugin(i),
            });
            last_plugin = Some(&r.plugin);
        }
        let picked = crate::repl::with_modal_sync(tui, || {
            let mut picker = crate::repl::picker::ListPicker {
                title: "Settings",
                items: &items,
                label: Box::new(|it: &Item| it.label.as_str()),
                trailing: Some(Box::new(|it: &Item| {
                    vec![ratatui::text::Span::styled(
                        it.value.clone(),
                        ratatui::style::Style::default().fg(crate::theme::theme().text_muted),
                    )]
                })),
                footer: Some(Box::new(|| {
                    vec![ratatui::text::Line::from(ratatui::text::Span::styled(
                        "Enter changes · Esc closes",
                        ratatui::style::Style::default().fg(crate::theme::theme().text_muted),
                    ))]
                })),
                on_highlight: None,
                gap_before: Some(Box::new(|it: &Item| it.gap)),
                selected,
                current: None,
            };
            crate::repl::picker::run_list_picker(&mut picker, &bg)
        });
        let Ok(Some(i)) = picked else { return };
        selected = i;
        match items[i].act {
            Act::Core(c) => update(|s| core_step(&CORE[c].kind, s)),
            Act::Plugin(p) => {
                if let Some(h) = home.as_deref() {
                    step_plugin(h, &prows[p]);
                }
            }
        }
    }
}
