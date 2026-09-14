//! Shell log upkeep for blocking-only bash.
//!
//! One tool, no background tasks: each `bash` call writes its full log to
//! `$GRAY_HOME/shell/<session>/bash-<uuid>.log` and the result header names
//! the path (grep it instead of rerunning). This module keeps the agent
//! tool timeout above bash's max and sweeps stale/oversized logs at startup.

use std::time::Duration;

/// Bash self-bounds at 600 s, so agents need headroom above that.
pub const SHELL_TOOL_TIMEOUT: Duration = Duration::from_secs(610);
/// Startup sweep: logs older than 7 days go.
const LOG_SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Shell transcript cap per log file: matches pump + gray.log 10MiB. The
/// age sweep alone let real usage reach 208MB; the sweep deletes oversized
/// files too (pre-cap leftovers), the pump stops live writes past this.
const SHELL_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Pure sweep check (unit-testable): age sweep plus size cap for pre-cap
/// oversized logs.
fn sweep_due(len: u64, age: Option<Duration>) -> bool {
    len > SHELL_LOG_MAX_BYTES || age.is_some_and(|a| a > LOG_SWEEP_AGE)
}

/// Startup sweep: delete `~/.gray/shell/*/*.log` older than 7 days or
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
            if !name.ends_with(".log") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_due_covers_age_and_size() {
        assert!(sweep_due(0, Some(LOG_SWEEP_AGE + Duration::from_secs(1))));
        assert!(!sweep_due(0, Some(LOG_SWEEP_AGE)));
        assert!(!sweep_due(0, None));
        assert!(sweep_due(SHELL_LOG_MAX_BYTES + 1, None));
        assert!(!sweep_due(SHELL_LOG_MAX_BYTES, None));
    }
}
