//! Environment manifest for project verification.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/verify/environment.py` (75 lines).
//!
//! Ported from superagent-ai/grok-cli `src/verify/environment.ts`.
//! The manifest lives at `<project>/.hermes/environment.json` and is the
//! user-editable source of truth: when present and valid it wins over fresh
//! static detection.
//!
//! Python source docstring (preserved):
//! ```text
//! Environment manifest for project verification.
//!
//! Ported from superagent-ai/grok-cli ``src/verify/environment.ts``.
//! The manifest lives at ``<project>/.hermes/environment.json`` and is the
//! user-editable source of truth: when present and valid it wins over fresh
//! static detection.
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 17-18
// ---------------------------------------------------------------------------

/// Mirrors `MANIFEST_VERSION = 1` (line 17).
pub const MANIFEST_VERSION: i32 = 1;

/// Mirrors `_MANIFEST_RELPATH = Path(".hermes") / "environment.json"` (line 18).
/// Stored as a string constant for `Path::join` composition.
pub const MANIFEST_RELPATH_STR: &str = ".hermes/environment.json";

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _MANIFEST_RELPATH_STR: &str = MANIFEST_RELPATH_STR;
#[allow(dead_code)]
const _MANIFEST_VERSION: i32 = MANIFEST_VERSION;

/// Returns the relative manifest path as a `PathBuf` (`.hermes/environment.json`).
/// Mirrors `_MANIFEST_RELPATH` Path composition (line 18).
fn manifest_relpath() -> PathBuf {
    Path::new(".hermes").join("environment.json")
}

#[allow(dead_code)]
fn _manifest_relpath() -> PathBuf {
    manifest_relpath()
}

// ---------------------------------------------------------------------------
// Recipe — minimal copy from `agent/verify/recipes.py` (lines 34-115)
// so this module is self-contained (mirrors the `pet_state` pattern that
// copies `PetState` inline). The full detection logic lives in the future
// `verify_recipes` port; this struct only needs `to_dict` / `from_dict`
// for manifest persistence.
// ---------------------------------------------------------------------------

/// A runnable verification recipe for a project.
///
/// Mirrors grok-cli's `VerifyRecipe` with a scoped field set:
/// `name` is the human label (grok's `appLabel`), `kind` the detector id
/// (grok's `appKind`), and command lists are shell strings executed in the
/// project root. Mirrors `agent/verify/recipes.py:Recipe` (lines 34-115).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub name: String,
    pub kind: String,
    pub bootstrap: Vec<String>,
    pub build: Vec<String>,
    pub test: Vec<String>,
    pub start: Option<String>,
    pub port: Option<u16>,
    pub readiness_path: String,
    pub evidence: Vec<String>,
}

impl Recipe {
    /// Create a new recipe — mirrors `Recipe(name=..., kind=...)` construction.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: "unknown".to_string(),
            bootstrap: Vec::new(),
            build: Vec::new(),
            test: Vec::new(),
            start: None,
            port: None,
            readiness_path: "/".to_string(),
            evidence: Vec::new(),
        }
    }

    /// Full constructor mirroring dataclass fields (lines 44-52).
    #[allow(clippy::too_many_arguments)]
    pub fn with_fields(
        name: impl Into<String>,
        kind: impl Into<String>,
        bootstrap: Vec<String>,
        build: Vec<String>,
        test: Vec<String>,
        start: Option<String>,
        port: Option<u16>,
        readiness_path: impl Into<String>,
        evidence: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            bootstrap,
            build,
            test,
            start,
            port,
            readiness_path: readiness_path.into(),
            evidence,
        }
    }

    /// Mirrors `to_dict(self) -> dict[str, Any]` (lines 54-65).
    pub fn to_dict(&self) -> Value {
        json!({
            "name": self.name,
            "kind": self.kind,
            "bootstrap": self.bootstrap,
            "build": self.build,
            "test": self.test,
            "start": self.start,
            "port": self.port,
            "readinessPath": self.readiness_path,
            "evidence": self.evidence,
        })
    }

    /// Tolerant loader mirroring grok's `normalizeVerifyRecipe` (lines 67-115).
    /// Returns `None` when `raw` is not a dict or lacks a valid `name`.
    pub fn from_dict(raw: &Value) -> Option<Self> {
        let obj = raw.as_object()?;
        // name = raw.get("name") or raw.get("appLabel") (line 72)
        let name_val = obj.get("name").or_else(|| obj.get("appLabel"))?;
        let name_str = name_val.as_str()?;
        let name_trim = name_str.trim();
        if name_trim.is_empty() {
            return None;
        }
        // kind = raw.get("kind") or raw.get("appKind") or "unknown" (lines 75-77)
        let kind_raw = obj.get("kind").or_else(|| obj.get("appKind"));
        let kind_str = match kind_raw {
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() { "unknown" } else { t }
            }
            Some(_) => "unknown",
            None => "unknown",
        };
        let kind = kind_str.to_string();

        // helper mirroring `as_strings` (lines 79-84)
        let bootstrap = as_strings(obj.get("bootstrap").or_else(|| obj.get("installCommands")));
        let build = as_strings(obj.get("build").or_else(|| obj.get("buildCommands")));
        let test = as_strings(obj.get("test").or_else(|| obj.get("testCommands")));
        let evidence = as_strings(obj.get("evidence"));

        // start = raw.get("start") or raw.get("startCommand") (lines 86-90)
        let start_raw = obj.get("start").or_else(|| obj.get("startCommand"));
        let start = match start_raw {
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() { None } else { Some(t.to_string()) }
            }
            _ => None,
        };

        // port handling (lines 92-99)
        let port_raw = obj.get("port").or_else(|| obj.get("startPort"));
        let port: Option<u16> = match port_raw {
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    if i > 0 && i < 65536 { Some(i as u16) } else { None }
                } else if let Some(u) = n.as_u64() {
                    if u > 0 && u < 65536 { Some(u as u16) } else { None }
                } else {
                    None
                }
            }
            Some(Value::String(s)) => {
                let t = s.trim();
                if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(candidate) = t.parse::<i64>() {
                        if candidate > 0 && candidate < 65536 {
                            Some(candidate as u16)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        // readiness = raw.get("readinessPath") or raw.get("readiness_path") or "/" (lines 101-103)
        let readiness_raw = obj.get("readinessPath").or_else(|| obj.get("readiness_path"));
        let readiness_path = match readiness_raw {
            Some(Value::String(s)) if s.starts_with('/') => s.clone(),
            Some(Value::String(_)) => "/".to_string(),
            _ => "/".to_string(),
        };
        // Python checks `isinstance(readiness, str) and readiness.startswith("/")` else "/"
        // Already handled: non-string or non-slash → "/"

        Some(Self {
            name: name_trim.to_string(),
            kind,
            bootstrap,
            build,
            test,
            start,
            port,
            readiness_path,
            evidence,
        })
    }
}

/// Mirrors `as_strings` helper inside `Recipe.from_dict` (lines 79-84).
fn as_strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_string()]
            }
        }
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => {
                    let t = s.trim();
                    if t.is_empty() { None } else { Some(t.to_string()) }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[allow(dead_code)]
fn _as_strings(value: Option<&Value>) -> Vec<String> {
    as_strings(value)
}

// ---------------------------------------------------------------------------
// Manifest helpers — mirrors lines 21-75
// ---------------------------------------------------------------------------

/// Path of the verify manifest for the project at `root`.
/// Mirrors `manifest_path(root: Path) -> Path` (lines 21-23).
pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(manifest_relpath())
}

#[allow(dead_code)]
fn _manifest_path(root: &Path) -> PathBuf {
    manifest_path(root)
}

/// Load the saved recipe from the manifest, tolerating malformed files.
///
/// Mirrors grok's `loadVerifyEnvironment`: any read/parse/shape problem
/// returns `None` rather than raising, so a corrupt manifest degrades to
/// fresh detection instead of breaking `hermes verify`.
///
/// Mirrors `load_manifest(root: Path) -> Recipe | None` (lines 26-46).
pub fn load_manifest(root: &Path) -> Option<Recipe> {
    let path = manifest_path(root);
    // Mirrors `try: raw = path.read_text(encoding="utf-8") except OSError: return None` (lines 34-37)
    let raw = fs::read_to_string(&path).ok()?;
    // Mirrors `try: manifest = json.loads(raw) except (JSONDecodeError, ValueError): return None` (lines 38-41)
    let manifest: Value = serde_json::from_str(&raw).ok()?;
    if !manifest.is_object() {
        return None;
    }
    // Accept both the wrapped {version, recipe} shape and a bare recipe. (line 44-45)
    let recipe_raw = manifest.get("recipe").unwrap_or(&manifest);
    Recipe::from_dict(recipe_raw)
}

/// Persist `recipe` as the project's verify manifest.
///
/// Writes the versioned wrapper shape (grok's `saveVerifyEnvironment`
/// equivalent) and returns the manifest path.
///
/// Mirrors `save_manifest(root: Path, recipe: Recipe) -> Path` (lines 49-63).
pub fn save_manifest(root: &Path, recipe: &Recipe) -> std::io::Result<PathBuf> {
    let path = manifest_path(root);
    if let Some(parent) = path.parent() {
        // Mirrors `path.parent.mkdir(parents=True, exist_ok=True)` (line 56)
        fs::create_dir_all(parent)?;
    }
    // Mirrors payload construction (lines 57-61)
    let payload = json!({
        "version": MANIFEST_VERSION,
        "recipe": recipe.to_dict(),
        "updatedAt": utc_now_iso(),
    });
    // Mirrors `json.dumps(payload, indent=2) + "\n"` (line 62)
    let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    fs::write(&path, pretty + "\n")?;
    Ok(path)
}

/// Return (recipe, source) where source is 'manifest' or 'detected'.
///
/// A saved manifest wins over fresh detection, matching grok-cli's
/// behavior where `.grok/environment.json` is the source of truth.
///
/// Mirrors `load_or_detect(root: Path) -> tuple[Recipe | None, str]` (lines 66-75).
pub fn load_or_detect(root: &Path) -> (Option<Recipe>, &'static str) {
    // Mirrors `saved = load_manifest(root); if saved is not None: return saved, "manifest"` (lines 72-74)
    if let Some(saved) = load_manifest(root) {
        return (Some(saved), "manifest");
    }
    // Mirrors `return detect_recipe(root), "detected"` (line 75)
    (detect_recipe(root), "detected")
}

// ---------------------------------------------------------------------------
// detect_recipe — mirrors `from agent.verify.recipes import detect_recipe` (line 15)
// The full detection order lives in `agent/verify/recipes.py:detect_recipe`
// (package.json wins, then Python, Go, Rust, Java, Makefile, docker-compose).
// This stub is the 1:1 import placeholder until the recipes port lands;
// it returns `None` so `load_or_detect` falls back to `None + "detected"`.
// ---------------------------------------------------------------------------

/// Detect a verification recipe for the project at `root`.
///
/// Placeholder mirroring `detect_recipe` from `agent.verify.recipes`
/// (imported at line 15, defined at recipes.py line 459). The real
/// implementation mirrors grok's `inferFallbackRecipe`.
///
pub fn detect_recipe(_root: &Path) -> Option<Recipe> {
    None
}

#[allow(dead_code)]
fn _detect_recipe(root: &Path) -> Option<Recipe> {
    detect_recipe(root)
}

// ---------------------------------------------------------------------------
// utc_now_iso — mirrors `datetime.now(timezone.utc).isoformat()` (line 60)
// Dependency-free RFC3339 via SystemTime + civil_from_days (no chrono).
// ---------------------------------------------------------------------------

fn utc_now_iso() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert days since Unix epoch (1970-01-01) to civil date (year, month, day).
/// Howard Hinnant's civil_from_days algorithm (public domain).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

#[allow(dead_code)]
fn _civil_from_days(z: i64) -> (i32, u32, u32) {
    civil_from_days(z)
}

#[allow(dead_code)]
fn _utc_now_iso() -> String {
    utc_now_iso()
}
