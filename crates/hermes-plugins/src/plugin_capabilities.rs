//! Plugin capability declarations + consent state (#64228).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/plugin_capabilities.py` (393 LOC).
//! Unifies the scattered per-plugin trust gates (`plugins.entries.<id>.allow_*`)
//! into one declared, diffable capability model with install/update-time consent.
//!
//! **This is NOT a sandbox.** In-process plugins remain trusted — capabilities govern
//! the host API surfaces Hermes hands out (which registrations succeed, which `ctx`
//! methods are live) and give the user an honest consent + audit trail.
//!
//! Canonical registry
//! ------------------
//! Every capability id maps 1:1 to a trust gate that **already exists** on the
//! enforcing surface. Legacy `allow_*` keys keep working verbatim (deprecated but honored).
//!
//! Consent state
//! -------------
//! Stored under `plugins.entries.<plugin_id>` as:
//! ```yaml
//! plugins:
//!   entries:
//!     <plugin_id>:
//!       granted_capabilities: [tools.override]
//!       capabilities_consent:
//!         hash: "<sha256 of declared set at consent time>"
//!         granted_at: "2026-08-12T00:00:00+00:00"
//! ```
//! Ground rule: everything defaults OFF. Any failure to read consent state
//! (missing config, corrupt YAML, wrong types) means **not granted**.
//!
//! Python surface ported line-for-line:
//!   - `CapabilitySpec` (`id`, `legacy_path`, `description`)
//!   - `CAPABILITY_REGISTRY`, `VALID_CAPABILITY_IDS`
//!   - `GRANTED_KEY`, `CONSENT_KEY`
//!   - `parse_declared_capabilities(raw, plugin_name)`
//!   - `capability_set_hash(capabilities)`
//!   - `_plugin_entry(plugin_id, config)` (fail-closed)
//!   - `granted_capabilities(plugin_id, config)`
//!   - `_legacy_gate_set(entry, spec)`
//!   - `plugin_capability_granted(plugin_id, capability, config)`
//!   - `_log_capability_decision(plugin_id, capability, allowed, evidence)`
//!   - `record_consent(plugin_id, granted, declared)`
//!   - `consent_hash(plugin_id, config)`
//!   - `pending_capabilities(plugin_id, declared, config)`
//!   - `declared_set_changed(plugin_id, declared, config)`

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors plugin_capabilities.py:77-139
// ---------------------------------------------------------------------------

/// Mirrors `CapabilitySpec` (lines 65-74) — frozen dataclass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySpec {
    /// e.g. `"tools.override"`
    pub id: &'static str,
    /// Path of the deprecated boolean under `plugins.entries.<plugin_id>`,
    /// e.g. `["allow_tool_override"]` or `["llm", "allow_model_override"]`.
    pub legacy_path: &'static [&'static str],
    /// One-line risk description shown on the consent screen.
    pub description: &'static str,
}

// Canonical registry — ONLY capabilities with an existing enforcing surface.
// Mirrors `CAPABILITY_REGISTRY` (lines 78-132).
pub static CAPABILITY_SPECS: &[CapabilitySpec] = &[
    CapabilitySpec {
        id: "tools.override",
        legacy_path: &["allow_tool_override"],
        description: "Replace built-in tools (e.g. shell_exec, write_file) — an override can intercept everything routed through that tool",
    },
    CapabilitySpec {
        id: "llm.provider_override",
        legacy_path: &["llm", "allow_provider_override"],
        description: "Run host-owned LLM calls against a provider other than your active one (uses your credentials)",
    },
    CapabilitySpec {
        id: "llm.model_override",
        legacy_path: &["llm", "allow_model_override"],
        description: "Choose which model host-owned LLM calls use (spend follows the chosen model)",
    },
    CapabilitySpec {
        id: "llm.agent_id_override",
        legacy_path: &["llm", "allow_agent_id_override"],
        description: "Attribute its LLM calls to a different agent id",
    },
    CapabilitySpec {
        id: "llm.profile_override",
        legacy_path: &["llm", "allow_profile_override"],
        description: "Run LLM calls under a different auth profile",
    },
    CapabilitySpec {
        id: "llm.task_override",
        legacy_path: &["llm", "allow_task_override"],
        description: "Route its LLM calls through the host's built-in auxiliary task lanes",
    },
    CapabilitySpec {
        id: "gateway.platform_actions",
        legacy_path: &["allow_platform_actions"],
        description: "Act on connected chat platforms as the gateway bot (add reactions, rename threads) via ctx.platform_actions",
    },
];

/// Mirrors `VALID_CAPABILITY_IDS = frozenset(CAPABILITY_REGISTRY)` (line 134).
pub fn valid_capability_ids() -> HashSet<&'static str> {
    CAPABILITY_SPECS.iter().map(|s| s.id).collect()
}

fn is_valid_capability(id: &str) -> bool {
    CAPABILITY_SPECS.iter().any(|s| s.id == id)
}

/// Lookup a spec by id — mirrors `CAPABILITY_REGISTRY.get(capability)`.
pub fn get_capability_spec(id: &str) -> Option<&'static CapabilitySpec> {
    CAPABILITY_SPECS.iter().find(|s| s.id == id)
}

/// Config keys under `plugins.entries.<plugin_id>` — mirrors lines 137-138.
pub const GRANTED_KEY: &str = "granted_capabilities";
pub const CONSENT_KEY: &str = "capabilities_consent";

// ---------------------------------------------------------------------------
// HERMES_HOME / config I/O helpers — mirrors hermes_constants + hermes_cli.config
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
/// Mirrors `hermes_constants.get_hermes_home()`.
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

pub fn get_config_path() -> PathBuf {
    get_hermes_home().join("config.yaml")
}

/// Load the merged config as `serde_json::Value` (best-effort, fail-closed).
/// Mirrors `hermes_cli.config.load_config() or {}` (lines 196, 302).
///
/// Tries `$HERMES_HOME/config.json`, then `config.yaml`, then `config.yml`.
/// First attempts `serde_json::from_str`; on failure falls back to a minimal
/// YAML subset parser (covers shapes emitted by `yaml.safe_dump` for this module).
/// Any failure yields `json!({})` — the ground rule: failure to read = not granted.
/// Real port would use `serde_yaml` (cf. `agent_plugins.rs`'s subset parser).
pub fn load_config_value() -> Value {
    for fname in ["config.json", "config.yaml", "config.yml"] {
        let path = get_hermes_home().join(fname);
        if let Ok(text) = fs::read_to_string(&path) {
            if text.trim().is_empty() {
                continue;
            }
            // Try JSON first (tests often write JSON for simplicity; JSON is valid YAML 1.2).
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.is_object() {
                    return v;
                }
                // If top-level is not object, treat as empty — fail-closed.
                return json!({});
            }
            // Fallback: minimal YAML parser for capabilities-consent shapes.
            if let Ok(map) = parse_yaml_simple(&text) {
                return Value::Object(map);
            }
        }
    }
    json!({})
}

/// Save config atomically to `$HERMES_HOME/config.yaml` (JSON pretty-printed; JSON is valid YAML).
/// Mirrors `hermes_cli.config.save_config(config)` (lines 343, 389).
pub fn save_config_value(config: &Value) -> std::io::Result<()> {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let data = serde_json::to_string_pretty(config).unwrap_or_else(|_| "{}".to_string());
    fs::write(&tmp, data)?;
    // Atomic rename (POSIX); on Windows, remove target first if exists.
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal YAML subset parser — mirrors agent_plugins.rs parse_yaml_simple
// ---------------------------------------------------------------------------

fn parse_yaml_scalar(s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    if trimmed == "[]" {
        return Value::Array(Vec::new());
    }
    if trimmed == "{}" {
        return Value::Object(Map::new());
    }
    if trimmed == "null" || trimmed == "~" || trimmed == "Null" || trimmed == "NULL" {
        return Value::Null;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "true" {
        return Value::Bool(true);
    }
    if lower == "false" {
        return Value::Bool(false);
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        return Value::String(inner.to_string());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return json!(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(trimmed.to_string())
}

fn parse_yaml_simple(yaml: &str) -> Result<Map<String, Value>, String> {
    let mut out: Map<String, Value> = Map::new();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent != 0 {
            i += 1;
            continue;
        }
        let colon_pos = match line.find(':') {
            Some(p) => p,
            None => {
                i += 1;
                continue;
            }
        };
        let key = line[..colon_pos].trim().to_string();
        let rest = line[colon_pos + 1..].trim().to_string();
        if key.is_empty() {
            i += 1;
            continue;
        }
        if !rest.is_empty() {
            out.insert(key, parse_yaml_scalar(&rest));
            i += 1;
        } else {
            let mut block_lines: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let nxt = lines[j];
                if nxt.trim().is_empty() {
                    j += 1;
                    continue;
                }
                let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                if nxt_indent == 0 {
                    break;
                }
                block_lines.push(nxt.to_string());
                j += 1;
            }
            if block_lines.is_empty() {
                out.insert(key, Value::Null);
                i += 1;
                continue;
            }
            let is_list = block_lines.iter().any(|l| l.trim_start().starts_with("- "));
            if is_list {
                let mut arr: Vec<Value> = Vec::new();
                for bl in block_lines {
                    let t = bl.trim();
                    if t.starts_with("- ") {
                        let item_str = t[2..].trim();
                        arr.push(parse_yaml_scalar(item_str));
                    } else if t == "-" {
                        arr.push(Value::String(String::new()));
                    }
                }
                out.insert(key, Value::Array(arr));
            } else {
                let mut submap: Map<String, Value> = Map::new();
                for bl in block_lines {
                    let t = bl.trim();
                    if t.is_empty() || t.starts_with('#') {
                        continue;
                    }
                    if let Some(cp) = t.find(':') {
                        let sk = t[..cp].trim().to_string();
                        let sv = t[cp + 1..].trim();
                        submap.insert(sk, parse_yaml_scalar(sv));
                    }
                }
                out.insert(key, Value::Object(submap));
            }
            i = j;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SHA256 — stdlib only (no `sha2` crate), mirrors sms_adapter.rs sha1 style
// ---------------------------------------------------------------------------

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let ml = (message.len() as u64) * 8;
    let mut padded = Vec::with_capacity(message.len() + 64);
    padded.extend_from_slice(message);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&ml.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            let j = i * 4;
            w[i] = ((chunk[j] as u32) << 24)
                | ((chunk[j + 1] as u32) << 16)
                | ((chunk[j + 2] as u32) << 8)
                | (chunk[j + 3] as u32);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, hv) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&hv.to_be_bytes());
    }
    out
}

fn hex_sha256(data: &[u8]) -> String {
    let hash = sha256(data);
    let mut s = String::with_capacity(64);
    for b in &hash {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// ---------------------------------------------------------------------------
// Helpers — truthiness / falsiness mirrors Python bool() semantics
// ---------------------------------------------------------------------------

fn value_is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn value_is_falsy(v: &Value) -> bool {
    !value_is_truthy(v)
}

// Current UTC timestamp in ISO8601 with seconds, matching
// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
// Tries `chrono` if linked, else falls back to seconds-since-epoch string
// (still round-trips for consent storage; real port would use `chrono`).
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    try_chrono_now_iso().unwrap_or_else(|| format!("{}", secs))
}

fn try_chrono_now_iso() -> Option<String> {
    // Placeholder for chrono-based ISO — when `chrono` is in Cargo.toml:
    //   return Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    None
}

// ---------------------------------------------------------------------------
// Declaration parsing — mirrors lines 145-184
// ---------------------------------------------------------------------------

/// Normalize a manifest `capabilities:` value into known capability ids.
///
/// Unknown ids are dropped with a warning (forward compat: a plugin built for
/// a newer Hermes may declare ids this build doesn't know; they can never be
/// granted here, so hiding them from the consent screen is the fail-closed
/// choice — the plugin must degrade gracefully).
///
/// Mirrors `parse_declared_capabilities(raw, plugin_name="?")` (lines 145-178).
pub fn parse_declared_capabilities(raw: &Value, plugin_name: &str) -> Vec<String> {
    if value_is_falsy(raw) {
        return Vec::new();
    }
    let arr = match raw.as_array() {
        Some(a) => a,
        None => {
            log::warn!(
                "Plugin {}: manifest 'capabilities' must be a list, got {} — ignoring",
                plugin_name,
                value_type_name(raw)
            );
            return Vec::new();
        }
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let known_sorted: Vec<&str> = {
        let mut v: Vec<&str> = CAPABILITY_SPECS.iter().map(|s| s.id).collect();
        v.sort();
        v
    };
    let known_str = known_sorted.join(", ");
    for item in arr {
        let s = match item.as_str() {
            Some(st) => st,
            None => {
                log::warn!(
                    "Plugin {}: ignoring non-string capability entry {:?}",
                    plugin_name, item
                );
                continue;
            }
        };
        let cap = s.trim().to_string();
        if cap.is_empty() {
            continue;
        }
        if is_valid_capability(&cap) {
            if seen.insert(cap.clone()) {
                out.push(cap);
            }
        } else {
            log::warn!(
                "Plugin {}: unknown capability {:?} (known: {}) — ignoring",
                plugin_name, cap, known_str
            );
        }
    }
    out
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Deterministic sha256 over a capability set (order-insensitive).
/// Mirrors `capability_set_hash(capabilities)` (lines 181-184).
pub fn capability_set_hash<I, S>(capabilities: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut set: BTreeSet<String> = BTreeSet::new();
    for cap in capabilities {
        set.insert(cap.as_ref().to_string());
    }
    let canon = set.into_iter().collect::<Vec<String>>().join("\n");
    hex_sha256(canon.as_bytes())
}

// ---------------------------------------------------------------------------
// Consent state (read side — fail closed on ANY error) — mirrors lines 191-278
// ---------------------------------------------------------------------------

/// Return `plugins.entries.<plugin_id>` or `{}` — never panics.
/// Mirrors `_plugin_entry(plugin_id, config=None)` (lines 191-203).
pub fn plugin_entry(plugin_id: &str, config: Option<&Value>) -> Map<String, Value> {
    // Load config if not supplied — catch-all fail-closed.
    let cfg_owned: Value;
    let cfg_ref: &Value = match config {
        Some(v) => v,
        None => {
            cfg_owned = load_config_value();
            // Use cfg_owned for the rest of this block; we need to return a Map
            // so we can't return a reference to a local. Instead we handle
            // the None case inline below without delegating to the Some branch.
            // To keep code simple, we duplicate the extraction logic here for the
            // owned value.
            return plugin_entry_from_value(plugin_id, &cfg_owned);
        }
    };
    plugin_entry_from_value(plugin_id, cfg_ref)
}

fn plugin_entry_from_value(plugin_id: &str, cfg: &Value) -> Map<String, Value> {
    // `cfg.get("plugins").or({}).get("entries").or({}).get(plugin_id).or({})`
    // Any non-object intermediate yields {}.
    let plugins = match cfg.get("plugins") {
        Some(Value::Object(m)) => m,
        _ => return Map::new(),
    };
    let entries = match plugins.get("entries") {
        Some(Value::Object(m)) => m,
        _ => return Map::new(),
    };
    match entries.get(plugin_id) {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

/// Return the set of capabilities the user has granted this plugin.
/// Fail-closed: missing/corrupt state yields the empty set.
/// Mirrors `granted_capabilities(plugin_id, config=None)` (lines 206-220).
pub fn granted_capabilities(plugin_id: &str, config: Option<&Value>) -> HashSet<String> {
    let entry = plugin_entry(plugin_id, config);
    let raw = match entry.get(GRANTED_KEY) {
        Some(v) => v,
        None => return HashSet::new(),
    };
    let arr = match raw.as_array() {
        Some(a) => a,
        None => return HashSet::new(),
    };
    let mut out = HashSet::new();
    for v in arr {
        if let Some(s) = v.as_str() {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() && is_valid_capability(&trimmed) {
                out.insert(trimmed);
            }
        }
    }
    out
}

/// True when the deprecated `allow_*` key for `spec` is truthy.
/// Mirrors `_legacy_gate_set(entry, spec)` (lines 223-230).
pub fn legacy_gate_set(entry: &Map<String, Value>, spec: &CapabilitySpec) -> bool {
    let mut current: Option<&Value> = None;
    for (idx, part) in spec.legacy_path.iter().enumerate() {
        if idx == 0 {
            current = entry.get(*part);
        } else {
            match current {
                Some(Value::Object(map)) => current = map.get(*part),
                _ => return false,
            }
        }
        if current.is_none() {
            return false;
        }
    }
    match current {
        Some(v) => {
            if v.is_null() {
                return false;
            }
            value_is_truthy(v)
        }
        None => false,
    }
}

/// Canonical check: is `capability` live for `plugin_id`?
///
/// True when EITHER:
/// * the capability appears in `granted_capabilities` (consent flow), OR
/// * the legacy `allow_*` config key is set (deprecated, still honored).
///
/// Unknown capability ids and any failure to read state return `False`
/// (ground rule 4: fail closed).
/// Mirrors `plugin_capability_granted(plugin_id, capability, config=None)` (lines 233-267).
pub fn plugin_capability_granted(plugin_id: &str, capability: &str, config: Option<&Value>) -> bool {
    let spec = match get_capability_spec(capability) {
        Some(s) => s,
        None => {
            log::debug!(
                "capability check for unknown id {:?} (plugin {}) — denied",
                capability, plugin_id
            );
            return false;
        }
    };
    let entry = plugin_entry(plugin_id, config);
    // Python does: if capability in granted_capabilities(plugin_id, config={"plugins": {"entries": {plugin_id: entry}}}):
    // Reproduce that synthetic config so the check is byte-identical to Python.
    let synthetic = json!({"plugins": {"entries": {plugin_id: entry.clone()}}});
    let granted = granted_capabilities(plugin_id, Some(&synthetic));
    if granted.contains(capability) {
        log_capability_decision(plugin_id, capability, true, "granted_capabilities");
        return true;
    }
    if legacy_gate_set(&entry, spec) {
        let evidence = format!(
            "legacy key plugins.entries.{}.{} (deprecated)",
            plugin_id,
            spec.legacy_path.join(".")
        );
        log_capability_decision(plugin_id, capability, true, &evidence);
        return true;
    }
    log_capability_decision(plugin_id, capability, false, "not granted");
    false
}

/// Audit line for capability gate decisions (the `checked_by` trail).
/// Mirrors `_log_capability_decision(...)` (lines 270-277).
pub fn log_capability_decision(plugin_id: &str, capability: &str, allowed: bool, evidence: &str) {
    log::info!(
        "capability_check plugin={} capability={} decision={} checked_by=plugin_capability_granted evidence={}",
        plugin_id,
        capability,
        if allowed { "allow" } else { "deny" },
        evidence
    );
}

// ---------------------------------------------------------------------------
// Consent state (write side) — mirrors lines 284-348
// ---------------------------------------------------------------------------

/// Persist a consent decision for `plugin_id`.
///
/// Writes `granted_capabilities` (union with any previously granted set),
/// the consent record (hash of the *declared* set the user saw + UTC timestamp),
/// and — so every existing enforcement site keeps working without changes — the
/// corresponding legacy `allow_*` keys for each newly granted capability.
///
/// Mirrors `record_consent(plugin_id, granted, declared)` (lines 284-348).
pub fn record_consent<S1, S2>(plugin_id: &str, granted: impl IntoIterator<Item = S1>, declared: impl IntoIterator<Item = S2>)
where
    S1: AsRef<str>,
    S2: AsRef<str>,
{
    // Deduplicate preserving order, filter to known ids — mirrors dict.fromkeys + VALID check.
    let granted_list = {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for g in granted {
            let s = g.as_ref().trim().to_string();
            if s.is_empty() || !is_valid_capability(&s) {
                continue;
            }
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
        out
    };
    let declared_list = {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in declared {
            let s = d.as_ref().trim().to_string();
            if s.is_empty() || !is_valid_capability(&s) {
                continue;
            }
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
        out
    };

    let mut config = load_config_value();
    if !config.is_object() {
        config = json!({});
    }
    let config_obj = config.as_object_mut().unwrap();

    // plugins_cfg = config.setdefault("plugins", {})
    let plugins_val = config_obj.entry("plugins".to_string()).or_insert(json!({}));
    if !plugins_val.is_object() {
        *plugins_val = json!({});
    }
    let plugins_obj = plugins_val.as_object_mut().unwrap();

    // entries = plugins_cfg.setdefault("entries", {})
    let entries_val = plugins_obj.entry("entries".to_string()).or_insert(json!({}));
    if !entries_val.is_object() {
        *entries_val = json!({});
    }
    let entries_obj = entries_val.as_object_mut().unwrap();

    // entry = entries.setdefault(plugin_id, {})
    let entry_val = entries_obj.entry(plugin_id.to_string()).or_insert(json!({}));
    if !entry_val.is_object() {
        *entry_val = json!({});
    }
    // We need mutable access to the entry object; keep entry_val as &mut Value for later bridging.
    // Clone the previous granted list before mutable borrow ends for bridging step.
    let previous = entry_val.get(GRANTED_KEY).cloned();
    let mut merged: Vec<String> = match previous {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    for cap in &granted_list {
        if !merged.contains(cap) {
            merged.push(cap.clone());
        }
    }
    // Dedup + filter + sort — mirrors `sorted(c for c in dict.fromkeys(merged) if isinstance(c,str) and c in VALID)`
    let mut dedup_seen = HashSet::new();
    let mut deduped: Vec<String> = Vec::new();
    for c in merged {
        if !is_valid_capability(&c) {
            continue;
        }
        if dedup_seen.insert(c.clone()) {
            deduped.push(c);
        }
    }
    deduped.sort();
    let entry_obj = entry_val.as_object_mut().unwrap();
    entry_obj.insert(GRANTED_KEY.to_string(), json!(deduped.clone()));
    entry_obj.insert(
        CONSENT_KEY.to_string(),
        json!({
            "hash": capability_set_hash(declared_list.clone()),
            "granted_at": now_iso(),
        }),
    );

    // Bridge: mirror each granted capability into its legacy gate so the
    // existing enforcement sites (which still read allow_*) honor the grant.
    for cap in &deduped {
        if let Some(spec) = get_capability_spec(cap) {
            match spec.legacy_path {
                [single] => {
                    entry_obj.insert(single.to_string(), json!(true));
                }
                [first, second] => {
                    let child = entry_obj.entry(first.to_string()).or_insert(json!({}));
                    if !child.is_object() {
                        *child = json!({});
                    }
                    if let Some(map) = child.as_object_mut() {
                        map.insert(second.to_string(), json!(true));
                    }
                }
                _ => {
                    // Generic fallback for longer paths (not currently in registry).
                    let mut cur = entry_val as *mut Value;
                    unsafe {
                        // Walk first n-1 parts, ensuring objects.
                        for part in &spec.legacy_path[..spec.legacy_path.len() - 1] {
                            let cur_ref = &mut *cur;
                            if !cur_ref.is_object() {
                                *cur_ref = json!({});
                            }
                            let map = cur_ref.as_object_mut().unwrap();
                            let child = map.entry(part.to_string()).or_insert(json!({}));
                            if !child.is_object() {
                                *child = json!({});
                            }
                            cur = child as *mut Value;
                        }
                        let cur_ref = &mut *cur;
                        if !cur_ref.is_object() {
                            *cur_ref = json!({});
                        }
                        cur_ref.as_object_mut().unwrap().insert(
                            spec.legacy_path[spec.legacy_path.len() - 1].to_string(),
                            json!(true),
                        );
                    }
                }
            }
        }
    }

    let _ = save_config_value(&config);
    let granted_str = deduped.join(",");
    let hash_short = capability_set_hash(declared_list).chars().take(12).collect::<String>();
    // Use entry_obj's hash directly for log (first 12 chars)
    let stored_hash = entry_obj
        .get(CONSENT_KEY)
        .and_then(|v| v.get("hash"))
        .and_then(|v| v.as_str())
        .unwrap_or(&hash_short);
    let short = if stored_hash.len() >= 12 { &stored_hash[..12] } else { stored_hash };
    log::info!(
        "capability_consent plugin={} granted={} declared_hash={}",
        plugin_id,
        if granted_str.is_empty() { "(none)".to_string() } else { granted_str },
        short
    );
}

/// Return the stored consent hash, or None when absent/corrupt.
/// Mirrors `consent_hash(plugin_id, config=None)` (lines 351-358).
pub fn consent_hash(plugin_id: &str, config: Option<&Value>) -> Option<String> {
    let entry = plugin_entry(plugin_id, config);
    let consent = entry.get(CONSENT_KEY)?;
    let map = consent.as_object()?;
    let h = map.get("hash")?.as_str()?;
    if h.is_empty() {
        None
    } else {
        Some(h.to_string())
    }
}

/// Capabilities declared by the plugin but not yet granted.
/// Mirrors `pending_capabilities(plugin_id, declared, config=None)` (lines 361-376).
pub fn pending_capabilities<S>(plugin_id: &str, declared: impl IntoIterator<Item = S>, config: Option<&Value>) -> Vec<String>
where
    S: AsRef<str>,
{
    let declared_list = {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in declared {
            let s = d.as_ref().trim().to_string();
            if s.is_empty() || !is_valid_capability(&s) {
                continue;
            }
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
        out
    };
    let granted = granted_capabilities(plugin_id, config);
    declared_list.into_iter().filter(|c| !granted.contains(c)).collect()
}

/// True when the declared set differs from what the user consented to.
/// No stored consent at all counts as changed (never consented).
/// Mirrors `declared_set_changed(plugin_id, declared, config=None)` (lines 379-393).
pub fn declared_set_changed<S>(plugin_id: &str, declared: impl IntoIterator<Item = S>, config: Option<&Value>) -> bool
where
    S: AsRef<str>,
{
    let stored = consent_hash(plugin_id, config);
    if stored.is_none() {
        return true;
    }
    let filtered: Vec<String> = {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in declared {
            let s = d.as_ref().trim().to_string();
            if s.is_empty() || !is_valid_capability(&s) {
                continue;
            }
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
        out
    };
    let current_hash = capability_set_hash(filtered);
    stored.unwrap() != current_hash
}

// ---------------------------------------------------------------------------
// Re-export helpers for external consumers / tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn capability_registry_has_seven() {
        assert_eq!(CAPABILITY_SPECS.len(), 7);
        assert!(is_valid_capability("tools.override"));
        assert!(is_valid_capability("gateway.platform_actions"));
        assert!(!is_valid_capability("unknown.cap"));
    }

    #[test]
    fn capability_set_hash_is_order_insensitive() {
        let h1 = capability_set_hash(vec!["tools.override".to_string(), "llm.model_override".to_string()]);
        let h2 = capability_set_hash(vec!["llm.model_override".to_string(), "tools.override".to_string()]);
        assert_eq!(h1, h2);
        let h_empty = capability_set_hash(Vec::<String>::new());
        assert_eq!(h_empty, hex_sha256(b""));
        // Known vector: empty set hash is sha256("")
        assert_eq!(h_empty, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn capability_set_hash_dedup() {
        let h1 = capability_set_hash(vec!["tools.override".to_string(), "tools.override".to_string()]);
        let h2 = capability_set_hash(vec!["tools.override".to_string()]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn parse_declared_capabilities_filters_unknown() {
        let raw = json!(["tools.override", "unknown.cap", "llm.model_override", 123, "tools.override"]);
        let out = parse_declared_capabilities(&raw, "test-plugin");
        assert_eq!(out, vec!["tools.override", "llm.model_override"]);
    }

    #[test]
    fn parse_declared_capabilities_empty_and_non_list() {
        assert_eq!(parse_declared_capabilities(&json!([]), "p"), Vec::<String>::new());
        assert_eq!(parse_declared_capabilities(&json!(null), "p"), Vec::<String>::new());
        assert_eq!(parse_declared_capabilities(&json!("not a list"), "p"), Vec::<String>::new());
        assert_eq!(parse_declared_capabilities(&json!(0), "p"), Vec::<String>::new());
    }

    #[test]
    fn granted_and_legacy_gate() {
        let cfg = json!({
            "plugins": {
                "entries": {
                    "myplugin": {
                        "granted_capabilities": ["tools.override"],
                        "llm": {"allow_model_override": true},
                        "allow_tool_override": false
                    }
                }
            }
        });
        let granted = granted_capabilities("myplugin", Some(&cfg));
        assert!(granted.contains("tools.override"));
        assert!(!granted.contains("llm.model_override"));

        // plugin_capability_granted via granted path
        assert!(plugin_capability_granted("myplugin", "tools.override", Some(&cfg)));
        // via legacy gate
        assert!(plugin_capability_granted("myplugin", "llm.model_override", Some(&cfg)));
        // not granted
        assert!(!plugin_capability_granted("myplugin", "llm.provider_override", Some(&cfg)));
        // unknown id denied
        assert!(!plugin_capability_granted("myplugin", "unknown.cap", Some(&cfg)));
    }

    #[test]
    fn legacy_gate_set_truthiness() {
        let spec = get_capability_spec("tools.override").unwrap();
        let mut entry = Map::new();
        entry.insert("allow_tool_override".to_string(), json!(true));
        assert!(legacy_gate_set(&entry, spec));
        entry.insert("allow_tool_override".to_string(), json!(false));
        assert!(!legacy_gate_set(&entry, spec));
        entry.insert("allow_tool_override".to_string(), json!(""));
        assert!(!legacy_gate_set(&entry, spec));
        entry.insert("allow_tool_override".to_string(), json!("yes"));
        assert!(legacy_gate_set(&entry, spec));
    }

    #[test]
    fn consent_hash_and_pending_and_declared_changed() {
        let cfg = json!({
            "plugins": {
                "entries": {
                    "myplugin": {
                        "granted_capabilities": ["tools.override"],
                        "capabilities_consent": {"hash": capability_set_hash(vec!["tools.override".to_string()]), "granted_at": "2026-08-12T00:00:00+00:00"}
                    }
                }
            }
        });
        assert_eq!(consent_hash("myplugin", Some(&cfg)).unwrap(), capability_set_hash(vec!["tools.override".to_string()]));
        let pending = pending_capabilities("myplugin", vec!["tools.override".to_string(), "llm.model_override".to_string()], Some(&cfg));
        assert_eq!(pending, vec!["llm.model_override"]);
        assert!(!declared_set_changed("myplugin", vec!["tools.override".to_string()], Some(&cfg)));
        assert!(declared_set_changed("myplugin", vec!["tools.override".to_string(), "llm.model_override".to_string()], Some(&cfg)));
        // No consent at all counts as changed
        assert!(declared_set_changed("unknown", vec!["tools.override".to_string()], Some(&cfg)));
    }

    #[test]
    fn record_consent_round_trip() {
        let tmp = std::env::temp_dir().join(format!("hermes-cap-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", &tmp); }
        // Ensure clean slate
        let _ = fs::remove_file(tmp.join("config.yaml"));
        let _ = fs::remove_file(tmp.join("config.json"));
        record_consent("plug1", vec!["tools.override".to_string()], vec!["tools.override".to_string(), "llm.model_override".to_string()]);
        let cfg = load_config_value();
        let granted = granted_capabilities("plug1", Some(&cfg));
        assert!(granted.contains("tools.override"));
        // Check legacy bridge
        let entry = plugin_entry("plug1", Some(&cfg));
        assert_eq!(entry.get("allow_tool_override"), Some(&json!(true)));
        // Second consent with additional cap should union
        record_consent("plug1", vec!["llm.model_override".to_string()], vec!["tools.override".to_string(), "llm.model_override".to_string()]);
        let cfg2 = load_config_value();
        let granted2 = granted_capabilities("plug1", Some(&cfg2));
        assert!(granted2.contains("tools.override"));
        assert!(granted2.contains("llm.model_override"));
        let entry2 = plugin_entry("plug1", Some(&cfg2));
        // llm legacy gate should be set
        assert_eq!(entry2.get("llm").and_then(|v| v.get("allow_model_override")), Some(&json!(true)));
        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
    }

    #[test]
    fn plugin_entry_fail_closed() {
        // Missing config returns empty
        let cfg = json!({});
        let entry = plugin_entry("nope", Some(&cfg));
        assert!(entry.is_empty());
        // Corrupt plugins field
        let cfg2 = json!({"plugins": "not an object"});
        let entry2 = plugin_entry("nope", Some(&cfg2));
        assert!(entry2.is_empty());
        // Corrupt entry type
        let cfg3 = json!({"plugins": {"entries": {"myplugin": "not an object"}}});
        let entry3 = plugin_entry("myplugin", Some(&cfg3));
        assert!(entry3.is_empty());
    }
}
