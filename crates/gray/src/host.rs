//! REPL/`-p` side of the plugin→host channel (`host/run` + `host/say`).
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

use gray_plugin::{HOST_RUN, HOST_SAY, HostHandler};

static SAY_QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Drain queued `host/say` lines (the REPL loop and print mode own rendering).
pub fn take_host_say() -> Vec<String> {
    SAY_QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn queue_say(text: String) {
    if let Ok(mut q) = SAY_QUEUE.lock() {
        q.push(text);
    }
}

/// Handler for sidecars spawned by [`crate::build_agent`]. `cwd` pins the
/// `host/run` child's working dir (the same dir the agent's tools see).
pub fn default_handler(cwd: PathBuf) -> HostHandler {
    std::sync::Arc::new(move |method: String, params: serde_json::Value| {
        let cwd = cwd.clone();
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>> =
            Box::pin(async move {
                match method.as_str() {
                    HOST_SAY => {
                        if let Some(text) = params.get("text").and_then(|t| t.as_str())
                            && !text.trim().is_empty()
                        {
                            queue_say(text.to_string());
                        }
                        serde_json::json!({"ok": true})
                    }
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
