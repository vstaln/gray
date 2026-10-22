//! Prompt history persistence: submitted composer prompts are kept in a
//! JSON-lines file under the gray home so up-arrow / ctrl-r history survives
//! restarts. One JSON string per line so multi-line drafts round-trip.

use std::path::{Path, PathBuf};

const CAP: usize = 500;

fn path() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|h| h.join("history.jsonl"))
}

pub(crate) fn load() -> Vec<String> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

fn load_from(path: &Path) -> Vec<String> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<String>(l).ok())
        .collect();
    if out.len() > CAP {
        out.drain(..out.len() - CAP);
    }
    out
}

/// Append `entry` to `history`, skipping empties and consecutive duplicates
/// and keeping at most CAP entries. Returns whether the history changed.
fn push(history: &mut Vec<String>, entry: &str) -> bool {
    if entry.is_empty() || history.last().is_some_and(|l| l == entry) {
        return false;
    }
    history.push(entry.to_string());
    if history.len() > CAP {
        let over = history.len() - CAP;
        history.drain(..over);
    }
    true
}

/// Append `entry` to `history` and persist it under the gray home.
pub(crate) fn record(history: &mut Vec<String>, entry: &str) {
    if !push(history, entry) {
        return;
    }
    if let Some(p) = path() {
        save_to(&p, history);
    }
}

fn save_to(path: &Path, history: &[String]) {
    let mut body = String::new();
    for h in history {
        if let Ok(line) = serde_json::to_string(h) {
            body.push_str(&line);
            body.push('\n');
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if std::fs::write(&tmp, body).is_err() {
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}
