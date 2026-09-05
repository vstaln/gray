//! Shared plugin→host runner core (`host/run` over a `gray -p` child).
//!
//! Both hosts (REPL/`-p` in `gray`, daemon in `gray-gateway`) serve sidecar
//! `host/*` requests through this: a subprocess, not an in-process agent
//! turn, because `Agent::run` futures are `!Send` (streaming sink) and the
//! sidecar transport (`sidecar.rs` reader) needs `Send`. Spawning the running
//! binary (`current_exe`) keeps dev and installed layouts working with no
//! `PATH` setup; the process env (`GRAY_HOME`) is inherited so the cron
//! sidecar and its runner see the same jobs.
//!
//! Ceiling: the host enforces a 30 s TTL per request — this helper reaps the
//! child at 28 s so a long turn reports a loud timeout instead of hanging
//! the sidecar. A timed-out turn's side effects already happened; only the
//! reply is lost (the error text says so).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

/// Run `prompt` through a fresh `gray -p` child, returning the `host/run`
/// result value (`{"text"}` or `{"error"}`). Loud on every failure, never hangs.
pub async fn run_prompt_child(cwd: &Path, prompt: &str) -> Value {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return json!({"error": format!("host/run: current_exe: {e}")}),
    };
    let mut child = match tokio::process::Command::new(exe)
        .arg("-p")
        .arg(prompt)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return json!({"error": format!("host/run spawn: {e}")}),
    };
    // Drain stdout concurrently: `wait()` alone deadlocks past the pipe buffer.
    let mut piped = child.stdout.take();
    let drain = tokio::spawn(async move {
        let mut v = Vec::new();
        if let Some(ref mut o) = piped {
            let _ = o.read_to_end(&mut v).await;
        }
        v
    });
    let status = match tokio::time::timeout(Duration::from_secs(28), child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return json!({"error": format!("host/run wait: {e}")}),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = drain.await;
            return json!({"error": "host/run timed out after 28s"});
        }
    };
    let bytes = drain.await.unwrap_or_default();
    if !status.success() {
        return json!({"error": format!("host/run: gray -p exited {status}")});
    }
    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    if text.is_empty() {
        json!({"error": "host/run: empty reply"})
    } else {
        json!({"text": text})
    }
}
