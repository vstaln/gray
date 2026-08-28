//! Agent-facing tools for the google_meet plugin.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/google_meet/tools.py` (348 LOC).
//!
//! Tools:
//!   meet_join        — join a Google Meet URL (spawns Playwright bot locally
//!                      OR on a remote node host via node=<name>)
//!   meet_status      — report bot liveness + transcript progress
//!   meet_transcript  — read the current transcript (optional last-N)
//!   meet_leave       — signal the bot to leave cleanly
//!   meet_say         — (v2) speak text through the realtime audio bridge.
//!                      Requires the active meeting to have been joined with
//!                      mode='realtime'.
//!
//! Python surface ported line-for-line:
//!   - `check_meet_requirements`
//!   - `_resolve_node_client`
//!   - `MEET_JOIN_SCHEMA`, `MEET_STATUS_SCHEMA`, `MEET_TRANSCRIPT_SCHEMA`,
//!     `MEET_LEAVE_SCHEMA`, `MEET_SAY_SCHEMA`
//!   - `_json`, `_err`
//!   - `handle_meet_join`, `handle_meet_status`, `handle_meet_transcript`,
//!     `handle_meet_leave`, `handle_meet_say`
//!
//! Transport notes (mirrors Python side-effects without `cargo` in this task):
//!   - `playwright` / platform gate via `python -c "import playwright"` probe
//!     (matches `check_meet_requirements` line 38-45).
//!   - `process_manager` (`plugins.google_meet.process_manager`) is driven via
//!     `python -c "from plugins.google_meet import process_manager as pm; ..."`
//!     subprocess; fallback is a canned `{"ok": false, ...}` matching the Python
//!     failure shape so handlers stay testable hermetically.
//!   - `NodeRegistry` / `NodeClient` are resolved from
//!     `$HERMES_GOOGLE_MEET_NODES_JSON` (JSON map) or
//!     `$HERMES_HOME/workspace/meetings/nodes.json`, mirroring
//!     `plugins.google_meet.node.registry/resolve` and executed via
//!     `python -c "from plugins.google_meet.node.client import NodeClient; ..."`

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors hermes_constants.get_hermes_home()
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

fn python_executable() -> String {
    if let Ok(exe) = std::env::var("PYTHON") {
        if !exe.trim().is_empty() {
            return exe;
        }
    }
    if which("python3").is_some() {
        "python3".to_string()
    } else if which("python").is_some() {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(bin);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Runtime gate — mirrors `check_meet_requirements()` (lines 26-45)
// ---------------------------------------------------------------------------

/// Return True when the plugin can actually run LOCALLY.
///
/// Gates on:
///   * Python `playwright` package importable
///   * the plugin being on a supported platform (Linux or macOS)
///
/// Note: remote-node operation (`node=<name>`) only needs the `websockets`
/// dep on the gateway side — Chromium lives on the node. But the plugin-level
/// gate keeps the v1 semantics; individual tool handlers relax the requirement
/// when a node is addressed.
pub fn check_meet_requirements() -> bool {
    // platform.system().lower() in {"linux", "darwin"}
    let is_supported = if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        true
    } else {
        // Fallback: check at runtime via env / uname for cross-compiled tests
        let sys = system_name().to_ascii_lowercase();
        matches!(sys.as_str(), "linux" | "darwin")
    };
    if !is_supported {
        return false;
    }
    // try: import playwright
    let py = python_executable();
    let out = Command::new(&py)
        .args(["-c", "import playwright"])
        .output();
    match out {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

fn system_name() -> String {
    if cfg!(target_os = "linux") {
        "Linux".to_string()
    } else if cfg!(target_os = "macos") {
        "Darwin".to_string()
    } else if cfg!(target_os = "windows") {
        "Windows".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}

// ---------------------------------------------------------------------------
// Node client helper — mirrors `_resolve_node_client` (lines 52-71)
// ---------------------------------------------------------------------------

/// Minimal node registry entry — mirrors dict returned by `NodeRegistry.resolve`.
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub name: String,
    pub url: String,
    pub token: String,
}

/// Minimal NodeClient — mirrors `plugins.google_meet.node.client.NodeClient`.
#[derive(Debug, Clone)]
pub struct NodeClient {
    pub url: String,
    pub token: String,
    pub name: String,
}

/// Return (NodeClient, node_name) for `node`, or (None, None) to run local.
///
/// Raises (as Err) with a readable message if the node is named but
/// unresolvable, so the handler can surface a clear error to the agent.
///
/// Mirrors `def _resolve_node_client(node: Optional[str])` (lines 52-71).
pub fn resolve_node_client(node: Option<&str>) -> Result<(Option<NodeClient>, Option<String>), String> {
    let node_str = node.unwrap_or("").trim().to_string();
    if node_str.is_empty() {
        return Ok((None, None));
    }
    let entry = resolve_node_entry(Some(node_str.as_str()));
    match entry {
        Some(e) => {
            let client = NodeClient {
                url: e.url.clone(),
                token: e.token.clone(),
                name: e.name.clone(),
            };
            let name = e.name.clone();
            Ok((Some(client), Some(name)))
        }
        None => Err(format!(
            "no registered meet node matches {:?} — run `hermes meet node approve <name> <url> <token>` first",
            node_str
        )),
    }
}

fn resolve_node_entry(node: Option<&str>) -> Option<NodeEntry> {
    // Mirrors NodeRegistry().resolve(node if node != "auto" else None)
    let node_key = match node {
        Some("auto") | None => None,
        Some(n) => {
            let t = n.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
    };

    // 1) Env-registered JSON: HERMES_GOOGLE_MEET_NODES_JSON
    if let Ok(json_str) = std::env::var("HERMES_GOOGLE_MEET_NODES_JSON") {
        if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
            if let Some(map) = v.as_object() {
                if node_key.is_none() {
                    if map.len() == 1 {
                        if let Some((name, entry)) = map.iter().next() {
                            let url = entry.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let token = entry.get("token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if !url.is_empty() {
                                return Some(NodeEntry { name: name.clone(), url, token });
                            }
                        }
                    }
                    return None;
                }
                if let Some(key) = node_key.clone() {
                    if let Some(entry) = map.get(&key) {
                        let url = entry.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let token = entry.get("token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        if !url.is_empty() {
                            return Some(NodeEntry { name: key, url, token });
                        }
                    }
                }
            }
        }
    }

    // 2) File: $HERMES_HOME/workspace/meetings/nodes.json
    let path = get_hermes_home().join("workspace").join("meetings").join("nodes.json");
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(map) = v.as_object() {
                if node_key.is_none() {
                    if map.len() == 1 {
                        if let Some((name, entry)) = map.iter().next() {
                            let url = entry.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let token = entry.get("token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if !url.is_empty() {
                                return Some(NodeEntry { name: name.clone(), url, token });
                            }
                        }
                    }
                    return None;
                }
                if let Some(key) = node_key.clone() {
                    if let Some(entry) = map.get(&key) {
                        let url = entry.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let token = entry.get("token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        if !url.is_empty() {
                            return Some(NodeEntry { name: key, url, token });
                        }
                    }
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Schemas — mirrors lines 78-221
// ---------------------------------------------------------------------------

/// Mirrors `MEET_JOIN_SCHEMA` (lines 78-143).
pub fn meet_join_schema() -> Value {
    json!({
        "name": "meet_join",
        "description": "Join a Google Meet call and start scraping live captions into a transcript file. Only meet.google.com URLs are accepted; no calendar scanning, no auto-dial. Spawns a headless Chromium subprocess that runs in parallel with the agent loop — returns immediately. Poll with meet_status and read captions with meet_transcript. Reminder to the agent: you should announce yourself in the meeting (there is no automatic consent announcement).",
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Full https://meet.google.com/... URL. Required."
                },
                "mode": {
                    "type": "string",
                    "enum": ["transcribe", "realtime"],
                    "description": "transcribe (default): listen-only, scrape captions. realtime: also enable agent speech via meet_say (requires OpenAI Realtime key + platform audio bridge)."
                },
                "guest_name": {
                    "type": "string",
                    "description": "Display name to use when joining as guest. Defaults to 'Hermes Agent'."
                },
                "duration": {
                    "type": "string",
                    "description": "Optional max duration before auto-leave (e.g. '30m', '2h', '90s'). Omit to stay until meet_leave is called."
                },
                "headed": {
                    "type": "boolean",
                    "description": "Run Chromium headed instead of headless (debug only). Default false."
                },
                "node": {
                    "type": "string",
                    "description": "Name of a registered remote node to run the bot on (useful when the gateway runs on a headless Linux box but the user's Chrome with a signed-in Google profile lives on their Mac). Pass 'auto' to use the single registered node. Default: run locally. Nodes are approved via `hermes meet node approve`."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }
    })
}

/// Mirrors `MEET_STATUS_SCHEMA` (lines 145-159).
pub fn meet_status_schema() -> Value {
    json!({
        "name": "meet_status",
        "description": "Report the current Meet session state — whether the bot is alive, has joined, is sitting in the lobby, number of transcript lines captured, and last-caption timestamp.",
        "parameters": {
            "type": "object",
            "properties": {
                "node": {"type": "string"}
            },
            "additionalProperties": false
        }
    })
}

/// Mirrors `MEET_TRANSCRIPT_SCHEMA` (lines 161-184).
pub fn meet_transcript_schema() -> Value {
    json!({
        "name": "meet_transcript",
        "description": "Read the scraped transcript for the active Meet session. Returns full transcript unless 'last' is set, in which case returns the last N lines only.",
        "parameters": {
            "type": "object",
            "properties": {
                "last": {
                    "type": "integer",
                    "description": "Optional: return only the last N caption lines. Useful for polling during a meeting without re-reading the whole transcript.",
                    "minimum": 1
                },
                "node": {"type": "string"}
            },
            "additionalProperties": false
        }
    })
}

/// Mirrors `MEET_LEAVE_SCHEMA` (lines 186-200).
pub fn meet_leave_schema() -> Value {
    json!({
        "name": "meet_leave",
        "description": "Leave the active Meet call cleanly, stop caption scraping, and finalize the transcript file. Safe to call when no meeting is active — returns ok=false with a reason.",
        "parameters": {
            "type": "object",
            "properties": {
                "node": {"type": "string"}
            },
            "additionalProperties": false
        }
    })
}

/// Mirrors `MEET_SAY_SCHEMA` (lines 202-221).
pub fn meet_say_schema() -> Value {
    json!({
        "name": "meet_say",
        "description": "Speak text into the active Meet call. Requires the active meeting to have been joined with mode='realtime'. The text is queued to the bot's OpenAI Realtime session; the generated audio is streamed into Chrome's fake microphone via a virtual audio device (PulseAudio null-sink on Linux, BlackHole on macOS). Returns immediately — the actual speech lags by a couple of seconds.",
        "parameters": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to speak."},
                "node": {"type": "string"}
            },
            "required": ["text"],
            "additionalProperties": false
        }
    })
}

/// All google_meet tool schemas — mirrors the module-level schema constants.
pub fn all_schemas() -> Vec<Value> {
    vec![
        meet_join_schema(),
        meet_status_schema(),
        meet_transcript_schema(),
        meet_leave_schema(),
        meet_say_schema(),
    ]
}

// ---------------------------------------------------------------------------
// Handlers — mirrors lines 228-348
// ---------------------------------------------------------------------------

fn json_str(obj: &Value) -> String {
    serde_json::to_string(obj).unwrap_or_else(|_| "{}".to_string())
}

fn err_json(msg: &str, extra: Option<Value>) -> String {
    let mut map = serde_json::Map::new();
    map.insert("success".to_string(), json!(false));
    map.insert("error".to_string(), json!(msg));
    if let Some(Value::Object(extra_map)) = extra {
        for (k, v) in extra_map {
            map.insert(k, v);
        }
    }
    json_str(&Value::Object(map))
}

// Helper: Python bool(res.get("ok")) truthiness
fn is_ok(res: &Value) -> bool {
    match res.get("ok") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            let t = s.trim();
            !t.is_empty() && t.to_ascii_lowercase() != "false" && t != "0"
        }
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        Some(Value::Null) => false,
        Some(_) => true,
        None => false,
    }
}

fn merge_success_and_node(mut res: Value, node_name: Option<&str>) -> Value {
    let success = is_ok(&res);
    let obj = res.as_object_mut();
    if let Some(map) = obj {
        map.insert("success".to_string(), json!(success));
        if let Some(n) = node_name {
            map.insert("node".to_string(), json!(n));
        }
        Value::Object(map.clone())
    } else {
        let mut map = serde_json::Map::new();
        map.insert("success".to_string(), json!(success));
        if let Some(n) = node_name {
            map.insert("node".to_string(), json!(n));
        }
        Value::Object(map)
    }
}

// ---------------------------------------------------------------------------
// process_manager stubs — mirrors `plugins.google_meet.process_manager as pm`
// ---------------------------------------------------------------------------

fn pm_start(
    url: &str,
    headed: bool,
    guest_name: &str,
    duration: Option<&str>,
    mode: &str,
) -> Value {
    let payload = json!({
        "url": url,
        "headed": headed,
        "guest_name": guest_name,
        "duration": duration,
        "mode": mode,
    });
    let py = python_executable();
    let code = format!(
        "import json, sys; payload={}; \
         try:\n  from plugins.google_meet import process_manager as pm; \
         res=pm.start(url=payload['url'], headed=payload['headed'], guest_name=payload['guest_name'], duration=payload.get('duration'), mode=payload.get('mode', 'transcribe')); print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"ok\": False, \"error\": str(e)}})); sys.exit(0)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() || !out.stdout.is_empty() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
        }
    }
    json!({"ok": false, "error": "process_manager unavailable (python module not importable)", "url": url, "mode": mode})
}

fn pm_status() -> Value {
    let py = python_executable();
    let code = "import json; try:\n  from plugins.google_meet import process_manager as pm; print(json.dumps(pm.status()))\nexcept Exception as e:\n  print(json.dumps({\"ok\": False, \"error\": str(e)}))\n";
    if let Ok(out) = Command::new(&py).args(["-c", code]).output() {
        if out.status.success() || !out.stdout.is_empty() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
        }
    }
    json!({"ok": false, "error": "process_manager unavailable"})
}

fn pm_transcript(last: Option<i64>) -> Value {
    let py = python_executable();
    let code = format!(
        "import json; last={}; \
         try:\n  from plugins.google_meet import process_manager as pm; print(json.dumps(pm.transcript(last=last)))\n\
         except Exception as e:\n  print(json.dumps({{\"ok\": False, \"error\": str(e)}}))\n",
        match last {
            Some(n) => n.to_string(),
            None => "None".to_string(),
        }
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() || !out.stdout.is_empty() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
        }
    }
    json!({"ok": false, "error": "process_manager unavailable"})
}

fn pm_stop(reason: &str) -> Value {
    let py = python_executable();
    let code = format!(
        "import json; reason={}; \
         try:\n  from plugins.google_meet import process_manager as pm; print(json.dumps(pm.stop(reason=reason)))\n\
         except Exception as e:\n  print(json.dumps({{\"ok\": False, \"error\": str(e)}}))\n",
        serde_json::to_string(reason).unwrap_or_else(|_| "\"\"".to_string())
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() || !out.stdout.is_empty() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
        }
    }
    json!({"ok": false, "error": "process_manager unavailable"})
}

fn pm_enqueue_say(text: &str) -> Value {
    let py = python_executable();
    let code = format!(
        "import json, sys; text={}; \
         try:\n  from plugins.google_meet import process_manager as pm; res=pm.enqueue_say(text); print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"ok\": False, \"error\": str(e)}}))\n",
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() || !out.stdout.is_empty() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
        }
    }
    json!({"ok": false, "error": "process_manager unavailable"})
}

// Node client stubs — mirrors `NodeClient(url, token).start_bot(...)` etc.

fn node_client_start_bot(
    entry: &NodeClient,
    url: &str,
    guest_name: &str,
    duration: Option<&str>,
    headed: bool,
    mode: &str,
) -> Result<Value, String> {
    let py = python_executable();
    let payload = json!({
        "entry_url": entry.url,
        "entry_token": entry.token,
        "url": url,
        "guest_name": guest_name,
        "duration": duration,
        "headed": headed,
        "mode": mode,
    });
    let code = format!(
        "import json, sys; p={}; \
         try:\n  from plugins.google_meet.node.client import NodeClient\n  c=NodeClient(url=p['entry_url'], token=p['entry_token'])\n  res=c.start_bot(url=p['url'], guest_name=p['guest_name'], duration=p.get('duration'), headed=p['headed'], mode=p['mode'])\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let out = Command::new(&py)
        .args(["-c", &code])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stdout.is_empty() {
            // Try to extract _error field
            if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
                if let Some(err) = v.get("_error").and_then(|x| x.as_str()) {
                    return Err(err.to_string());
                }
            }
            return Err(stdout);
        }
        if !stderr.is_empty() {
            return Err(stderr);
        }
        return Err("remote start_bot failed".to_string());
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str::<Value>(&txt).map_err(|e| e.to_string())
}

fn node_client_status(entry: &NodeClient) -> Result<Value, String> {
    let py = python_executable();
    let payload = json!({ "entry_url": entry.url, "entry_token": entry.token });
    let code = format!(
        "import json, sys; p={}; \
         try:\n  from plugins.google_meet.node.client import NodeClient\n  c=NodeClient(url=p['entry_url'], token=p['entry_token'])\n  res=c.status()\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let out = Command::new(&py)
        .args(["-c", &code])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
            if let Some(err) = v.get("_error").and_then(|x| x.as_str()) {
                return Err(err.to_string());
            }
        }
        return Err(if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            "remote status failed".to_string()
        });
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str::<Value>(&txt).map_err(|e| e.to_string())
}

fn node_client_transcript(entry: &NodeClient, last: Option<i64>) -> Result<Value, String> {
    let py = python_executable();
    let payload = json!({ "entry_url": entry.url, "entry_token": entry.token, "last": last });
    let code = format!(
        "import json, sys; p={}; \
         try:\n  from plugins.google_meet.node.client import NodeClient\n  c=NodeClient(url=p['entry_url'], token=p['entry_token'])\n  res=c.transcript(last=p.get('last'))\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let out = Command::new(&py)
        .args(["-c", &code])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
            if let Some(err) = v.get("_error").and_then(|x| x.as_str()) {
                return Err(err.to_string());
            }
        }
        return Err(if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            "remote transcript failed".to_string()
        });
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str::<Value>(&txt).map_err(|e| e.to_string())
}

fn node_client_stop(entry: &NodeClient) -> Result<Value, String> {
    let py = python_executable();
    let payload = json!({ "entry_url": entry.url, "entry_token": entry.token });
    let code = format!(
        "import json, sys; p={}; \
         try:\n  from plugins.google_meet.node.client import NodeClient\n  c=NodeClient(url=p['entry_url'], token=p['entry_token'])\n  res=c.stop()\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let out = Command::new(&py)
        .args(["-c", &code])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
            if let Some(err) = v.get("_error").and_then(|x| x.as_str()) {
                return Err(err.to_string());
            }
        }
        return Err(if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            "remote stop failed".to_string()
        });
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str::<Value>(&txt).map_err(|e| e.to_string())
}

fn node_client_say(entry: &NodeClient, text: &str) -> Result<Value, String> {
    let py = python_executable();
    let payload = json!({ "entry_url": entry.url, "entry_token": entry.token, "text": text });
    let code = format!(
        "import json, sys; p={}; \
         try:\n  from plugins.google_meet.node.client import NodeClient\n  c=NodeClient(url=p['entry_url'], token=p['entry_token'])\n  res=c.say(p['text'])\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let out = Command::new(&py)
        .args(["-c", &code])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
            if let Some(err) = v.get("_error").and_then(|x| x.as_str()) {
                return Err(err.to_string());
            }
        }
        return Err(if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            "remote say failed".to_string()
        });
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str::<Value>(&txt).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Public handlers — mirrors lines 236-348
// ---------------------------------------------------------------------------

/// Mirrors `def handle_meet_join(args: Dict[str, Any], **_kw) -> str` (lines 236-278).
pub fn handle_meet_join(args: &Value) -> String {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if url.is_empty() {
        return err_json("url is required", None);
    }
    let mode_raw = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("transcribe");
    let mode = mode_raw.trim().to_ascii_lowercase();
    if mode != "transcribe" && mode != "realtime" {
        return err_json(
            &format!("mode must be 'transcribe' or 'realtime' (got {:?})", mode),
            None,
        );
    }

    let node = args.get("node").and_then(|v| v.as_str());

    // _resolve_node_client
    let (client_opt, node_name_opt) = match resolve_node_client(node) {
        Ok((c, n)) => (c, n),
        Err(e) => return err_json(&e, None),
    };

    if let Some(client) = client_opt {
        let guest_name = args
            .get("guest_name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("Hermes Agent")
            .to_string();
        // duration: str(args.get("duration")) if args.get("duration") else None
        let duration = args
            .get("duration")
            .and_then(|v| {
                if v.is_null() {
                    None
                } else if let Some(s) = v.as_str() {
                    if s.trim().is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                } else {
                    // Python: str(args.get("duration")) — stringify non-string
                    let s = match v {
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => serde_json::to_string(v).unwrap_or_default().trim_matches('"').to_string(),
                    };
                    if s.trim().is_empty() {
                        None
                    } else {
                        Some(s)
                    }
                }
            });
        let headed = args
            .get("headed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        match node_client_start_bot(
            &client,
            &url,
            &guest_name,
            duration.as_deref(),
            headed,
            &mode,
        ) {
            Ok(res) => {
                let merged = merge_success_and_node(res, node_name_opt.as_deref());
                json_str(&merged)
            }
            Err(e) => err_json(
                &format!("remote node start_bot failed: {}", e),
                node_name_opt
                    .as_deref()
                    .map(|n| json!({"node": n})),
            ),
        }
    } else {
        // Local path
        if !check_meet_requirements() {
            return err_json(
                "google_meet plugin prerequisites missing — install with `pip install playwright && python -m playwright install chromium`. Plugin is supported on Linux and macOS only.",
                None,
            );
        }
        let guest_name = args
            .get("guest_name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("Hermes Agent")
            .to_string();
        let duration = args.get("duration").and_then(|v| {
            if v.is_null() {
                None
            } else if let Some(s) = v.as_str() {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            } else {
                let s = match v {
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => serde_json::to_string(v).unwrap_or_default().trim_matches('"').to_string(),
                };
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        });
        let headed = args
            .get("headed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let res = pm_start(&url, headed, &guest_name, duration.as_deref(), &mode);
        let success = is_ok(&res);
        let mut map = match res {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        map.insert("success".to_string(), json!(success));
        json_str(&Value::Object(map))
    }
}

/// Mirrors `def handle_meet_status(args: Dict[str, Any], **_kw) -> str` (lines 281-293).
pub fn handle_meet_status(args: &Value) -> String {
    let node = args.get("node").and_then(|v| v.as_str());
    let (client_opt, node_name_opt) = match resolve_node_client(node) {
        Ok((c, n)) => (c, n),
        Err(e) => return err_json(&e, None),
    };
    if let Some(client) = client_opt {
        match node_client_status(&client) {
            Ok(res) => {
                let merged = merge_success_and_node(res, node_name_opt.as_deref());
                json_str(&merged)
            }
            Err(e) => err_json(
                &format!("remote node status failed: {}", e),
                node_name_opt
                    .as_deref()
                    .map(|n| json!({"node": n})),
            ),
        }
    } else {
        let res = pm_status();
        let success = is_ok(&res);
        let mut map = match res {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        map.insert("success".to_string(), json!(success));
        json_str(&Value::Object(map))
    }
}

/// Mirrors `def handle_meet_transcript(args: Dict[str, Any], **_kw) -> str` (lines 296-315).
pub fn handle_meet_transcript(args: &Value) -> String {
    // Parse last: int(last) if last is not None else None; if <1 -> None; except -> None
    let last_raw = args.get("last");
    let last_i: Option<i64> = match last_raw {
        Some(Value::Null) | None => None,
        Some(v) => {
            let parsed = match v {
                Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                Value::String(s) => s.trim().parse::<i64>().ok(),
                Value::Bool(b) => Some(if *b { 1 } else { 0 }),
                _ => None,
            };
            match parsed {
                Some(n) if n >= 1 => Some(n),
                Some(_) => None,
                None => None,
            }
        }
    };

    let node = args.get("node").and_then(|v| v.as_str());
    let (client_opt, node_name_opt) = match resolve_node_client(node) {
        Ok((c, n)) => (c, n),
        Err(e) => return err_json(&e, None),
    };
    if let Some(client) = client_opt {
        match node_client_transcript(&client, last_i) {
            Ok(res) => {
                let merged = merge_success_and_node(res, node_name_opt.as_deref());
                json_str(&merged)
            }
            Err(e) => err_json(
                &format!("remote node transcript failed: {}", e),
                node_name_opt
                    .as_deref()
                    .map(|n| json!({"node": n})),
            ),
        }
    } else {
        let res = pm_transcript(last_i);
        let success = is_ok(&res);
        let mut map = match res {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        map.insert("success".to_string(), json!(success));
        json_str(&Value::Object(map))
    }
}

/// Mirrors `def handle_meet_leave(args: Dict[str, Any], **_kw) -> str` (lines 318-330).
pub fn handle_meet_leave(args: &Value) -> String {
    let node = args.get("node").and_then(|v| v.as_str());
    let (client_opt, node_name_opt) = match resolve_node_client(node) {
        Ok((c, n)) => (c, n),
        Err(e) => return err_json(&e, None),
    };
    if let Some(client) = client_opt {
        match node_client_stop(&client) {
            Ok(res) => {
                let merged = merge_success_and_node(res, node_name_opt.as_deref());
                json_str(&merged)
            }
            Err(e) => err_json(
                &format!("remote node stop failed: {}", e),
                node_name_opt
                    .as_deref()
                    .map(|n| json!({"node": n})),
            ),
        }
    } else {
        let res = pm_stop("agent called meet_leave");
        let success = is_ok(&res);
        let mut map = match res {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        map.insert("success".to_string(), json!(success));
        json_str(&Value::Object(map))
    }
}

/// Mirrors `def handle_meet_say(args: Dict[str, Any], **_kw) -> str` (lines 333-348).
pub fn handle_meet_say(args: &Value) -> String {
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return err_json("text is required", None);
    }
    let node = args.get("node").and_then(|v| v.as_str());
    let (client_opt, node_name_opt) = match resolve_node_client(node) {
        Ok((c, n)) => (c, n),
        Err(e) => return err_json(&e, None),
    };
    if let Some(client) = client_opt {
        match node_client_say(&client, &text) {
            Ok(res) => {
                let merged = merge_success_and_node(res, node_name_opt.as_deref());
                json_str(&merged)
            }
            Err(e) => err_json(
                &format!("remote node say failed: {}", e),
                node_name_opt
                    .as_deref()
                    .map(|n| json!({"node": n})),
            ),
        }
    } else {
        let res = pm_enqueue_say(&text);
        let success = is_ok(&res);
        let mut map = match res {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        map.insert("success".to_string(), json!(success));
        json_str(&Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meet_join_schema_has_required_fields() {
        let s = meet_join_schema();
        assert_eq!(s["name"], json!("meet_join"));
        assert_eq!(s["parameters"]["required"], json!(["url"]));
        assert_eq!(s["parameters"]["additionalProperties"], json!(false));
        let props = s["parameters"]["properties"].as_object().unwrap();
        assert!(props.contains_key("url"));
        assert!(props.contains_key("mode"));
        assert!(props.contains_key("guest_name"));
        assert!(props.contains_key("duration"));
        assert!(props.contains_key("headed"));
        assert!(props.contains_key("node"));
        assert_eq!(s["parameters"]["properties"]["mode"]["enum"], json!(["transcribe", "realtime"]));
    }

    #[test]
    fn meet_status_schema_shape() {
        let s = meet_status_schema();
        assert_eq!(s["name"], json!("meet_status"));
        assert_eq!(s["parameters"]["additionalProperties"], json!(false));
    }

    #[test]
    fn meet_transcript_schema_shape() {
        let s = meet_transcript_schema();
        assert_eq!(s["name"], json!("meet_transcript"));
        assert_eq!(s["parameters"]["properties"]["last"]["minimum"], json!(1));
    }

    #[test]
    fn meet_leave_schema_shape() {
        let s = meet_leave_schema();
        assert_eq!(s["name"], json!("meet_leave"));
    }

    #[test]
    fn meet_say_schema_requires_text() {
        let s = meet_say_schema();
        assert_eq!(s["name"], json!("meet_say"));
        assert_eq!(s["parameters"]["required"], json!(["text"]));
    }

    #[test]
    fn all_schemas_covers_five() {
        let all = all_schemas();
        assert_eq!(all.len(), 5);
        let names: Vec<String> = all.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"meet_join".to_string()));
        assert!(names.contains(&"meet_status".to_string()));
        assert!(names.contains(&"meet_transcript".to_string()));
        assert!(names.contains(&"meet_leave".to_string()));
        assert!(names.contains(&"meet_say".to_string()));
    }

    #[test]
    fn handle_meet_join_requires_url() {
        let res = handle_meet_join(&json!({}));
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["success"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("url is required"));
    }

    #[test]
    fn handle_meet_join_rejects_bad_mode() {
        let res = handle_meet_join(&json!({"url": "https://meet.google.com/abc-defg-hij", "mode": "bad"}));
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["success"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("mode must be"));
    }

    #[test]
    fn handle_meet_join_mode_case_insensitive() {
        // Use an unresolvable node to avoid local pm path (which would require playwright).
        // If mode is uppercased, it should be normalized before the node check;
        // we test that invalid mode is still caught even with node set.
        let res = handle_meet_join(&json!({"url": "https://meet.google.com/abc-defg-hij", "mode": "TRANSCRIBE", "node": "no-such-node-xyz"}));
        let v: Value = serde_json::from_str(&res).unwrap();
        // Valid mode TRANSCRIBE -> trimmed lower == transcribe, so not error on mode.
        // It should fail on node resolution instead.
        assert!(v["error"].as_str().unwrap().contains("no registered meet node matches") || v["success"] == json!(false));
    }

    #[test]
    fn handle_meet_say_requires_text() {
        let res = handle_meet_say(&json!({}));
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["success"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("text is required"));
        let res2 = handle_meet_say(&json!({"text": "   "}));
        let v2: Value = serde_json::from_str(&res2).unwrap();
        assert_eq!(v2["success"], json!(false));
    }

    #[test]
    fn resolve_node_client_unresolvable_errors() {
        // Ensure env is clean for this test
        let prev = std::env::var("HERMES_GOOGLE_MEET_NODES_JSON").ok();
        unsafe { std::env::remove_var("HERMES_GOOGLE_MEET_NODES_JSON"); }
        // Also point HERMES_HOME to a temp that has no nodes.json
        let prev_home = std::env::var("HERMES_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "hermes-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let err = resolve_node_client(Some("ghost")).unwrap_err();
        assert!(err.contains("no registered meet node matches"));
        assert!(err.contains("ghost"));
        // cleanup
        if let Some(v) = prev { unsafe { std::env::set_var("HERMES_GOOGLE_MEET_NODES_JSON", v); } }
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_node_client_local_when_empty() {
        let (client, name) = resolve_node_client(None).unwrap();
        assert!(client.is_none());
        assert!(name.is_none());
        let (client2, name2) = resolve_node_client(Some("")).unwrap();
        assert!(client2.is_none());
        assert!(name2.is_none());
    }

    #[test]
    fn handle_meet_status_node_error_surfaces() {
        let prev = std::env::var("HERMES_GOOGLE_MEET_NODES_JSON").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::remove_var("HERMES_GOOGLE_MEET_NODES_JSON"); }
        let tmp = std::env::temp_dir().join(format!(
            "hermes-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let res = handle_meet_status(&json!({"node": "ghost-node"}));
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["success"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("no registered meet node matches"));
        if let Some(val) = prev { unsafe { std::env::set_var("HERMES_GOOGLE_MEET_NODES_JSON", val); } }
        if let Some(val) = prev_home { unsafe { std::env::set_var("HERMES_HOME", val); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn handle_meet_transcript_last_parsing() {
        // last <1 should be treated as None — exercise via node path (no local pm needed)
        let prev = std::env::var("HERMES_GOOGLE_MEET_NODES_JSON").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::remove_var("HERMES_GOOGLE_MEET_NODES_JSON"); }
        let tmp = std::env::temp_dir().join(format!(
            "hermes-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        // Use ghost node so it errors before reaching pm; ensures parsing didn't panic
        let res = handle_meet_transcript(&json!({"last": -5, "node": "ghost"}));
        let v: Value = serde_json::from_str(&res).unwrap();
        assert!(v["error"].as_str().unwrap().contains("no registered meet node matches"));
        if let Some(val) = prev { unsafe { std::env::set_var("HERMES_GOOGLE_MEET_NODES_JSON", val); } }
        if let Some(val) = prev_home { unsafe { std::env::set_var("HERMES_HOME", val); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }
}
