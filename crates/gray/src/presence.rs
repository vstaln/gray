use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static RETURNED: Mutex<Option<u64>> = Mutex::new(None);
static LAST_ACTIVE: Mutex<Option<Instant>> = Mutex::new(None);
static TURN_GEN: AtomicU64 = AtomicU64::new(0);
static RETURN_GEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn activity() {
    *LAST_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

pub(crate) fn turn_started() {
    TURN_GEN.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn recap_fresh() -> bool {
    !crate::repl::TURN_IN_FLIGHT.load(Ordering::Relaxed)
        && TURN_GEN.load(Ordering::Relaxed) == RETURN_GEN.load(Ordering::Relaxed)
}

pub(crate) fn focus(gained: bool, turn_running: bool) {
    gray_core::agent::TERMINAL_FOCUSED.store(gained, Ordering::Relaxed);
    if !gained || turn_running {
        return;
    }
    if let Some(t) = LAST_ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(Instant::now())
    {
        *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.elapsed().as_secs());
        crate::host::request_wake();
    }
}

pub(crate) fn take_return() -> Option<u64> {
    let secs = RETURNED.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    RETURN_GEN.store(TURN_GEN.load(Ordering::Relaxed), Ordering::Relaxed);
    Some(secs)
}
