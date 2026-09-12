//! Shell wake drain (briefs 3A + 3B REPL rows, 2E lifecycle leftovers).
//!
//! Queue-then-drain like [`crate::host::take_host_say`]: a background task
//! owns the only [`registry`](gray_tools::shell::registry) broadcast
//! subscription, coalesces 500 ms windows into one multi-line message, and
//! queues it. The REPL loop owns rendering/injection (system notice, never
//! as if the user typed it). `UserInput` never injects (it only wakes
//! `sleep`, which holds its own subscription).
//!
//! Mid-turn `steer` vs idle inject: the agent is `&mut`-borrowed by
//! `run_streaming`, so the drain cannot steer concurrently. Mid-turn exits
//! stay queued and surface at the next loop-top as a synthetic follow-up
//! turn (same text, one turn later) — or via `steer` when `wake_on_exit`
//! is off. Never injects during compaction: the loop-top is idle by
//! construction, and auto-compact runs inside the turn.
//!
//! Config: `shell.wake_on_exit` has no config-file section yet, so it is
//! `GRAY_SHELL_WAKE_ON_EXIT` (default true; `0/false/no/off` disables).
//! Print mode skips idle injection unless explicitly
//! enabled via that env var.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Coalesce window: events within 500 ms become one multi-line message.
pub const WAKE_COALESCE: Duration = Duration::from_millis(500);
/// `shutdown_session` overall deadline on quit (brief 2E).
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);
/// 2E startup sweep: logs older than 7 days go.
const LOG_SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Shell transcript cap per tN.log: matches pump + gray.log 10MiB. The
/// age sweep alone let real usage reach 208MB; the sweep deletes oversized
/// files too (pre-cap leftovers), the pump stops live writes past this.
const SHELL_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// WAKE_QUEUE cap: the REPL drains each loop-top, but print mode may
/// never drain, so an uncleared queue would grow unbounded. Past this the
/// oldest entries drop (freshest wake wins).
pub const WAKE_QUEUE_CAP: usize = 100;
/// Bash + sleep self-bound at 600 s, so agents need headroom above that.
pub const SHELL_TOOL_TIMEOUT: Duration = Duration::from_secs(610);

static WAKE_QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CURRENT_SESSION: Mutex<String> = Mutex::new(String::new());

/// Drain queued wake messages (the REPL loop and print mode own rendering).
pub fn take_shell_wake() -> Vec<String> {
    WAKE_QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

pub(crate) fn queue_shell_wake(text: String) {
    if text.trim().is_empty() {
        return;
    }
    if let Ok(mut q) = WAKE_QUEUE.lock() {
        capped_push(&mut q, text);
    }
}

/// Pure queue cap (unit-testable): push then drop oldest past the cap, so
/// the freshest wake survives. Mirrors pump `MemView` drain style.
fn capped_push(q: &mut Vec<String>, text: String) {
    q.push(text);
    let excess = q.len().saturating_sub(WAKE_QUEUE_CAP);
    if excess > 0 {
        // ponytail: drain is O(excess); never Vec::remove(0) in a loop.
        q.drain(..excess);
    }
}

/// Pure sweep check (unit-testable): age sweep plus size cap for pre-cap
/// oversized logs.
fn sweep_due(len: u64, age: Option<Duration>) -> bool {
    len > SHELL_LOG_MAX_BYTES || age.is_some_and(|a| a > LOG_SWEEP_AGE)
}

/// Idle wake-ups start a turn when true (default). False in `-p`
/// unless `GRAY_SHELL_WAKE_ON_EXIT` explicitly enables.
pub fn wake_on_exit() -> bool {
    match std::env::var("GRAY_SHELL_WAKE_ON_EXIT") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Registry session key: the REPL session id, else `"nosession"`.
pub fn shell_session_key(session_id: Option<&str>) -> String {
    session_id
        .filter(|s| !s.is_empty())
        .unwrap_or("nosession")
        .to_string()
}

/// Point the background drain at `session` (call each loop-top; cheap).
pub fn set_drain_session(session: &str) {
    if let Ok(mut cur) = CURRENT_SESSION.lock()
        && cur.as_str() != session
    {
        *cur = session.to_string();
    }
}

fn drain_session() -> String {
    CURRENT_SESSION
        .lock()
        .ok()
        .map(|g| g.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "nosession".to_string())
}

/// Info snapshot for `id` under the current session (or the pre-session
/// `"nosession"` key, so first-turn tasks still resolve after resume).
fn info_for(
    id: gray_tools::shell::contract::TaskId,
) -> Option<gray_tools::shell::contract::TaskInfo> {
    let reg = gray_tools::shell::registry::registry();
    let cur = drain_session();
    reg.get(&cur, id).or_else(|| {
        (cur != "nosession")
            .then(|| reg.get("nosession", id))
            .flatten()
    })
}

/// Ours iff the id is known under the current session (or `"nosession"`).
fn ours(id: gray_tools::shell::contract::TaskId) -> bool {
    info_for(id).is_some()
}

/// Spawn the session drain: the process's long-lived wake subscription.
/// Coalesces 500 ms windows; `Lagged(n)` becomes the brief's drop line.
pub fn spawn_shell_drain() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = gray_tools::shell::registry::registry()
            .wake_tx()
            .subscribe();
        loop {
            let first = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    queue_shell_wake(format!(
                        "[shell] {n} task events were dropped; shell_output() to list"
                    ));
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let deadline = Instant::now() + WAKE_COALESCE;
            let mut batch = vec![first];
            let mut dropped: Option<u64> = None;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Ok(ev)) => batch.push(ev),
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                        dropped = Some(n);
                        break;
                    }
                    _ => break,
                }
            }
            let mut lines = Vec::new();
            if let Some(n) = dropped {
                lines.push(format!(
                    "[shell] {n} task events were dropped; shell_output() to list"
                ));
            }
            for ev in &batch {
                match ev {
                    gray_tools::shell::contract::WakeEvent::Exited { id, .. }
                    | gray_tools::shell::contract::WakeEvent::PatternMatched { id, .. } => {
                        if !ours(*id) {
                            continue;
                        }
                        if !gray_tools::shell::wake::should_inject(ev) {
                            continue;
                        }
                        if let Some(info) = info_for(*id) {
                            lines.push(gray_tools::shell::wake::format_wake(ev, &info));
                        }
                    }
                    // UserInput only wakes sleep (never a turn).
                    gray_tools::shell::contract::WakeEvent::UserInput => {}
                }
            }
            if lines.is_empty() {
                continue;
            }
            // Shown as a system notice + synthetic turn at the loop-top
            // (never as if the user typed it).
            queue_shell_wake(lines.join("\n"));
        }
    })
}

/// Shutdown one registry session with a 3 s deadline. Returns background
/// tasks stopped (for the `"stopped N background tasks"` quit line).
pub async fn shutdown_shell_session(session: &str) -> usize {
    let reg = gray_tools::shell::registry::registry();
    let running = reg
        .list(session)
        .iter()
        .filter(|t| matches!(t.state, gray_tools::shell::contract::TaskState::Running))
        .count();
    let _ = tokio::time::timeout(SHUTDOWN_DEADLINE, reg.shutdown_session(session)).await;
    running
}

/// 2E startup sweep: delete `~/.gray/shell/*/t*.log` older than 7 days or
/// bigger than 10MiB (size catches pre-cap runaway logs the pump now stops).
pub fn sweep_old_shell_logs() {
    let Ok(home) = crate::setup::gray_home() else {
        return;
    };
    let base = home.join("shell");
    let now = std::time::SystemTime::now();
    let Ok(sessions) = std::fs::read_dir(&base) else {
        return;
    };
    for sess in sessions.flatten() {
        let Ok(files) = std::fs::read_dir(sess.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('t') || !name.ends_with(".log") {
                continue;
            }
            let meta = f.metadata().ok();
            let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let age = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| now.duration_since(t).ok());
            if sweep_due(len, age) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

// UNRUN (cargo test banned under X per AGENTS.md; verify in TTY/CI).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_queue_drops_oldest_past_cap() {
        let mut q = Vec::new();
        for i in 0..WAKE_QUEUE_CAP + 5 {
            capped_push(&mut q, format!("m{i}"));
        }
        assert_eq!(q.len(), WAKE_QUEUE_CAP);
        assert_eq!(q.first().unwrap(), "m5");
        assert_eq!(
            q.last().unwrap().as_str(),
            format!("m{}", WAKE_QUEUE_CAP + 4).as_str()
        );
    }

    #[test]
    fn sweep_due_covers_age_and_size() {
        assert!(sweep_due(0, Some(LOG_SWEEP_AGE + Duration::from_secs(1))));
        assert!(!sweep_due(0, Some(LOG_SWEEP_AGE)));
        assert!(!sweep_due(0, None));
        assert!(sweep_due(SHELL_LOG_MAX_BYTES + 1, None));
        assert!(!sweep_due(SHELL_LOG_MAX_BYTES, None));
    }
}
