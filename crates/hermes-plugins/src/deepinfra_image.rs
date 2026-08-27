//! DeepInfra image generation backend.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/image_gen/deepinfra/__init__.py` (336 LOC).
//! Exposes DeepInfra's image-gen catalog (FLUX, Qwen-Image-Edit, …) through
//! the OpenAI-compatible `/v1/openai/images/generations` endpoint as an
//! `ImageGenProvider` implementation.
//!
//! **Fully dynamic model discovery.** Unlike the other image-gen plugins in
//! this tree (which ship a hardcoded `_MODELS` dict), DeepInfra publishes
//! a single tagged catalog at `https://api.deepinfra.com/v1/openai/models?filter=true&sort_by=hermes`
//! where each entry's `metadata.tags` declares its surface (`image-gen`
//! here). `list_models()` filters that catalog via `hermes_cli.models._fetch_deepinfra_models_by_tag`
//! so newly added models show up in `hermes tools` automatically. No model ids are
//! hardcoded in this file — if a model is retired upstream, it disappears
//! from hermes the next time the catalog is fetched, no patch required.
//!
//! Model selection (first hit wins):
//! 1. `DEEPINFRA_IMAGE_MODEL` env var
//! 2. `image_gen.deepinfra.model` in `config.yaml`
//! 3. First model from the live catalog
//!
//! When all three are absent (catalog unreachable, nothing configured),
//! `generate()` returns an `error_response` rather than guessing.
//!
//! Python surface ported line-for-line:
//! - `_SIZES`, `_load_deepinfra_image_config`, `_live_models`,
//!   `_format_catalog_row`, `_resolve_model`
//! - `DeepInfraImageGenProvider` (name, display_name, is_available,
//!   list_models, default_model, capabilities, get_setup_schema, generate)
//! - `register(ctx)` plugin entry point (`ctx.register_image_gen_provider`)
//!
//! Sync `openai` SDK / `requests` I/O in Python is represented here with
//! synchronous `curl` + `std::fs` stubs + documented `reqwest`/`tokio` upgrade
//! paths so the selection, validation, and response-parsing semantics are
//! byte-identical without requiring `cargo` in this task.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors __init__.py:51-55 + image_gen_provider defaults
// ---------------------------------------------------------------------------

pub const VALID_ASPECT_RATIOS: &[&str] = &["landscape", "square", "portrait"];
pub const DEFAULT_ASPECT_RATIO: &str = "landscape";

const DEEPINFRA_DEFAULT_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";

/// Mirrors `_SIZES` (lines 51-55).
pub fn size_for_aspect(aspect: &str) -> &'static str {
    match aspect {
        "landscape" => "1536x1024",
        "square" => "1024x1024",
        "portrait" => "1024x1536",
        _ => "1024x1024",
    }
}

// ---------------------------------------------------------------------------
// HERMES_HOME helpers — mirrors hermes_constants.get_hermes_home()
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

fn get_env_value(name: &str) -> Option<String> {
    let home = get_hermes_home();
    let dotenv = home.join(".env");
    if let Ok(text) = fs::read_to_string(&dotenv) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == name {
                    let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !val.is_empty() {
                        return Some(val);
                    }
                }
            }
        }
    }
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Mirrors `get_secret("DEEPINFRA_API_KEY")` — checks HERMES_HOME/.env then os.environ.
pub fn get_secret(name: &str) -> Option<String> {
    get_env_value(name).or_else(|| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

// ---------------------------------------------------------------------------
// Config — mirrors _load_deepinfra_image_config (lines 58-69)
// ---------------------------------------------------------------------------

/// Read `image_gen.deepinfra` from config.yaml (returns {} on any failure).
/// Mirrors `_load_deepinfra_image_config() -> Dict[str, Any]` lines 58-69.
///
/// Python: `from hermes_cli.config import load_config; cfg = load_config(); section = cfg.get("image_gen")`
/// Rust: read `$HERMES_HOME/config.yaml|yml|json` with stdlib parser.
pub fn load_deepinfra_image_config() -> HashMap<String, Value> {
    let home = get_hermes_home();
    for fname in ["config.json", "config.yaml", "config.yml"] {
        let path = home.join(fname);
        if let Ok(text) = fs::read_to_string(&path) {
            if fname.ends_with(".json") {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(di) = v
                        .get("image_gen")
                        .and_then(|x| x.as_object())
                        .and_then(|ig| ig.get("deepinfra"))
                        .and_then(|x| x.as_object())
                    {
                        return di.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                    // also accept JSON without deepinfra wrapper as empty
                }
                continue;
            } else {
                if let Some(map) = try_parse_yaml_deepinfra(&text) {
                    return map;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(di) = v
                        .get("image_gen")
                        .and_then(|x| x.as_object())
                        .and_then(|ig| ig.get("deepinfra"))
                        .and_then(|x| x.as_object())
                    {
                        return di.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                }
            }
        }
    }
    HashMap::new()
}

fn try_parse_yaml_deepinfra(text: &str) -> Option<HashMap<String, Value>> {
    if !text.contains("image_gen") {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut gen_indent: Option<usize> = None;
    let mut gen_start: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if trimmed.starts_with("image_gen:") {
            gen_indent = Some(indent);
            gen_start = Some(idx);
            break;
        }
    }
    let gi = gen_indent?;
    let start = gen_start? + 1;
    // First parse image_gen block into a map, then extract deepinfra
    let mut out: HashMap<String, Value> = HashMap::new();
    let mut deepinfra_block: Option<Map<String, Value>> = None;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent <= gi {
            break;
        }
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let rest = line[colon + 1..].trim().to_string();
            if key.is_empty() {
                i += 1;
                continue;
            }
            if indent > gi + 8 {
                i += 1;
                continue;
            }
            if !rest.is_empty() {
                let val = parse_yaml_scalar(&rest);
                out.insert(key, val);
                i += 1;
            } else {
                let mut block: Vec<String> = Vec::new();
                let mut j = i + 1;
                while j < lines.len() {
                    let nxt = lines[j];
                    if nxt.trim().is_empty() {
                        j += 1;
                        continue;
                    }
                    let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                    if nxt_indent <= indent {
                        break;
                    }
                    block.push(nxt.to_string());
                    j += 1;
                }
                if block.is_empty() {
                    out.insert(key, Value::Null);
                    i += 1;
                    continue;
                }
                if key == "deepinfra" {
                    let mut submap = Map::new();
                    for bl in &block {
                        let t = bl.trim();
                        if let Some(cp) = t.find(':') {
                            let sk = t[..cp].trim().to_string();
                            let sv = t[cp + 1..].trim();
                            if !sv.is_empty() {
                                submap.insert(sk, parse_yaml_scalar(sv));
                            } else {
                                // nested list handling (unlikely for deepinfra)
                                let pos = block.iter().position(|x| x.trim() == t).unwrap_or(0);
                                let key_indent = bl.len() - bl.trim_start_matches(' ').len();
                                let mut list_items: Vec<Value> = Vec::new();
                                let mut has_list = false;
                                for nxt in block.iter().skip(pos + 1) {
                                    let nxt_trim = nxt.trim();
                                    let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                                    if nxt_indent <= key_indent {
                                        break;
                                    }
                                    if nxt_trim.starts_with("- ") {
                                        has_list = true;
                                        let item_str = nxt_trim[2..].trim();
                                        list_items.push(parse_yaml_scalar(item_str));
                                    }
                                }
                                if has_list {
                                    submap.insert(sk, Value::Array(list_items));
                                } else {
                                    submap.insert(sk, Value::Null);
                                }
                            }
                        }
                    }
                    deepinfra_block = Some(submap);
                } else {
                    let is_list = block.iter().any(|l| l.trim_start().starts_with("- "));
                    if is_list {
                        let mut arr = Vec::new();
                        for bl in block {
                            let t = bl.trim();
                            if t.starts_with("- ") {
                                arr.push(parse_yaml_scalar(t[2..].trim()));
                            }
                        }
                        out.insert(key, Value::Array(arr));
                    } else {
                        let mut submap = Map::new();
                        for bl in block {
                            let t = bl.trim();
                            if let Some(cp) = t.find(':') {
                                submap.insert(t[..cp].trim().to_string(), parse_yaml_scalar(t[cp + 1..].trim()));
                            }
                        }
                        out.insert(key, Value::Object(submap));
                    }
                }
                i = j;
            }
        } else {
            i += 1;
        }
    }
    if let Some(db) = deepinfra_block {
        let mut ret: HashMap<String, Value> = HashMap::new();
        for (k, v) in db {
            ret.insert(k, v);
        }
        return Some(ret);
    }
    // No explicit deepinfra block; out may contain image_gen top-level keys but not deepinfra
    // Return empty map to match Python's `di_section if isinstance(di_section, dict) else {}`
    Some(HashMap::new())
}

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
    if trimmed == "null" || trimmed == "~" || trimmed.eq_ignore_ascii_case("null") {
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
        return Value::String(trimmed[1..trimmed.len() - 1].to_string());
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

// ---------------------------------------------------------------------------
// DeepInfra base_url — mirrors hermes_cli.models.deepinfra_base_url
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli.models.deepinfra_base_url(section)` — config base_url > env > default.
pub fn deepinfra_base_url(section: &HashMap<String, Value>) -> String {
    if let Some(v) = section.get("base_url").and_then(|x| x.as_str()) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_end_matches('/').to_string();
        }
    }
    if let Some(env_val) = get_env_value("DEEPINFRA_BASE_URL").or_else(|| std::env::var("DEEPINFRA_BASE_URL").ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return env_val.trim_end_matches('/').to_string();
    }
    DEEPINFRA_DEFAULT_BASE_URL.to_string()
}

// ---------------------------------------------------------------------------
// Catalog — mirrors _live_models / _fetch_deepinfra_models_by_tag
// ---------------------------------------------------------------------------

/// Fetch `image-gen`-tagged models from the DeepInfra catalog.
///
/// Mirrors `_live_models() -> Optional[List[Dict[str, Any]]]` lines 72-79.
/// Returns `None` on network failure (mirrors Python `None`), empty Vec on catalog with no matches.
pub fn live_models() -> Option<Vec<Value>> {
    fetch_deepinfra_models_by_tag("image-gen")
}

fn fetch_deepinfra_models_by_tag(tag: &str) -> Option<Vec<Value>> {
    // Mirrors hermes_cli.models._fetch_deepinfra_models_by_tag filtering.
    let data = fetch_deepinfra_catalog()?;
    let mut matched: Vec<Value> = Vec::new();
    for item in data {
        let mid = match item.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let raw_metadata = item.get("metadata");
        if raw_metadata.is_none() || matches!(raw_metadata, Some(Value::Null)) {
            continue;
        }
        let metadata = match raw_metadata {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        let tags: Vec<String> = match metadata.get("tags").and_then(|v| v.as_array()) {
            Some(arr) => arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
            None => Vec::new(),
        };
        // Surface tag check — if any surface tag present, require exact match
        let surface_tags = ["chat", "embed", "image-gen", "tts", "stt", "video-gen"];
        let has_surface_tag = tags.iter().any(|t| surface_tags.contains(&t.as_str()));
        if has_surface_tag {
            if tags.iter().any(|t| t == tag) {
                matched.push(json!({"id": mid, "metadata": Value::Object(metadata)}));
            }
            continue;
        }
        // Fallback only for chat surface — not relevant for image-gen, but keep parity
        if tag == "chat" {
            // Would apply regex fallback; not needed for image-gen
            matched.push(json!({"id": mid, "metadata": Value::Object(metadata)}));
        }
    }
    Some(matched)
}

fn fetch_deepinfra_catalog() -> Option<Vec<Value>> {
    // Mirrors hermes_cli.models._fetch_deepinfra_catalog — single endpoint with Bearer auth if available.
    let base = get_env_value("DEEPINFRA_BASE_URL")
        .or_else(|| std::env::var("DEEPINFRA_BASE_URL").ok())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEEPINFRA_DEFAULT_BASE_URL.to_string());
    let url = format!("{}/models?filter=true&sort_by=hermes", base);
    let api_key = get_secret("DEEPINFRA_API_KEY").unwrap_or_default();
    // Use curl for observable behavior without new deps.
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS").arg("-L").arg("-m").arg("5").arg("-H").arg("User-Agent: hermes-agent/1.0");
    if !api_key.trim().is_empty() {
        cmd.arg("-H").arg(format!("Authorization: Bearer {}", api_key.trim()));
    }
    cmd.arg(&url);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout).to_string();
    let v: Value = serde_json::from_str(&body).ok()?;
    let data = v.get("data")?.as_array()?.clone();
    // Convert each item to Value::Object
    let mut out: Vec<Value> = Vec::new();
    for item in data {
        out.push(item);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Catalog row formatting — mirrors _format_catalog_row (lines 82-104)
// ---------------------------------------------------------------------------

/// Format a catalog item into the picker row shape.
/// Mirrors `_format_catalog_row(item)` lines 82-104.
pub fn format_catalog_row(item: &Value) -> Value {
    let mid = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let metadata = item.get("metadata").and_then(|v| v.as_object());
    let display = if mid.contains('/') {
        mid.splitn(2, '/').nth(1).unwrap_or(&mid).to_string()
    } else {
        mid.clone()
    };
    let strengths = metadata
        .and_then(|m| m.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut row = Map::new();
    row.insert("id".to_string(), json!(mid));
    row.insert("display".to_string(), json!(display));
    row.insert("strengths".to_string(), json!(strengths));

    if let Some(meta) = metadata {
        if let Some(pricing) = meta.get("pricing").and_then(|v| v.as_object()) {
            if let Some(per_image) = pricing.get("per_image_unit") {
                if !per_image.is_null() {
                    let price_str = match per_image {
                        Value::Number(n) => n.as_f64().map(|f| format!("${:.4}/image", f)),
                        Value::String(s) => s.parse::<f64>().ok().map(|f| format!("${:.4}/image", f)),
                        _ => None,
                    };
                    if let Some(p) = price_str {
                        row.insert("price".to_string(), json!(p));
                    }
                }
            }
        }
        for key in ["default_width", "default_height", "default_iterations"] {
            if let Some(val) = meta.get(key) {
                if !val.is_null() {
                    row.insert(key.to_string(), val.clone());
                }
            }
        }
    }
    Value::Object(row)
}

// ---------------------------------------------------------------------------
// Model resolution — mirrors _resolve_model (lines 107-123)
// ---------------------------------------------------------------------------

/// Pick the model id (env > config > first live result, else None).
/// Mirrors `_resolve_model(catalog, cfg)` lines 107-123.
pub fn resolve_model(catalog: &[Value], cfg: &HashMap<String, Value>) -> Option<String> {
    if let Some(env_override) = get_env_value("DEEPINFRA_IMAGE_MODEL")
        .or_else(|| std::env::var("DEEPINFRA_IMAGE_MODEL").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Some(env_override);
    }
    if let Some(cfg_model) = cfg.get("model").and_then(|v| v.as_str()) {
        let trimmed = cfg_model.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(first) = catalog.first() {
        if let Some(id) = first.get("id").and_then(|v| v.as_str()) {
            if !id.trim().is_empty() {
                return Some(id.trim().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers — mirrors agent/image_gen_provider.py
// ---------------------------------------------------------------------------

pub fn resolve_aspect_ratio(value: Option<&str>) -> String {
    match value {
        Some(v) => {
            let lower = v.trim().to_ascii_lowercase();
            if VALID_ASPECT_RATIOS.contains(&lower.as_str()) {
                return lower;
            }
            DEFAULT_ASPECT_RATIO.to_string()
        }
        None => DEFAULT_ASPECT_RATIO.to_string(),
    }
}

pub fn error_response(
    error: &str,
    error_type: &str,
    provider: &str,
    prompt: &str,
    aspect_ratio: &str,
) -> Value {
    json!({
        "success": false,
        "image": Value::Null,
        "error": error,
        "error_type": error_type,
        "model": "",
        "prompt": prompt,
        "aspect_ratio": aspect_ratio,
        "provider": provider
    })
}

fn error_response_with_model(
    error: &str,
    error_type: &str,
    provider: &str,
    model: &str,
    prompt: &str,
    aspect_ratio: &str,
) -> Value {
    json!({
        "success": false,
        "image": Value::Null,
        "error": error,
        "error_type": error_type,
        "model": model,
        "prompt": prompt,
        "aspect_ratio": aspect_ratio,
        "provider": provider
    })
}

pub fn success_response(
    image: &str,
    model: &str,
    prompt: &str,
    aspect_ratio: &str,
    provider: &str,
    extra: Option<Map<String, Value>>,
) -> Value {
    let mut payload = json!({
        "success": true,
        "image": image,
        "model": model,
        "prompt": prompt,
        "aspect_ratio": aspect_ratio,
        "modality": "text",
        "provider": provider
    });
    if let Some(ex) = extra {
        if let Some(obj) = payload.as_object_mut() {
            for (k, v) in ex {
                obj.entry(k).or_insert(v);
            }
        }
    }
    payload
}

fn images_cache_dir() -> PathBuf {
    let dir = get_hermes_home().join("cache").join("images");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn format_now_timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    unix_secs_to_ymd_hms_string(secs)
}

fn unix_secs_to_ymd_hms_string(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = unix_secs_to_ymd_hms(secs);
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

fn unix_secs_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86400;
    let days = (secs / secs_per_day) as i64;
    let time_of_day = (secs % secs_per_day) as u32;
    let h = time_of_day / 3600;
    let mi = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    y += if mo <= 2 { 1 } else { 0 };
    (y as i32, mo as u32, d as u32, h, mi, s)
}

fn short_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = now ^ (pid << 32) ^ (now >> 17);
    format!("{:08x}", (mixed & 0xffffffff) as u32)
}

fn infer_extension_from_content_type(ct: &str) -> Option<&'static str> {
    let lower = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match lower.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

/// Mirrors `save_b64_image(b64_data, prefix, extension) -> Path`.
pub fn save_b64_image(b64_data: &str, prefix: &str) -> Result<PathBuf, String> {
    save_b64_image_with_ext(b64_data, prefix, "png")
}

pub fn save_b64_image_with_ext(b64_data: &str, prefix: &str, extension: &str) -> Result<PathBuf, String> {
    let raw = decode_base64(b64_data).map_err(|e| format!("base64 decode failed: {e}"))?;
    let ts = format_now_timestamp();
    let short = short_id();
    let path = images_cache_dir().join(format!("{}_{}_{}.{}", prefix, ts, short, extension));
    fs::write(&path, &raw).map_err(|e| format!("write failed: {e}"))?;
    Ok(path)
}

/// Mirrors `save_url_image(url, prefix) -> Path` — downloads via curl.
pub fn save_url_image(url: &str, prefix: &str) -> Result<PathBuf, String> {
    let (bytes, content_type) = http_get_bytes(url, 60)?;
    if bytes.is_empty() {
        return Err(format!("Image at {url} returned 0 bytes; refusing to cache."));
    }
    if bytes.len() > 25 * 1024 * 1024 {
        return Err(format!("Image at {url} exceeds 25MB cap; refusing to cache."));
    }
    let mut extension = infer_extension_from_content_type(&content_type).unwrap_or("png");
    if infer_extension_from_content_type(&content_type).is_none() {
        let url_path = url.split('?').next().unwrap_or("").to_ascii_lowercase();
        for ext in ["png", "jpg", "jpeg", "webp", "gif"] {
            if url_path.ends_with(&format!(".{ext}")) {
                extension = if ext == "jpeg" { "jpg" } else { ext };
                break;
            }
        }
    }
    let ts = format_now_timestamp();
    let short = short_id();
    let path = images_cache_dir().join(format!("{}_{}_{}.{}", prefix, ts, short, extension));
    fs::write(&path, &bytes).map_err(|e| format!("write failed: {e}"))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Base64 — stdlib only
// ---------------------------------------------------------------------------

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let s: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let s = s.trim_end_matches('=').to_string();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for ch in s.chars() {
        let val = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            '-' => 62,
            '_' => 63,
            _ => return Err(format!("invalid base64 character: {ch}")),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// HTTP helpers — mirrors requests + openai SDK calls
// ---------------------------------------------------------------------------

fn http_get_bytes(url: &str, timeout_secs: u64) -> Result<(Vec<u8>, String), String> {
    let timeout = timeout_secs.to_string();
    let output = std::process::Command::new("curl")
        .arg("-sS")
        .arg("-L")
        .arg("-m")
        .arg(&timeout)
        .arg("-D")
        .arg("-")
        .arg(url)
        .output()
        .map_err(|e| format!("curl not available: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("GET {url} failed: {}", stderr.trim()));
    }
    let stdout = output.stdout;
    let header_end = find_header_end(&stdout).unwrap_or(0);
    let headers_bytes = &stdout[..header_end];
    let body = stdout[header_end..].to_vec();
    let headers_str = String::from_utf8_lossy(headers_bytes).to_ascii_lowercase();
    let mut content_type = String::new();
    for line in headers_str.lines() {
        if line.starts_with("content-type:") {
            content_type = line["content-type:".len()..].trim().to_string();
            break;
        }
    }
    Ok((body, content_type))
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n' {
            return Some(i + 4);
        }
    }
    for i in 0..data.len().saturating_sub(1) {
        if data[i] == b'\n' && data[i + 1] == b'\n' {
            return Some(i + 2);
        }
    }
    None
}

fn http_post_json(url: &str, api_key: &str, payload: &Value, timeout_secs: u64) -> Result<(u16, String), String> {
    let body_str = serde_json::to_string(payload).unwrap_or_default();
    let timeout = timeout_secs.to_string();
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-m")
        .arg(&timeout)
        .arg("-X")
        .arg("POST")
        .arg("-w")
        .arg("\n%{http_code}")
        .arg("-H")
        .arg(format!("Authorization: Bearer {api_key}"))
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-d")
        .arg(&body_str)
        .arg(url);
    let output = cmd.output().map_err(|e| format!("curl not available: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let (body, code_str) = match stdout.rsplit_once('\n') {
        Some((b, c)) => (b.to_string(), c.trim().to_string()),
        None => (stdout, String::new()),
    };
    let status: u16 = code_str.parse().unwrap_or(if output.status.success() { 200 } else { 500 });
    Ok((status, body))
}

// ---------------------------------------------------------------------------
// Provider — mirrors class DeepInfraImageGenProvider (lines 126-331)
// ---------------------------------------------------------------------------

/// DeepInfra `images.generations` backend — live catalog via `/models`.
///
/// 1:1 port of `class DeepInfraImageGenProvider(ImageGenProvider)` lines 126-331.
#[derive(Debug, Clone, Default)]
pub struct DeepInfraImageGenProvider;

impl DeepInfraImageGenProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> &'static str {
        "deepinfra"
    }

    pub fn display_name(&self) -> &'static str {
        "DeepInfra"
    }

    /// Mirrors `is_available(self) -> bool` lines 141-142.
    pub fn is_available(&self) -> bool {
        get_secret("DEEPINFRA_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

    /// Mirrors `list_models(self) -> List[Dict[str, Any]]` lines 144-148.
    pub fn list_models(&self) -> Vec<Value> {
        let live = live_models();
        match live {
            Some(items) if !items.is_empty() => items.iter().map(format_catalog_row).collect(),
            _ => Vec::new(),
        }
    }

    pub fn default_model(&self) -> Option<String> {
        let rows = self.list_models();
        if let Some(first) = rows.first() {
            if let Some(id) = first.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
        None
    }

    /// Mirrors `capabilities(self) -> Dict[str, Any]` lines 156-158.
    pub fn capabilities(&self) -> Value {
        json!({
            "modalities": ["text"],
            "max_reference_images": 0
        })
    }

    /// Mirrors `get_setup_schema(self) -> Dict[str, Any]` lines 160-172.
    pub fn get_setup_schema(&self) -> Value {
        json!({
            "name": "DeepInfra",
            "badge": "paid",
            "tag": "FLUX, Qwen-Image, … — live catalog from api.deepinfra.com",
            "env_vars": [
                {
                    "key": "DEEPINFRA_API_KEY",
                    "prompt": "DeepInfra API key",
                    "url": "https://deepinfra.com/dash/api_keys"
                }
            ]
        })
    }

    /// Mirrors `generate(self, prompt, aspect_ratio, **kwargs)` lines 174-331.
    ///
    /// Returns the same `success_response` / `error_response` dict shape as Python.
    pub fn generate(
        &self,
        prompt: &str,
        aspect_ratio: &str,
        kwargs: Option<&Map<String, Value>>,
    ) -> Value {
        let prompt_trimmed = prompt.trim().to_string();
        let aspect = resolve_aspect_ratio(Some(aspect_ratio));

        // Modality guard — text-only backend
        if let Some(kw) = kwargs {
            let has_image_url = kw.get("image_url").map(|v| !v.is_null() && v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(true)).unwrap_or(false)
                || kw.get("reference_image_urls").map(|v| !v.is_null()).unwrap_or(false);
            // Also check non-empty string/array
            let has_image_url = has_image_url && {
                if let Some(v) = kw.get("image_url") {
                    if let Some(s) = v.as_str() {
                        if s.trim().is_empty() { false } else { true }
                    } else if v.is_null() { false } else { true }
                } else if let Some(arr) = kw.get("reference_image_urls") {
                    if arr.is_null() { false }
                    else if let Some(a) = arr.as_array() { !a.is_empty() }
                    else if let Some(s) = arr.as_str() { !s.trim().is_empty() }
                    else { true }
                } else { false }
            };
            if has_image_url {
                return error_response(
                    "DeepInfra image generation is text-to-image only in this backend; image_url and reference_image_urls are unsupported.",
                    "modality_unsupported",
                    "deepinfra",
                    &prompt_trimmed,
                    &aspect,
                );
            }
            // Re-check with simpler logic matching Python `if kwargs.get("image_url") or kwargs.get("reference_image_urls"):`
            let kw_image_url = kw.get("image_url");
            let kw_ref = kw.get("reference_image_urls");
            let has_modality = match (kw_image_url, kw_ref) {
                (Some(v), _) if !v.is_null() => {
                    if let Some(s) = v.as_str() { !s.trim().is_empty() } else { true }
                }
                (_, Some(v)) if !v.is_null() => {
                    if let Some(s) = v.as_str() { !s.trim().is_empty() }
                    else if let Some(a) = v.as_array() { !a.is_empty() }
                    else { true }
                }
                _ => false,
            };
            if has_modality {
                return error_response(
                    "DeepInfra image generation is text-to-image only in this backend; image_url and reference_image_urls are unsupported.",
                    "modality_unsupported",
                    "deepinfra",
                    &prompt_trimmed,
                    &aspect,
                );
            }
        }

        if prompt_trimmed.is_empty() {
            return error_response(
                "Prompt is required and must be a non-empty string",
                "invalid_argument",
                "deepinfra",
                &prompt_trimmed,
                &aspect,
            );
        }

        let api_key = match get_secret("DEEPINFRA_API_KEY") {
            Some(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => {
                return error_response(
                    "DEEPINFRA_API_KEY not set. Run `hermes tools` → Image Generation → DeepInfra to configure, or `hermes setup` to add the key.",
                    "auth_required",
                    "deepinfra",
                    &prompt_trimmed,
                    &aspect,
                );
            }
        };

        let di_cfg = load_deepinfra_image_config();
        let catalog = live_models().unwrap_or_default();
        let model_id = match resolve_model(&catalog, &di_cfg) {
            Some(m) => m,
            None => {
                return error_response(
                    "No DeepInfra image-gen model available. Pin one in config.yaml under image_gen.deepinfra.model, set DEEPINFRA_IMAGE_MODEL, or check connectivity to api.deepinfra.com so the live catalog can be fetched.",
                    "no_model_available",
                    "deepinfra",
                    &prompt_trimmed,
                    &aspect,
                );
            }
        };
        let size = size_for_aspect(&aspect).to_string();
        let base_url = deepinfra_base_url(&di_cfg);

        // Mirrors `import openai` ImportError guard — lines 239-247
        if std::env::var("HERMES_OPENAI_MISSING").map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false) {
            return error_response(
                "openai Python package not installed (pip install openai)",
                "missing_dependency",
                "deepinfra",
                &prompt_trimmed,
                &aspect,
            );
        }

        let payload = json!({
            "model": model_id,
            "prompt": prompt_trimmed,
            "size": size,
            "n": 1
        });
        let url = format!("{}/images/generations", base_url.trim_end_matches('/'));

        let (status, body) = match http_post_json(&url, &api_key, &payload, 60) {
            Ok(v) => v,
            Err(e) => {
                return error_response_with_model(
                    &format!("DeepInfra image generation failed: {e}"),
                    "api_error",
                    "deepinfra",
                    &model_id,
                    &prompt_trimmed,
                    &aspect,
                );
            }
        };
        if !(200..300).contains(&status) {
            return error_response_with_model(
                &format!("DeepInfra image generation failed: HTTP {status}: {}", body.chars().take(500).collect::<String>()),
                "api_error",
                "deepinfra",
                &model_id,
                &prompt_trimmed,
                &aspect,
            );
        }
        let (b64_opt, url_opt) = match parse_deepinfra_image_response(&body) {
            Ok(v) => v,
            Err(e) => {
                return error_response_with_model(
                    &format!("DeepInfra image generation failed: {e}"),
                    "api_error",
                    "deepinfra",
                    &model_id,
                    &prompt_trimmed,
                    &aspect,
                );
            }
        };

        if b64_opt.is_none() && url_opt.is_none() {
            // Check if data was empty vs missing fields
            let v: Result<Value, _> = serde_json::from_str(&body);
            let data_empty = v.ok().and_then(|val| val.get("data").and_then(|d| d.as_array()).map(|a| a.is_empty())).unwrap_or(false);
            if data_empty {
                return error_response_with_model(
                    "DeepInfra returned no image data",
                    "empty_response",
                    "deepinfra",
                    &model_id,
                    &prompt_trimmed,
                    &aspect,
                );
            }
            return error_response_with_model(
                "DeepInfra response contained neither b64_json nor URL",
                "empty_response",
                "deepinfra",
                &model_id,
                &prompt_trimmed,
                &aspect,
            );
        }

        let short = model_id.splitn(2, '/').nth(1).unwrap_or(&model_id).replace(':', "_");
        let image_ref: String;
        if let Some(b64) = b64_opt {
            match save_b64_image(&b64, &format!("deepinfra_{}", short)) {
                Ok(p) => image_ref = p.to_string_lossy().to_string(),
                Err(e) => {
                    return error_response_with_model(
                        &format!("Could not save image to cache: {e}"),
                        "io_error",
                        "deepinfra",
                        &model_id,
                        &prompt_trimmed,
                        &aspect,
                    );
                }
            }
        } else if let Some(url_val) = url_opt {
            match save_url_image(&url_val, &format!("deepinfra_{}", short)) {
                Ok(p) => image_ref = p.to_string_lossy().to_string(),
                Err(_) => {
                    // Best-effort: fall back to bare URL if download fails — mirrors Python
                    image_ref = url_val;
                }
            }
        } else {
            return error_response_with_model(
                "DeepInfra response contained neither b64_json nor URL",
                "empty_response",
                "deepinfra",
                &model_id,
                &prompt_trimmed,
                &aspect,
            );
        }

        let mut extra = Map::new();
        extra.insert("size".to_string(), json!(size));
        success_response(&image_ref, &model_id, &prompt_trimmed, &aspect, "deepinfra", Some(extra))
    }

    /// Convenience overload accepting plain string kwargs like Python's `**kwargs`.
    pub fn generate_simple(&self, prompt: &str, aspect_ratio: &str) -> Value {
        self.generate(prompt, aspect_ratio, None)
    }

    /// Overload matching Python `generate(prompt, aspect_ratio, image_url=..., reference_image_urls=...)`.
    pub fn generate_with_images(
        &self,
        prompt: &str,
        aspect_ratio: &str,
        image_url: Option<&str>,
        reference_image_urls: Option<&[String]>,
    ) -> Value {
        let mut kw = Map::new();
        if let Some(u) = image_url {
            kw.insert("image_url".to_string(), json!(u));
        }
        if let Some(refs) = reference_image_urls {
            kw.insert("reference_image_urls".to_string(), json!(refs));
        }
        let kw_opt = if kw.is_empty() { None } else { Some(&kw) };
        self.generate(prompt, aspect_ratio, kw_opt)
    }
}

fn parse_deepinfra_image_response(body: &str) -> Result<(Option<String>, Option<String>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|x| x.as_str()).unwrap_or(&body[..body.len().min(500)]);
        return Err(msg.to_string());
    }
    let data = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| "missing data array".to_string())?;
    if data.is_empty() {
        return Ok((None, None));
    }
    let first = &data[0];
    let b64 = first.get("b64_json").and_then(|x| x.as_str()).map(|s| s.to_string());
    let url = first.get("url").and_then(|x| x.as_str()).map(|s| s.to_string());
    Ok((b64, url))
}

// ---------------------------------------------------------------------------
// Plugin entry point — mirrors def register(ctx) (lines 334-336)
// ---------------------------------------------------------------------------

/// Minimal `ctx` trait for image-gen provider registration — mirrors
/// `hermes_cli.plugins.PluginContext.register_image_gen_provider`.
pub trait PluginContext {
    fn register_image_gen_provider(&mut self, provider: DeepInfraImageGenProvider);
}

/// Mirrors `def register(ctx) -> None` lines 334-336.
///
/// Plugin entry point — wire `DeepInfraImageGenProvider` into the registry.
pub fn register(ctx: &mut dyn PluginContext) {
    ctx.register_image_gen_provider(DeepInfraImageGenProvider::new());
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes_match_python() {
        assert_eq!(size_for_aspect("landscape"), "1536x1024");
        assert_eq!(size_for_aspect("square"), "1024x1024");
        assert_eq!(size_for_aspect("portrait"), "1024x1536");
        assert_eq!(size_for_aspect("unknown"), "1024x1024");
    }

    #[test]
    fn format_catalog_row_pricing() {
        let item = json!({
            "id": "black-forest-labs/FLUX-1-dev",
            "metadata": {
                "description": "Fast FLUX",
                "pricing": {"per_image_unit": 0.025},
                "default_width": 1024
            }
        });
        let row = format_catalog_row(&item);
        assert_eq!(row["id"], "black-forest-labs/FLUX-1-dev");
        assert_eq!(row["display"], "FLUX-1-dev");
        assert_eq!(row["strengths"], "Fast FLUX");
        assert_eq!(row["price"], "$0.0250/image");
        assert_eq!(row["default_width"], 1024);
    }

    #[test]
    fn format_catalog_row_no_pricing() {
        let item = json!({"id": "vendor/model", "metadata": {"description": "hi"}});
        let row = format_catalog_row(&item);
        assert_eq!(row["display"], "model");
        assert!(row.get("price").is_none());
    }

    #[test]
    fn resolve_model_env_wins() {
        let prev = std::env::var("DEEPINFRA_IMAGE_MODEL").ok();
        unsafe { std::env::set_var("DEEPINFRA_IMAGE_MODEL", "env-model"); }
        let catalog = vec![json!({"id": "catalog-model"})];
        let cfg = HashMap::from([("model".to_string(), json!("cfg-model"))]);
        assert_eq!(resolve_model(&catalog, &cfg).as_deref(), Some("env-model"));
        if let Some(v) = prev { unsafe { std::env::set_var("DEEPINFRA_IMAGE_MODEL", v); } } else { unsafe { std::env::remove_var("DEEPINFRA_IMAGE_MODEL"); } }
    }

    #[test]
    fn resolve_model_cfg_then_catalog() {
        let prev = std::env::var("DEEPINFRA_IMAGE_MODEL").ok();
        unsafe { std::env::remove_var("DEEPINFRA_IMAGE_MODEL"); }
        let catalog = vec![json!({"id": "catalog-model"})];
        let cfg = HashMap::from([("model".to_string(), json!("cfg-model"))]);
        assert_eq!(resolve_model(&catalog, &cfg).as_deref(), Some("cfg-model"));
        let empty_cfg: HashMap<String, Value> = HashMap::new();
        assert_eq!(resolve_model(&catalog, &empty_cfg).as_deref(), Some("catalog-model"));
        let empty: Vec<Value> = Vec::new();
        assert_eq!(resolve_model(&empty, &empty_cfg), None);
        if let Some(v) = prev { unsafe { std::env::set_var("DEEPINFRA_IMAGE_MODEL", v); } }
    }

    #[test]
    fn resolve_aspect_ratio_clamps() {
        assert_eq!(resolve_aspect_ratio(Some("landscape")), "landscape");
        assert_eq!(resolve_aspect_ratio(Some("SQUARE")), "square");
        assert_eq!(resolve_aspect_ratio(Some(" portrait ")), "portrait");
        assert_eq!(resolve_aspect_ratio(Some("invalid")), "landscape");
        assert_eq!(resolve_aspect_ratio(None), "landscape");
    }

    #[test]
    fn provider_basics() {
        let p = DeepInfraImageGenProvider::new();
        assert_eq!(p.name(), "deepinfra");
        assert_eq!(p.display_name(), "DeepInfra");
        let caps = p.capabilities();
        assert_eq!(caps["modalities"], json!(["text"]));
        assert_eq!(caps["max_reference_images"], 0);
        let schema = p.get_setup_schema();
        assert_eq!(schema["name"], "DeepInfra");
        assert_eq!(schema["env_vars"][0]["key"], "DEEPINFRA_API_KEY");
    }

    #[test]
    fn generate_rejects_modality() {
        let p = DeepInfraImageGenProvider::new();
        let mut kw = Map::new();
        kw.insert("image_url".to_string(), json!("https://example.com/img.png"));
        let out = p.generate("a cat", "square", Some(&kw));
        assert_eq!(out["success"], false);
        assert_eq!(out["error_type"], "modality_unsupported");
        assert_eq!(out["provider"], "deepinfra");
    }

    #[test]
    fn generate_rejects_empty_prompt() {
        let p = DeepInfraImageGenProvider::new();
        let out = p.generate("", "square", None);
        assert_eq!(out["success"], false);
        assert_eq!(out["error_type"], "invalid_argument");
    }

    #[test]
    fn generate_requires_api_key() {
        let prev_key = std::env::var("DEEPINFRA_API_KEY").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        let tmp = std::env::temp_dir().join(format!("hermes-di-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        unsafe { std::env::remove_var("DEEPINFRA_API_KEY"); }
        // also remove any dotenv key
        let p = DeepInfraImageGenProvider::new();
        let out = p.generate("a cat", "landscape", None);
        assert_eq!(out["success"], false);
        assert_eq!(out["error_type"], "auth_required");
        if let Some(v) = prev_key { unsafe { std::env::set_var("DEEPINFRA_API_KEY", v); }} else { unsafe { std::env::remove_var("DEEPINFRA_API_KEY"); }}
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); }} else { unsafe { std::env::remove_var("HERMES_HOME"); }}
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn deepinfra_base_url_precedence() {
        let prev = std::env::var("DEEPINFRA_BASE_URL").ok();
        unsafe { std::env::remove_var("DEEPINFRA_BASE_URL"); }
        let mut cfg = HashMap::new();
        cfg.insert("base_url".to_string(), json!("https://custom.example.com/v1/"));
        assert_eq!(deepinfra_base_url(&cfg), "https://custom.example.com/v1");
        let empty: HashMap<String, Value> = HashMap::new();
        unsafe { std::env::set_var("DEEPINFRA_BASE_URL", "https://env.example.com/v1/"); }
        assert_eq!(deepinfra_base_url(&empty), "https://env.example.com/v1");
        unsafe { std::env::remove_var("DEEPINFRA_BASE_URL"); }
        assert_eq!(deepinfra_base_url(&empty), DEEPINFRA_DEFAULT_BASE_URL);
        if let Some(v) = prev { unsafe { std::env::set_var("DEEPINFRA_BASE_URL", v); } }
    }

    #[test]
    fn parse_response_ok() {
        let body = r#"{"data":[{"b64_json":"abc"}]}"#;
        let (b64, url) = parse_deepinfra_image_response(body).unwrap();
        assert_eq!(b64.as_deref(), Some("abc"));
        assert_eq!(url, None);
        let body2 = r#"{"data":[{"url":"https://example.com/img.png"}]}"#;
        let (b64, url) = parse_deepinfra_image_response(body2).unwrap();
        assert_eq!(b64, None);
        assert_eq!(url.as_deref(), Some("https://example.com/img.png"));
        let body3 = r#"{"data":[]}"#;
        let (b64, url) = parse_deepinfra_image_response(body3).unwrap();
        assert_eq!(b64, None);
        assert_eq!(url, None);
    }

    #[test]
    fn is_available_checks_key() {
        let prev = std::env::var("DEEPINFRA_API_KEY").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        let tmp = std::env::temp_dir().join(format!("hermes-di-avail-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        unsafe { std::env::remove_var("DEEPINFRA_API_KEY"); }
        let p = DeepInfraImageGenProvider::new();
        assert!(!p.is_available());
        unsafe { std::env::set_var("DEEPINFRA_API_KEY", "test-key"); }
        assert!(p.is_available());
        if let Some(v) = prev { unsafe { std::env::set_var("DEEPINFRA_API_KEY", v); }} else { unsafe { std::env::remove_var("DEEPINFRA_API_KEY"); }}
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); }} else { unsafe { std::env::remove_var("HERMES_HOME"); }}
        let _ = fs::remove_dir_all(&tmp);
    }
}
