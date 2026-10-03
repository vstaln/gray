//! Restart and shutdown notices for any chat adapter (hermes
//! `gateway_restart_notification` parity), kept platform-agnostic: core owns
//! the record, the crash detection and the wording; an adapter (Discord
//! today, Telegram or Slack tomorrow) only posts what it is handed.
//!
//! An adapter keeps two files in its own state dir:
//! - `lifecycle.json`: `running` while its daemon is up, `stopped` after a
//!   clean exit. Still `running` at the next boot means the last run died
//!   (crash, SIGKILL, OOM, reboot).
//! - `restart_pending`: left by a restart command before it signals the
//!   daemon, so the SIGTERM reads as "restarting" rather than "shutting
//!   down" (hermes' `.restart_pending.json`). The next boot removes it.
//!
//! Adapters drive it through `gray gateway lifecycle boot|stop|restart
//! --dir <state dir>`, which answers JSON; the gateway daemon itself uses
//! the same record for its own runs.

use serde_json::{Value, json};
use std::path::Path;

pub const STATE_FILE: &str = "lifecycle.json";
pub const RESTART_MARKER: &str = "restart_pending";

/// How the previous run ended, read once at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Previous {
    /// No record: first boot, nothing to announce.
    FirstBoot,
    /// Exited on a signal after saying so.
    Clean { restart: bool },
    /// Still marked running: crash, SIGKILL, OOM, or power loss.
    Crashed,
}

impl Previous {
    pub fn as_str(self) -> &'static str {
        match self {
            Previous::FirstBoot => "first_boot",
            Previous::Clean { restart: true } => "restart",
            Previous::Clean { restart: false } => "clean",
            Previous::Crashed => "crashed",
        }
    }
}

/// Reads how the last run ended, then records this one as running.
pub fn boot(dir: &Path) -> Previous {
    let restart_marker = dir.join(RESTART_MARKER);
    let restart_requested = restart_marker.exists();
    let _ = std::fs::remove_file(&restart_marker);
    let previous = std::fs::read(dir.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let previous = match previous {
        None => Previous::FirstBoot,
        Some(state) => match state.get("state").and_then(Value::as_str) {
            Some("stopped") => Previous::Clean {
                restart: restart_requested
                    || state.get("restart").and_then(Value::as_bool) == Some(true),
            },
            _ => Previous::Crashed,
        },
    };
    write(
        dir,
        json!({"state": "running", "at": crate::cron::now_secs()}),
    );
    previous
}

/// Whether a restart (not a plain stop) was asked for.
pub fn restart_requested(dir: &Path) -> bool {
    dir.join(RESTART_MARKER).exists()
}

/// Called by a restart command before the old process is signalled.
pub fn request_restart(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(RESTART_MARKER), b"")?;
    Ok(())
}

/// Called by a plain stop: a marker left by a restart that never happened
/// must not turn this stop into "restarting".
pub fn clear_restart(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(RESTART_MARKER));
}

/// Records a clean exit; the next boot announces "online", not "crashed".
/// Returns whether this exit is a restart.
pub fn mark_stopped(dir: &Path) -> bool {
    let restart = restart_requested(dir);
    write(
        dir,
        json!({"state": "stopped", "restart": restart, "at": crate::cron::now_secs()}),
    );
    restart
}

fn write(dir: &Path, state: Value) {
    let _ = std::fs::create_dir_all(dir);
    let _ = crate::cron::store::atomic_write_json(&dir.join(STATE_FILE), &state);
}

/// The home-channel notice for this boot; `interrupted` is how many turns
/// were running when the last run ended.
pub fn startup_notice(previous: Previous, interrupted: usize) -> Option<String> {
    let mut text = match previous {
        Previous::FirstBoot => return None,
        Previous::Clean { restart: true } => "Gateway restarted. gray is back and ready.",
        Previous::Clean { restart: false } => "Gateway online. gray is back and ready.",
        Previous::Crashed => {
            "Gateway is back after an unexpected stop (crash, kill, or reboot). gray is ready."
        }
    }
    .to_string();
    match interrupted {
        0 => {}
        1 => text.push_str("\n1 turn was cut short; send a message in its chat to continue."),
        n => text.push_str(&format!(
            "\n{n} turns were cut short; send a message in their chats to continue."
        )),
    }
    Some(text)
}

/// Sent to each chat whose turn is about to be interrupted.
pub fn active_notice(restart: bool) -> &'static str {
    if restart {
        "Gateway restarting: your current task will be interrupted. Send any message after the restart to pick up where you left off."
    } else {
        "Gateway shutting down: your current task will be interrupted. Once it is back online, send any message to pick up where you left off."
    }
}

/// Sent to the home channel when nothing is running there.
pub fn home_notice(restart: bool) -> &'static str {
    if restart {
        "Gateway restarting. Back in a moment."
    } else {
        "Gateway shutting down."
    }
}

/// `gray gateway lifecycle boot`: the record moves to running; the answer
/// says how the last run ended and what the home channel should hear.
pub fn boot_json(dir: &Path, interrupted: usize) -> Value {
    let previous = boot(dir);
    json!({
        "previous": previous.as_str(),
        "notice": startup_notice(previous, interrupted),
    })
}

/// `gray gateway lifecycle stop`: the record moves to stopped; the answer
/// carries what chats with a running turn and the home channel should hear.
pub fn stop_json(dir: &Path) -> Value {
    let restart = mark_stopped(dir);
    json!({
        "restart": restart,
        "active_notice": active_notice(restart),
        "home_notice": home_notice(restart),
    })
}

#[path = "lifecycle_tests.rs"]
#[cfg(test)]
mod tests;
