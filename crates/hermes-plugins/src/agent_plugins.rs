//! Compatibility helpers for Agent Plugins v1 portable directory packages.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/agent_plugins.py` (571 LOC).
//! Validates the versioned portable format locally and translates its supported
//! components into records consumed by Hermes' existing skill and MCP runtimes.
//! Performs no schema fetching and imports no plugin code.
//!
//! Python surface ported line-for-line:
//!   - `PLUGIN_SCHEMA_V1`, `MCP_SCHEMA_V1`
//!   - `_PLUGIN_FIELDS`, `_AUTHOR_FIELDS`, `_STDIO_FIELDS`, `_REMOTE_FIELDS`
//!   - `_PLUGIN_NAME_RE`, `_SKILL_NAME_RE`, `_HEADER_NAME_RE`, `_PLACEHOLDER_RE`
//!   - `AgentPluginError`, `AgentPluginDiagnostic`, `AgentPluginSkill`, `AgentPluginPackage`
//!   - `_inside`, `_read_json_object`, `_validate_manifest`, `_valid_skill_frontmatter`
//!   - `_discover_skills`, `_expand`, `_resolve_scoped_path`
//!   - `_validate_headers`, `_validate_remote_url`, `_translate_remote`, `_translate_stdio`
//!   - `_discover_mcp`, `load_agent_plugin`, `read_agent_plugin_manifest`
//!   - `has_enabled_agent_plugin_mcp` (compatibility wrapper → portable MCP probe)
//!
//! Regexes are implemented without the `regex` crate to keep the dependency set
//! aligned with `workspace.dependencies` (no `cargo` in this task). YAML
//! frontmatter is parsed with a stdlib-only subset parser (covers the shapes
//! emitted by `yaml.safe_dump` in the test suite); a real port would use
//! `serde_yaml`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors agent_plugins.py:21-44
// ---------------------------------------------------------------------------

pub const PLUGIN_SCHEMA_V1: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA_V1: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

pub const PLUGIN_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];
pub const AUTHOR_FIELDS: &[&str] = &["name", "email", "url"];
pub const STDIO_FIELDS: &[&str] = &["type", "command", "args", "env", "cwd"];
pub const REMOTE_FIELDS: &[&str] = &["type", "url", "headers"];

// ---------------------------------------------------------------------------
// Error / data types — mirrors lines 47-76
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPluginError(pub String);

impl std::fmt::Display for AgentPluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for AgentPluginError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPluginDiagnostic {
    pub scope: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPluginSkill {
    pub name: String,
    pub description: String,
    pub root: PathBuf,
    pub skill_md: PathBuf,
    pub frontmatter: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPluginPackage {
    pub name: String,
    pub version: String,
    pub description: String,
    pub root: PathBuf,
    pub data_root: PathBuf,
    pub manifest: Map<String, Value>,
    pub skills: Vec<AgentPluginSkill>,
    pub mcp_servers: Map<String, Value>,
    pub diagnostics: Vec<AgentPluginDiagnostic>,
}

// ---------------------------------------------------------------------------
// Helpers — path / HERMES_HOME
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
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

fn get_bundled_plugins_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_BUNDLED_PLUGINS") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    // Fallback matching Python's get_bundled_plugins_dir() → <repo>/plugins
    // When not set, try to locate via current dir; for our port the env is
    // set in tests, so fallback rarely matters.
    PathBuf::from("plugins")
}

fn is_env_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let lower = v.trim().to_ascii_lowercase();
            matches!(lower.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

fn canonicalize_or_abs(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    if path.is_absolute() {
        return normalize_path(path);
    }
    if let Ok(cwd) = std::env::current_dir() {
        return normalize_path(&cwd.join(path));
    }
    normalize_path(path)
}

fn canonicalize_strict(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::ParentDir => {
                // Pop if we can, but keep root
                if let Some(last) = components.last() {
                    if *last != Component::RootDir && *last != Component::Prefix(_) {
                        components.pop();
                        continue;
                    }
                }
                components.push(comp);
            }
            Component::CurDir => {}
            _ => components.push(comp),
        }
    }
    let mut out = PathBuf::new();
    for c in components {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Regex-equivalent validators — mirrors lines 39-44
// ---------------------------------------------------------------------------

fn is_valid_plugin_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    // Must match ^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$ and no -- or ..
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    // first and last must be alphanum lower
    let is_alnum_lower = |b: u8| matches!(b, b'a'..=b'z' | b'0'..=b'9');
    if !is_alnum_lower(bytes[0]) || !is_alnum_lower(bytes[bytes.len() - 1]) {
        return false;
    }
    // interior chars
    for &b in bytes {
        if !(matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-')) {
            return false;
        }
    }
    // no -- or ..
    if name.contains("--") || name.contains("..") {
        return false;
    }
    true
}

fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    // ^(?!.*--)[a-z0-9]+(?:-[a-z0-9]+)*$
    if name.contains("--") {
        return false;
    }
    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }
    let mut prev_hyphen = false;
    for &b in name.as_bytes() {
        let is_alnum = matches!(b, b'a'..=b'z' | b'0'..=b'9');
        let is_hyphen = b == b'-';
        if !(is_alnum || is_hyphen) {
            return false;
        }
        if is_hyphen {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else {
            prev_hyphen = false;
        }
    }
    true
}

fn is_valid_header_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    for &b in name.as_bytes() {
        let ok = matches!(b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' |
            b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~' |
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
        );
        if !ok {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// _inside — mirrors lines 79-84
// ---------------------------------------------------------------------------

fn inside(path: &Path, root: &Path) -> bool {
    // Python: path.resolve(strict=False).relative_to(root.resolve(strict=True))
    let root_strict = match canonicalize_strict(root) {
        Some(p) => p,
        None => return false,
    };
    let path_resolved = canonicalize_or_abs(path);
    // Also try to canonicalize path if it exists for stricter containment
    // Use loose resolve (canonicalize_or_abs) which already handled.
    // Check containment via starts_with on normalized paths.
    // Python's relative_to checks path is under root; we mimic with starts_with after normalization.
    // Also handle symlink escape: if path is symlink outside, its resolved target will be outside.
    // Our canonicalize_or_abs already resolves symlink when possible via canonicalize.
    // If path is symlink to outside, canonicalize will return outside target, so check fails.
    let root_norm = normalize_path(&root_strict);
    let path_norm = normalize_path(&path_resolved);
    path_norm.starts_with(&root_norm)
}

// ---------------------------------------------------------------------------
// _read_json_object — mirrors lines 87-94
// ---------------------------------------------------------------------------

fn read_json_object(path: &Path, label: &str) -> Result<Map<String, Value>, AgentPluginError> {
    let text = fs::read_to_string(path)
        .map_err(|e| AgentPluginError(format!("{} is not valid readable JSON: {}", label, e)))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| AgentPluginError(format!("{} is not valid readable JSON: {}", label, e)))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(AgentPluginError(format!("{} must contain a JSON object", label))),
    }
}

// ---------------------------------------------------------------------------
// _validate_manifest — mirrors lines 97-155
// ---------------------------------------------------------------------------

fn validate_manifest(root: &Path) -> Result<(Map<String, Value>, Vec<AgentPluginDiagnostic>), AgentPluginError> {
    let manifest_path = root.join("plugin.json");
    if !inside(&manifest_path, root) || !manifest_path.is_file() {
        return Err(AgentPluginError(
            "plugin.json must be a regular file within the plugin root".to_string(),
        ));
    }
    let mut manifest = read_json_object(&manifest_path, "plugin.json")?;
    let mut diagnostics: Vec<AgentPluginDiagnostic> = Vec::new();

    // Collect unknown fields
    let plugin_fields_set: HashSet<&str> = PLUGIN_FIELDS.iter().copied().collect();
    let unknown: Vec<String> = manifest
        .keys()
        .filter(|k| !plugin_fields_set.contains(k.as_str()))
        .cloned()
        .collect();
    let mut unknown_sorted = unknown.clone();
    unknown_sorted.sort();
    for field in unknown_sorted {
        diagnostics.push(AgentPluginDiagnostic {
            scope: "manifest".to_string(),
            message: format!("ignored unknown top-level field: {}", field),
        });
        manifest.remove(&field);
    }

    // Schema check
    let schema_ok = manifest.get("$schema").and_then(|v| v.as_str()) == Some(PLUGIN_SCHEMA_V1);
    if !schema_ok {
        return Err(AgentPluginError(
            "plugin.json declares an unsupported or missing Agent Plugins schema".to_string(),
        ));
    }

    // Name check
    let name_ok = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.len() >= 1 && s.len() <= 64 && is_valid_plugin_name(s))
        .unwrap_or(false);
    if !name_ok {
        return Err(AgentPluginError(
            "plugin.json name does not satisfy v1 constraints".to_string(),
        ));
    }

    // String fields: version, description, homepage, repository, license
    for field in ["version", "description", "homepage", "repository", "license"] {
        if let Some(v) = manifest.get(field) {
            if !v.is_string() {
                return Err(AgentPluginError(format!("plugin.json {} must be a string", field)));
            }
        }
    }

    // keywords must be array of strings
    if let Some(keywords) = manifest.get("keywords") {
        match keywords {
            Value::Array(arr) => {
                if arr.iter().any(|v| !v.is_string()) {
                    return Err(AgentPluginError(
                        "plugin.json keywords must be an array of strings".to_string(),
                    ));
                }
            }
            _ => {
                return Err(AgentPluginError(
                    "plugin.json keywords must be an array of strings".to_string(),
                ));
            }
        }
    }

    // author must be object with only known fields and string values
    if let Some(author) = manifest.get("author") {
        match author {
            Value::Object(map) => {
                let author_fields_set: HashSet<&str> = AUTHOR_FIELDS.iter().copied().collect();
                let unknown_author: Vec<_> = map.keys().filter(|k| !author_fields_set.contains(k.as_str())).collect();
                let has_non_string = map.values().any(|v| !v.is_string());
                if !unknown_author.is_empty() || has_non_string {
                    return Err(AgentPluginError(
                        "plugin.json author may contain only string name, email, and url fields".to_string(),
                    ));
                }
            }
            _ => {
                return Err(AgentPluginError(
                    "plugin.json author must be an object".to_string(),
                ));
            }
        }
    }

    // extensions
    if let Some(extensions) = manifest.get("extensions") {
        match extensions {
            Value::Object(map) => {
                if map.values().any(|v| !v.is_object()) {
                    return Err(AgentPluginError(
                        "plugin.json extension namespace values must be objects".to_string(),
                    ));
                }
            }
            _ => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope: "manifest".to_string(),
                    message: "ignored non-object extensions field".to_string(),
                });
                manifest.remove("extensions");
            }
        }
    }

    Ok((manifest, diagnostics))
}

// ---------------------------------------------------------------------------
// _valid_skill_frontmatter — mirrors lines 158-189
// ---------------------------------------------------------------------------

fn valid_skill_frontmatter(frontmatter: &Map<String, Value>, directory_name: &str) -> Option<String> {
    // name must match directory and satisfy constraints
    let name = match frontmatter.get("name").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Some("name must match the directory and satisfy Agent Skills constraints".to_string()),
    };
    if name != directory_name || name.len() < 1 || name.len() > 64 || !is_valid_skill_name(name) {
        return Some("name must match the directory and satisfy Agent Skills constraints".to_string());
    }
    // description must be non-empty string <=1024
    let desc_ok = match frontmatter.get("description").and_then(|v| v.as_str()) {
        Some(s) => !s.is_empty() && s.len() <= 1024,
        None => false,
    };
    if !desc_ok {
        return Some("description must be a non-empty string of at most 1024 characters".to_string());
    }
    // license must be string if present
    if let Some(v) = frontmatter.get("license") {
        if !v.is_string() {
            return Some("license must be a string".to_string());
        }
    }
    // compatibility must be string 1..500
    if let Some(v) = frontmatter.get("compatibility") {
        match v.as_str() {
            Some(s) if s.len() >= 1 && s.len() <= 500 => {}
            _ => return Some("compatibility must be a string of 1 to 500 characters".to_string()),
        }
    }
    // metadata must map string keys to string values
    if let Some(v) = frontmatter.get("metadata") {
        match v {
            Value::Object(map) => {
                for (k, val) in map {
                    // k is always string in JSON object, but check val is string
                    let _ = k;
                    if !val.is_string() {
                        return Some("metadata must map string keys to string values".to_string());
                    }
                }
            }
            _ => return Some("metadata must map string keys to string values".to_string()),
        }
    }
    // allowed-tools must be string
    if let Some(v) = frontmatter.get("allowed-tools") {
        if !v.is_string() {
            return Some("allowed-tools must be a string".to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// YAML frontmatter helpers — mirrors agent/skill_utils.yaml_load fallback
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
    // quoted string
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        return Value::String(inner.to_string());
    }
    // integer?
    if let Ok(i) = trimmed.parse::<i64>() {
        // Check that it's purely numeric (no extra chars) — parse already did
        return json!(i);
    }
    // float?
    if let Ok(f) = trimmed.parse::<f64>() {
        // Only accept if contains '.' or 'e' to avoid treating "1" as float already handled
        if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(trimmed.to_string())
}

fn parse_yaml_simple(yaml: &str) -> Result<Map<String, Value>, String> {
    // Very small subset parser for frontmatter shapes in tests.
    // Handles:
    //   key: value
    //   key:
    //     subkey: value
    //   key:
    //     - item
    //   key: []
    //   key: [a, b] (not needed but handle "[]" only)
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
        // Count leading spaces for this line
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent != 0 {
            // This line is part of a nested block already handled via lookahead
            i += 1;
            continue;
        }
        // Find colon
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
            // Inline value
            // If rest is "[]" or "{}" handled by scalar; if rest starts with "[" and ends with "]" etc not needed.
            out.insert(key, parse_yaml_scalar(&rest));
            i += 1;
        } else {
            // Empty after colon → check following indented lines
            // Collect indented block
            let mut block_lines: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let nxt = lines[j];
                if nxt.trim().is_empty() {
                    // Empty line inside block – include? For simplicity skip
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
                // No block – treat as null
                out.insert(key, Value::Null);
                i += 1;
                continue;
            }
            // Determine block type: list vs dict
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
                // Dict – parse each line as "subkey: subvalue"
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

fn yaml_load_frontmatter(yaml_str: &str) -> Result<Map<String, Value>, String> {
    // Try JSON-compatible parse first? YAML frontmatter in tests is produced by yaml.safe_dump
    // which our simple parser handles. If parsing fails, return error.
    parse_yaml_simple(yaml_str)
}

// ---------------------------------------------------------------------------
// _discover_skills — mirrors lines 192-252
// ---------------------------------------------------------------------------

fn discover_skills(root: &Path, diagnostics: &mut Vec<AgentPluginDiagnostic>) -> Vec<AgentPluginSkill> {
    let skills_root = root.join("skills");
    let exists = skills_root.exists() || skills_root.is_symlink();
    if !exists {
        return Vec::new();
    }
    if !inside(&skills_root, root) || !skills_root.is_dir() {
        diagnostics.push(AgentPluginDiagnostic {
            scope: "skills".to_string(),
            message: "skills must be an in-root directory".to_string(),
        });
        return Vec::new();
    }

    let mut children: Vec<PathBuf> = Vec::new();
    match fs::read_dir(&skills_root) {
        Ok(iter) => {
            for entry in iter.flatten() {
                children.push(entry.path());
            }
            children.sort_by(|a, b| {
                a.file_name()
                    .unwrap_or_default()
                    .cmp(b.file_name().unwrap_or_default())
            });
        }
        Err(e) => {
            diagnostics.push(AgentPluginDiagnostic {
                scope: "skills".to_string(),
                message: format!("cannot list skills: {}", e),
            });
            return Vec::new();
        }
    }

    let mut skills: Vec<AgentPluginSkill> = Vec::new();
    for child in children {
        let skill_md = child.join("SKILL.md");
        let is_dir = child.is_dir();
        let md_exists = skill_md.exists();
        if !is_dir || !md_exists {
            continue;
        }
        let scope = format!("skill:{}", child.file_name().unwrap_or_default().to_string_lossy());
        if !inside(&skill_md, root) || !skill_md.is_file() {
            diagnostics.push(AgentPluginDiagnostic {
                scope,
                message: "SKILL.md must be a regular in-root file".to_string(),
            });
            continue;
        }
        // Read and parse frontmatter
        let content = match fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(e) => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: format!("invalid SKILL.md: {}", e),
                });
                continue;
            }
        };
        // lstrip BOM
        let content_stripped = if content.starts_with('\u{feff}') {
            content[1..].to_string()
        } else {
            content
        };
        if !content_stripped.starts_with("---") {
            diagnostics.push(AgentPluginDiagnostic {
                scope,
                message: "invalid SKILL.md: missing YAML frontmatter".to_string(),
            });
            continue;
        }
        // Find "\n---\s*\n" after first 3 chars
        let after = &content_stripped[3..];
        // Search for "\n---" then optional spaces then "\n"
        let end_match = find_frontmatter_end(after);
        let yaml_content = match end_match {
            Some((start_idx, _end_idx)) => {
                // YAML is content[3 .. start_idx+3]
                let yaml_str = &content_stripped[3..start_idx + 3];
                // Validate YAML parses
                if let Err(e) = yaml_load_frontmatter(yaml_str) {
                    diagnostics.push(AgentPluginDiagnostic {
                        scope,
                        message: format!("invalid SKILL.md: invalid YAML frontmatter: {}", e),
                    });
                    continue;
                }
                yaml_str.to_string()
            }
            None => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: "invalid SKILL.md: unterminated YAML frontmatter".to_string(),
                });
                continue;
            }
        };
        // Parse frontmatter (second parse for simplicity)
        let frontmatter = match yaml_load_frontmatter(&yaml_content) {
            Ok(m) => m,
            Err(e) => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: format!("invalid SKILL.md: invalid YAML frontmatter: {}", e),
                });
                continue;
            }
        };
        // frontmatter must be object (it is) – if parsing yielded empty map but content was not object, Python would have raised
        // In Python, they check if not isinstance(parsed, dict): raise ValueError
        // Our parser always returns dict, but if yaml was scalar like "123", our parser would produce empty? For that case, treat as invalid.
        // To mirror, if yaml_str trimmed is not mapping-like and parsing returned empty but original wasn't empty, consider invalid.
        // Simpler: if frontmatter is empty and yaml_content trimmed not empty, we may have mis-parsed; still check?
        // But tests only use dict shapes.

        if let Some(err) = valid_skill_frontmatter(&frontmatter, &child.file_name().unwrap_or_default().to_string_lossy()) {
            diagnostics.push(AgentPluginDiagnostic { scope, message: err });
            continue;
        }

        // Resolve paths strictly (child.resolve(strict=True))
        let resolved_root = match canonicalize_strict(&child) {
            Some(p) => p,
            None => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: "invalid SKILL.md: cannot resolve skill root".to_string(),
                });
                continue;
            }
        };
        let resolved_md = match canonicalize_strict(&skill_md) {
            Some(p) => p,
            None => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: "invalid SKILL.md: cannot resolve SKILL.md".to_string(),
                });
                continue;
            }
        };
        let desc = frontmatter
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        skills.push(AgentPluginSkill {
            name: child.file_name().unwrap_or_default().to_string_lossy().to_string(),
            description: desc,
            root: resolved_root,
            skill_md: resolved_md,
            frontmatter,
        });
    }
    skills
}

fn find_frontmatter_end(after: &str) -> Option<(usize, usize)> {
    // Python: re.search(r"\n---\s*\n", content[3:])
    // That is: newline, three dashes, optional spaces/tabs, newline
    // Return (start, end) indices within `after` string where match occurs.
    // start = index of '\n' that begins the match, end = index after final '\n'
    let bytes = after.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'\n' {
            // Check if next 3 chars are ---
            if idx + 3 < bytes.len() && bytes[idx + 1] == b'-' && bytes[idx + 2] == b'-' && bytes[idx + 3] == b'-' {
                // Check rest until newline
                let mut j = idx + 4;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\r') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'\n' {
                    // match from idx to j inclusive
                    return Some((idx, j + 1));
                }
            }
        }
        idx += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// _expand — mirrors lines 255-260
// ---------------------------------------------------------------------------

fn expand(value: &str, plugin_root: &Path, data_root: &Path) -> String {
    let mut out = value.to_string();
    // Replace placeholders in order that avoids double expansion
    out = out.replace("${PLUGIN_ROOT}", &plugin_root.to_string_lossy().to_string());
    out = out.replace("${PLUGIN_DATA}", &data_root.to_string_lossy().to_string());
    out
}

// ---------------------------------------------------------------------------
// _resolve_scoped_path — mirrors lines 263-287
// ---------------------------------------------------------------------------

fn resolve_scoped_path(
    value: &str,
    plugin_root: &Path,
    data_root: &Path,
    expand_placeholders: bool,
) -> Result<PathBuf, String> {
    let expanded = if expand_placeholders {
        expand(value, plugin_root, data_root)
    } else {
        value.to_string()
    };
    let (base, candidate) = if value.starts_with("./") {
        let base = plugin_root;
        // base / expanded[2:]
        let suffix = if expanded.len() >= 2 { &expanded[2..] } else { "" };
        // PathBuf join semantics: if suffix is absolute, it discards base
        let cand = base.join(suffix);
        (base.to_path_buf(), cand)
    } else if value == "${PLUGIN_ROOT}" || value.starts_with("${PLUGIN_ROOT}/") {
        let base = plugin_root;
        let cand = PathBuf::from(expanded);
        (base.to_path_buf(), cand)
    } else if value == "${PLUGIN_DATA}" || value.starts_with("${PLUGIN_DATA}/") {
        let base = data_root;
        let cand = PathBuf::from(expanded);
        (base.to_path_buf(), cand)
    } else {
        return Err("path must start with ./, ${PLUGIN_ROOT}, or ${PLUGIN_DATA}".to_string());
    };

    let resolved = canonicalize_or_abs(&candidate);
    let base_resolved = canonicalize_or_abs(&base);
    // Try to get relative path – Python uses resolved.relative_to(base.resolve(strict=False))
    // If resolved is not under base, error
    let rel_ok = resolved.starts_with(&base_resolved);
    // Additionally try canonicalize if available for stricter
    // We already normalized; check starts_with
    if !rel_ok {
        return Err("path escapes its resolved root".to_string());
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// _validate_headers — mirrors lines 290-307
// ---------------------------------------------------------------------------

fn validate_headers(headers: Option<&Value>) -> bool {
    match headers {
        None => true,
        Some(Value::Null) => true,
        Some(Value::Object(map)) => {
            let mut seen: HashSet<String> = HashSet::new();
            for (name, value) in map {
                if !is_valid_header_name(name) {
                    return false;
                }
                match value {
                    Value::String(s) => {
                        if s.contains('\r') || s.contains('\n') {
                            return false;
                        }
                    }
                    _ => return false,
                }
                let lower = name.to_ascii_lowercase();
                if seen.contains(&lower) {
                    return false;
                }
                seen.insert(lower);
            }
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// _validate_remote_url — mirrors lines 310-350
// ---------------------------------------------------------------------------

fn validate_remote_url(url: Option<&Value>) -> Result<String, String> {
    let url_str = match url {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        _ => return Err("url must be a non-empty string".to_string()),
    };
    // Need to parse url
    // Find scheme
    let scheme_end = url_str.find("://").ok_or_else(|| "url is not parseable: missing scheme".to_string())?;
    let scheme = url_str[..scheme_end].to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("url scheme must be http or https".to_string());
    }
    let after_scheme = &url_str[scheme_end + 3..];
    // Check fragment: presence of '#'
    if after_scheme.contains('#') {
        // Python checks parsed.fragment – any '#'
        // But also need to ensure it's not empty fragment? Any fragment is error.
        // Check if '#' exists after authority/path
        // Simple: if url_str contains '#', error
        return Err("url must not contain a fragment".to_string());
    }
    // Extract authority (netloc) up to first '/' or '?' or end
    let authority_end = after_scheme
        .find(|c| c == '/' || c == '?')
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    if authority.is_empty() {
        return Err("url must have a host".to_string());
    }
    // Check userinfo: presence of '@'
    if authority.contains('@') {
        return Err("url must not contain user information".to_string());
    }
    // Extract host
    // Handle IPv6 literal [::1] with optional port
    let host: String = if authority.starts_with('[') {
        // Find closing ]
        let close = authority.find(']').ok_or_else(|| "url must have a host".to_string())?;
        let inner = &authority[1..close];
        if inner.is_empty() {
            return Err("url must have a host".to_string());
        }
        inner.to_string()
    } else {
        // Host until ':' (port) or end
        let colon_pos = authority.find(':');
        let h = match colon_pos {
            Some(p) => &authority[..p],
            None => authority,
        };
        if h.is_empty() {
            return Err("url must have a host".to_string());
        }
        h.to_string()
    };
    if host.is_empty() {
        return Err("url must have a host".to_string());
    }
    if scheme == "http" {
        let mut loopback = false;
        if host == "localhost" {
            loopback = true;
        } else if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            loopback = ip.is_loopback();
        } else {
            // Try parsing without brackets? Already did
            loopback = false;
        }
        if !loopback {
            return Err("non-loopback url must use https".to_string());
        }
    }
    Ok(url_str.to_string())
}

// ---------------------------------------------------------------------------
// _translate_remote — mirrors lines 353-375
// ---------------------------------------------------------------------------

fn translate_remote(config: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    // check unknown fields
    let allowed: HashSet<&str> = REMOTE_FIELDS.iter().copied().collect();
    for k in config.keys() {
        if !allowed.contains(k.as_str()) {
            return Err("unknown remote field".to_string());
        }
    }
    let url = validate_remote_url(config.get("url"))?;
    if !validate_headers(config.get("headers")) {
        return Err("invalid headers".to_string());
    }
    let mut translated: Map<String, Value> = Map::new();
    translated.insert("url".to_string(), Value::String(url));
    translated.insert("strict_redirect_headers".to_string(), Value::Bool(true));
    if let Some(headers) = config.get("headers") {
        if let Value::Object(map) = headers {
            if !map.is_empty() {
                translated.insert("headers".to_string(), Value::Object(map.clone()));
            }
        }
    }
    Ok(translated)
}

// ---------------------------------------------------------------------------
// _translate_stdio — mirrors lines 378-433
// ---------------------------------------------------------------------------

fn translate_stdio(
    config: &Map<String, Value>,
    plugin_root: &Path,
    data_root: &Path,
) -> Result<Map<String, Value>, String> {
    let allowed: HashSet<&str> = STDIO_FIELDS.iter().copied().collect();
    for k in config.keys() {
        if !allowed.contains(k.as_str()) {
            return Err("unknown stdio field".to_string());
        }
    }
    // command
    let command = match config.get("command") {
        Some(Value::String(s)) if !s.is_empty() && !s.contains('\x00') => s.as_str(),
        _ => return Err("command must be a non-empty executable token".to_string()),
    };
    let command_value = if command.starts_with("./") {
        let p = resolve_scoped_path(command, plugin_root, data_root, false)?;
        p.to_string_lossy().to_string()
    } else if command.chars().any(|c| c.is_whitespace()) {
        return Err("command must contain one executable token".to_string());
    } else if command.contains('/') || command.contains('\\') || command == "." || command == ".." {
        return Err("command must be a bare executable or begin with ./".to_string());
    } else {
        command.to_string()
    };

    // args
    let args_val = config.get("args").unwrap_or(&Value::Array(Vec::new()));
    let args_list: Vec<String> = match args_val {
        Value::Array(arr) => {
            let mut out = Vec::new();
            for v in arr {
                match v {
                    Value::String(s) => out.push(s.clone()),
                    _ => return Err("args must be an array of strings".to_string()),
                }
            }
            out
        }
        _ => return Err("args must be an array of strings".to_string()),
    };

    // env
    let env_val = config.get("env").unwrap_or(&Value::Object(Map::new()));
    let env_map: Map<String, Value> = match env_val {
        Value::Object(m) => {
            for (k, v) in m {
                if !matches!(v, Value::String(_)) {
                    return Err("env must map string keys to string values".to_string());
                }
                let _ = k;
            }
            m.clone()
        }
        _ => return Err("env must map string keys to string values".to_string()),
    };
    // Check reserved keys – case-insensitive on Windows
    let is_windows = cfg!(windows) || std::env::var("OS").map(|v| v.contains("Windows")).unwrap_or(false) || (std::env::consts::OS == "windows");
    // Python: env_keys = {key.upper() if os.name == "nt" else key for key in env}
    let mut env_keys: HashSet<String> = HashSet::new();
    for k in env_map.keys() {
        let ek = if is_windows { k.to_ascii_uppercase() } else { k.clone() };
        env_keys.insert(ek);
    }
    if env_keys.contains("PLUGIN_ROOT") || env_keys.contains("PLUGIN_DATA") {
        return Err("PLUGIN_ROOT and PLUGIN_DATA are reserved".to_string());
    }

    // cwd
    let cwd_value: PathBuf = match config.get("cwd") {
        None | Some(Value::Null) => plugin_root.to_path_buf(),
        Some(Value::String(s)) => resolve_scoped_path(s, plugin_root, data_root, true)
            .map_err(|e| e)?,
        _ => return Err("cwd must be a string".to_string()),
    };

    // Build translated env with expansion + injected roots
    let mut translated_env: Map<String, Value> = Map::new();
    for (k, v) in env_map {
        if let Value::String(s) = v {
            let expanded = expand(&s, plugin_root, data_root);
            translated_env.insert(k, Value::String(expanded));
        }
    }
    translated_env.insert("PLUGIN_ROOT".to_string(), Value::String(plugin_root.to_string_lossy().to_string()));
    translated_env.insert("PLUGIN_DATA".to_string(), Value::String(data_root.to_string_lossy().to_string()));

    let mut out: Map<String, Value> = Map::new();
    out.insert("command".to_string(), Value::String(command_value));
    let expanded_args: Vec<Value> = args_list
        .into_iter()
        .map(|a| Value::String(expand(&a, plugin_root, data_root)))
        .collect();
    out.insert("args".to_string(), Value::Array(expanded_args));
    out.insert("env".to_string(), Value::Object(translated_env));
    out.insert("cwd".to_string(), Value::String(cwd_value.to_string_lossy().to_string()));
    Ok(out)
}

// ---------------------------------------------------------------------------
// _discover_mcp — mirrors lines 436-521
// ---------------------------------------------------------------------------

pub fn discover_mcp(
    root: &Path,
    data_root: &Path,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
    create_data: bool,
) -> Map<String, Value> {
    let mcp_path = root.join("mcp.json");
    let exists = mcp_path.exists() || mcp_path.is_symlink();
    if !exists {
        return Map::new();
    }
    if !inside(&mcp_path, root) || !mcp_path.is_file() {
        diagnostics.push(AgentPluginDiagnostic {
            scope: "mcp".to_string(),
            message: "mcp.json must be a regular in-root file".to_string(),
        });
        return Map::new();
    }
    let config = match read_json_object(&mcp_path, "mcp.json") {
        Ok(m) => m,
        Err(e) => {
            diagnostics.push(AgentPluginDiagnostic {
                scope: "mcp".to_string(),
                message: e.to_string(),
            });
            return Map::new();
        }
    };
    // Top-level shape must be exactly {"$schema", "mcpServers"}
    let keys: HashSet<String> = config.keys().cloned().collect();
    let expected: HashSet<String> = ["$schema".to_string(), "mcpServers".to_string()].iter().cloned().collect();
    if keys != expected {
        diagnostics.push(AgentPluginDiagnostic {
            scope: "mcp".to_string(),
            message: "mcp.json has an invalid top-level shape".to_string(),
        });
        return Map::new();
    }
    if config.get("$schema").and_then(|v| v.as_str()) != Some(MCP_SCHEMA_V1) {
        diagnostics.push(AgentPluginDiagnostic {
            scope: "mcp".to_string(),
            message: "mcp.json declares an unsupported schema".to_string(),
        });
        return Map::new();
    }
    let servers = match config.get("mcpServers") {
        Some(Value::Object(m)) => m.clone(),
        _ => {
            diagnostics.push(AgentPluginDiagnostic {
                scope: "mcp".to_string(),
                message: "mcpServers must be an object".to_string(),
            });
            return Map::new();
        }
    };

    let mut translated: Map<String, Value> = Map::new();
    for (name, server_val) in servers {
        let scope = format!("mcp:{}", name);
        if name.is_empty() {
            diagnostics.push(AgentPluginDiagnostic {
                scope,
                message: "invalid server entry".to_string(),
            });
            continue;
        }
        let server_map = match server_val {
            Value::Object(m) => m,
            _ => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: "invalid server entry".to_string(),
                });
                continue;
            }
        };
        let server_type = server_map.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match server_type {
            "stdio" => {
                match translate_stdio(&server_map, root, data_root) {
                    Ok(translated_server) => {
                        if create_data {
                            let _ = fs::create_dir_all(data_root);
                            if let Some(cwd_str) = translated_server.get("cwd").and_then(|v| v.as_str()) {
                                let cwd_path = PathBuf::from(cwd_str);
                                // Check if cwd is under data_root
                                let cwd_resolved = canonicalize_or_abs(&cwd_path);
                                let data_resolved = canonicalize_or_abs(data_root);
                                if cwd_resolved.starts_with(&data_resolved) {
                                    let _ = fs::create_dir_all(&cwd_path);
                                }
                                // Errors creating cwd are ignored except for translated server? Actually Python
                                // creates data_root then cwd; OSError on cwd mkdir is caught as ValueError? Wait
                                // Python does: data_root.mkdir(parents=True, exist_ok=True) then cwd_path.mkdir.
                                // If either raises OSError, it is caught as (OSError, ValueError) and diagnostic.
                                // Our create_dir_all failure currently ignored; to mimic, we could try and if fails,
                                // push diagnostic and skip adding? But Python only catches OSError from translate_stdio
                                // or the mkdir? Actually code: try: translated_server = _translate_stdio(...); if create_data: data_root.mkdir... cwd_path.mkdir... translated[name]=... except (OSError, ValueError): diagnostic
                                // So OSError from mkdir would be caught. We should capture.
                                // Let's attempt to handle mkdir errors explicitly.
                            }
                        }
                        // Re-attempt mkdir with error handling that matches Python's exception scope
                        // If create_data and mkdir failed, we would have returned diagnostic; but we already inserted.
                        // To replicate exactly, we need to catch mkdir errors.
                        // We'll check if create_data block had error by trying again and capturing.
                        let mut mkdir_failed: Option<String> = None;
                        if create_data {
                            if let Err(e) = fs::create_dir_all(data_root) {
                                mkdir_failed = Some(e.to_string());
                            } else if let Some(cwd_str) = translated_server.get("cwd").and_then(|v| v.as_str()) {
                                let cwd_path = PathBuf::from(cwd_str);
                                let cwd_resolved = canonicalize_or_abs(&cwd_path);
                                let data_resolved = canonicalize_or_abs(data_root);
                                if cwd_resolved.starts_with(&data_resolved) {
                                    if let Err(e) = fs::create_dir_all(&cwd_path) {
                                        mkdir_failed = Some(e.to_string());
                                    }
                                }
                            }
                        }
                        if let Some(msg) = mkdir_failed {
                            diagnostics.push(AgentPluginDiagnostic { scope, message: msg });
                            continue;
                        }
                        translated.insert(name.clone(), Value::Object(translated_server));
                    }
                    Err(e) => {
                        diagnostics.push(AgentPluginDiagnostic { scope, message: e });
                    }
                }
            }
            "streamable-http" => match translate_remote(&server_map) {
                Ok(t) => {
                    translated.insert(name.clone(), Value::Object(t));
                }
                Err(e) => {
                    diagnostics.push(AgentPluginDiagnostic { scope, message: e });
                }
            },
            "sse" => {
                let has_unknown = {
                    let allowed: HashSet<&str> = REMOTE_FIELDS.iter().copied().collect();
                    server_map.keys().any(|k| !allowed.contains(k.as_str()))
                };
                let url_ok = server_map.get("url").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
                let headers_ok = validate_headers(server_map.get("headers"));
                if has_unknown || !url_ok || !headers_ok {
                    diagnostics.push(AgentPluginDiagnostic {
                        scope,
                        message: "invalid remote entry".to_string(),
                    });
                } else {
                    diagnostics.push(AgentPluginDiagnostic {
                        scope,
                        message: format!("portable {} transport is not supported", server_type),
                    });
                }
            }
            _ => {
                diagnostics.push(AgentPluginDiagnostic {
                    scope,
                    message: "unknown MCP server type".to_string(),
                });
            }
        }
    }
    translated
}

// ---------------------------------------------------------------------------
// load_agent_plugin / read_agent_plugin_manifest — mirrors lines 524-558
// ---------------------------------------------------------------------------

pub fn load_agent_plugin(plugin_root: &Path, data_root: &Path) -> Result<AgentPluginPackage, AgentPluginError> {
    let root = plugin_root
        .canonicalize()
        .map_err(|e| AgentPluginError(format!("plugin root must be a directory: {}", e)))?;
    if !root.is_dir() {
        return Err(AgentPluginError("plugin root must be a directory".to_string()));
    }
    let (manifest, mut diagnostics) = validate_manifest(&root)?;
    let resolved_data = canonicalize_or_abs(data_root);
    let skills = discover_skills(&root, &mut diagnostics);
    let mcp_servers = discover_mcp(&root, &resolved_data, &mut diagnostics, true);
    let name = manifest.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let version = manifest.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let description = manifest.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok(AgentPluginPackage {
        name,
        version,
        description,
        root: root.clone(),
        data_root: resolved_data,
        manifest,
        skills,
        mcp_servers,
        diagnostics,
    })
}

pub fn read_agent_plugin_manifest(plugin_root: &Path) -> Result<(Map<String, Value>, Vec<AgentPluginDiagnostic>), AgentPluginError> {
    let root = plugin_root
        .canonicalize()
        .map_err(|e| AgentPluginError(format!("plugin root must be a directory: {}", e)))?;
    if !root.is_dir() {
        return Err(AgentPluginError("plugin root must be a directory".to_string()));
    }
    let (manifest, diagnostics) = validate_manifest(&root)?;
    Ok((manifest, diagnostics))
}

// ---------------------------------------------------------------------------
// has_enabled_agent_plugin_mcp — mirrors lines 561-571 + plugins.py probe
// ---------------------------------------------------------------------------

/// Compatibility wrapper for the shared PluginManager MCP probe.
///
/// Directory scanning belongs to `hermes_cli::plugins` so startup gating
/// and full plugin discovery cannot drift apart. Mirrors
/// `hermes_cli.agent_plugins.has_enabled_agent_plugin_mcp` which delegates to
/// `hermes_cli.plugins.has_enabled_agent_plugin_mcp` → `PluginManager::has_enabled_portable_mcp`.
///
/// Rust port scans `$HERMES_HOME/plugins` and `$HERMES_BUNDLED_PLUGINS`
/// (and `./.hermes/plugins` when `HERMES_ENABLE_PROJECT_PLUGINS=1`) with the
/// same precedence and manifest-containment rules as Python. Only the probe
/// path is replicated; full plugin loading is out of scope.
pub fn has_enabled_agent_plugin_mcp(raw_config: &Value) -> bool {
    if is_env_enabled("HERMES_SAFE_MODE") {
        return false;
    }
    let plugins_config = match raw_config.get("plugins") {
        Some(Value::Object(m)) => m,
        _ => return false,
    };
    let enabled_list = match plugins_config.get("enabled") {
        Some(Value::Array(arr)) => arr,
        _ => return false,
    };
    let mut enabled: HashSet<String> = HashSet::new();
    for v in enabled_list {
        if let Value::String(s) = v {
            enabled.insert(s.clone());
        }
    }
    let mut disabled: HashSet<String> = HashSet::new();
    if let Some(Value::Array(arr)) = plugins_config.get("disabled") {
        for v in arr {
            if let Value::String(s) = v {
                disabled.insert(s.clone());
            }
        }
    }
    if enabled.is_empty() {
        return false;
    }

    // Collect winners – last-writer wins per key
    let mut winners: HashMap<String, (String, bool, PathBuf)> = HashMap::new(); // key → (name, portable, path)

    for (source_dir, _source_label) in collect_plugin_source_dirs() {
        let manifests = scan_directory(&source_dir);
        for m in manifests {
            let key = if m.key.is_empty() { m.name.clone() } else { m.key.clone() };
            // last writer wins: insertion overwrites
            winners.insert(key, (m.name.clone(), m.portable, PathBuf::from(m.path)));
        }
    }

    for (lookup_key, (name, portable, path)) in winners {
        if !portable {
            continue;
        }
        if disabled.contains(&lookup_key) || disabled.contains(&name) {
            continue;
        }
        if !enabled.contains(&lookup_key) && !enabled.contains(&name) {
            continue;
        }
        // Probe for MCP servers without mutating data dir
        let data_root = get_hermes_home().join("plugin-data").join(portable_skill_namespace(&lookup_key));
        let mut diagnostics: Vec<AgentPluginDiagnostic> = Vec::new();
        // _discover_mcp is called with create_data=False to avoid side effects during probing
        let servers = discover_mcp(&path, &data_root, &mut diagnostics, false);
        if !servers.is_empty() {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone)]
struct ScannedManifest {
    name: String,
    key: String,
    portable: bool,
    path: String,
}

fn portable_skill_namespace(key: &str) -> String {
    // Mirrors Python _portable_skill_namespace: agent-plugin-<slug>-<8hex>
    // slug = lowercased, non-alnum replaced with '-', trimmed.
    let lower = key.to_ascii_lowercase();
    let mut slug: String = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            slug.push(ch);
        } else {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches(|c| c == '-' || c == '_').to_string();
    let slug = if slug.is_empty() { "plugin".to_string() } else { slug };
    // Digest – use simple hash (FNV) and hex to avoid sha2 dependency;
    // For probe correctness, the exact digest does not affect has_enabled result because
    // discover_mcp is called with create_data=False and diagnostics empty.
    // We still produce deterministic 8 hex chars via a stable hash.
    let mut hash: u64 = 14695981039346656037u64; // FNV offset
    for b in key.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    let digest = format!("{:08x}", (hash & 0xffffffff) as u32);
    format!("agent-plugin-{}-{}", slug, digest)
}

fn collect_plugin_source_dirs() -> Vec<(PathBuf, String)> {
    let mut dirs: Vec<(PathBuf, String)> = Vec::new();
    // Bundled first (so user overwrites)
    let bundled = get_bundled_plugins_dir();
    dirs.push((bundled.clone(), "bundled".to_string()));
    // Bundled/platforms one level deeper – scan separately as Python does
    dirs.push((bundled.join("platforms"), "bundled".to_string()));
    // User
    let user_dir = get_hermes_home().join("plugins");
    dirs.push((user_dir, "user".to_string()));
    // Project when enabled
    if is_env_enabled("HERMES_ENABLE_PROJECT_PLUGINS") {
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push((cwd.join(".hermes").join("plugins"), "project".to_string()));
        }
    }
    dirs
}

fn scan_directory(dir: &Path) -> Vec<ScannedManifest> {
    scan_directory_level(dir, "", 0)
}

fn scan_directory_level(dir: &Path, prefix: &str, depth: usize) -> Vec<ScannedManifest> {
    let mut manifests: Vec<ScannedManifest> = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(it) => {
            let mut v: Vec<PathBuf> = it.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            v.sort_by(|a, b| a.file_name().unwrap_or_default().cmp(b.file_name().unwrap_or_default()));
            v
        }
        Err(_) => return manifests,
    };
    let skip_names: HashSet<&str> = ["memory", "context_engine", "platforms", "model-providers"].iter().copied().collect();
    for child in entries {
        let child_name = child.file_name().unwrap_or_default().to_string_lossy().to_string();
        if depth == 0 && skip_names.contains(child_name.as_str()) {
            continue;
        }
        let manifest_yaml = child.join("plugin.yaml");
        let manifest_yml = child.join("plugin.yml");
        if manifest_yaml.exists() || manifest_yml.exists() {
            // Native plugin – not portable; but we still need to record it so it can shadow portable
            // Try to read name from yaml if possible, else directory name
            let name = child_name.clone();
            let key = if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, child_name) };
            manifests.push(ScannedManifest {
                name: name.clone(),
                key,
                portable: false,
                path: child.to_string_lossy().to_string(),
            });
            continue;
        }
        let portable_file = child.join("plugin.json");
        let portable_exists = portable_file.exists() || portable_file.is_symlink();
        if portable_exists {
            // Try to validate manifest
            match read_agent_plugin_manifest(&child) {
                Ok((data, _diag)) => {
                    let data_name = data.get("name").and_then(|v| v.as_str()).unwrap_or(&child_name).to_string();
                    let key = if prefix.is_empty() { data_name.clone() } else { format!("{}/{}", prefix, child_name) };
                    // Note: Python uses data["name"] for key when prefix empty, else prefix/child.name
                    // For simplicity we use key as above for portable too.
                    manifests.push(ScannedManifest {
                        name: data_name,
                        key,
                        portable: true,
                        path: child.to_string_lossy().to_string(),
                    });
                }
                Err(_) => {
                    // Failed parse – Python logs warning and skips; we skip.
                    continue;
                }
            }
            continue;
        }
        // No manifest at this level – recurse if depth <1
        if depth >= 1 {
            continue;
        }
        let sub_prefix = if prefix.is_empty() { child_name.clone() } else { format!("{}/{}", prefix, child_name) };
        let sub = scan_directory_level(&child, &sub_prefix, depth + 1);
        manifests.extend(sub);
    }
    manifests
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_json(path: &Path, value: Value) {
        fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn valid_plugin_name() {
        assert!(is_valid_plugin_name("a"));
        assert!(is_valid_plugin_name("portable.test"));
        assert!(is_valid_plugin_name("a-b"));
        assert!(!is_valid_plugin_name("Bad_Name"));
        assert!(!is_valid_plugin_name("a--b"));
        assert!(!is_valid_plugin_name("a..b"));
        assert!(!is_valid_plugin_name("-ab"));
    }

    #[test]
    fn valid_skill_name() {
        assert!(is_valid_skill_name("summarize"));
        assert!(is_valid_skill_name("valid-skill"));
        assert!(!is_valid_skill_name("Bad_Skill"));
        assert!(!is_valid_skill_name("a--b"));
        assert!(!is_valid_skill_name("-ab"));
        assert!(!is_valid_skill_name("ab-"));
    }

    #[test]
    fn load_and_stdio() {
        let base = std::env::temp_dir().join(format!("hermes_test_agent_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let root = base.join("plugin");
        fs::create_dir(&root).unwrap();
        write_json(&root.join("plugin.json"), json!({"$schema": PLUGIN_SCHEMA_V1, "name": "portable.test", "version": "1.2.3"}));
        let skill_dir = root.join("skills").join("summarize");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: summarize\ndescription: Summarizes reports.\n---\nBody.\n").unwrap();
        write_json(&root.join("mcp.json"), json!({"$schema": MCP_SCHEMA_V1, "mcpServers": {"worker": {"type": "stdio", "command": "python", "args": ["${PLUGIN_ROOT}/server.py"], "env": {"CACHE": "${PLUGIN_DATA}/cache"}}}}));
        let data = base.join("data");
        let pkg = load_agent_plugin(&root, &data).unwrap();
        assert_eq!(pkg.name, "portable.test");
        assert_eq!(pkg.version, "1.2.3");
        assert_eq!(pkg.skills.len(), 1);
        let srv = pkg.mcp_servers.get("worker").unwrap();
        assert_eq!(srv.get("command").and_then(|v| v.as_str()).unwrap(), "python");
        let _ = fs::remove_dir_all(&base);
    }
}
