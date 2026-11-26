//! Prompt history persistence: submitted composer prompts are kept in a
//! JSON-lines file under the gray home so up-arrow / ctrl-r history survives
//! restarts.

use std::path::{Path, PathBuf};

const CAP: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Entry {
    pub text: String,
    pub attachments: Vec<PathBuf>,
}

fn path() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|h| h.join("history.jsonl"))
}

pub(crate) fn load() -> Vec<Entry> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

fn load_from(path: &Path) -> Vec<Entry> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            serde_json::from_str::<Entry>(l).ok().or_else(|| {
                serde_json::from_str::<String>(l).ok().map(|text| Entry {
                    text,
                    attachments: Vec::new(),
                })
            })
        })
        .collect();
    if out.len() > CAP {
        out.drain(..out.len() - CAP);
    }
    out
}

/// Append `entry` to `history`, skipping empties and consecutive duplicates
/// and keeping at most CAP entries. Returns whether the history changed.
fn push(history: &mut Vec<Entry>, entry: Entry) -> bool {
    if (entry.text.is_empty() && entry.attachments.is_empty()) || history.last() == Some(&entry) {
        return false;
    }
    history.push(entry);
    if history.len() > CAP {
        let over = history.len() - CAP;
        history.drain(..over);
    }
    true
}

/// Append `entry` to `history` and persist it under the gray home.
pub(crate) fn record(history: &mut Vec<Entry>, text: &str, attachments: &[PathBuf]) {
    if !push(
        history,
        Entry {
            text: text.to_string(),
            attachments: attachments.to_vec(),
        },
    ) {
        return;
    }
    if let Some(p) = path() {
        save_to(&p, history);
    }
}

fn save_to(path: &Path, history: &[Entry]) {
    let mut body = String::new();
    for h in history {
        let line = if h.attachments.is_empty() {
            serde_json::to_string(&h.text)
        } else {
            serde_json::to_string(h)
        };
        if let Ok(line) = line {
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
