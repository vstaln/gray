//! `activity.jsonl`: what the agent did, decided not to say, and deferred.
//! Append-only, one JSON object per line, rotated to `activity.jsonl.1` past
//! a size cap. Best-effort: a write failure never stops the gateway.

use std::io::Write as _;
use std::path::Path;

const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub fn log(dir: &Path, what: &str, detail: serde_json::Value) {
    let path = dir.join("activity.jsonl");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, dir.join("activity.jsonl.1"));
    }
    let mut row = serde_json::json!({ "at": crate::cron::now_secs(), "what": what });
    if let (Some(map), serde_json::Value::Object(extra)) = (row.as_object_mut(), detail) {
        map.extend(extra);
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = options.open(&path) {
        let _ = writeln!(f, "{row}");
    }
}

/// Last `n` rows, oldest first.
pub fn tail(dir: &Path, n: usize) -> Vec<serde_json::Value> {
    let body = std::fs::read_to_string(dir.join("activity.jsonl")).unwrap_or_default();
    let rows: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let from = rows.len().saturating_sub(n);
    rows[from..].to_vec()
}

/// One human line for `gray gateway activity`.
pub fn render(row: &serde_json::Value) -> String {
    let at = row.get("at").and_then(|v| v.as_i64()).unwrap_or(0);
    let when = chrono::DateTime::from_timestamp(at, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    let what = row.get("what").and_then(|v| v.as_str()).unwrap_or("?");
    let mut rest = Vec::new();
    if let Some(obj) = row.as_object() {
        for (k, v) in obj {
            if k == "at" || k == "what" {
                continue;
            }
            let v = match v {
                serde_json::Value::String(s) => one_line(s, 120),
                other => one_line(&other.to_string(), 120),
            };
            rest.push(format!("{k}={v}"));
        }
    }
    format!("{when}  {what:<12} {}", rest.join(" "))
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    } else {
        flat
    }
}
