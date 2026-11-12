use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct GraySession {
    pub id: String,
    pub timestamp: i64,
    pub cwd: String,
    pub model: String,
    pub preview: String,
    pub live: bool,
    /// Spawned by another gray session (`gray subagents run`).
    pub subagent: bool,
}

const SUBAGENT_PREAMBLE: &str = "You are a subagent of a parent Gray session";

/// One readable line from a session's first prompt: the subagent preamble
/// and markdown headings skipped, whitespace collapsed.
pub fn clean_preview(text: &str) -> (String, bool) {
    let subagent = text.trim_start().starts_with(SUBAGENT_PREAMBLE);
    let body = if subagent {
        // The task proper follows the `# Task` heading.
        text.split_once("# Task").map(|(_, t)| t).unwrap_or(text)
    } else {
        text
    };
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(SUBAGENT_PREAMBLE))
        .unwrap_or("");
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    (line.chars().take(120).collect(), subagent)
}

/// Whether session `id` is open in a running gray right now.
pub fn is_live(id: &str) -> bool {
    sessions_dir().is_some_and(|d| lock_held(&d.join(format!("{id}.open"))))
}

/// gray holds an exclusive lock on `<id>.open` while the session is open;
/// a leftover file from a crashed or killed session is unlocked.
fn lock_held(open: &std::path::Path) -> bool {
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(open)
    else {
        return false;
    };
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

pub fn sessions_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return Some(PathBuf::from(h).join("sessions"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".gray/sessions"))
}

pub fn list() -> Vec<GraySession> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.extension().map(|x| x == "jsonl").unwrap_or(false) {
            continue;
        }
        // Stream: session logs can be large and only the head is needed.
        let Ok(file) = std::fs::File::open(&p) else {
            continue;
        };
        let mut lines = BufReader::new(file).lines().map_while(Result::ok);
        let Some(head) = lines.next() else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&head) else {
            continue;
        };
        if v.get("version").is_none() {
            continue;
        }
        let first_prompt = lines
            .take(400)
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
            .find(|e| e["message"]["role"] == "user")
            .and_then(|e| e["message"]["content"].as_array().cloned())
            .and_then(|c| c.iter().find_map(|p| p["text"].as_str().map(String::from)))
            .unwrap_or_default();
        let (preview, subagent) = clean_preview(&first_prompt);
        let open = p.with_extension("open");
        let live = open.exists() && lock_held(&open);
        out.push(GraySession {
            id: v["id"].as_str().unwrap_or_default().to_string(),
            timestamp: v["timestamp"].as_i64().unwrap_or(0),
            cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
            model: v["model"].as_str().unwrap_or_default().to_string(),
            preview,
            live,
            subagent,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    out.truncate(200);
    out
}

#[cfg(test)]
mod tests {
    use super::clean_preview;

    #[test]
    fn subagent_preamble_is_skipped() {
        let (p, sub) = clean_preview(
            "You are a subagent of a parent Gray session.\n\n# Task\n\nYou are building the   works\npage",
        );
        assert!(sub);
        assert_eq!(p, "You are building the works");
    }

    #[test]
    fn plain_prompt_is_one_line() {
        let (p, sub) = clean_preview("# Heading\n  fix   the bug\nmore");
        assert!(!sub);
        assert_eq!(p, "fix the bug");
    }
}
