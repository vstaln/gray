use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

const MESSAGE_MAX_CHARS: usize = 200;

const TITLE_SAVE: &str = "\x1b[22;0t";
const TITLE_RESTORE: &str = "\x1b[23;0t";
const PROGRESS_BUSY: &str = "\x1b]9;4;3;0\x07";
const PROGRESS_IDLE: &str = "\x1b]9;4;0;0\x07";
const BELL: &str = "\x07";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Title {
    Off,
    Idle,
    Working,
}

struct State {
    started: bool,
    pushed: bool,
    title: Title,
}

static STATE: Mutex<State> = Mutex::new(State {
    started: false,
    pushed: false,
    title: Title::Off,
});

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn emit(seq: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

fn dir_label(cwd: &Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| cwd.display().to_string())
}

fn title_sequence(title: Title, cwd: &Path) -> Option<String> {
    let dir = dir_label(cwd);
    match title {
        Title::Off => None,
        Title::Idle => Some(format!("\x1b]0;{dir} · gray\x07")),
        Title::Working => Some(format!("\x1b]0;● {dir} · gray\x07")),
    }
}

fn apply_title(st: &mut State, title: Title, cwd: &Path) {
    if !crate::settings::terminal_title() {
        st.title = Title::Off;
        return;
    }
    if !st.pushed {
        st.pushed = true;
        st.title = Title::Off;
        emit(TITLE_SAVE);
    }
    if st.title == title {
        return;
    }
    if let Some(seq) = title_sequence(title, cwd) {
        emit(&seq);
    }
    st.title = title;
}

pub(crate) fn repl_start(interactive: bool, cwd: &Path) {
    let mut st = state();
    st.started = interactive && std::io::stdout().is_terminal();
    st.pushed = false;
    st.title = Title::Off;
    if !st.started {
        return;
    }
    apply_title(&mut st, Title::Idle, cwd);
}

pub(crate) fn repl_exit() {
    let mut st = state();
    if !st.started {
        return;
    }
    st.started = false;
    let pushed = std::mem::take(&mut st.pushed);
    st.title = Title::Off;
    drop(st);
    if pushed {
        emit(TITLE_RESTORE);
    }
    if crate::settings::terminal_progress() {
        emit(PROGRESS_IDLE);
    }
}

pub(crate) fn idle(cwd: &Path) {
    let mut st = state();
    if !st.started {
        return;
    }
    apply_title(&mut st, Title::Idle, cwd);
}

pub(crate) fn turn_start(cwd: &Path) {
    let mut st = state();
    if !st.started {
        return;
    }
    apply_title(&mut st, Title::Working, cwd);
    let progress = crate::settings::terminal_progress();
    drop(st);
    if progress {
        emit(PROGRESS_BUSY);
    }
}

pub(crate) fn turn_end(cwd: &Path) {
    let mut st = state();
    if !st.started {
        return;
    }
    apply_title(&mut st, Title::Idle, cwd);
    let progress = crate::settings::terminal_progress();
    drop(st);
    if progress {
        emit(PROGRESS_IDLE);
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MESSAGE_MAX_CHARS)
        .collect()
}

pub(crate) fn notify(title: &str, body: &str) -> bool {
    let title = clean(title);
    let body = clean(body);
    let title = if title.trim().is_empty() {
        "gray".to_string()
    } else {
        title
    };
    let tty = std::io::stdout().is_terminal();
    if tty {
        emit(BELL);
    }
    let spawned = std::process::Command::new("notify-send")
        .arg(&title)
        .arg(&body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if !spawned && tty {
        let message = if body.is_empty() {
            title
        } else {
            format!("{title}: {body}")
        };
        emit(&format!("\x1b]9;{message}\x07"));
    }
    spawned || tty
}
