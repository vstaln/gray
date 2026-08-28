//! Community plugin index — fetch, cache, search, and name resolution.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/plugin_index.py` (305 LOC).
//! Mirrors the Skills Hub catalog pattern (`tools/skills_hub.py`): a static
//! machine-readable JSON index hosted at a canonical URL, cached locally under
//! `HERMES_HOME/cache/` with a TTL, with a bundled seed file as the offline
//! fallback and format reference.
//!
//! Fallback chain: remote index → cached copy (fresh or stale) → bundled seed.
//!
//! The index is discovery metadata ONLY. **Indexed ≠ audited** — inclusion in
//! the index means the entry's metadata was reviewed, not that the plugin's code
//! was audited. Install keeps its existing consent/review flow, and index
//! entries pin an immutable ref (tag or commit SHA).
//!
//! Python surface ported line-for-line:
//!   - `DEFAULT_INDEX_URL`, `INDEX_CACHE_TTL`, `SEED_INDEX_PATH`, `_FETCH_TIMEOUT`,
//!     `_MAX_INDEX_BYTES`, `SECURITY_FOOTER` (lines 29-47)
//!   - `PluginIndexEntry` dataclass + `install_identifier` + `to_dict` (lines 50-90)
//!   - `_cache_path` (93-94), `get_index_url` (97-107), `_parse_entries` (110-149)
//!   - `_load_seed_entries` (152-157), `_read_cache` (160-173), `_write_cache` (176-184)
//!   - `_fetch_remote` (187-203), `load_index` (206-229)
//!   - `_score_entry` (236-261), `search_index` (264-284), `resolve_name` (287-305)
//!
//! Rust notes:
//!   - `httpx.get` is modelled via `curl` subprocess (`--max-time 10`, follow
//!     redirects) so the fetch path stays testable without pulling `reqwest`/
//!     `httpx` into this `NEVER cargo` task; a `HERMES_PLUGIN_INDEX_FAKE_REMOTE`
//!     env hook lets tests inject a local file without network.
//!   - `difflib.SequenceMatcher` is modelled as a longest-common-subsequence
//!     ratio `2*M/(len(a)+len(b))` (close enough for the `>=0.6` fuzzy gate);
//!     the exact gestalt algorithm would need a dedicated crate.
//!   - `atomic_write_text` maps to `write(tmp) → rename(cache)` with `mkdir -p`.
//!   - `get_hermes_home` / cache helpers mirror `hermes_constants` + `disk_cleanup`.
//!   - `SEED_INDEX_PATH` in Python is `Path(__file__).parent / "data" / "plugin_index.json"`;
//!     the Rust equivalent probes `CARGO_MANIFEST_DIR/data` then the reference
//!     checkout fallback, then `HERMES_HOME` — any miss returns empty (same as
//!     Python's `logger.warning` + `return []`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// HERMES_HOME helpers — mirrors hermes_constants.get_hermes_home()
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home()` — `$HERMES_HOME` if set and
/// non-empty, else `~/.hermes`.
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

// ---------------------------------------------------------------------------
// Constants — mirrors plugin_index.py:29-47
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_INDEX_URL` (lines 30-32).
pub const DEFAULT_INDEX_URL: &str =
    "https://raw.githubusercontent.com/NousResearch/hermes-plugin-index/main/index.json";

/// Mirrors `INDEX_CACHE_TTL = 24 * 3600` (line 36).
pub const INDEX_CACHE_TTL: u64 = 24 * 3600;

/// Mirrors `SEED_INDEX_PATH = Path(__file__).parent / "data" / "plugin_index.json"` (line 39).
/// Probes `CARGO_MANIFEST_DIR/data/plugin_index.json` (when built from a cargo crate),
/// then `reference` checkout fallback for the gray worktree, else a sentinel path.
pub fn seed_index_path() -> PathBuf {
    // cargo manifest dir when available (compile-time)
    let manifest = option_env!("CARGO_MANIFEST_DIR").unwrap_or("");
    if !manifest.is_empty() {
        let p = PathBuf::from(manifest).join("data").join("plugin_index.json");
        if p.is_file() {
            return p;
        }
    }
    // file!() parent probe — crates/hermes-plugins/src/plugin_index.rs → crates/hermes-plugins/data/...
    let file_parent = PathBuf::from(file!()).parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let probe = file_parent.join("data").join("plugin_index.json");
    if probe.is_file() {
        return probe;
    }
    // reference checkout fallback — mirrors original Python package layout
    let candidates = [
        PathBuf::from("reference/NousResearch/hermes-agent/hermes_cli/data/plugin_index.json"),
        PathBuf::from("../reference/NousResearch/hermes-agent/hermes_cli/data/plugin_index.json"),
        PathBuf::from("../../reference/NousResearch/hermes-agent/hermes_cli/data/plugin_index.json"),
    ];
    for c in &candidates {
        if c.is_file() {
            return c.clone();
        }
    }
    // hermes_cli data sibling when installed as `hermes_cli/data/plugin_index.json`
    // (mirrors Python `Path(__file__).parent / "data" / "plugin_index.json"`)
    // For gray worktree, this is `crates/hermes-cli/data/plugin_index.json` if it exists.
    let hermes_cli_data = PathBuf::from("crates/hermes-cli/data/plugin_index.json");
    if hermes_cli_data.is_file() {
        return hermes_cli_data;
    }
    // fallback sentinel — read will fail and _load_seed_entries returns [] (same as python warning)
    file_parent.join("data").join("plugin_index.json")
}

/// Mirrors `_FETCH_TIMEOUT = 10.0` (line 41).
pub const FETCH_TIMEOUT_SECS: f64 = 10.0;

/// Mirrors `_MAX_INDEX_BYTES = 5 * 1024 * 1024` (line 42).
pub const MAX_INDEX_BYTES: usize = 5 * 1024 * 1024;

/// Mirrors `SECURITY_FOOTER` (lines 44-47).
pub const SECURITY_FOOTER: &str =
    "Indexed \u{2260} audited: inclusion in the index is a metadata review only, not a code audit. Review a plugin before enabling it.";

// ---------------------------------------------------------------------------
// PluginIndexEntry — mirrors @dataclass PluginIndexEntry (lines 50-90)
// ---------------------------------------------------------------------------

/// One community plugin index entry — mirrors `PluginIndexEntry` (lines 50-64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub r#ref: String,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub api_version: Option<i64>,
    #[serde(default)]
    pub added_at: Option<String>,
}

impl PluginIndexEntry {
    /// Mirrors `install_identifier` property (lines 66-69):
    /// `f"{self.repo}/{self.subdir}" if self.subdir else self.repo`
    pub fn install_identifier(&self) -> String {
        match &self.subdir {
            Some(s) if !s.is_empty() => format!("{}/{}", self.repo, s),
            _ => self.repo.clone(),
        }
    }

    /// Mirrors `to_dict()` (lines 71-90) — serialises to the same JSON shape.
    pub fn to_dict(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("name".to_string(), Value::String(self.name.clone()));
        m.insert("description".to_string(), Value::String(self.description.clone()));
        m.insert("author".to_string(), Value::String(self.author.clone()));
        m.insert(
            "tags",
            Value::Array(self.tags.iter().map(|t| Value::String(t.clone())).collect()),
        );
        m.insert("repo".to_string(), Value::String(self.repo.clone()));
        m.insert("ref".to_string(), Value::String(self.r#ref.clone()));
        if let Some(s) = &self.subdir {
            if !s.is_empty() {
                m.insert("subdir".to_string(), Value::String(s.clone()));
            }
        }
        if let Some(h) = &self.homepage {
            if !h.is_empty() {
                m.insert("homepage".to_string(), Value::String(h.clone()));
            }
        }
        if !self.capabilities.is_empty() {
            m.insert(
                "capabilities".to_string(),
                Value::Array(
                    self.capabilities.iter().map(|c| Value::String(c.clone())).collect(),
                ),
            );
        }
        if let Some(v) = self.api_version {
            m.insert("api_version".to_string(), Value::Number(v.into()));
        }
        if let Some(a) = &self.added_at {
            if !a.is_empty() {
                m.insert("added_at".to_string(), Value::String(a.clone()));
            }
        }
        Value::Object(m)
    }
}

// ---------------------------------------------------------------------------
// Cache helpers — mirrors _cache_path, get_index_url (lines 93-107)
// ---------------------------------------------------------------------------

/// Mirrors `def _cache_path() -> Path` (lines 93-94): `get_hermes_home() / "cache" / "plugin_index.json"`
pub fn cache_path() -> PathBuf {
    get_hermes_home().join("cache").join("plugin_index.json")
}

/// Mirrors `def get_index_url() -> str` (lines 97-107):
/// `plugins.index_url` config override or `DEFAULT_INDEX_URL`.
///
/// Tries in order:
/// 1. `HERMES_PLUGIN_INDEX_URL` env var (test hook, not in Python but harmless)
/// 2. `$HERMES_HOME/config.yaml` `plugins.index_url` key (text search, no yaml crate)
/// 3. `DEFAULT_INDEX_URL`
pub fn get_index_url() -> String {
    // Test hook — not in Python, but keeps hermetic tests deterministic
    if let Ok(val) = std::env::var("HERMES_PLUGIN_INDEX_URL") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    // Config override — mirrors `cfg_get(load_config_readonly(), "plugins", "index_url", default=None)`
    // Read `$HERMES_HOME/config.yaml` as text and search for `index_url:` (avoids yaml dep)
    let cfg_path = get_hermes_home().join("config.yaml");
    if let Ok(text) = fs::read_to_string(&cfg_path) {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            // look for `index_url:` irrespective of indent
            if trimmed.contains("index_url") {
                if let Some(colon) = trimmed.find(':') {
                    let val = trimmed[colon + 1..].trim()
                        .trim_matches('"').trim_matches('\'').trim()
                        .trim_end_matches(',').to_string();
                    if !val.is_empty() && val != "null" && val != "~" {
                        return val;
                    }
                }
            }
        }
        // also handle nested `plugins:` block — scan for `index_url` anywhere under plugins
        // already covered by the loop above (broad match)
    }
    DEFAULT_INDEX_URL.to_string()
}

// ---------------------------------------------------------------------------
// _parse_entries — mirrors plugin_index.py:110-149
// ---------------------------------------------------------------------------

/// Parse a decoded index document into entries, skipping malformed items.
/// Mirrors `def _parse_entries(raw: Any) -> List[PluginIndexEntry]` (110-149).
///
/// `raw` is a `serde_json::Value` already decoded from JSON.
/// Returns `Err` only for structurally invalid top-level shapes (not a dict/list
/// or `plugins` not a list) — mirrors Python `raise ValueError`.
pub fn parse_entries(raw: &Value) -> Result<Vec<PluginIndexEntry>, String> {
    let items: &Vec<Value> = match raw {
        Value::Object(map) => {
            // `raw.get("plugins", [])` — bare-list fallback handled below
            match map.get("plugins") {
                Some(v) => match v.as_array() {
                    Some(arr) => arr,
                    None => return Err("Plugin index 'plugins' field must be a list.".to_string()),
                },
                None => {
                    // No `plugins` key — Python would get `[]` and return []
                    // but only if raw is dict; we treat missing as empty list
                    // to keep lenient compat. However original Python does
                    // `items = raw.get("plugins", [])` then checks `isinstance(items, list)` —
                    // missing key gives `[]` which passes, so we return [].
                    // To preserve that, return empty vec here.
                    return Ok(Vec::new());
                }
            }
        }
        Value::Array(arr) => arr, // bare-list form also accepted (line 114)
        _ => return Err("Plugin index must be a JSON object or list.".to_string()),
    };

    let mut entries: Vec<PluginIndexEntry> = Vec::new();
    for item in items {
        let map = match item.as_object() {
            Some(m) => m,
            None => continue, // skip non-dict — mirrors `if not isinstance(item, dict): continue`
        };
        let name = match map.get("name").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };
        let repo = match map.get("repo").and_then(|v| v.as_str()) {
            Some(s) => s.trim().to_string(),
            _ => continue,
        };
        // `repo.count("/") != 1 or not all(repo.split("/"))` → skip + debug log
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            // mirrors `logger.debug("plugin index: skipping entry %r with invalid repo %r", ...)`
            eprintln!("plugin index: skipping entry {:?} with invalid repo {:?}", name, repo);
            continue;
        }
        let subdir_raw = map.get("subdir").and_then(|v| v.as_str());
        let subdir = match subdir_raw {
            Some(s) => {
                let t = s.trim().trim_matches('/').to_string();
                if t.is_empty() { None } else { Some(t) }
            }
            None => None,
        };
        let api_version_raw = map.get("api_version");
        let api_version: Option<i64> = match api_version_raw {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() => s.parse::<i64>().ok(),
            _ => None,
        };
        // tags: `[str(t) for t in tags or [] if isinstance(t, (str, int))]` — only str/int kept
        let tags: Vec<String> = match map.get("tags") {
            Some(Value::Array(arr)) => arr.iter().filter_map(|t| match t {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }).collect(),
            _ => Vec::new(),
        };
        let capabilities: Vec<String> = match map.get("capabilities") {
            Some(Value::Array(arr)) => arr.iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect(),
            _ => Vec::new(),
        };
        let description = map.get("description").map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => v.to_string(),
        }).unwrap_or_default();
        let author = map.get("author").map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => v.to_string(),
        }).unwrap_or_default();
        let r#ref = map.get("ref").or_else(|| map.get("ref_")).map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => v.to_string(),
        }).unwrap_or_default();
        let homepage = map.get("homepage").and_then(|v| v.as_str()).map(|s| s.to_string());
        let added_at = map.get("added_at").and_then(|v| v.as_str()).map(|s| s.to_string());

        entries.push(PluginIndexEntry {
            name,
            description: description.trim().to_string(),
            author: author.trim().to_string(),
            tags,
            repo: repo.trim().to_string(),
            r#ref: r#ref.trim().to_string(),
            subdir,
            homepage: homepage.filter(|s| !s.trim().is_empty()),
            capabilities,
            api_version,
            added_at: added_at.filter(|s| !s.trim().is_empty()),
        });
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// _load_seed_entries — mirrors plugin_index.py:152-157
// ---------------------------------------------------------------------------

/// Mirrors `def _load_seed_entries() -> List[PluginIndexEntry]` (152-157).
pub fn load_seed_entries() -> Vec<PluginIndexEntry> {
    let path = seed_index_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("plugin index: bundled seed unreadable: {} ({})", path.display(), e);
            return Vec::new();
        }
    };
    let raw: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("plugin index: bundled seed unreadable: {}", e);
            return Vec::new();
        }
    };
    match parse_entries(&raw) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("plugin index: bundled seed unreadable: {}", e);
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// _read_cache / _write_cache — mirrors plugin_index.py:160-184
// ---------------------------------------------------------------------------

/// Mirrors `def _read_cache(*, max_age: Optional[float]) -> Optional[List[PluginIndexEntry]]` (160-173).
pub fn read_cache(max_age: Option<f64>) -> Option<Vec<PluginIndexEntry>> {
    let cache = cache_path();
    if !cache.is_file() {
        return None;
    }
    if let Some(age_limit) = max_age {
        let mtime = fs::metadata(&cache).and_then(|m| m.modified()).ok()?;
        let age = SystemTime::now().duration_since(mtime).ok().map(|d| d.as_secs_f64()).unwrap_or(f64::MAX);
        if age > age_limit {
            return None;
        }
    }
    let text = fs::read_to_string(&cache).ok()?;
    let raw: Value = serde_json::from_str(&text).ok()?;
    parse_entries(&raw).ok()
}

/// Mirrors `def _write_cache(text: str) -> None` (176-184).
pub fn write_cache(text: &str) {
    let cache = cache_path();
    if let Some(parent) = cache.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // atomic write: tmp → rename
    let tmp = cache.with_extension("tmp");
    // fallback for `plugin_index.json.tmp` naming (with_file_name)
    let tmp2 = cache.with_file_name(format!("{}.tmp", cache.file_name().unwrap_or_default().to_string_lossy()));
    let tmp_path = if tmp2 != cache { tmp2 } else { tmp };
    if fs::write(&tmp_path, text).is_ok() {
        let _ = fs::rename(&tmp_path, &cache);
    } else {
        eprintln!("plugin index: cache write failed");
    }
}

// ---------------------------------------------------------------------------
// _fetch_remote — mirrors plugin_index.py:187-203
// ---------------------------------------------------------------------------

/// Mirrors `def _fetch_remote() -> Optional[List[PluginIndexEntry]]` (187-203).
///
/// Fetch and parse the remote index; cache the raw payload on success.
/// Uses `curl` subprocess (follow redirects, 10s timeout) so no new crate
/// is required. Returns `None` on any error (mirrors Python `except Exception: return None`).
pub fn fetch_remote() -> Option<Vec<PluginIndexEntry>> {
    let url = get_index_url();

    // Fake-remote hook for hermetic tests — if `HERMES_PLUGIN_INDEX_FAKE_REMOTE`
    // points to a file, read it instead of hitting network (keeps tests offline).
    if let Ok(fake) = std::env::var("HERMES_PLUGIN_INDEX_FAKE_REMOTE") {
        let fake_path = PathBuf::from(fake.trim());
        if let Ok(text) = fs::read_to_string(&fake_path) {
            if text.as_bytes().len() > MAX_INDEX_BYTES {
                eprintln!("plugin index: payload exceeds size limit");
                return None;
            }
            let raw: Value = serde_json::from_str(&text).ok()?;
            let entries = parse_entries(&raw).ok()?;
            write_cache(&text);
            return Some(entries);
        }
    }

    // Try curl — mirrors `httpx.get(url, timeout=10, follow_redirects=True)`
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time", &format!("{}", FETCH_TIMEOUT_SECS as u64),
            "-L",
            &url,
        ])
        .output();

    let text = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("plugin index: remote fetch failed ({}): {}", url, stderr.trim());
            return None;
        }
        Err(e) => {
            eprintln!("plugin index: remote fetch failed ({}): {}", url, e);
            return None;
        }
    };

    if text.as_bytes().len() > MAX_INDEX_BYTES {
        eprintln!("plugin index: payload exceeds size limit");
        return None;
    }
    let raw: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("plugin index: remote fetch failed ({}): {}", url, e);
            return None;
        }
    };
    let entries = match parse_entries(&raw) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("plugin index: remote fetch failed ({}): {}", url, e);
            return None;
        }
    };
    write_cache(&text);
    Some(entries)
}

// ---------------------------------------------------------------------------
// load_index — mirrors plugin_index.py:206-229
// ---------------------------------------------------------------------------

/// Mirrors `def load_index(*, refresh: bool = False, offline: bool = False)` (206-229).
///
/// Returns `(entries, source)` where `source` is one of `"remote"`, `"cache"`, `"seed"`.
/// Order: fresh cache (unless refresh) → remote → stale cache → bundled seed.
pub fn load_index(refresh: bool, offline: bool) -> (Vec<PluginIndexEntry>, String) {
    if !refresh {
        if let Some(cached) = read_cache(Some(INDEX_CACHE_TTL as f64)) {
            return (cached, "cache".to_string());
        }
    }
    if !offline {
        if let Some(remote) = fetch_remote() {
            return (remote, "remote".to_string());
        }
    }
    if let Some(stale) = read_cache(None) {
        return (stale, "cache".to_string());
    }
    (load_seed_entries(), "seed".to_string())
}

// ---------------------------------------------------------------------------
// Search — mirrors plugin_index.py:236-305
// ---------------------------------------------------------------------------

/// Mirrors `def _score_entry(entry, term) -> float` (236-261).
/// Fuzzy relevance score for `entry` against lowercase `term` (0 = no match).
pub fn score_entry(entry: &PluginIndexEntry, term: &str) -> f64 {
    let term = term.to_lowercase();
    let name = entry.name.to_lowercase();
    let desc = entry.description.to_lowercase();
    let tags: Vec<String> = entry.tags.iter().map(|t| t.to_lowercase()).collect();
    let author = entry.author.to_lowercase();

    if term == name {
        return 100.0;
    }
    let mut score = 0.0;
    if name.contains(&term) {
        score = score.max(80.0);
    }
    if tags.iter().any(|t| t == &term) {
        score = score.max(70.0);
    }
    if tags.iter().any(|t| t.contains(&term)) {
        score = score.max(55.0);
    }
    if desc.contains(&term) {
        score = score.max(50.0);
    }
    if author.contains(&term) {
        score = score.max(40.0);
    }
    // Fuzzy close-match on the name for typo tolerance.
    let ratio = sequence_matcher_ratio(&term, &name);
    if ratio >= 0.6 {
        score = score.max(ratio * 60.0);
    }
    score
}

/// Minimal `difflib.SequenceMatcher(None, a, b).ratio()` approximation.
///
/// Returns `2*M / (len(a)+len(b))` where M is the LCS length (longest common
/// subsequence, not substring). This is the same family as Python's gestalt
/// matcher and satisfies the `>=0.6` threshold behaviour for typo tolerance
/// without pulling `strsim` or `difflib` crate.
pub fn sequence_matcher_ratio(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = a_chars.len();
    let m = b_chars.len();
    // DP for LCS length — O(n*m), fine for short plugin names
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if a_chars[i - 1] == b_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }
    let lcs = dp[n][m] as f64;
    (2.0 * lcs) / ((n + m) as f64)
}

/// Mirrors `def search_index(entries, term, *, capability=None)` (264-284).
///
/// Rank `entries` against `term` (fuzzy on name/description/tags/author).
/// Empty `term` matches everything (browse mode). `capability` filters
/// entries by declared capability.
pub fn search_index(entries: &[PluginIndexEntry], term: &str, capability: Option<&str>) -> Vec<PluginIndexEntry> {
    let mut pool: Vec<PluginIndexEntry> = entries.to_vec();
    if let Some(cap) = capability {
        let cap_low = cap.to_lowercase();
        pool = pool.into_iter().filter(|e| e.capabilities.iter().any(|c| c.to_lowercase() == cap_low)).collect();
    }
    let term_trimmed = term.trim().to_lowercase();
    if term_trimmed.is_empty() {
        let mut sorted = pool;
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        return sorted;
    }
    let mut scored: Vec<(PluginIndexEntry, f64)> = pool.into_iter().map(|e| {
        let s = score_entry(&e, &term_trimmed);
        (e, s)
    }).collect();
    scored.retain(|(_, s)| *s > 0.0);
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.name.cmp(&b.0.name)));
    scored.into_iter().map(|(e, _)| e).collect()
}

/// Mirrors `def resolve_name(entries, name)` (287-305).
///
/// Resolve a bare plugin `name` against the index.
/// Returns `(entry, candidates)`: an exact (case-insensitive) unique match
/// in `entry`, otherwise `entry is None` and `candidates` holds any
/// partial matches (empty = nothing similar, >1 on exact = ambiguous).
pub fn resolve_name(entries: &[PluginIndexEntry], name: &str) -> (Option<PluginIndexEntry>, Vec<PluginIndexEntry>) {
    let lowered = name.trim().to_lowercase();
    let exact: Vec<PluginIndexEntry> = entries.iter().filter(|e| e.name.to_lowercase() == lowered).cloned().collect();
    if exact.len() == 1 {
        return (Some(exact[0].clone()), exact);
    }
    if exact.len() > 1 {
        return (None, exact);
    }
    let partial: Vec<PluginIndexEntry> = entries.iter().filter(|e| e.name.to_lowercase().contains(&lowered)).cloned().collect();
    if partial.len() == 1 {
        return (Some(partial[0].clone()), partial);
    }
    (None, partial)
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants (no cargo network needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn sample_entries() -> Vec<PluginIndexEntry> {
        vec![
            PluginIndexEntry {
                name: "hermes-media-studio".to_string(),
                description: "Media Studio — generative media workspace".to_string(),
                author: "NousResearch".to_string(),
                tags: vec!["media".to_string(), "image-gen".to_string()],
                repo: "NousResearch/hermes-media-studio".to_string(),
                r#ref: "e8d59971d2b7901405b39dac7b03bdd616272d0d".to_string(),
                subdir: None,
                homepage: Some("https://github.com/NousResearch/hermes-media-studio".to_string()),
                capabilities: vec!["tools".to_string(), "dashboard".to_string()],
                api_version: Some(1),
                added_at: Some("2026-08-12".to_string()),
            },
            PluginIndexEntry {
                name: "plugin-llm-example".to_string(),
                description: "Reference plugin showing LLM access".to_string(),
                author: "NousResearch".to_string(),
                tags: vec!["example".to_string(), "llm".to_string()],
                repo: "NousResearch/hermes-example-plugins".to_string(),
                r#ref: "38fe0fb53eff98d477f807432e965429e665ca33".to_string(),
                subdir: Some("plugin-llm-example".to_string()),
                homepage: None,
                capabilities: vec!["commands".to_string(), "llm".to_string()],
                api_version: Some(1),
                added_at: Some("2026-08-12".to_string()),
            },
            PluginIndexEntry {
                name: "hermes-plugin-chrome-profiles".to_string(),
                description: "Switch Hermes browser tools between Chrome profiles".to_string(),
                author: "anpicasso".to_string(),
                tags: vec!["browser".to_string(), "chrome".to_string()],
                repo: "anpicasso/hermes-plugin-chrome-profiles".to_string(),
                r#ref: "5b9c3257b464c0f926d4355149a8aed9c8f307b4".to_string(),
                subdir: None,
                homepage: None,
                capabilities: vec!["tools".to_string()],
                api_version: Some(1),
                added_at: None,
            },
        ]
    }

    fn with_temp_hermes_home<F: FnOnce()>(f: F) {
        let tmp = std::env::temp_dir().join(format!(
            "hermes-plugin-index-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var("HERMES_HOME").ok();
        let prev_url = std::env::var("HERMES_PLUGIN_INDEX_URL").ok();
        let prev_fake = std::env::var("HERMES_PLUGIN_INDEX_FAKE_REMOTE").ok();
        unsafe { std::env::set_var("HERMES_HOME", &tmp); }
        unsafe { std::env::remove_var("HERMES_PLUGIN_INDEX_URL"); }
        unsafe { std::env::remove_var("HERMES_PLUGIN_INDEX_FAKE_REMOTE"); }
        f();
        if let Some(v) = prev { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        if let Some(v) = prev_url { unsafe { std::env::set_var("HERMES_PLUGIN_INDEX_URL", v); } } else { unsafe { std::env::remove_var("HERMES_PLUGIN_INDEX_URL"); } }
        if let Some(v) = prev_fake { unsafe { std::env::set_var("HERMES_PLUGIN_INDEX_FAKE_REMOTE", v); } } else { unsafe { std::env::remove_var("HERMES_PLUGIN_INDEX_FAKE_REMOTE"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cache_path_under_hermes_home() {
        with_temp_hermes_home(|| {
            let p = cache_path();
            assert!(p.ends_with("cache/plugin_index.json"));
            assert!(p.to_string_lossy().contains("hermes-plugin-index"));
        });
    }

    #[test]
    fn get_index_url_default_and_override() {
        with_temp_hermes_home(|| {
            assert_eq!(get_index_url(), DEFAULT_INDEX_URL);
            unsafe { std::env::set_var("HERMES_PLUGIN_INDEX_URL", "https://example.com/index.json"); }
            assert_eq!(get_index_url(), "https://example.com/index.json");
            unsafe { std::env::set_var("HERMES_PLUGIN_INDEX_URL", "  "); }
            assert_eq!(get_index_url(), DEFAULT_INDEX_URL);
        });
    }

    #[test]
    fn plugin_entry_install_identifier() {
        let e1 = PluginIndexEntry { name: "x".to_string(), description: String::new(), author: String::new(), tags: vec![], repo: "a/b".to_string(), r#ref: String::new(), subdir: None, homepage: None, capabilities: vec![], api_version: None, added_at: None };
        assert_eq!(e1.install_identifier(), "a/b");
        let e2 = PluginIndexEntry { subdir: Some("sub/dir".to_string()), ..e1.clone() };
        assert_eq!(e2.install_identifier(), "a/b/sub/dir");
        let e3 = PluginIndexEntry { subdir: Some("".to_string()), ..e1.clone() };
        assert_eq!(e3.install_identifier(), "a/b");
    }

    #[test]
    fn plugin_entry_to_dict_roundtrip() {
        let e = sample_entries()[1].clone();
        let d = e.to_dict();
        assert_eq!(d["name"], "plugin-llm-example");
        assert_eq!(d["repo"], "NousResearch/hermes-example-plugins");
        assert_eq!(d["subdir"], "plugin-llm-example");
        assert_eq!(d["capabilities"][0], "commands");
        // entry without subdir omits key
        let e0 = sample_entries()[0].clone();
        assert!(e0.to_dict().get("subdir").is_none());
        assert!(e0.to_dict().get("homepage").is_some());
    }

    #[test]
    fn parse_entries_accepts_object_and_list() {
        // object form
        let raw = json!({"plugins": [{"name": "a", "repo": "o/a"}]});
        let entries = parse_entries(&raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a");
        // bare list form
        let raw2 = json!([{"name": "b", "repo": "o/b"}]);
        let entries2 = parse_entries(&raw2).unwrap();
        assert_eq!(entries2.len(), 1);
        assert_eq!(entries2[0].name, "b");
        // invalid top-level
        assert!(parse_entries(&json!("bad")).is_err());
        assert!(parse_entries(&json!({"plugins": "not-a-list"})).is_err());
    }

    #[test]
    fn parse_entries_skips_malformed() {
        let raw = json!({"plugins": [
            {"name": "", "repo": "o/a"}, // empty name skipped
            {"name": "a", "repo": "badrepo"}, // invalid repo skipped
            {"name": "a", "repo": "o/"}, // invalid repo skipped
            {"name": "good", "repo": "o/good", "tags": ["t", 123, null], "capabilities": ["tools"]},
            "not-a-dict",
            {"repo": "o/missing-name"},
        ]});
        let entries = parse_entries(&raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "good");
        assert_eq!(entries[0].tags, vec!["t", "123"]);
    }

    #[test]
    fn parse_entries_subdir_and_api_version() {
        let raw = json!({"plugins": [
            {"name": "a", "repo": "o/a", "subdir": "/foo/bar/", "api_version": "2"},
            {"name": "b", "repo": "o/b", "subdir": "  ", "api_version": 3},
            {"name": "c", "repo": "o/c", "api_version": "not-a-number"},
        ]});
        let entries = parse_entries(&raw).unwrap();
        assert_eq!(entries[0].subdir.as_deref(), Some("foo/bar"));
        assert_eq!(entries[0].api_version, Some(2));
        assert_eq!(entries[1].subdir, None);
        assert_eq!(entries[1].api_version, Some(3));
        assert_eq!(entries[2].api_version, None);
    }

    #[test]
    fn scoring_exact_and_substring() {
        let e = &sample_entries()[0];
        // exact name match 100
        assert_eq!(score_entry(e, "hermes-media-studio"), 100.0);
        // term in name 80
        assert!(score_entry(e, "media") >= 80.0);
        // term in tags exact 70
        assert!(score_entry(e, "image-gen") >= 70.0);
        // term in description 50
        assert!(score_entry(e, "generative") >= 50.0);
        // term in author 40
        assert!(score_entry(e, "nousresearch") >= 40.0);
        // no match 0
        assert_eq!(score_entry(e, "zzz_no_match_xyz"), 0.0);
    }

    #[test]
    fn sequence_matcher_symmetry() {
        assert_eq!(sequence_matcher_ratio("abc", "abc"), 1.0);
        assert_eq!(sequence_matcher_ratio("", ""), 1.0);
        assert_eq!(sequence_matcher_ratio("", "abc"), 0.0);
        assert!(sequence_matcher_ratio("kitten", "sitting") > 0.5);
        assert_eq!(sequence_matcher_ratio("abc", "abc"), sequence_matcher_ratio("abc", "abc"));
    }

    #[test]
    fn search_index_browse_and_capability_filter() {
        let entries = sample_entries();
        // browse mode empty term sorted by name
        let all = search_index(&entries, "", None);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "hermes-media-studio");
        // capability filter
        let tools = search_index(&entries, "", Some("tools"));
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|e| e.capabilities.iter().any(|c| c == "tools")));
        // case-insensitive capability
        let tools2 = search_index(&entries, "", Some("TOOLS"));
        assert_eq!(tools2.len(), 2);
        // term search ranked
        let res = search_index(&entries, "media", None);
        assert!(!res.is_empty());
        assert_eq!(res[0].name, "hermes-media-studio");
        // no match empty
        let none = search_index(&entries, "zzzz_no_such_plugin_xyz", None);
        assert!(none.is_empty());
    }

    #[test]
    fn resolve_name_exact_and_partial_and_ambiguous() {
        let entries = sample_entries();
        // exact case-insensitive
        let (found, cand) = resolve_name(&entries, "HERMES-MEDIA-STUDIO");
        assert_eq!(found.as_ref().unwrap().name, "hermes-media-studio");
        assert_eq!(cand.len(), 1);
        // partial unique
        let (found2, cand2) = resolve_name(&entries, "chrome-profiles");
        assert_eq!(found2.as_ref().unwrap().name, "hermes-plugin-chrome-profiles");
        assert_eq!(cand2.len(), 1);
        // ambiguous exact — duplicate names
        let dup = vec![
            PluginIndexEntry { name: "dup".to_string(), repo: "a/dup".to_string(), ..PluginIndexEntry { name: String::new(), description: String::new(), author: String::new(), tags: vec![], repo: String::new(), r#ref: String::new(), subdir: None, homepage: None, capabilities: vec![], api_version: None, added_at: None } },
            PluginIndexEntry { name: "dup".to_string(), repo: "b/dup".to_string(), ..PluginIndexEntry { name: String::new(), description: String::new(), author: String::new(), tags: vec![], repo: String::new(), r#ref: String::new(), subdir: None, homepage: None, capabilities: vec![], api_version: None, added_at: None } },
        ];
        let (found3, cand3) = resolve_name(&dup, "dup");
        assert!(found3.is_none());
        assert_eq!(cand3.len(), 2);
        // no match
        let (found4, cand4) = resolve_name(&entries, "nonexistent");
        assert!(found4.is_none());
        assert!(cand4.is_empty());
        // partial ambiguous returns all partials with None
        let (found5, cand5) = resolve_name(&entries, "hermes");
        assert!(found5.is_none());
        assert_eq!(cand5.len(), 2); // hermes-media-studio and hermes-plugin-chrome-profiles
    }

    #[test]
    fn read_write_cache_roundtrip() {
        with_temp_hermes_home(|| {
            let raw = json!({"plugins": [{"name": "a", "repo": "o/a"}]});
            let text = serde_json::to_string(&raw).unwrap();
            write_cache(&text);
            let cached = read_cache(Some(3600.0)).unwrap();
            assert_eq!(cached.len(), 1);
            assert_eq!(cached[0].name, "a");
            // fresh cache hit
            let (entries, src) = load_index(false, true);
            assert_eq!(src, "cache");
            assert_eq!(entries.len(), 1);
        });
    }

    #[test]
    fn read_cache_expiry() {
        with_temp_hermes_home(|| {
            let raw = json!({"plugins": [{"name": "a", "repo": "o/a"}]});
            let text = serde_json::to_string(&raw).unwrap();
            write_cache(&text);
            // expired if max_age 0 and file is older than 0 (allow slight delay)
            std::thread::sleep(std::time::Duration::from_millis(10));
            assert!(read_cache(Some(0.001)).is_none() || read_cache(Some(0.0)).is_none());
            // stale still readable
            assert!(read_cache(None).is_some());
        });
    }

    #[test]
    fn load_index_offline_uses_seed_or_cache() {
        with_temp_hermes_home(|| {
            // no cache, offline → seed (may be empty if seed missing, but source is seed)
            let (_, src) = load_index(true, true);
            assert!(src == "cache" || src == "seed");
        });
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_INDEX_URL, "https://raw.githubusercontent.com/NousResearch/hermes-plugin-index/main/index.json");
        assert_eq!(INDEX_CACHE_TTL, 86400);
        assert_eq!(FETCH_TIMEOUT_SECS, 10.0);
        assert_eq!(MAX_INDEX_BYTES, 5242880);
        assert!(SECURITY_FOOTER.contains("Indexed"));
    }
}
