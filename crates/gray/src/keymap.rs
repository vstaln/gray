//! Named editor/app actions and the keys bound to them.
//!
//! Both composer input loops (the idle prompt and the mid-turn key
//! watcher) resolve a key press to actions here, so the bindings cannot
//! drift between the two. Users rebind in `~/.gray/keybindings.json` using
//! pi's action ids and key syntax, so a pi `keybindings.json` carries over:
//!
//! ```json
//! { "tui.editor.cursorWordLeft": ["alt+left", "ctrl+b"],
//!   "app.exit": [],
//!   "/compact": "ctrl+shift+k" }
//! ```
//!
//! A configured value replaces the action's defaults; `[]` unbinds it. A
//! key starting with `/` binds a slash command (submitted as if typed).
//! Ctrl+C always cancels/clears regardless of config: it is the escape
//! hatch out of raw mode and cannot be unbound.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// An action a key can trigger. Order of [`ALL`] is resolution priority:
/// when one key is bound to several actions, the first that applies in
/// the current context wins (popup selection before editing, Ctrl+D exits
/// on an empty draft before it deletes forward).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    SelectUp,
    SelectDown,
    SelectCancel,
    Tab,
    Clear,
    Exit,
    Interrupt,
    PasteClipboard,
    NewLine,
    Submit,
    HistoryPrevious,
    HistoryNext,
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorLineStart,
    CursorLineEnd,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
}

/// Every action in resolution-priority order.
pub const ALL: &[Action] = &[
    Action::SelectUp,
    Action::SelectDown,
    Action::SelectCancel,
    Action::Tab,
    Action::Clear,
    Action::Exit,
    Action::Interrupt,
    Action::PasteClipboard,
    Action::NewLine,
    Action::Submit,
    Action::HistoryPrevious,
    Action::HistoryNext,
    Action::CursorUp,
    Action::CursorDown,
    Action::CursorLeft,
    Action::CursorRight,
    Action::CursorWordLeft,
    Action::CursorWordRight,
    Action::CursorLineStart,
    Action::CursorLineEnd,
    Action::DeleteCharBackward,
    Action::DeleteCharForward,
    Action::DeleteWordBackward,
    Action::DeleteWordForward,
    Action::DeleteToLineStart,
    Action::DeleteToLineEnd,
];

impl Action {
    /// pi's id for this action (`keybindings.json` key).
    pub fn id(self) -> &'static str {
        match self {
            Action::SelectUp => "tui.select.up",
            Action::SelectDown => "tui.select.down",
            Action::SelectCancel => "tui.select.cancel",
            Action::Tab => "tui.input.tab",
            Action::Clear => "app.clear",
            Action::Exit => "app.exit",
            Action::Interrupt => "app.interrupt",
            Action::PasteClipboard => "app.clipboard.pasteImage",
            Action::NewLine => "tui.input.newLine",
            Action::Submit => "tui.input.submit",
            Action::HistoryPrevious => "tui.editor.historyPrevious",
            Action::HistoryNext => "tui.editor.historyNext",
            Action::CursorUp => "tui.editor.cursorUp",
            Action::CursorDown => "tui.editor.cursorDown",
            Action::CursorLeft => "tui.editor.cursorLeft",
            Action::CursorRight => "tui.editor.cursorRight",
            Action::CursorWordLeft => "tui.editor.cursorWordLeft",
            Action::CursorWordRight => "tui.editor.cursorWordRight",
            Action::CursorLineStart => "tui.editor.cursorLineStart",
            Action::CursorLineEnd => "tui.editor.cursorLineEnd",
            Action::DeleteCharBackward => "tui.editor.deleteCharBackward",
            Action::DeleteCharForward => "tui.editor.deleteCharForward",
            Action::DeleteWordBackward => "tui.editor.deleteWordBackward",
            Action::DeleteWordForward => "tui.editor.deleteWordForward",
            Action::DeleteToLineStart => "tui.editor.deleteToLineStart",
            Action::DeleteToLineEnd => "tui.editor.deleteToLineEnd",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        ALL.iter().copied().find(|a| a.id() == id)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Action::SelectUp => "Move popup selection up",
            Action::SelectDown => "Move popup selection down",
            Action::SelectCancel => "Close the completion popup",
            Action::Tab => "Complete the selected entry",
            Action::Clear => "Clear the draft, or exit on an empty prompt",
            Action::Exit => "Exit when the draft is empty",
            Action::Interrupt => "Cancel the turn, or clear the draft",
            Action::PasteClipboard => "Paste image or text from the clipboard",
            Action::NewLine => "Insert a new line",
            Action::Submit => "Send (queues while a turn runs)",
            Action::HistoryPrevious => "Previous prompt from history",
            Action::HistoryNext => "Next prompt from history",
            Action::CursorUp => "Cursor up, history at the top",
            Action::CursorDown => "Cursor down, history at the bottom",
            Action::CursorLeft => "Cursor left",
            Action::CursorRight => "Cursor right",
            Action::CursorWordLeft => "Cursor word left",
            Action::CursorWordRight => "Cursor word right",
            Action::CursorLineStart => "Cursor to line start",
            Action::CursorLineEnd => "Cursor to line end",
            Action::DeleteCharBackward => "Delete character backward",
            Action::DeleteCharForward => "Delete character forward",
            Action::DeleteWordBackward => "Delete word backward",
            Action::DeleteWordForward => "Delete word forward",
            Action::DeleteToLineStart => "Delete to line start",
            Action::DeleteToLineEnd => "Delete to line end",
        }
    }

    fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::SelectUp => &["up", "ctrl+p"],
            Action::SelectDown => &["down", "ctrl+n"],
            Action::SelectCancel => &["escape"],
            Action::Tab => &["tab"],
            Action::Clear => &["ctrl+c"],
            Action::Exit => &["ctrl+d"],
            Action::Interrupt => &["escape"],
            Action::PasteClipboard => &["ctrl+v", "ctrl+shift+v", "shift+insert"],
            Action::NewLine => &["shift+enter", "alt+enter", "ctrl+j", "ctrl+m"],
            Action::Submit => &["enter"],
            Action::HistoryPrevious | Action::HistoryNext => &[],
            Action::CursorUp => &["up"],
            Action::CursorDown => &["down"],
            Action::CursorLeft => &["left", "ctrl+b"],
            Action::CursorRight => &["right", "ctrl+f"],
            Action::CursorWordLeft => &["alt+left", "ctrl+left", "alt+b"],
            Action::CursorWordRight => &["alt+right", "ctrl+right", "alt+f"],
            Action::CursorLineStart => &["home", "ctrl+a"],
            Action::CursorLineEnd => &["end", "ctrl+e"],
            Action::DeleteCharBackward => &["backspace"],
            Action::DeleteCharForward => &["delete", "ctrl+d"],
            Action::DeleteWordBackward => &["ctrl+w", "alt+backspace", "ctrl+backspace"],
            Action::DeleteWordForward => &["alt+d", "alt+delete", "ctrl+delete"],
            Action::DeleteToLineStart => &["ctrl+u"],
            Action::DeleteToLineEnd => &["ctrl+k"],
        }
    }
}

/// A normalized key chord: modifiers (ctrl/alt/shift/super) plus a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub code: ChordKey,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub sup: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChordKey {
    Char(char),
    Enter,
    Tab,
    Esc,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
}

impl Chord {
    /// Normalizes a terminal key event so `shift+a`, `A` and Shift-reported
    /// symbols compare equal to what a user writes.
    pub fn from_event(ev: &KeyEvent) -> Option<Chord> {
        let m = ev.modifiers;
        let mut shift = m.contains(KeyModifiers::SHIFT);
        let code = match ev.code {
            KeyCode::Char(c) => {
                if c.is_ascii_uppercase() {
                    shift = true;
                    ChordKey::Char(c.to_ascii_lowercase())
                } else {
                    // A shifted symbol (`?`, `{`) already encodes Shift.
                    if !c.is_ascii_alphabetic() {
                        shift = false;
                    }
                    ChordKey::Char(c)
                }
            }
            KeyCode::Enter => ChordKey::Enter,
            KeyCode::Tab => ChordKey::Tab,
            KeyCode::BackTab => {
                shift = true;
                ChordKey::Tab
            }
            KeyCode::Esc => ChordKey::Esc,
            KeyCode::Backspace => ChordKey::Backspace,
            KeyCode::Delete => ChordKey::Delete,
            KeyCode::Insert => ChordKey::Insert,
            KeyCode::Home => ChordKey::Home,
            KeyCode::End => ChordKey::End,
            KeyCode::PageUp => ChordKey::PageUp,
            KeyCode::PageDown => ChordKey::PageDown,
            KeyCode::Up => ChordKey::Up,
            KeyCode::Down => ChordKey::Down,
            KeyCode::Left => ChordKey::Left,
            KeyCode::Right => ChordKey::Right,
            KeyCode::F(n) => ChordKey::F(n),
            _ => return None,
        };
        Some(Chord {
            code,
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            shift,
            sup: m.contains(KeyModifiers::SUPER),
        })
    }

    /// Parses pi's syntax: `ctrl+shift+x`, `alt+enter`, `pageUp`, `f5`, `?`.
    pub fn parse(s: &str) -> Result<Chord, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".into());
        }
        // `ctrl++` / a bare `+`: the key itself is the last segment.
        let (mods, key) = match s.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if s == "+" => ("", "+"),
            None => match s.rfind('+') {
                Some(i) => (&s[..i], &s[i + 1..]),
                None => ("", s),
            },
        };
        let mut c = Chord {
            code: ChordKey::Esc,
            ctrl: false,
            alt: false,
            shift: false,
            sup: false,
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "meta" | "option" | "opt" => c.alt = true,
                "shift" => c.shift = true,
                "super" | "cmd" | "win" => c.sup = true,
                other => return Err(format!("unknown modifier `{other}` in `{s}`")),
            }
        }
        let lower = key.to_ascii_lowercase();
        c.code = match lower.as_str() {
            "escape" | "esc" => ChordKey::Esc,
            "enter" | "return" => ChordKey::Enter,
            "tab" => ChordKey::Tab,
            "space" => ChordKey::Char(' '),
            "backspace" => ChordKey::Backspace,
            "delete" | "del" => ChordKey::Delete,
            "insert" | "ins" => ChordKey::Insert,
            "home" => ChordKey::Home,
            "end" => ChordKey::End,
            "pageup" => ChordKey::PageUp,
            "pagedown" => ChordKey::PageDown,
            "up" => ChordKey::Up,
            "down" => ChordKey::Down,
            "left" => ChordKey::Left,
            "right" => ChordKey::Right,
            f if f.len() >= 2
                && f.starts_with('f')
                && f[1..].chars().all(|d| d.is_ascii_digit()) =>
            {
                let n: u8 = f[1..].parse().map_err(|_| format!("bad key `{s}`"))?;
                if !(1..=24).contains(&n) {
                    return Err(format!("bad function key `{s}`"));
                }
                ChordKey::F(n)
            }
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => {
                        if ch.is_ascii_uppercase() {
                            c.shift = true;
                        }
                        if !ch.is_ascii_alphabetic() {
                            // Shifted symbols are written as the symbol.
                            c.shift = false;
                        }
                        ChordKey::Char(ch.to_ascii_lowercase())
                    }
                    _ => return Err(format!("unknown key `{key}` in `{s}`")),
                }
            }
        };
        Ok(c)
    }

    /// Canonical display form (`ctrl+shift+x`).
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("ctrl+");
        }
        if self.alt {
            out.push_str("alt+");
        }
        if self.shift {
            out.push_str("shift+");
        }
        if self.sup {
            out.push_str("super+");
        }
        match self.code {
            ChordKey::Char(' ') => out.push_str("space"),
            ChordKey::Char(c) => out.push(c),
            ChordKey::Enter => out.push_str("enter"),
            ChordKey::Tab => out.push_str("tab"),
            ChordKey::Esc => out.push_str("escape"),
            ChordKey::Backspace => out.push_str("backspace"),
            ChordKey::Delete => out.push_str("delete"),
            ChordKey::Insert => out.push_str("insert"),
            ChordKey::Home => out.push_str("home"),
            ChordKey::End => out.push_str("end"),
            ChordKey::PageUp => out.push_str("pageUp"),
            ChordKey::PageDown => out.push_str("pageDown"),
            ChordKey::Up => out.push_str("up"),
            ChordKey::Down => out.push_str("down"),
            ChordKey::Left => out.push_str("left"),
            ChordKey::Right => out.push_str("right"),
            ChordKey::F(n) => out.push_str(&format!("f{n}")),
        }
        out
    }
}

/// What a key resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Action(Action),
    /// Submit this slash command as if typed (`/compact`).
    Command(String),
}

/// The active key table plus problems found loading it.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    by_action: BTreeMap<Action, Vec<Chord>>,
    commands: Vec<(Chord, String)>,
    /// `keybindings.json` problems: unknown ids, bad keys.
    pub warnings: Vec<String>,
    /// Ids that were overridden by the user file.
    pub overridden: Vec<String>,
    pub source: Option<PathBuf>,
}

impl Keymap {
    pub fn defaults() -> Keymap {
        let mut km = Keymap::default();
        for &a in ALL {
            let chords = a
                .defaults()
                .iter()
                .map(|k| Chord::parse(k).expect("default key parses"))
                .collect();
            km.by_action.insert(a, chords);
        }
        km
    }

    /// Defaults overlaid with a `keybindings.json` body.
    pub fn from_json(body: &str) -> Keymap {
        let mut km = Keymap::defaults();
        let value: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                km.warnings.push(format!("not valid JSON: {e}"));
                return km;
            }
        };
        let Some(obj) = value.as_object() else {
            km.warnings
                .push("expected an object of action id → key(s)".into());
            return km;
        };
        for (id, keys) in obj {
            let list: Vec<String> = match keys {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(a) => {
                    let mut out = Vec::new();
                    for k in a {
                        match k.as_str() {
                            Some(s) => out.push(s.to_string()),
                            None => km.warnings.push(format!("{id}: keys must be strings")),
                        }
                    }
                    out
                }
                serde_json::Value::Null => Vec::new(),
                _ => {
                    km.warnings
                        .push(format!("{id}: expected a key string or a list"));
                    continue;
                }
            };
            let mut chords = Vec::new();
            for k in &list {
                match Chord::parse(k) {
                    Ok(c) => chords.push(c),
                    Err(e) => km.warnings.push(format!("{id}: {e}")),
                }
            }
            if id.starts_with('/') {
                for c in chords {
                    km.commands.push((c, id.clone()));
                }
                km.overridden.push(id.clone());
            } else if let Some(a) = Action::from_id(id) {
                km.by_action.insert(a, chords);
                km.overridden.push(id.clone());
            } else if !is_known_pi_only(id) {
                km.warnings.push(format!("unknown action `{id}`"));
            }
        }
        // Ctrl+C is the raw-mode escape hatch: always clears/cancels.
        let ctrl_c = Chord::parse("ctrl+c").expect("ctrl+c parses");
        let clear = km.by_action.entry(Action::Clear).or_default();
        if !clear.contains(&ctrl_c) {
            clear.push(ctrl_c);
        }
        km
    }

    /// Everything bound to `chord`, slash commands first (they are the
    /// user's explicit choice), then actions in priority order.
    pub fn resolve(&self, chord: &Chord) -> Vec<Binding> {
        let mut out: Vec<Binding> = self
            .commands
            .iter()
            .filter(|(c, _)| c == chord)
            .map(|(_, cmd)| Binding::Command(cmd.clone()))
            .collect();
        for &a in ALL {
            if self.by_action.get(&a).is_some_and(|v| v.contains(chord)) {
                out.push(Binding::Action(a));
            }
        }
        out
    }

    pub fn keys_for(&self, a: Action) -> &[Chord] {
        self.by_action.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn commands(&self) -> &[(Chord, String)] {
        &self.commands
    }
}

/// pi ids gray has no equivalent for yet: accepted silently so a pi
/// `keybindings.json` loads without a wall of warnings.
fn is_known_pi_only(id: &str) -> bool {
    id.starts_with("app.") || id.starts_with("tui.")
}

static ACTIVE: RwLock<Option<Keymap>> = RwLock::new(None);

/// The active keymap (defaults until [`load_into_active`] runs).
pub fn active() -> Keymap {
    ACTIVE
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(Keymap::defaults)
}

pub fn set_active(km: Keymap) {
    if let Ok(mut g) = ACTIVE.write() {
        *g = Some(km);
    }
}

/// Resolves a live key event against the active keymap.
pub fn resolve_event(ev: &KeyEvent) -> Vec<Binding> {
    let Some(chord) = Chord::from_event(ev) else {
        return Vec::new();
    };
    match ACTIVE.read() {
        Ok(g) => match g.as_ref() {
            Some(km) => km.resolve(&chord),
            None => DEFAULTS.with(|d| d.resolve(&chord)),
        },
        Err(_) => DEFAULTS.with(|d| d.resolve(&chord)),
    }
}

thread_local! {
    static DEFAULTS: Keymap = Keymap::defaults();
}

pub fn path_in(gray_home: &Path) -> PathBuf {
    gray_home.join("keybindings.json")
}

/// Reads `<gray_home>/keybindings.json` (defaults when absent).
pub fn load_in(gray_home: &Path) -> Keymap {
    let path = path_in(gray_home);
    match std::fs::read_to_string(&path) {
        Ok(body) => {
            let mut km = Keymap::from_json(&body);
            km.source = Some(path);
            km
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Keymap::defaults(),
        Err(e) => {
            let mut km = Keymap::defaults();
            km.warnings.push(format!("{}: {e}", path.display()));
            km
        }
    }
}

/// Loads the user's file into the active keymap; returns its warnings.
pub fn load_into_active(gray_home: &Path) -> Vec<String> {
    let km = load_in(gray_home);
    let warnings = km.warnings.clone();
    set_active(km);
    warnings
}

#[path = "keymap_tests.rs"]
#[cfg(test)]
mod tests;
