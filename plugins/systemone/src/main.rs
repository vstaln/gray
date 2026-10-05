//! gray-systemone: sidecar plugin for System One decision models.
//! NDJSON over stdio, sidecar protocol 1.1. One POST /v1/systemone wire
//! covers every backend; SYSTEMONE_BASE_URL picks which.

mod client;
mod config;
mod find;
mod judge;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

use config::Config;

fn manifest() -> Value {
    json!({
        "name": "systemone",
        "version": "0.1.0",
        "protocol": "1.1",
        "tools": [
            {
                "name": "judge",
                "description": "Ask a System One decision model typed questions about a state \
                     and get probabilities back, not prose. Cheap and fast: use it to classify, \
                     route, rank, verify, or answer yes-no checks. Batch independent questions \
                     in one call — each key of `questions` is one typed question (choice = pick \
                     an option, score = rate on criteria levels, noul = yes/no probability). \
                     The model cannot generate text; it only returns structured answers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "state": {
                            "description": "The state to judge: a string, object, or array.",
                        },
                        "questions": {
                            "type": "object",
                            "description": "id → {type: choice|score|noul, instructions, criteria?}",
                        },
                    },
                    "required": ["state", "questions"],
                },
            },
            {
                "name": "semantic_find",
                "description": "Semantic search over files: a System One decision model judges \
                     each chunk of the given files/directories against a plain-language query \
                     and reports the highest-scoring locations. Use when keyword grep is not \
                     enough; narrow the search with paths, a glob, or required keywords. \
                     Note: with a remote backend, file contents leave the machine.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "paths": {"type": "array", "items": {"type": "string"}},
                        "glob": {"type": "string"},
                        "keywords": {"type": "array", "items": {"type": "string"}},
                        "window": {"type": "integer"},
                        "top_k": {"type": "integer"},
                        "threshold": {"type": "number"},
                    },
                    "required": ["query", "paths"],
                },
            },
        ],
        "commands": ["/s1"],
        "hooks": [],
    })
}

fn session_cwd(params: &Value) -> PathBuf {
    params
        .get("session")
        .and_then(|s| s.get("cwd"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn tool_call(params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("args").cloned().unwrap_or(Value::Null);
    let cfg = Config::resolve();
    match name {
        "judge" => match judge::run(&cfg, &args) {
            Ok(content) => json!({"content": content}),
            Err(content) => json!({"content": content, "is_error": true}),
        },
        "semantic_find" => match find::run(&cfg, &args, &session_cwd(params)) {
            Ok(content) => json!({"content": content}),
            Err(content) => json!({"content": content, "is_error": true}),
        },
        other => json!({
            "content": format!("unknown tool `{other}`"),
            "is_error": true,
        }),
    }
}

const HELP: &str = "/s1 status — show backend, model, key, and /v1/models
/s1 noul <question> -- <state text> — one yes/no check, prints p(yes)
env: SYSTEMONE_BASE_URL (or TYPESAFE_BASE_URL; default http://localhost:11435),
SYSTEMONE_MODEL, SYSTEMONE_API_KEY (or TYPESAFE_API_KEY)";

fn status_text(cfg: &Config) -> String {
    let mut out = format!(
        "base: {}\nmodel: {}\nkey: {}\n",
        cfg.base,
        cfg.model,
        if cfg.key.is_some() { "set" } else { "unset" }
    );
    match client::get_models(cfg) {
        Ok(v) => {
            let ids: Vec<String> = v
                .get("data")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("id").and_then(Value::as_str).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                out.push_str("models: (none reported)\n");
            } else {
                out.push_str("models:\n");
                for id in ids {
                    out.push_str(&format!("  {id}\n"));
                }
            }
        }
        Err(e) => out.push_str(&format!("models: {e}\n")),
    }
    out
}

fn noul_text(cfg: &Config, argv: &[String]) -> String {
    let Some(sep) = argv.iter().position(|a| a == "--") else {
        return "usage: /s1 noul <question> -- <state text>".to_string();
    };
    let question = argv[..sep].join(" ");
    let state = argv[sep + 1..].join(" ");
    if question.trim().is_empty() || state.trim().is_empty() {
        return "usage: /s1 noul <question> -- <state text>".to_string();
    }
    let args = json!({
        "state": state,
        "questions": {
            "q": {"type": "noul", "instructions": question}
        },
    });
    match judge::run(cfg, &args) {
        Ok(content) => {
            let v: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
            match v
                .get("answers")
                .and_then(|a| a.get("q"))
                .and_then(judge::answer_probability)
            {
                Some(p) => format!("p(yes)={p:.2}"),
                None => format!("backend answered but no probability found: {content}"),
            }
        }
        Err(e) => e,
    }
}

fn command_run(params: &Value) -> Value {
    let argv: Vec<String> = params
        .get("argv")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cfg = Config::resolve();
    let text = match argv.first().map(String::as_str) {
        None | Some("status") => status_text(&cfg),
        Some("noul") => noul_text(&cfg, &argv[1..]),
        Some(_) => HELP.to_string(),
    };
    json!({"text": text})
}

fn dispatch(req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str)?;
    match method {
        "plugin/manifest" => Some(manifest()),
        "tool/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            Some(tool_call(&params))
        }
        "command/run" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            Some(command_run(&params))
        }
        _ => None,
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue; // malformed lines: ignore
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let id = req.get("id").cloned();
        match id {
            None => {
                if method == "plugin/shutdown" {
                    return;
                }
                continue;
            }
            Some(id) => {
                let mut out = stdout.lock();
                if method == "plugin/shutdown" {
                    let _ = writeln!(out, "{}", json!({"id": id, "result": {}}));
                    let _ = out.flush();
                    return;
                }
                let reply = match dispatch(&req) {
                    Some(result) => json!({"id": id, "result": result}),
                    None => json!({
                        "id": id,
                        "error": {"message": format!("unknown method {method}")},
                    }),
                };
                let _ = writeln!(out, "{reply}");
                let _ = out.flush();
            }
        }
    }
}
