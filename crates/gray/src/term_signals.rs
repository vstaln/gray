use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const STREAK_IDLE_RESET: Duration = Duration::from_secs(15 * 60);
const NOTIFY_MIN_TURN: Duration = Duration::from_secs(10);
const MESSAGE_MAX_CHARS: usize = 100;

const TITLE_SAVE: &str = "\x1b[22;0t";
const TITLE_RESTORE: &str = "\x1b[23;0t";
const PROGRESS_BUSY: &str = "\x1b]9;4;3;0\x07";
const PROGRESS_IDLE: &str = "\x1b]9;4;0;0\x07";
const BELL: &str = "\x07";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnEnd {
    Ok,
    Failed,
    Cancelled,
}

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
    streak_start: Option<Instant>,
    last_turn_end: Option<Instant>,
    reminded: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    started: false,
    pushed: false,
    title: Title::Off,
    streak_start: None,
    last_turn_end: None,
    reminded: false,
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
    st.streak_start = None;
    st.last_turn_end = None;
    st.reminded = false;
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
    st.streak_start = None;
    st.last_turn_end = None;
    st.reminded = false;
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
    let now = Instant::now();
    let streak_broken = st
        .last_turn_end
        .is_some_and(|end| now.saturating_duration_since(end) > STREAK_IDLE_RESET);
    if st.streak_start.is_none() || streak_broken {
        st.streak_start = Some(now);
        st.reminded = false;
    }
    apply_title(&mut st, Title::Working, cwd);
    let progress = crate::settings::terminal_progress();
    drop(st);
    if progress {
        emit(PROGRESS_BUSY);
    }
}

pub(crate) fn turn_end(cwd: &Path, end: TurnEnd, took: Duration, final_text: Option<&str>) {
    let mut st = state();
    if !st.started {
        return;
    }
    let now = Instant::now();
    st.last_turn_end = Some(now);
    apply_title(&mut st, Title::Idle, cwd);
    let streak_minutes = st
        .streak_start
        .map(|start| now.saturating_duration_since(start).as_secs() / 60)
        .unwrap_or(0);
    let remind = crate::settings::break_reminder_minutes()
        .filter(|minutes| !st.reminded && streak_minutes >= u64::from(*minutes));
    if remind.is_some() {
        st.reminded = true;
    }
    let progress = crate::settings::terminal_progress();
    let away = !crate::away::is_focused();
    drop(st);
    if progress {
        emit(PROGRESS_IDLE);
    }
    if let Some(minutes) = remind {
        crate::host::queue_say(format!(
            "You've been working for {minutes} min. Time for a short break?"
        ));
    }
    if away && took >= NOTIFY_MIN_TURN && crate::settings::notify_on_finish() {
        notify_turn_end(end, final_text);
    }
}

fn notify_turn_end(end: TurnEnd, final_text: Option<&str>) {
    let message = turn_message(end, final_text);
    emit(BELL);
    let spawned = std::process::Command::new("notify-send")
        .arg("gray")
        .arg(&message)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if !spawned {
        emit(&format!("\x1b]9;{message}\x07"));
    }
}

fn turn_message(end: TurnEnd, final_text: Option<&str>) -> String {
    let line = final_text
        .and_then(|text| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| {
            l.chars()
                .filter(|c| !c.is_control())
                .take(MESSAGE_MAX_CHARS)
                .collect::<String>()
        })
        .filter(|l| !l.is_empty());
    match (line, end) {
        (Some(line), _) => line,
        (None, TurnEnd::Ok) => "turn finished".to_string(),
        (None, TurnEnd::Failed | TurnEnd::Cancelled) => "turn failed".to_string(),
    }
}
