use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

static RETURNED: Mutex<Option<u64>> = Mutex::new(None);
static LAST_ACTIVE: Mutex<Option<Instant>> = Mutex::new(None);

pub(crate) fn activity() {
    *LAST_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

pub(crate) fn focus(gained: bool, turn_running: bool) {
    gray_core::agent::TERMINAL_FOCUSED.store(gained, Ordering::Relaxed);
    if !gained || turn_running {
        return;
    }
    let last = *LAST_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = last {
        *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.elapsed().as_secs());
        crate::host::request_wake();
    }
}

pub(crate) fn take_return() -> Option<u64> {
    RETURNED.lock().unwrap_or_else(|e| e.into_inner()).take()
}
