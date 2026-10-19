use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct GraySession {
    pub id: String,
    pub timestamp: i64,
    pub cwd: String,
    pub model: String,
    pub preview: String,
    pub live: bool,
}

pub fn sessions_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("GRAY_USER_HOME") {
        return Some(PathBuf::from(h).join("sessions"));
    }
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
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let mut lines = text.lines();
        let Some(head) = lines.next() else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(head) else {
            continue;
        };
        if v.get("version").is_none() {
            continue;
        }
        let preview = lines
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|e| e["message"]["role"] == "user")
            .and_then(|e| e["message"]["content"].as_array().cloned())
            .and_then(|c| c.iter().find_map(|p| p["text"].as_str().map(String::from)))
            .unwrap_or_default();
        let live = p.with_extension("open").exists();
        out.push(GraySession {
            id: v["id"].as_str().unwrap_or_default().to_string(),
            timestamp: v["timestamp"].as_i64().unwrap_or(0),
            cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
            model: v["model"].as_str().unwrap_or_default().to_string(),
            preview: preview.chars().take(80).collect(),
            live,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    out.truncate(200);
    out
}
