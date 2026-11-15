use std::sync::Mutex;

static GHOST: Mutex<Option<String>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Option<String>> {
    GHOST.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn set(text: &str) {
    let text = text.trim();
    *slot() = (!text.is_empty()).then(|| text.to_string());
    crate::host::request_wake();
}

pub(crate) fn discard() {
    slot().take();
}

pub(crate) fn shown(input: &str, running: bool) -> Option<String> {
    let mut slot = slot();
    let text = slot.as_ref()?;
    if running || !input.trim().is_empty() {
        *slot = None;
        return None;
    }
    Some(text.clone())
}

pub(crate) fn take() -> Option<String> {
    slot().take()
}
