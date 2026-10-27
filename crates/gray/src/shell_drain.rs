use std::time::Duration;

pub const SHELL_TOOL_TIMEOUT: Duration = Duration::from_secs(3660);

const LOG_SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

const SHELL_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn sweep_due(len: u64, age: Option<Duration>) -> bool {
    len > SHELL_LOG_MAX_BYTES || age.is_some_and(|a| a > LOG_SWEEP_AGE)
}

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
        let dir_age = sess
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok());
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

        if dir_age.is_some_and(|a| a > LOG_SWEEP_AGE) {
            let _ = std::fs::remove_dir(sess.path());
        }
    }
}
