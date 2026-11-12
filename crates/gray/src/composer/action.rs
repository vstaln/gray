use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::RwLock;
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

pub(crate) fn load(home: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
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
            let mut chords = Vec::new();
            for key in strs("keys") {
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
    ACTIONS
        .read()
        .ok()?
        .iter()
        .find(|a| a.chords.contains(&chord))
        .cloned()
}

#[derive(Default)]
pub(crate) enum ActionState {
    #[default]
    Idle,
    Running {
        child: Child,
        started: Instant,
        busy: String,
    },
    Error(String),
}

impl ActionState {
    fn start(&mut self, action: PluginAction) {
        let (bin, args) = action.argv.split_first().expect("cli_argv is non-empty");
        match Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                *self = ActionState::Running {
                    child,
                    started: Instant::now(),
                    busy: format!("{}  (Esc to cancel)", action.busy),
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
        let ActionState::Running { child, .. } = self else {
            return None;
        };
        match child.try_wait() {
            Ok(Some(status)) => self.finish(status),
            Ok(None) => None,
            Err(e) => {
                *self = ActionState::Error(e.to_string());
                None
            }
        }
    }

    fn finish(&mut self, status: ExitStatus) -> Option<String> {
        let ActionState::Running { child, .. } = self else {
            return None;
        };
        let (mut out, mut err) = (String::new(), String::new());
        if let Some(pipe) = child.stdout.as_mut() {
            let _ = pipe.read_to_string(&mut out);
        }
        if let Some(pipe) = child.stderr.as_mut() {
            let _ = pipe.read_to_string(&mut err);
        }
        let text = out.trim().to_string();
        if status.success() && !text.is_empty() {
            *self = ActionState::Idle;
            return Some(text);
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

    pub(crate) fn banner(&self) -> Option<(&str, bool)> {
        match self {
            ActionState::Idle => None,
            ActionState::Running { busy, .. } => Some((busy, false)),
            ActionState::Error(e) => Some((e, true)),
        }
    }
}

impl Tui {
    pub(crate) fn plugin_action_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(action) = for_event(ev) else {
            return false;
        };
        if !matches!(self.action, ActionState::Running { .. }) {
            self.action.start(action);
        }
        true
    }

    /// Start the composer action bound to `key` (e.g. `f8`), as a key press
    /// would. False when no plugin binds that key.
    pub(crate) fn start_action_for_key(&mut self, key: &str) -> bool {
        let Ok(chord) = Chord::parse(key) else {
            return false;
        };
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
