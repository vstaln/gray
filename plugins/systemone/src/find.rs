//! `semantic_find` tool: a decision model scores file chunks against a
//! plain-language query; cheaper than grep when keywords are not enough.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::client;
use crate::config::Config;
use crate::judge;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_CHUNKS: usize = 256;
const BATCH: usize = 32;
const TIME_BUDGET: Duration = Duration::from_secs(22);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Don't start a batch with less than this left: even an instant reply
/// leaves no slack to do anything with it before the host's 30s TTL.
const MIN_BATCH_BUDGET: Duration = Duration::from_secs(3);

/// Per-request timeout for the next batch: the lesser of the normal
/// 20s timeout and the remaining time budget. `None` when too little
/// remains to be worth sending.
fn request_timeout(remaining: Duration) -> Option<Duration> {
    if remaining < MIN_BATCH_BUDGET {
        return None;
    }
    Some(remaining.min(REQUEST_TIMEOUT))
}

struct Chunk {
    path: String,
    start: usize,
    end: usize,
    text: String,
}

/// Basenames that must never leave the machine via a remote judge.
fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with(".env")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.starts_with("id_rsa")
        || n.starts_with("id_ed25519")
        || n == "auth.json"
        || n == "gateway.yaml"
        || n.ends_with(".p12")
}

fn resolve_path(session_cwd: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        session_cwd.join(path)
    }
}

/// Expand `paths` into candidate files: directories go through
/// `rg --files` (respects .gitignore), explicit files are used directly.
fn list_files(
    session_cwd: &Path,
    paths: &[String],
    glob: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for raw in paths {
        let path = resolve_path(session_cwd, raw);
        if path.is_dir() {
            let mut cmd = std::process::Command::new("rg");
            cmd.arg("--files").current_dir(session_cwd);
            if let Some(g) = glob {
                cmd.arg("-g").arg(g);
            }
            cmd.arg(&path);
            let out = cmd.output().map_err(|e| {
                format!(
                    "semantic_find: could not run `rg` ({e}) — install ripgrep or pass file paths"
                )
            })?;
            if !out.status.success() && out.stdout.is_empty() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!(
                    "semantic_find: `rg --files` failed: {}",
                    stderr.trim()
                ));
            }
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                files.push(resolve_path(session_cwd, line));
            }
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

/// Read a file into chunkable text, or None when it must be skipped
/// (missing, too big, binary/non-UTF-8, or a secret-ish name).
fn read_candidate(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if is_secret_name(&name) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn chunk_file(path: &str, text: &str, window: usize) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let end = (i + window).min(lines.len());
        chunks.push(Chunk {
            path: path.to_string(),
            start: i + 1,
            end,
            text: lines[i..end].join("\n"),
        });
        i = end;
    }
    chunks
}

fn matches_keywords(text: &str, keywords: &[String]) -> bool {
    let lower = text.to_ascii_lowercase();
    keywords
        .iter()
        .any(|k| lower.contains(&k.to_ascii_lowercase()))
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn preview(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .map(|l| l.trim().chars().take(160).collect())
        .collect()
}

/// Run the tool against the session cwd. `Err(msg)` is `is_error` content.
pub fn run(cfg: &Config, args: &Value, session_cwd: &Path) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or("semantic_find: missing required `query` string")?;
    let paths: Vec<String> = match args.get("paths").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        None => return Err("semantic_find: missing required `paths` array".into()),
    };
    if paths.is_empty() {
        return Err("semantic_find: `paths` must list at least one file or directory".into());
    }
    let glob = args.get("glob").and_then(Value::as_str);
    let keywords: Vec<String> = args
        .get("keywords")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let window = arg_u64(args, "window")
        .map(|w| w as usize)
        .unwrap_or(40)
        .clamp(5, 120);
    let top_k = arg_u64(args, "top_k")
        .map(|k| k as usize)
        .unwrap_or(10)
        .min(50);
    let threshold = arg_f64(args, "threshold").unwrap_or(0.5);

    let files = list_files(session_cwd, &paths, glob)?;
    let mut chunks: Vec<Chunk> = Vec::new();
    for path in &files {
        let Some(text) = read_candidate(path) else {
            continue;
        };
        let display = path
            .strip_prefix(session_cwd)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        for c in chunk_file(&display, &text, window) {
            // Stable order: file order, then line order.
            if !keywords.is_empty() && !matches_keywords(&c.text, &keywords) {
                continue;
            }
            chunks.push(c);
        }
    }

    if chunks.is_empty() {
        return Ok(format!(
            "semantic_find: no candidate chunks (searched {} file(s) under given paths; \
             skipped oversized/binary/secret-named files)",
            files.len()
        ));
    }

    let total = chunks.len();
    let skipped_by_cap = total.saturating_sub(MAX_CHUNKS);
    let mut scored: Vec<(usize, f64)> = Vec::new();
    let mut judged = 0usize;
    let deadline = Instant::now() + TIME_BUDGET;
    let mut model_used = cfg.model.clone();

    let mut stopped_early: Option<String> = None;
    for batch in chunks
        .iter()
        .take(MAX_CHUNKS)
        .collect::<Vec<_>>()
        .chunks(BATCH)
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(timeout) = request_timeout(remaining) else {
            break;
        };
        let mut questions = serde_json::Map::new();
        for (offset, c) in batch.iter().enumerate() {
            let id = format!("c{}", judged + offset);
            questions.insert(
                id,
                json!({
                    "type": "noul",
                    "instructions": {
                        "chunk": format!("{}:{}-{}\n{}", c.path, c.start, c.end, c.text),
                        "question": "Does `chunk` contain code or text that answers, \
                                     implements, or is directly about `query`?",
                    },
                }),
            );
        }
        let body = json!({
            "state": {"query": query},
            "model": cfg.model,
            "questions": Value::Object(questions),
        });
        let resp = match client::post_systemone(cfg, &body, timeout) {
            Ok(r) => r,
            Err(e) => {
                if scored.is_empty() && judged == 0 {
                    return Err(e);
                }
                // Partial coverage beats losing the whole result.
                stopped_early = Some(e);
                break;
            }
        };
        if let Some(m) = resp.get("model").and_then(Value::as_str) {
            model_used = m.to_string();
        }
        let answers = resp.get("answers").cloned().unwrap_or(Value::Null);
        for (offset, _) in batch.iter().enumerate() {
            let id = format!("c{}", judged + offset);
            let idx = judged + offset;
            if let Some(p) = answers.get(&id).and_then(judge::answer_probability) {
                scored.push((idx, p));
            }
        }
        judged += batch.len();
    }

    let unjudged = chunks.len().min(MAX_CHUNKS) - judged;
    let stopped_note = stopped_early
        .map(|e| format!("; stopped early: {e}"))
        .unwrap_or_default();
    let mut out = format!(
        "semantic_find: judged {judged}/{total} chunks ({skipped_by_cap} skipped by cap, \
         {unjudged} unjudged by time budget) with {model_used} @ {}{stopped_note}\n",
        cfg.base
    );

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let hits: Vec<&(usize, f64)> = scored.iter().filter(|(_, p)| *p >= threshold).collect();
    let show: Vec<(usize, f64)> = if hits.is_empty() {
        out.push_str(&format!(
            "no chunks cleared threshold {threshold:.2}; best 3 by score:\n"
        ));
        scored.iter().take(3).copied().collect()
    } else {
        hits.iter().take(top_k).map(|h| **h).collect()
    };
    for (idx, p) in show {
        let c = &chunks[idx];
        out.push_str(&format!("{}:{}-{}  p={p:.2}\n", c.path, c.start, c.end));
        for line in preview(&c.text) {
            out.push_str(&format!("    {line}\n"));
        }
    }
    Ok(out)
}

#[path = "find_tests.rs"]
#[cfg(test)]
mod tests;
