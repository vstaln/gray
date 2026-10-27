use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct CronCard {
    pub name: String,
    pub body: String,
    pub failed: bool,
    pub reminder: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionDelivery {
    pub card: String,
    pub prompt: String,
    pub cron: Option<CronCard>,
}

/// Platform tag written into a cron job's `origin` when it was created from a
/// REPL session — the tag `jobs.rs` uses to pick out this session's wakes.
pub const SESSION_PLATFORM: &str = "repl";

pub(crate) fn session_inbox(home: &std::path::Path, session: &str) -> Option<PathBuf> {
    let safe = !session.is_empty()
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| home.join("cron").join("inbox").join(session))
}

pub(crate) fn write_entry(dir: &std::path::Path, body: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = format!("{stamp:020}-{}.json", uuid::Uuid::new_v4());
    let tmp = dir.join(format!(".{name}.tmp"));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&tmp)?, body.as_bytes())?;
    std::fs::rename(&tmp, dir.join(name))?;
    Ok(())
}

pub fn session_inbox_pending(home: &std::path::Path, session: &str) -> bool {
    session_inbox(home, session)
        .and_then(|d| std::fs::read_dir(d).ok())
        .is_some_and(|mut it| {
            it.any(|e| {
                let Ok(e) = e else { return false };
                if e.file_name().to_string_lossy().starts_with('.') {
                    return false;
                }

                !std::fs::read_to_string(e.path()).is_ok_and(|t| crate::session_send::is_held(&t))
            })
        })
}

pub fn drain_session_inbox(home: &std::path::Path, session: &str) -> Vec<SessionDelivery> {
    let Some(dir) = session_inbox(home, session) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path);

        if text
            .as_deref()
            .is_ok_and(crate::session_send::is_send_entry)
        {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("cron inbox: cannot remove {}: {e}", path.display());
            continue;
        }
        let parsed = text
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                let field = |k: &str| v.get(k)?.as_str().map(str::to_string);
                let cron = field("name").map(|name| CronCard {
                    name,
                    body: field("body").unwrap_or_default(),
                    failed: field("status").as_deref() == Some("failed"),
                    reminder: v.get("reminder").and_then(|r| r.as_bool()).unwrap_or(false),
                    elapsed_ms: v.get("elapsed_ms").and_then(|e| e.as_u64()).unwrap_or(0),
                });
                Some(SessionDelivery {
                    card: field("card")?,
                    prompt: field("prompt")?,
                    cron,
                })
            });
        match parsed {
            Some(entry) => out.push(entry),
            None => log::warn!("cron inbox: dropped unreadable {}", path.display()),
        }
    }
    out
}
