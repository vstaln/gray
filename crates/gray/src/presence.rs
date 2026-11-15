use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const AWAY_AFTER: Duration = Duration::from_secs(60);

static LOST_AT: Mutex<Option<Instant>> = Mutex::new(None);
static RETURNED: Mutex<Option<u64>> = Mutex::new(None);
static SAW_FOCUS: AtomicBool = AtomicBool::new(false);
static LAST_INPUT: Mutex<Option<Instant>> = Mutex::new(None);

fn returned(away: Duration) {
    *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = Some(away.as_secs());
    crate::host::request_wake();
}

pub(crate) fn focus(gained: bool, turn_running: bool) {
    SAW_FOCUS.store(true, Ordering::Relaxed);
    gray_core::agent::TERMINAL_FOCUSED.store(gained, Ordering::Relaxed);
    let mut lost = LOST_AT.lock().unwrap_or_else(|e| e.into_inner());
    if !gained {
        lost.get_or_insert_with(Instant::now);
        return;
    }
    if let Some(away) = lost.take().map(|t| t.elapsed())
        && away >= AWAY_AFTER
        && !turn_running
    {
        returned(away);
    }
}

pub(crate) fn key(turn_running: bool) {
    let now = Instant::now();
    let gap = LAST_INPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(now);
    if turn_running || SAW_FOCUS.load(Ordering::Relaxed) {
        return;
    }
    if let Some(away) = gap.map(|t| now.saturating_duration_since(t))
        && away >= AWAY_AFTER
    {
        returned(away);
    }
}

pub(crate) fn take_return() -> Option<u64> {
    RETURNED.lock().unwrap_or_else(|e| e.into_inner()).take()
}
