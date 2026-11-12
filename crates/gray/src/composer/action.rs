use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;

use crate::composer::Tui;
use crate::keymap::Chord;

const BLINK: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct PluginAction {
    plugin: String,
    chords: Vec<Chord>,
    argv: Vec<String>,
    busy: String,
}

static ACTIONS: RwLock<Vec<PluginAction>> = RwLock::new(Vec::new());

/// The gray home the actions came from and the keybindings.json mtime they
/// saw, so an edited keymap (e.g. a plugin changing its hotkey) applies live.
static SOURCE: RwLock<Option<(std::path::PathBuf, Option<std::time::SystemTime>)>> =
    RwLock::new(None);

fn keymap_mtime(home: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::keymap::path_in(home))
        .and_then(|m| m.modified())
        .ok()
}

fn refresh() {
    let Some((home, seen)) = SOURCE.read().ok().and_then(|g| g.clone()) else {
        return;
    };
    if keymap_mtime(&home) != seen {
        load(&home);
    }
}

pub(crate) fn load(home: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Ok(mut g) = SOURCE.write() {
        *g = Some((home.to_path_buf(), keymap_mtime(home)));
    }
    let overrides: serde_json::Map<String, serde_json::Value> =
        std::fs::read_to_string(crate::keymap::path_in(home))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let Ok(lock) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home)) else {
        return warnings;
    };
    let mut actions = Vec::new();
    for (name, entry) in &lock.plugins {
        let Some(cli) = entry.cli_argv.as_ref().filter(|_| entry.enabled) else {
            continue;
        };
        let path = home.join("plugins").join(format!("{name}-manifest.json"));
        let Some(list) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("composer_actions").cloned())
        else {
            continue;
        };
        for a in list.as_array().into_iter().flatten() {
            let strs = |k: &str| -> Vec<String> {
                a.get(k)
                    .and_then(|x| x.as_array())
                    .map(|x| {
                        x.iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let mut keys = strs("keys");
            if let Some(user) = a
                .get("id")
                .and_then(|id| id.as_str())
                .and_then(|id| overrides.get(&format!("plugin.{name}.{id}")))
            {
                keys = match user {
                    serde_json::Value::String(k) => vec![k.clone()],
                    v => v
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect(),
                };
            }
            let mut chords = Vec::new();
            for key in keys {
                match Chord::parse(&key) {
                    Ok(c) => chords.push(c),
                    Err(e) => warnings.push(format!("plugin {name}: composer action key: {e}")),
                }
            }
            actions.push(PluginAction {
                plugin: name.clone(),
                chords,
                argv: cli.iter().cloned().chain(strs("argv")).collect(),
                busy: a
                    .get("busy")
                    .and_then(|b| b.as_str())
                    .unwrap_or(name)
                    .to_string(),
            });
        }
    }
    if let Ok(mut g) = ACTIONS.write() {
        *g = actions;
    }
    warnings
}

fn for_event(ev: &KeyEvent) -> Option<PluginAction> {
    let chord = Chord::from_event(ev)?;
    refresh();
    ACTIONS
        .read()
        .ok()?
        .iter()
        .find(|a| a.chords.contains(&chord))
        .cloned()
}

pub(crate) enum ActionBanner {
    Running { text: String, hint: String },
    Error(String),
}

#[derive(Default)]
pub(crate) enum ActionState {
    #[default]
    Idle,
    /// The child reads `down`/`up` lines on stdin as its key is pressed and
    /// released, and its latest stderr line replaces `busy` as the status.
    Running {
        child: Child,
        started: Instant,
        busy: String,
        chords: Vec<Chord>,
        status: Arc<Mutex<String>>,
        inserts: Arc<Mutex<String>>,
        inserted: bool,
    },
    Error(String),
}

impl ActionState {
    fn start(&mut self, action: PluginAction) {
        let (bin, args) = action.argv.split_first().expect("cli_argv is non-empty");
        match Command::new(bin)
            .args(args)
            .env("GRAY_ACTION_INSERT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                let status = Arc::new(Mutex::new(String::new()));
                let inserts = Arc::new(Mutex::new(String::new()));
                if let Some(err) = child.stderr.take() {
                    let status = status.clone();
                    let inserts = inserts.clone();
                    std::thread::spawn(move || {
                        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                            if let Some(text) = line.strip_prefix("insert\t") {
                                inserts.lock().expect("inserts lock").push_str(text);
                            } else if !line.trim().is_empty() {
                                *status.lock().expect("status lock") = line;
                            }
                        }
                    });
                }
                *self = ActionState::Running {
                    child,
                    started: Instant::now(),
                    busy: action.busy,
                    chords: action.chords,
                    status,
                    inserts,
                    inserted: false,
                };
            }
            Err(e) => {
                *self = ActionState::Error(format!("{}: cannot run {bin}: {e}", action.plugin))
            }
        }
    }

    pub(crate) fn cancel(&mut self) -> bool {
        match std::mem::take(self) {
            ActionState::Idle => false,
            ActionState::Error(_) => true,
            ActionState::Running { mut child, .. } => {
                let _ = child.kill();
                let _ = child.wait();
                true
            }
        }
    }

    fn poll(&mut self) -> Option<String> {
        let ActionState::Running {
            child,
            inserts,
            inserted,
            ..
        } = self
        else {
            return None;
        };
        let partial = std::mem::take(&mut *inserts.lock().expect("inserts lock"));
        *inserted |= !partial.is_empty();
        let rest = match child.try_wait() {
            Ok(Some(status)) => self.finish(status),
            Ok(None) => None,
            Err(e) => {
                *self = ActionState::Error(e.to_string());
                None
            }
        };
        let text = partial + rest.as_deref().unwrap_or_default();
        (!text.is_empty()).then_some(text)
    }

    fn finish(&mut self, status: ExitStatus) -> Option<String> {
        let ActionState::Running {
            child,
            status: line,
            inserts,
            inserted,
            ..
        } = self
        else {
            return None;
        };
        let mut out = String::new();
        if let Some(pipe) = child.stdout.as_mut() {
            let _ = pipe.read_to_string(&mut out);
        }
        // The stderr reader may still be draining the last line.
        std::thread::sleep(Duration::from_millis(20));
        let err = line.lock().expect("status lock").clone();
        let late = std::mem::take(&mut *inserts.lock().expect("inserts lock"));
        let typed = *inserted || !late.is_empty();
        let text = late + out.trim();
        if status.success() && (typed || !text.is_empty()) {
            *self = ActionState::Idle;
            return (!text.is_empty()).then_some(text);
        }
        let detail = [err.trim(), text.as_str()]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(|s| s.lines().last().unwrap_or(s).chars().take(200).collect())
            .unwrap_or_else(|| format!("no output ({status})"));
        *self = ActionState::Error(detail);
        None
    }

    pub(crate) fn prompt_glyph(&self) -> Option<&'static str> {
        let ActionState::Running { started, .. } = self else {
            return None;
        };
        let on = (started.elapsed().as_millis() / BLINK.as_millis()).is_multiple_of(2);
        Some(if on { "●" } else { "·" })
    }

    pub(crate) fn banner(&self) -> Option<ActionBanner> {
        match self {
            ActionState::Idle => None,
            ActionState::Running { busy, status, .. } => {
                let s = status.lock().expect("status lock");
                let line = if s.is_empty() {
                    busy.as_str()
                } else {
                    s.as_str()
                };
                let (text, hint) = line.split_once('\t').unwrap_or((line, ""));
                let hint = [hint.trim(), "esc cancel"]
                    .into_iter()
                    .filter(|h| !h.is_empty())
                    .collect::<Vec<_>>()
                    .join(" \u{b7} ");
                Some(ActionBanner::Running {
                    text: text.trim_end().to_string(),
                    hint,
                })
            }
            ActionState::Error(e) => Some(ActionBanner::Error(e.clone())),
        }
    }

    /// Tell the running action its key went down or up. False when `ev`
    /// is not that action's key.
    fn key(&mut self, ev: &KeyEvent, down: bool) -> bool {
        let ActionState::Running { child, chords, .. } = self else {
            return false;
        };
        if !Chord::from_event(ev).is_some_and(|c| chords.contains(&c)) {
            return false;
        }
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(if down { b"down\n" } else { b"up\n" });
            let _ = stdin.flush();
        }
        true
    }
}

impl Tui {
    pub(crate) fn plugin_action_key(&mut self, ev: &KeyEvent) -> bool {
        if self.action.key(ev, true) {
            return true;
        }
        let Some(action) = for_event(ev) else {
            return false;
        };
        if !matches!(self.action, ActionState::Running { .. }) {
            self.action.start(action);
        }
        true
    }

    pub(crate) fn plugin_action_key_up(&mut self, ev: &KeyEvent) {
        self.action.key(ev, false);
    }

    /// Start the composer action bound to `key` (e.g. `f8`), as a key press
    /// would. False when no plugin binds that key.
    pub(crate) fn start_action_for_key(&mut self, key: &str) -> bool {
        let Ok(chord) = Chord::parse(key) else {
            return false;
        };
        refresh();
        let Some(action) = ACTIONS
            .read()
            .ok()
            .and_then(|a| a.iter().find(|a| a.chords.contains(&chord)).cloned())
        else {
            return false;
        };
        if !matches!(self.action, ActionState::Running { .. }) {
            self.action.start(action);
        }
        let _ = self.draw();
        true
    }

    pub(crate) fn action_tick(&mut self) -> bool {
        let running = matches!(self.action, ActionState::Running { .. });
        if let Some(text) = self.action.poll() {
            self.textarea.insert_str(&text);
            self.history_idx = None;
            self.sel = 0;
        }
        running
    }
}
