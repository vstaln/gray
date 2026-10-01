//! REPL/`-p` side of the plugin→host channel (`host/run`, `host/say`,
//! `host/background`, `host/ask`).
//!
//! Installed on every sidecar at spawn by [`crate::build_agent`] via the
//! shared builder;
//! the transport replies `{"error":…}` when no handler is set, so a missing
//! install fails loudly instead of hanging the plugin. `host/run` delegates
//! to the shared subprocess runner (`gray_plugin::host::run_prompt_child`);
//! `host/say` queues into [`take_host_say`], drained at the top of the REPL
//! loop (TUI-safe paint; same pattern as `take_profile_warnings`).

use std::path::PathBuf;
use std::sync::Mutex;

use gray_plugin::{HOST_ASK, HOST_RUN, HOST_SAY, HostHandler};

// Weak: the host must not keep an exited composer alive.
static TUI: Mutex<std::sync::Weak<std::sync::Mutex<crate::composer::Tui>>> =
    Mutex::new(std::sync::Weak::new());

pub(crate) fn register_tui(tui: &crate::composer::SharedTui) {
    *TUI.lock().expect("background registration") = std::sync::Arc::downgrade(tui);
}

fn background(params: serde_json::Value) -> anyhow::Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        path: Option<PathBuf>,
    }
    anyhow::ensure!(
        params.get("path").is_some(),
        "missing background path (null clears)"
    );
    let request: Request = serde_json::from_value(params)?;
    let tui = TUI
        .lock()
        .map_err(|_| anyhow::anyhow!("TUI unavailable"))?
        .upgrade()
        .ok_or_else(|| anyhow::anyhow!("background requires an interactive Gray TUI"))?;
    let mut tui = tui.lock().map_err(|_| anyhow::anyhow!("TUI unavailable"))?;
    tui.set_background(request.path.as_deref())
}

// bounded queue; a chatty sidecar must not grow this without limit
const MAX_SAY_QUEUE: usize = 32;
static SAY_QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Drain queued `host/say` lines (the REPL loop and print mode own rendering).
pub fn take_host_say() -> Vec<String> {
    SAY_QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

pub(crate) fn queue_say(text: String) {
    if let Ok(mut q) = SAY_QUEUE.lock() {
        if q.len() >= MAX_SAY_QUEUE {
            q.remove(0);
        }
        q.push(text);
    }
    // Paint it now, not when the user next presses Enter.
    request_wake();
}

/// Set when something wants the idle REPL's attention (a queued say line, a
/// cron delivery in this session's inbox, a finished background job). The
/// prompt returns early on it with an empty draft; the loop top clears it and
/// does the work (painting, or a turn).
static WAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn request_wake() {
    WAKE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn wake_requested() -> bool {
    WAKE.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn clear_wake() {
    WAKE.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// The session the interactive REPL is showing: the inbox poller (a separate
/// task) reads it, and `/new` / `/resume` change it under the poller.
static LIVE_SESSION: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_live_session(sid: Option<&str>) {
    if let Ok(mut s) = LIVE_SESSION.lock() {
        *s = sid.map(str::to_string);
    }
}

pub(crate) fn live_session() -> Option<String> {
    LIVE_SESSION.lock().ok().and_then(|s| s.clone())
}

/// Handler for sidecars spawned by [`crate::build_agent`]. `cwd` pins the
/// `host/run` child's working dir (the same dir the agent's tools see).
pub fn default_handler(cwd: PathBuf) -> HostHandler {
    std::sync::Arc::new(move |method: String, params: serde_json::Value| {
        let cwd = cwd.clone();
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>> =
            Box::pin(async move {
                match method.as_str() {
                    "host/background" => match background(params) {
                        Ok(()) => serde_json::json!({"ok": true}),
                        Err(e) => serde_json::json!({"error": format!("{e:#}")}),
                    },
                    HOST_SAY => {
                        if let Some(text) = params.get("text").and_then(|t| t.as_str())
                            && !text.trim().is_empty()
                        {
                            queue_say(text.to_string());
                        }
                        serde_json::json!({"ok": true})
                    }
                    HOST_ASK => crate::ask::handle_ask(params).await,
                    HOST_RUN => {
                        let prompt = params
                            .get("prompt")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        if prompt.trim().is_empty() {
                            return serde_json::json!({"error": "host/run: missing prompt"});
                        }
                        gray_plugin::host::run_prompt_child(&cwd, &prompt).await
                    }
                    _ => serde_json::json!({"error": format!("unknown host method {method}")}),
                }
            });
        fut
    })
}

#[path = "host_tests.rs"]
#[cfg(test)]
mod tests;
