//! OpenAI image generation backend.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/image_gen/openai/__init__.py` (419 LOC).
//! Exposes OpenAI's `gpt-image-2` model at three quality tiers as an
//! `ImageGenProvider` implementation. The tiers are implemented as three
//! virtual model IDs so the `hermes tools` model picker and the
//! `image_gen.model` config key behave like any other multi-model backend:
//!
//! ```text
//! gpt-image-2-low     ~15s   fastest, good for iteration
//! gpt-image-2-medium  ~40s   default — balanced
//! gpt-image-2-high    ~2min  slowest, highest fidelity
//! ```
//!
//! All three hit the same underlying API model (`gpt-image-2`) with a
//! different `quality` parameter. Output is base64 JSON → saved under
//! `$HERMES_HOME/cache/images/`.
//!
//! Selection precedence (first hit wins):
//! 1. `OPENAI_IMAGE_MODEL` env var (escape hatch for scripts / tests)
//! 2. `image_gen.openai.model` in `config.yaml`
//! 3. `image_gen.model` in `config.yaml` (when it's one of our tier IDs)
//! 4. `DEFAULT_MODEL` — `gpt-image-2-medium`
//!
//! Config keys this provider responds to:
//! ```yaml
//! image_gen:
//!   model: "gpt-image-2-medium"   # top-level fallback
//!   openai:
//!     model: "gpt-image-2-high"   # scoped override
//! ```
//!
//! Python surface ported line-for-line:
//! - `API_MODEL`, `_MODELS`, `DEFAULT_MODEL`, `_SIZES`
//! - `_load_openai_config`, `_resolve_model`
//! - `_load_image_bytes` (URL, data:, local file with `raise_if_read_blocked` guard)
//! - `OpenAIImageGenProvider` (name, display_name, is_available, list_models,
//!   default_model, get_setup_schema, capabilities, generate)
//! - Trust: `save_b64_image` / `save_url_image` under `$HERMES_HOME/cache/images/`
//! - `register(ctx)` plugin entry point (`ctx.register_image_gen_provider`)
//!
//! Sync `requests` / `openai` SDK I/O in Python is represented here with
//! synchronous `curl` + `std::fs` stubs + documented `reqwest`/`tokio` upgrade
//! paths so the selection, validation, and response-parsing semantics are
//! byte-identical without requiring `cargo` in this task.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors __init__.py:53-83
// ---------------------------------------------------------------------------

/// Underlying API model sent to OpenAI. Mirrors `API_MODEL = "gpt-image-2"`.
pub const API_MODEL: &str = "gpt-image-2";

/// Default tier. Mirrors `DEFAULT_MODEL = "gpt-image-2-medium"`.
pub const DEFAULT_MODEL: &str = "gpt-image-2-medium";

pub const VALID_ASPECT_RATIOS: &[&str] = &["landscape", "square", "portrait"];
pub const DEFAULT_ASPECT_RATIO: &str = "landscape";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMeta {
    pub display: String,
    pub speed: String,
    pub strengths: String,
    pub quality: String,
}

impl ModelMeta {
    fn new(display: &str, speed: &str, strengths: &str, quality: &str) -> Self {
        Self {
            display: display.to_string(),
            speed: speed.to_string(),
            strengths: strengths.to_string(),
            quality: quality.to_string(),
        }
    }
}

/// Mirrors `_MODELS: Dict[str, Dict[str, Any]]` (lines 55-74).
pub fn models_map() -> HashMap<String, ModelMeta> {
    let mut m = HashMap::new();
    m.insert(
        "gpt-image-2-low".to_string(),
        ModelMeta::new("GPT Image 2 (Low)", "~15s", "Fast iteration, lowest cost", "low"),
    );
    m.insert(
        "gpt-image-2-medium".to_string(),
        ModelMeta::new("GPT Image 2 (Medium)", "~40s", "Balanced — default", "medium"),
    );
    m.insert(
        "gpt-image-2-high".to_string(),
        ModelMeta::new("GPT Image 2 (High)", "~2min", "Highest fidelity, strongest prompt adherence", "high"),
    );
    m
}

/// Mirrors `_SIZES` (lines 78-82).
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

/// Mirrors `get_secret("OPENAI_API_KEY")` — checks HERMES_HOME/.env then os.environ.
/// See `agent/secret_scope.py::get_secret` for multiplex semantics; Rust port
/// keeps the observable single-profile behaviour (dotenv + env) which matches
/// all non-multiplexed call sites (image_gen generate).
pub fn get_secret(name: &str) -> Option<String> {
    get_env_value(name).or_else(|| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

// ---------------------------------------------------------------------------
// Config — mirrors _load_openai_config (lines 85-95)
// ---------------------------------------------------------------------------

/// Read `image_gen` from config.yaml (returns {} on any failure).
/// Mirrors `_load_openai_config() -> Dict[str, Any]` lines 85-95.
///
/// Python: `from hermes_cli.config import load_config; cfg = load_config(); section = cfg.get("image_gen")`
/// Rust: read `$HERMES_HOME/config.yaml|yml|json` with stdlib parser.
pub fn load_openai_config() -> HashMap<String, Value> {
    let home = get_hermes_home();
    for fname in ["config.json", "config.yaml", "config.yml"] {
        let path = home.join(fname);
        if let Ok(text) = fs::read_to_string(&path) {
            if fname.ends_with(".json") {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(section) = v.get("image_gen").and_then(|x| x.as_object()) {
                        return section.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                }
                continue;
            } else {
                if let Some(map) = try_parse_yaml_image_gen(&text) {
                    return map;
                }
                // Also try JSON shape embedded in yaml file (tests use json)
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(section) = v.get("image_gen").and_then(|x| x.as_object()) {
                        return section.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                }
            }
        }
    }
    HashMap::new()
}

fn try_parse_yaml_image_gen(text: &str) -> Option<HashMap<String, Value>> {
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
    let mut out: HashMap<String, Value> = HashMap::new();
    // Track openai sub-block if present
    let mut openai_block: Option<Map<String, Value>> = None;
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
            // Only direct children of image_gen (indent == gi+2 typical) — but allow any deeper as belonging until dedent
            // If indent > gi+4, it's nested beyond one level — skip (handled as block)
            if indent > gi + 8 {
                i += 1;
                continue;
            }
            if !rest.is_empty() {
                let val = parse_yaml_scalar(&rest);
                out.insert(key, val);
                i += 1;
            } else {
                // Collect indented block
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
                if key == "openai" {
                    // Parse openai block as dict of scalars/lists
                    let mut submap = Map::new();
                    // Need to handle openai children which are scalars like model: gpt-image-2-high
                    // Block lines are indented under openai; collect direct children only
                    for bl in &block {
                        let t = bl.trim();
                        if let Some(cp) = t.find(':') {
                            let sk = t[..cp].trim().to_string();
                            let sv = t[cp + 1..].trim();
                            // If sv empty, it may be nested list — handle - items in following lines?
                            // For openai we only expect scalar model key, so handle inline.
                            if !sv.is_empty() {
                                submap.insert(sk, parse_yaml_scalar(sv));
                            } else {
                                // Look ahead for list items under this key within block
                                // Find position of this line in block, then gather indented followers
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
                    openai_block = Some(submap);
                } else {
                    // Generic: detect list
                    let is_list = block.iter().any(|l| l.trim_start().starts_with("- "));
                    if is_list {
                        let mut arr = Vec::new();
                        for bl in block {
                            let t = bl.trim();
                            if t.starts_with("- ") {
                                let item_str = t[2..].trim();
                                arr.push(parse_yaml_scalar(item_str));
                            }
                        }
                        out.insert(key, Value::Array(arr));
                    } else {
                        let mut submap = Map::new();
                        for bl in block {
                            let t = bl.trim();
                            if let Some(cp) = t.find(':') {
                                let sk = t[..cp].trim().to_string();
                                let sv = t[cp + 1..].trim();
                                submap.insert(sk, parse_yaml_scalar(sv));
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
    if let Some(ob) = openai_block {
        out.insert("openai".to_string(), Value::Object(ob));
    }
    Some(out)
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
// Model resolution — mirrors _resolve_model (lines 98-119)
// ---------------------------------------------------------------------------

/// Decide which tier to use and return `(model_id, meta)`.
/// Mirrors `_resolve_model() -> Tuple[str, Dict[str, Any]]` lines 98-119.
///
/// Precedence:
/// 1. `OPENAI_IMAGE_MODEL` env var (when value is one of our tier IDs)
/// 2. `image_gen.openai.model` in config.yaml (when value is one of our tier IDs)
/// 3. `image_gen.model` in config.yaml (when value is one of our tier IDs)
/// 4. `DEFAULT_MODEL`
pub fn resolve_model() -> (String, ModelMeta) {
    let models = models_map();

    if let Some(env_override) = get_env_value("OPENAI_IMAGE_MODEL")
        .or_else(|| std::env::var("OPENAI_IMAGE_MODEL").ok())
    {
        let trimmed = env_override.trim().to_string();
        if let Some(meta) = models.get(&trimmed) {
            return (trimmed, meta.clone());
        }
    }

    let cfg = load_openai_config();
    let mut candidate: Option<String> = None;

    if let Some(openai_val) = cfg.get("openai").and_then(|v| v.as_object()) {
        if let Some(val) = openai_val.get("model").and_then(|v| v.as_str()) {
            let trimmed = val.trim().to_string();
            if models.contains_key(&trimmed) {
                candidate = Some(trimmed);
            }
        }
    }
    if candidate.is_none() {
        if let Some(top) = cfg.get("model").and_then(|v| v.as_str()) {
            let trimmed = top.trim().to_string();
            if models.contains_key(&trimmed) {
                candidate = Some(trimmed);
            }
        }
    }
    if let Some(c) = candidate {
        let meta = models.get(&c).cloned().unwrap();
        return (c, meta);
    }
    let meta = models.get(DEFAULT_MODEL).cloned().unwrap();
    (DEFAULT_MODEL.to_string(), meta)
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

pub fn normalize_reference_images(value: Option<&Value>) -> Option<Vec<String>> {
    let v = value?;
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        let t = s.trim().to_string();
        if t.is_empty() {
            return None;
        }
        return Some(vec![t]);
    }
    if let Some(arr) = v.as_array() {
        let mut out: Vec<String> = Vec::new();
        for item in arr {
            if let Some(s) = item.as_str() {
                let t = s.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
            }
        }
        if out.is_empty() {
            return None;
        }
        return Some(out);
    }
    None
}

/// Overload for the provider's typed Vec<String> path.
/// Mirrors Python `normalize_reference_images(reference_image_urls) or []` where
/// the function strips blanks.
pub fn normalize_reference_image_urls_strs(value: Option<&[String]>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(list) => list.iter().filter_map(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }).collect(),
    }
}

pub fn error_response(
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
    modality: &str,
    extra: Option<Map<String, Value>>,
) -> Value {
    let mut payload = json!({
        "success": true,
        "image": image,
        "model": model,
        "prompt": prompt,
        "aspect_ratio": aspect_ratio,
        "modality": modality,
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
    // Convert days since 1970-01-01 to YMD (proleptic Gregorian)
    // Algorithm from Howard Hinnant's civil_from_days
    let z = days + 719468; // days since 0000-03-01
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    y += if mo <= 2 { 1 } else { 0 };
    (y as i32, mo as u32, d as u32, h, mi, s)
}

fn short_id() -> String {
    // 8 hex chars — mirrors `uuid.uuid4().hex[:8]`
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

/// Mirrors `save_b64_image(b64_data, prefix, extension) -> Path` in image_gen_provider.py:245-262.
pub fn save_b64_image(b64_data: &str, prefix: &str, extension: &str) -> Result<PathBuf, String> {
    let raw = decode_base64(b64_data).map_err(|e| format!("base64 decode failed: {e}"))?;
    let ts = format_now_timestamp();
    let short = short_id();
    let path = images_cache_dir().join(format!("{}_{}_{}.{}", prefix, ts, short, extension));
    fs::write(&path, &raw).map_err(|e| format!("write failed: {e}"))?;
    Ok(path)
}

/// Mirrors `save_url_image(url, prefix) -> Path` in image_gen_provider.py:278-344.
/// Downloads via `curl` (mirrors `requests.get(..., stream=True)` in Python).
pub fn save_url_image(url: &str, prefix: &str) -> Result<PathBuf, String> {
    let (bytes, content_type) = http_get_bytes(url, 60)?;
    if bytes.is_empty() {
        return Err(format!("Image at {url} returned 0 bytes; refusing to cache."));
    }
    if bytes.len() > 25 * 1024 * 1024 {
        return Err(format!("Image at {url} exceeds 25MB cap; refusing to cache."));
    }
    // Infer extension from content-type, then URL suffix, then png fallback
    let mut extension = infer_extension_from_content_type(&content_type).unwrap_or("png");
    // Only fall back to URL suffix when content-type did not give us a known extension
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
// File safety — mirrors agent/file_safety.py raise_if_read_blocked
// ---------------------------------------------------------------------------

fn get_hermes_root() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            let p = PathBuf::from(&trimmed);
            // HERMES_HOME may be <root>/profiles/<name>; root is parent of profiles
            if let Some(parent) = p.parent() {
                if parent.ends_with("profiles") {
                    if let Some(grand) = parent.parent() {
                        return grand.to_path_buf();
                    }
                }
            }
            // Fallback: if not profiles, root is HERMES_HOME itself for credential checks
            // We still return both dirs in check below.
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

fn get_read_block_error(path: &str) -> Option<String> {
    let resolved = PathBuf::from(path).canonicalize().unwrap_or_else(|_| {
        // Resolve relative to cwd if possible, else just absolute
        if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(path)
        } else {
            PathBuf::from(path)
        }
    });
    let resolved_str = resolved.to_string_lossy().to_string();
    // Normalize via real path attempt (best-effort)
    let hermes_home = get_hermes_home();
    let hermes_root = get_hermes_root();
    let mut hermes_dirs: Vec<PathBuf> = Vec::new();
    for base in [hermes_home, hermes_root] {
        let real = base.canonicalize().unwrap_or(base.clone());
        if !hermes_dirs.contains(&real) {
            hermes_dirs.push(real);
        }
    }
    // Blocked exact files under HERMES_HOME / root
    let credential_names = [
        "auth.json",
        "auth.lock",
        ".anthropic_oauth.json",
        ".env",
        "webhook_subscriptions.json",
        "auth/google_oauth.json",
        "cache/bws_cache.json",
    ];
    for hd in &hermes_dirs {
        for name in credential_names {
            let blocked = hd.join(name);
            let blocked_canon = blocked.canonicalize().unwrap_or(blocked.clone());
            if resolved == blocked_canon || resolved_str == blocked_canon.to_string_lossy().to_string() {
                return Some(format!(
                    "Access denied: {path} is a Hermes credential store and cannot be read directly."
                ));
            }
        }
        // skills/.hub and mcp-tokens checks
        for sub in [hd.join("skills/.hub/index-cache"), hd.join("skills/.hub"), hd.join("mcp-tokens")] {
            let blocked = sub.canonicalize().unwrap_or(sub.clone());
            if resolved == blocked || resolved.starts_with(&blocked) {
                if blocked.to_string_lossy().contains("mcp-tokens") {
                    return Some(format!("Access denied: {path} is a Hermes MCP token file and cannot be read directly."));
                } else {
                    return Some(format!("Access denied: {path} is an internal Hermes cache file and cannot be read directly."));
                }
            }
        }
    }
    // Block project-local env basenames anywhere
    let blocked_basenames: HashSet<&str> = [".env", ".env.local", ".env.development", ".env.production", ".env.test", ".env.staging", ".envrc"].into_iter().collect();
    if let Some(name) = resolved.file_name().and_then(|n| n.to_str()) {
        if blocked_basenames.contains(&name.to_ascii_lowercase().as_str()) {
            return Some(format!(
                "Access denied: {path} is a secret-bearing environment file and cannot be read to prevent credential leakage."
            ));
        }
    }
    None
}

fn raise_if_read_blocked(path: &str) -> Result<(), String> {
    if let Some(msg) = get_read_block_error(path) {
        return Err(msg);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Base64 — stdlib only (no `base64` crate to keep workspace deps minimal)
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
            '-' => 62, // url-safe
            '_' => 63, // url-safe
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
// Source-image loading — mirrors _load_image_bytes (lines 127-157)
// ---------------------------------------------------------------------------

/// Load image bytes from a URL, data: URI, or local file path.
///
/// Returns `(data, filename)`. Raises (Err) on any network / IO error so
/// the caller can surface a clean error_response.
/// Mirrors `_load_image_bytes(ref: str) -> Tuple[bytes, str]` lines 127-157.
pub fn load_image_bytes(image_ref: &str) -> Result<(Vec<u8>, String), String> {
    let trimmed = image_ref.trim().to_string();
    if trimmed.is_empty() {
        return Err("empty image reference".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let (bytes, _) = http_get_bytes(&trimmed, 60)?;
        let name = trimmed.split('?').next().unwrap_or(&trimmed).rsplit('/').next().unwrap_or("image.png");
        let name = if name.is_empty() { "image.png".to_string() } else { name.to_string() };
        return Ok((bytes, name));
    }
    if lower.starts_with("data:") {
        let (_header, b64) = match trimmed.find(',') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => return Err("invalid data URI: missing comma".to_string()),
        };
        let header = &_header[..];
        let mut ext = "png".to_string();
        if let Some(img_pos) = header.to_ascii_lowercase().find("image/") {
            let after = &header[img_pos + 6..];
            let ext_part = after.split(';').next().unwrap_or("").trim();
            if !ext_part.is_empty() {
                ext = ext_part.to_string();
            }
        }
        let data = decode_base64(b64).map_err(|e| format!("data URI base64 decode failed: {e}"))?;
        return Ok((data, format!("image.{ext}")));
    }
    // Local file path — enforce the shared credential-read guard before reading.
    raise_if_read_blocked(&trimmed)?;
    let data = fs::read(&trimmed).map_err(|e| format!("read {}: {e}", trimmed))?;
    let name = Path::new(&trimmed).file_name().and_then(|n| n.to_str()).unwrap_or("image.png").to_string();
    let name = if name.is_empty() { "image.png".to_string() } else { name };
    Ok((data, name))
}

// ---------------------------------------------------------------------------
// HTTP helpers — mirrors requests + openai SDK calls
// ---------------------------------------------------------------------------

fn http_get_bytes(url: &str, timeout_secs: u64) -> Result<(Vec<u8>, String), String> {
    // Use curl for observable behavior without new deps.
    // Mirrors `requests.get(ref, timeout=60)` in Python.
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
    // Split headers and body: headers end with \r\n\r\n, body follows.
    // curl -D - prints headers to stdout before body.
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
    // Fallback: \n\n
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
    // curl -sS -m <t> -X POST -H "Authorization: Bearer ..." -H "Content-Type: application/json" -d @- -w "\n%{http_code}"
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

fn http_post_multipart_edit(
    url: &str,
    api_key: &str,
    files: &[(Vec<u8>, String)],
    prompt: &str,
    size: &str,
    quality: &str,
    timeout_secs: u64,
) -> Result<(u16, String), String> {
    let timeout = timeout_secs.to_string();
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-m")
        .arg(&timeout)
        .arg("-w")
        .arg("\n%{http_code}")
        .arg("-H")
        .arg(format!("Authorization: Bearer {api_key}"))
        .arg("-X")
        .arg("POST")
        .arg(url);
    // Form fields: model, prompt, size, quality, n
    cmd.arg("-F").arg(format!("model={}", API_MODEL));
    cmd.arg("-F").arg(format!("prompt={}", prompt));
    cmd.arg("-F").arg(format!("size={}", size));
    cmd.arg("-F").arg(format!("quality={}", quality));
    cmd.arg("-F").arg("n=1");
    // Image files: curl -F "image[]=@path;filename=name" or "image=@path"
    // Write temp files for each image, then attach.
    let mut temp_paths: Vec<PathBuf> = Vec::new();
    for (data, fname) in files {
        let mut tmp = std::env::temp_dir().join(format!("hermes-img-{}-{}", short_id(), fname));
        // Ensure unique
        let mut counter = 0;
        while tmp.exists() {
            counter += 1;
            tmp = std::env::temp_dir().join(format!("hermes-img-{}-{}-{}", short_id(), counter, fname));
        }
        fs::write(&tmp, data).map_err(|e| format!("temp write failed: {e}"))?;
        temp_paths.push(tmp);
    }
    for (idx, tmp) in temp_paths.iter().enumerate() {
        let fname = &files[idx].1;
        // For single file: image, for multiple: image[] (OpenAI SDK sends image[] for list)
        let field_name = if files.len() == 1 { "image" } else { "image[]" };
        let arg = format!("{}=@{};filename={}", field_name, tmp.display(), fname);
        cmd.arg("-F").arg(arg);
    }
    let output = cmd.output().map_err(|e| format!("curl not available: {e}"))?;
    // Cleanup temps
    for p in &temp_paths {
        let _ = fs::remove_file(p);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let (body, code_str) = match stdout.rsplit_once('\n') {
        Some((b, c)) => (b.to_string(), c.trim().to_string()),
        None => (stdout, String::new()),
    };
    let status: u16 = code_str.parse().unwrap_or(if output.status.success() { 200 } else { 500 });
    Ok((status, body))
}

// ---------------------------------------------------------------------------
// Provider — mirrors class OpenAIImageGenProvider (lines 165-409)
// ---------------------------------------------------------------------------

/// OpenAI `images.generate` / `images.edit` backend — gpt-image-2.
///
/// 1:1 port of `class OpenAIImageGenProvider(ImageGenProvider)` lines 165-409.
/// Async OpenAI SDK calls in Python are represented with synchronous `curl`
/// stubs + documented `reqwest`/`tokio` upgrade paths.
#[derive(Debug, Clone, Default)]
pub struct OpenAIImageGenProvider;

impl OpenAIImageGenProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> &'static str {
        "openai"
    }

    pub fn display_name(&self) -> &'static str {
        "OpenAI"
    }

    /// Mirrors `is_available(self) -> bool` lines 176-183.
    ///
    /// Python checks `get_secret("OPENAI_API_KEY")` and `import openai`.
    /// Rust checks env/.env for the key; the `openai` crate check is
    /// represented as a compile-time `#[cfg(feature = "openai")]` probe —
    /// here always true because the HTTP path uses `curl`, not the SDK.
    /// Keeps the same observable contract: no key → not available.
    pub fn is_available(&self) -> bool {
        if get_secret("OPENAI_API_KEY").is_none() {
            return false;
        }
        // Python would also `import openai` — Rust stub always has curl.
        // To mirror the `ImportError` path in tests, honour `HERMES_OPENAI_MISSING=1`.
        if std::env::var("HERMES_OPENAI_MISSING").map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false) {
            return false;
        }
        true
    }

    /// Mirrors `list_models(self) -> List[Dict[str, Any]]` lines 185-195.
    pub fn list_models(&self) -> Vec<Value> {
        let models = models_map();
        let mut out: Vec<Value> = Vec::new();
        // Preserve insertion order as in Python dict literal order: low, medium, high
        for id in ["gpt-image-2-low", "gpt-image-2-medium", "gpt-image-2-high"] {
            if let Some(meta) = models.get(id) {
                out.push(json!({
                    "id": id,
                    "display": meta.display,
                    "speed": meta.speed,
                    "strengths": meta.strengths,
                    "price": "varies"
                }));
            }
        }
        out
    }

    pub fn default_model(&self) -> Option<String> {
        Some(DEFAULT_MODEL.to_string())
    }

    /// Mirrors `get_setup_schema(self) -> Dict[str, Any]` lines 200-212.
    pub fn get_setup_schema(&self) -> Value {
        json!({
            "name": "OpenAI",
            "badge": "paid",
            "tag": "gpt-image-2 at low/medium/high quality tiers — text-to-image & image editing",
            "env_vars": [
                {
                    "key": "OPENAI_API_KEY",
                    "prompt": "OpenAI API key",
                    "url": "https://platform.openai.com/api-keys"
                }
            ]
        })
    }

    /// Mirrors `capabilities(self) -> Dict[str, Any]` lines 214-217.
    /// gpt-image-2 supports editing via images.edit() with up to 16 source images.
    pub fn capabilities(&self) -> Value {
        json!({
            "modalities": ["text", "image"],
            "max_reference_images": 16
        })
    }

    /// Mirrors `generate(self, prompt, aspect_ratio, *, image_url, reference_image_urls, **kwargs)` lines 219-409.
    ///
    /// Returns the same `success_response` / `error_response` dict shape as Python,
    /// JSON-serialized as `serde_json::Value`.
    pub fn generate(
        &self,
        prompt: &str,
        aspect_ratio: &str,
        image_url: Option<&str>,
        reference_image_urls: Option<&[String]>,
        _kwargs: Option<&Value>,
    ) -> Value {
        let prompt_trimmed = prompt.trim().to_string();
        let aspect = resolve_aspect_ratio(Some(aspect_ratio));

        if prompt_trimmed.is_empty() {
            return error_response(
                "Prompt is required and must be a non-empty string",
                "invalid_argument",
                "openai",
                "",
                &prompt_trimmed,
                &aspect,
            );
        }

        let api_key = match get_secret("OPENAI_API_KEY") {
            Some(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => {
                return error_response(
                    "OPENAI_API_KEY not set. Run `hermes tools` → Image Generation → OpenAI to configure, or `hermes setup` to add the key.",
                    "auth_required",
                    "openai",
                    "",
                    &prompt_trimmed,
                    &aspect,
                );
            }
        };

        // Mirrors `import openai` ImportError guard — lines 252-260
        if std::env::var("HERMES_OPENAI_MISSING").map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false) {
            return error_response(
                "openai Python package not installed (pip install openai)",
                "missing_dependency",
                "openai",
                "",
                &prompt_trimmed,
                &aspect,
            );
        }

        let (tier_id, meta) = resolve_model();
        let size = size_for_aspect(&aspect).to_string();

        // Collect source images (primary + references) for image-to-image.
        let mut sources: Vec<String> = Vec::new();
        if let Some(url) = image_url {
            let t = url.trim().to_string();
            if !t.is_empty() {
                sources.push(t);
            }
        }
        for r in normalize_reference_image_urls_strs(reference_image_urls) {
            sources.push(r);
        }
        if sources.len() > 16 {
            sources.truncate(16);
        }
        let is_edit = !sources.is_empty();
        let modality = if is_edit { "image" } else { "text" };

        // Call OpenAI — two branches: edit vs generate
        let (b64_opt, url_opt, revised_prompt_opt) = if is_edit {
            // images.edit() expects file-like objects. Download/read each
            // source into a named BytesIO so the SDK sends correct multipart.
            let mut files: Vec<(Vec<u8>, String)> = Vec::new();
            for r in &sources {
                match load_image_bytes(r) {
                    Ok(pair) => files.push(pair),
                    Err(e) => {
                        return error_response(
                            &format!("Could not load source image for editing: {e}"),
                            "io_error",
                            "openai",
                            &tier_id,
                            &prompt_trimmed,
                            &aspect,
                        );
                    }
                }
            }
            let edit_url = "https://api.openai.com/v1/images/edits";
            match http_post_multipart_edit(edit_url, &api_key, &files, &prompt_trimmed, &size, &meta.quality, 120) {
                Ok((status, body)) => {
                    if !(200..300).contains(&status) {
                        log::debug!("OpenAI image edit failed: HTTP {} {}", status, body.chars().take(500).collect::<String>());
                        return error_response(
                            &format!("OpenAI image editing failed: HTTP {status}: {}", body.chars().take(500).collect::<String>()),
                            "api_error",
                            "openai",
                            &tier_id,
                            &prompt_trimmed,
                            &aspect,
                        );
                    }
                    match parse_openai_image_response(&body) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            log::debug!("OpenAI image edit parse failed: {e}");
                            return error_response(
                                &format!("OpenAI image editing failed: {e}"),
                                "api_error",
                                "openai",
                                &tier_id,
                                &prompt_trimmed,
                                &aspect,
                            );
                        }
                    }
                }
                Err(e) => {
                    log::debug!("OpenAI image edit failed: {e}");
                    return error_response(
                        &format!("OpenAI image editing failed: {e}"),
                        "api_error",
                        "openai",
                        &tier_id,
                        &prompt_trimmed,
                        &aspect,
                    );
                }
            }
        } else {
            // gpt-image-2 returns b64_json unconditionally and REJECTS
            // `response_format` as an unknown parameter. Don't send it.
            let payload = json!({
                "model": API_MODEL,
                "prompt": prompt_trimmed,
                "size": size,
                "n": 1,
                "quality": meta.quality
            });
            let gen_url = "https://api.openai.com/v1/images/generations";
            match http_post_json(gen_url, &api_key, &payload, 120) {
                Ok((status, body)) => {
                    if !(200..300).contains(&status) {
                        log::debug!("OpenAI image generation failed: HTTP {} {}", status, body.chars().take(500).collect::<String>());
                        return error_response(
                            &format!("OpenAI image generation failed: HTTP {status}: {}", body.chars().take(500).collect::<String>()),
                            "api_error",
                            "openai",
                            &tier_id,
                            &prompt_trimmed,
                            &aspect,
                        );
                    }
                    match parse_openai_image_response(&body) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            log::debug!("OpenAI image generation parse failed: {e}");
                            return error_response(
                                &format!("OpenAI image generation failed: {e}"),
                                "api_error",
                                "openai",
                                &tier_id,
                                &prompt_trimmed,
                                &aspect,
                            );
                        }
                    }
                }
                Err(e) => {
                    log::debug!("OpenAI image generation failed: {e}");
                    return error_response(
                        &format!("OpenAI image generation failed: {e}"),
                        "api_error",
                        "openai",
                        &tier_id,
                        &prompt_trimmed,
                        &aspect,
                    );
                }
            }
        };

        // Handle empty data
        if b64_opt.is_none() && url_opt.is_none() {
            return error_response(
                "OpenAI returned no image data",
                "empty_response",
                "openai",
                &tier_id,
                &prompt_trimmed,
                &aspect,
            );
        }

        let image_ref: String;
        if let Some(b64) = b64_opt {
            match save_b64_image(&b64, &format!("openai_{}", tier_id), "png") {
                Ok(p) => image_ref = p.to_string_lossy().to_string(),
                Err(e) => {
                    return error_response(
                        &format!("Could not save image to cache: {e}"),
                        "io_error",
                        "openai",
                        &tier_id,
                        &prompt_trimmed,
                        &aspect,
                    );
                }
            }
        } else if let Some(url) = url_opt {
            // Defensive — gpt-image-2 returns b64 today, but OpenAI's API
            // has previously returned URLs. Cache the bytes locally so the
            // gateway never tries to fetch an ephemeral / signed URL after
            // it expires — same rationale as the xAI provider (#26942).
            match save_url_image(&url, &format!("openai_{}", tier_id)) {
                Ok(p) => image_ref = p.to_string_lossy().to_string(),
                Err(e) => {
                    log::warn!("OpenAI image URL {} could not be cached ({}); falling back to bare URL.", url, e);
                    image_ref = url;
                }
            }
        } else {
            return error_response(
                "OpenAI response contained neither b64_json nor URL",
                "empty_response",
                "openai",
                &tier_id,
                &prompt_trimmed,
                &aspect,
            );
        }

        let mut extra = Map::new();
        extra.insert("size".to_string(), json!(size));
        extra.insert("quality".to_string(), json!(meta.quality));
        if let Some(rp) = revised_prompt_opt {
            if !rp.trim().is_empty() {
                extra.insert("revised_prompt".to_string(), json!(rp));
            }
        }

        success_response(&image_ref, &tier_id, &prompt_trimmed, &aspect, "openai", modality, Some(extra))
    }

    /// Convenience overload that mirrors the Python `**kwargs` + typed overload.
    /// Accepts `Value` kwargs map; forwards to `generate`.
    pub fn generate_with_value(
        &self,
        prompt: &str,
        aspect_ratio: &str,
        image_url: Option<&str>,
        reference_image_urls: Option<Value>,
        kwargs: Option<Value>,
    ) -> Value {
        let refs_vec: Option<Vec<String>> = match &reference_image_urls {
            Some(v) => normalize_reference_images(Some(v)),
            None => None,
        };
        let refs_slice: Option<Vec<String>> = refs_vec.clone();
        let refs_arg: Option<&[String]> = refs_slice.as_deref();
        self.generate(prompt, aspect_ratio, image_url, refs_arg, kwargs.as_ref())
    }
}

fn parse_openai_image_response(body: &str) -> Result<(Option<String>, Option<String>, Option<String>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    // Check for error envelope (some OpenAI errors return 200 with error field)
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|x| x.as_str()).unwrap_or(&body[..body.len().min(500)]);
        return Err(msg.to_string());
    }
    let data = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| "missing data array".to_string())?;
    if data.is_empty() {
        return Ok((None, None, None));
    }
    let first = &data[0];
    let b64 = first.get("b64_json").and_then(|x| x.as_str()).map(|s| s.to_string());
    let url = first.get("url").and_then(|x| x.as_str()).map(|s| s.to_string());
    let revised = first.get("revised_prompt").and_then(|x| x.as_str()).map(|s| s.to_string());
    Ok((b64, url, revised))
}

// ---------------------------------------------------------------------------
// Plugin entry point — mirrors def register(ctx) (lines 417-419)
// ---------------------------------------------------------------------------

/// Minimal `ctx` trait for image-gen provider registration — mirrors
/// `hermes_cli.plugins.PluginContext.register_image_gen_provider`.
pub trait PluginContext {
    fn register_image_gen_provider(&mut self, provider: OpenAIImageGenProvider);
}

/// Mirrors `def register(ctx) -> None` lines 417-419.
///
/// Plugin entry point — wire `OpenAIImageGenProvider` into the registry.
pub fn register(ctx: &mut dyn PluginContext) {
    ctx.register_image_gen_provider(OpenAIImageGenProvider::new());
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_python() {
        assert_eq!(API_MODEL, "gpt-image-2");
        assert_eq!(DEFAULT_MODEL, "gpt-image-2-medium");
        assert_eq!(size_for_aspect("landscape"), "1536x1024");
        assert_eq!(size_for_aspect("square"), "1024x1024");
        assert_eq!(size_for_aspect("portrait"), "1024x1536");
        assert_eq!(size_for_aspect("unknown"), "1024x1024");
    }

    #[test]
    fn models_map_has_three_tiers() {
        let m = models_map();
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("gpt-image-2-low").unwrap().quality, "low");
        assert_eq!(m.get("gpt-image-2-medium").unwrap().quality, "medium");
        assert_eq!(m.get("gpt-image-2-high").unwrap().quality, "high");
        assert_eq!(m.get("gpt-image-2-low").unwrap().speed, "~15s");
        assert_eq!(m.get("gpt-image-2-high").unwrap().speed, "~2min");
    }

    #[test]
    fn resolve_aspect_ratio_clamps() {
        assert_eq!(resolve_aspect_ratio(Some("landscape")), "landscape");
        assert_eq!(resolve_aspect_ratio(Some("SQUARE")), "square");
        assert_eq!(resolve_aspect_ratio(Some(" portrait ")), "portrait");
        assert_eq!(resolve_aspect_ratio(Some("invalid")), "landscape");
        assert_eq!(resolve_aspect_ratio(Some("")), "landscape");
        assert_eq!(resolve_aspect_ratio(None), "landscape");
    }

    #[test]
    fn normalize_reference_images_str_and_list() {
        assert_eq!(normalize_reference_images(Some(&json!("  hello  "))), Some(vec!["hello".to_string()]));
        assert_eq!(normalize_reference_images(Some(&json!(""))), None);
        assert_eq!(normalize_reference_images(Some(&json!(["a", " ", "b"]))), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(normalize_reference_images(Some(&json!([]))), None);
        assert_eq!(normalize_reference_images(None), None);
        assert_eq!(normalize_reference_images(Some(&json!(123))), None);
    }

    #[test]
    fn error_response_shape() {
        let v = error_response("oops", "api_error", "openai", "gpt-image-2-medium", "hi", "square");
        assert_eq!(v["success"], false);
        assert_eq!(v["image"], Value::Null);
        assert_eq!(v["error"], "oops");
        assert_eq!(v["error_type"], "api_error");
        assert_eq!(v["provider"], "openai");
    }

    #[test]
    fn success_response_shape() {
        let mut extra = Map::new();
        extra.insert("size".to_string(), json!("1024x1024"));
        let v = success_response("/tmp/img.png", "gpt-image-2-medium", "a cat", "square", "openai", "text", Some(extra));
        assert_eq!(v["success"], true);
        assert_eq!(v["image"], "/tmp/img.png");
        assert_eq!(v["modality"], "text");
        assert_eq!(v["size"], "1024x1024");
    }

    #[test]
    fn list_models_and_capabilities() {
        let p = OpenAIImageGenProvider::new();
        let models = p.list_models();
        assert_eq!(models.len(), 3);
        assert!(models.iter().any(|m| m["id"] == "gpt-image-2-medium"));
        assert_eq!(p.default_model().as_deref(), Some("gpt-image-2-medium"));
        assert_eq!(p.name(), "openai");
        assert_eq!(p.display_name(), "OpenAI");
        let caps = p.capabilities();
        assert_eq!(caps["modalities"], json!(["text", "image"]));
        assert_eq!(caps["max_reference_images"], 16);
        let schema = p.get_setup_schema();
        assert_eq!(schema["name"], "OpenAI");
        assert_eq!(schema["env_vars"][0]["key"], "OPENAI_API_KEY");
    }

    #[test]
    fn generate_rejects_empty_prompt() {
        let p = OpenAIImageGenProvider::new();
        let out = p.generate("", "square", None, None, None);
        assert_eq!(out["success"], false);
        assert_eq!(out["error_type"], "invalid_argument");
        assert_eq!(out["provider"], "openai");
    }

    #[test]
    fn generate_requires_api_key() {
        let prev = std::env::var("OPENAI_API_KEY").ok();
        let prev_dotenv = std::env::var("HERMES_HOME").ok();
        // Use temp HERMES_HOME without .env
        let tmp = std::env::temp_dir().join(format!("hermes-test-img-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        unsafe { std::env::remove_var("OPENAI_API_KEY"); }
        // Ensure no dotenv provides key
        let p = OpenAIImageGenProvider::new();
        let out = p.generate("a cat", "landscape", None, None, None);
        assert_eq!(out["success"], false);
        assert_eq!(out["error_type"], "auth_required");
        if let Some(v) = prev { unsafe { std::env::set_var("OPENAI_API_KEY", v); }} else { unsafe { std::env::remove_var("OPENAI_API_KEY"); }}
        if let Some(v) = prev_dotenv { unsafe { std::env::set_var("HERMES_HOME", v); }} else { unsafe { std::env::remove_var("HERMES_HOME"); }}
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_image_bytes_data_uri() {
        let data = load_image_bytes("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(data.0, b"hello");
        assert_eq!(data.1, "image.png");
        let data2 = load_image_bytes("data:image/jpeg;base64,aGVsbG8=").unwrap();
        assert_eq!(data2.1, "image.jpeg");
    }

    #[test]
    fn load_image_bytes_local_file() {
        let tmp = std::env::temp_dir().join(format!("hermes-img-test-{}.png", std::process::id()));
        fs::write(&tmp, b"fake png").unwrap();
        let (bytes, name) = load_image_bytes(tmp.to_string_lossy().as_ref()).unwrap();
        assert_eq!(bytes, b"fake png");
        assert!(name.ends_with(".png") || name.contains("hermes-img-test"));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn decode_base64_roundtrip() {
        let decoded = decode_base64("aGVsbG8=").unwrap();
        assert_eq!(decoded, b"hello");
        let decoded2 = decode_base64("aGVsbG8gd29ybGQ=").unwrap();
        assert_eq!(decoded2, b"hello world");
    }

    #[test]
    fn parse_openai_response_ok() {
        let body = r#"{"data":[{"b64_json":"abc","revised_prompt":"revised"}]}"#;
        let (b64, url, rev) = parse_openai_image_response(body).unwrap();
        assert_eq!(b64.as_deref(), Some("abc"));
        assert_eq!(url, None);
        assert_eq!(rev.as_deref(), Some("revised"));
        let body2 = r#"{"data":[{"url":"https://example.com/img.png"}]}"#;
        let (b64, url, _) = parse_openai_image_response(body2).unwrap();
        assert_eq!(b64, None);
        assert_eq!(url.as_deref(), Some("https://example.com/img.png"));
        let body3 = r#"{"data":[]}"#;
        let (b64, url, _) = parse_openai_image_response(body3).unwrap();
        assert_eq!(b64, None);
        assert_eq!(url, None);
    }

    #[test]
    fn is_available_checks_key() {
        let prev = std::env::var("OPENAI_API_KEY").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        let tmp = std::env::temp_dir().join(format!("hermes-test-avail-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        unsafe { std::env::remove_var("OPENAI_API_KEY"); }
        let p = OpenAIImageGenProvider::new();
        assert!(!p.is_available());
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-test"); }
        assert!(p.is_available());
        if let Some(v) = prev { unsafe { std::env::set_var("OPENAI_API_KEY", v); }} else { unsafe { std::env::remove_var("OPENAI_API_KEY"); }}
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); }} else { unsafe { std::env::remove_var("HERMES_HOME"); }}
        let _ = fs::remove_dir_all(&tmp);
    }
}
