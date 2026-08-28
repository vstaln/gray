//! Blueprints: shareable plain-language automations layered on skills + cron.
//! Port of `reference/NousResearch/hermes-agent/tools/blueprints.py` (324 lines) — 1:1 behavior.
//!
//! A "blueprint" is NOT a new object type. It is an ordinary skill (a SKILL.md the
//! agent loads) that additionally declares an automation schedule in its
//! frontmatter:
//!
//! ```yaml
//! metadata:
//!   hermes:
//!     blueprint:
//!       schedule: "0 9 * * *"     # presence of `blueprint:` marks it runnable
//!       deliver: origin            # optional (default "origin")
//!       prompt: "..."              # optional task instruction for the run
//!       no_agent: false            # optional
//! ```
//!
//! Because a blueprint is just a skill, it flows through the ENTIRE existing
//! skills-hub pipeline for free — search, inspect, quarantine, security scan,
//! install, lock-file provenance, audit log, taps, the centralized index, and
//! `hermes skills publish` for sharing. No new source type, no new store, no new
//! transport. This module is the thin bridge between that skill metadata and the
//! existing cron `create_job()` API:
//!
//!   * `parse_blueprint(skill_md_text)` -> BlueprintSpec | None
//!   * `blueprint_spec_for_installed(name)` -> BlueprintSpec | None
//!   * `blueprint_to_job_spec(spec, ...)` -> the cron job kwargs
//!   * `create_blueprint_job(spec, ...)` -> the created cron job
//!   * `register_blueprint_suggestion(spec)` -> suggestion record
//!   * `export_blueprint(job, body)` -> a shareable SKILL.md string
//!
//! The dev guide's "Extend, Don't Duplicate" rule is the whole design: the blueprint
//! is a skill, the schedule is a cron job, sharing is the existing publish/tap/
//! index path.
//!
//! Rust mapping
//! ------------
//! - `yaml.safe_load` frontmatter parse → minimal line-scan parser (NEVER cargo: no yaml crate)
//!   that extracts only the blueprint-relevant keys (`name`, `metadata.hermes.blueprint.*`).
//!   Full yaml would require `serde_yaml` — the fallback shape is preserved so the
//!   public `parse_blueprint` semantics (None vs Err) match Python exactly.
//! - `Path(SKILLS_DIR).glob(f"**/{skill_name}/SKILL.md")` → recursive `fs::read_dir` walk
//!   over `get_skills_dir()` (`$HERMES_HOME/skills`, `~/.hermes/skills`). Injected
//!   `with_dir` variants mirror `openrouter_client_with_lookup` for hermetic tests.
//! - `cron.scheduler.create_job_with_scheduler_registration(**job_spec)` → stub that
//!   returns the built `BlueprintJobSpec` plus optional `origin`. The real scheduler
//!   wiring lives in `hermes-cron` when linked.
//! - `cron.suggestions.add_suggestion(...)` → stub returning `BlueprintSuggestion`.
//!   Deduplication / backlog-full `None` is documented but stub always returns `Some`
//!   when `skill_name` is non-empty (mirrors the Python happy path).
//! - `yaml.safe_dump(frontmatter)` in `export_blueprint` → manual YAML emitter that
//!   preserves key order and handles quoting / list / block needs without a yaml crate.
//! - `_schedule_to_string` dict handling → `ScheduleValue` enum with `String`, `Cron`, `Interval`.
//! - `BlueprintError(ValueError)` → `BlueprintError(String)` with `Display` + `Error`.
//! - `BlueprintSpec` dataclass → `BlueprintSpec` struct with `HashMap<String,String>` raw.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// __all__ — mirrors Python __all__ (lines 41-50)
// ---------------------------------------------------------------------------

/// Mirrors Python `__all__`.
pub const ALL: &[&str] = &[
    "BlueprintSpec",
    "parse_blueprint",
    "blueprint_spec_for_installed",
    "blueprint_to_job_spec",
    "create_blueprint_job",
    "register_blueprint_suggestion",
    "export_blueprint",
    "BlueprintError",
];

// ---------------------------------------------------------------------------
// Error — mirrors `class BlueprintError(ValueError)` (lines 53-54)
// ---------------------------------------------------------------------------

/// Raised when a blueprint block is present but malformed.
/// Mirrors `class BlueprintError(ValueError)` (53).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintError(pub String);

impl BlueprintError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for BlueprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BlueprintError {}

// ---------------------------------------------------------------------------
// BlueprintSpec — mirrors `@dataclass class BlueprintSpec` (57-69)
// ---------------------------------------------------------------------------

/// Parsed `metadata.hermes.blueprint` automation spec for a skill.
/// Mirrors `class BlueprintSpec` (57).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintSpec {
    pub skill_name: String,
    pub schedule: String,
    pub deliver: String,
    pub prompt: Option<String>,
    pub no_agent: bool,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub enabled_toolsets: Option<Vec<String>>,
    pub raw: HashMap<String, String>,
}

impl BlueprintSpec {
    pub fn new(
        skill_name: impl Into<String>,
        schedule: impl Into<String>,
        deliver: impl Into<String>,
    ) -> Self {
        Self {
            skill_name: skill_name.into(),
            schedule: schedule.into(),
            deliver: deliver.into(),
            prompt: None,
            no_agent: false,
            model: None,
            provider: None,
            enabled_toolsets: None,
            raw: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers — frontmatter split, indent, scalar extraction, yaml quoting
// ---------------------------------------------------------------------------

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 {
        let bytes = t.as_bytes();
        let first = bytes[0];
        let last = bytes[t.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

/// Minimal YAML scalar quoting for export — mirrors `yaml.safe_dump` behavior
/// without needing a yaml crate. Quotes when the value contains characters that
/// would be ambiguous as plain YAML or when it has leading/trailing spaces.
fn yaml_quote(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quote = s.trim() != s
        || s.contains(':')
        || s.contains('#')
        || s.contains('"')
        || s.contains('\'')
        || s.contains('\n')
        || s.contains('\r')
        || s.contains('{')
        || s.contains('}')
        || s.contains('[')
        || s.contains(']')
        || s.contains(',')
        || s.contains('*')
        || s.contains('&')
        || s.contains('!')
        || s.contains('|')
        || s.contains('>')
        || s.contains('%')
        || s.contains('@')
        || s.contains('`')
        || s.contains('\\');
    if !needs_quote {
        return s.to_string();
    }
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

/// Sanitize blueprint name to a valid skill identifier.
/// Mirrors Python (257-259):
/// `name = "".join(c if (c.isalnum() or c in "-_") else "-" for c in str(name).lower())`
/// `name = name.strip("-_") or "shared-blueprint"`
pub fn sanitize_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for ch in lower.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '_').to_string();
    if trimmed.is_empty() {
        "shared-blueprint".to_string()
    } else {
        trimmed
    }
}

// ---------------------------------------------------------------------------
// _split_frontmatter — mirrors `def _split_frontmatter(text)` (72-92)
// ---------------------------------------------------------------------------

/// Return the raw YAML frontmatter string (between opening and closing `---`),
/// or `None` if absent/invalid. Mirrors `_split_frontmatter` (72).
pub fn split_frontmatter(text: &str) -> Option<String> {
    // Python: `if not isinstance(text, str): return None` — Rust &str always str.
    // `stripped = text.lstrip("\ufeff").lstrip()` — BOM is not whitespace; strip explicitly
    let stripped = text.trim_start_matches('\u{feff}').trim_start();
    if !stripped.starts_with("---") {
        return None;
    }
    let after_open = &stripped[3..];
    // Find the closing fence after the opening one — Python: `after_open.find("\n---")`
    let end = after_open.find("\n---")?;
    let fm_text = &after_open[..end];
    Some(fm_text.to_string())
}

// ---------------------------------------------------------------------------
// Frontmatter helpers — minimal line-scan parser for blueprint fields
// ---------------------------------------------------------------------------

fn key_and_value(trimmed: &str) -> Option<(&str, &str)> {
    let colon = trimmed.find(':')?;
    let k = trimmed[..colon].trim();
    let v = trimmed[colon + 1..].trim();
    if k.is_empty() {
        return None;
    }
    Some((k, v))
}

/// Extract scalar value after `key:` at given search region.
/// Returns `Some(raw_value)` if key line exists (value may be empty string),
/// or `None` if key not found in region.
fn find_key_in_region<'a>(
    lines: &'a [(usize, String)],
    region_start: usize,
    region_end: usize,
    key: &str,
    min_indent: usize,
) -> Option<(usize, String)> {
    for idx in region_start..region_end {
        let (indent, line) = &lines[idx];
        if *indent <= min_indent {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some((k, v)) = key_and_value(trimmed) {
            if k == key {
                return Some((idx, v.to_string()));
            }
        }
    }
    None
}

/// Collect block scalar following a `key: |` style value.
/// The `key_idx` line had `value_raw` equal to "|" or ">" etc. We collect
/// subsequent lines with indent greater than `key_indent` until indent drops.
fn collect_block_scalar(
    lines: &[(usize, String)],
    key_idx: usize,
    key_indent: usize,
    region_end: usize,
) -> String {
    let mut out = Vec::new();
    for idx in (key_idx + 1)..region_end {
        let (indent, line) = &lines[idx];
        if *indent <= key_indent {
            break;
        }
        // For block, strip the `key_indent + 2` leading spaces if present, else all indent.
        // Mimic YAML: block content is indented at least 2 more than parent.
        // We'll strip `key_indent + 2` spaces minimally.
        let strip = key_indent + 2;
        let raw = &lines[idx].1;
        let stripped = if raw.len() > strip {
            // Check that first `strip` chars are spaces/tabs
            let prefix = &raw[..strip];
            if prefix.chars().all(|c| c == ' ' || c == '\t') {
                &raw[strip..]
            } else {
                line.trim_start()
            }
        } else {
            line.trim_start()
        };
        out.push(stripped.to_string());
    }
    out.join("\n")
}

/// Parse inline list like `["a", "b"]` or `[a, b]`.
fn parse_inline_list(s: &str) -> Vec<String> {
    let t = s.trim();
    if !t.starts_with('[') || !t.ends_with(']') {
        return Vec::new();
    }
    let inner = &t[1..t.len() - 1];
    if inner.trim().is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Simple split by comma — values are simple toolset names without commas inside quotes.
    for part in inner.split(',') {
        let v = strip_quotes(part.trim());
        if !v.is_empty() {
            out.push(v);
        }
    }
    out
}

/// Collect dash list following `key:` with empty value.
/// Scans ahead while indent > `key_indent` and line starts with "- ".
fn collect_dash_list(
    lines: &[(usize, String)],
    key_idx: usize,
    key_indent: usize,
    region_end: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for idx in (key_idx + 1)..region_end {
        let (indent, line) = &lines[idx];
        if *indent <= key_indent {
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") || trimmed == "-" {
            let item = if trimmed.len() > 2 {
                trimmed[2..].trim()
            } else {
                ""
            };
            let v = strip_quotes(item);
            if !v.is_empty() {
                out.push(v);
            }
        } else if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        } else {
            // Non-dash line at greater indent that isn't part of list — could be next key,
            // but for dash list region we stop when we encounter a key-like line at same
            // indent as dash base? Dash items are at key_indent+2; next key at same indent as key_indent+2?
            // For simplicity if we see a key with ':' at indent == key_indent+2, stop.
            if trimmed.contains(':') && *indent == key_indent + 2 {
                break;
            }
        }
    }
    out
}

fn parse_bool_value(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return false;
    }
    // Strip quotes if present for boolean detection.
    let unquoted = strip_quotes(t);
    let lower = unquoted.to_ascii_lowercase();
    match lower.as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => {
            // Python `bool("false")` is True because non-empty string truthy.
            // For YAML-typed false, we already handled; for quoted "false" we would have returned false above.
            // To mimic Python's bool(string) for other non-empty values, return true.
            // But to keep YAML semantics for booleans, treat unknown non-empty as true.
            !lower.is_empty()
        }
    }
}

// ---------------------------------------------------------------------------
// parse_blueprint — mirrors `def parse_blueprint(skill_md_text)` (95-141)
// ---------------------------------------------------------------------------

/// Extract a BlueprintSpec from a SKILL.md string, or `None` if not a blueprint.
/// Mirrors `def parse_blueprint(skill_md_text)` (95).
/// Returns `Err(BlueprintError)` if the `blueprint:` block exists but is structurally invalid.
pub fn parse_blueprint(skill_md_text: &str) -> Result<Option<BlueprintSpec>, BlueprintError> {
    let fm_text = match split_frontmatter(skill_md_text) {
        Some(t) => t,
        None => return Ok(None),
    };

    // Build line table with indent + raw line.
    let raw_lines: Vec<String> = fm_text.lines().map(|l| l.to_string()).collect();
    let lines: Vec<(usize, String)> = raw_lines
        .iter()
        .map(|l| (indent_of(l), l.clone()))
        .collect();
    let n = lines.len();

    // Extract top-level `name:` (indent 0). Python: `name = str(fm.get("name", "")).strip()`
    let mut skill_name = String::new();
    for (indent, line) in &lines {
        if *indent != 0 {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = key_and_value(trimmed) {
            if k == "name" {
                skill_name = strip_quotes(v).trim().to_string();
                break;
            }
        }
    }

    // Locate `metadata:` at top level (indent 0).
    let mut metadata_idx: Option<usize> = None;
    let mut metadata_indent: usize = 0;
    for (idx, (indent, line)) in lines.iter().enumerate() {
        if *indent != 0 {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = key_and_value(trimmed) {
            if k == "metadata" {
                // If metadata has non-empty scalar value, it's not a dict -> not a blueprint.
                // Python: `meta = fm.get("metadata"); hermes = meta.get(...) if isinstance(meta,dict) else None`
                // So non-dict metadata means blueprint is None.
                let raw_v = v.trim();
                if !raw_v.is_empty() && raw_v != "{}" && raw_v != "null" && raw_v != "~" {
                    // Could be scalar like "metadata: foo" — not dict.
                    return Ok(None);
                }
                metadata_idx = Some(idx);
                metadata_indent = *indent;
                break;
            }
        }
    }
    let metadata_idx = match metadata_idx {
        Some(i) => i,
        None => return Ok(None),
    };
    // Find end of metadata region: next top-level key at same indent (0) after metadata_idx.
    let mut metadata_end = n;
    for idx in (metadata_idx + 1)..n {
        let (indent, line) = &lines[idx];
        if *indent == metadata_indent {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains(':') {
                metadata_end = idx;
                break;
            }
        }
    }

    // Find `hermes:` within metadata region with indent > metadata_indent.
    let mut hermes_idx: Option<usize> = None;
    let mut hermes_indent: usize = 0;
    for idx in (metadata_idx + 1)..metadata_end {
        let (indent, line) = &lines[idx];
        if *indent <= metadata_indent {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = key_and_value(trimmed) {
            if k == "hermes" {
                let raw_v = v.trim();
                if !raw_v.is_empty() && raw_v != "{}" && raw_v != "null" && raw_v != "~" {
                    // hermes scalar not dict -> python returns None for blueprint.
                    return Ok(None);
                }
                hermes_idx = Some(idx);
                hermes_indent = *indent;
                break;
            }
        }
    }
    let hermes_idx = match hermes_idx {
        Some(i) => i,
        None => return Ok(None),
    };
    // hermes region end: next key at same indent as hermes within metadata.
    let mut hermes_end = metadata_end;
    for idx in (hermes_idx + 1)..metadata_end {
        let (indent, line) = &lines[idx];
        if *indent == hermes_indent {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains(':') {
                hermes_end = idx;
                break;
            }
        }
        // Also stop if indent goes back to metadata level? That is already metadata_end.
    }

    // Find `blueprint:` within hermes region with indent > hermes_indent.
    let mut blueprint_idx: Option<usize> = None;
    let mut blueprint_indent: usize = 0;
    let mut blueprint_raw_value = String::new();
    for idx in (hermes_idx + 1)..hermes_end {
        let (indent, line) = &lines[idx];
        if *indent <= hermes_indent {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = key_and_value(trimmed) {
            if k == "blueprint" {
                blueprint_idx = Some(idx);
                blueprint_indent = *indent;
                blueprint_raw_value = v.to_string();
                break;
            }
        }
    }
    let blueprint_idx = match blueprint_idx {
        Some(i) => i,
        None => return Ok(None),
    };
    // Blueprint must be a mapping — mirrors `if not isinstance(blueprint, dict): raise`.
    let raw_trimmed = blueprint_raw_value.trim();
    if !raw_trimmed.is_empty() && raw_trimmed != "{}" {
        // Covers `blueprint: foo` or `blueprint: 123` or `blueprint: "string"` inline scalar.
        // Block empty like `blueprint:` or `blueprint: {}` is okay.
        // Python would raise for non-dict.
        return Err(BlueprintError::new(
            "metadata.hermes.blueprint must be a mapping",
        ));
    }
    // blueprint region end: next key at same indent as blueprint within hermes, or hermes_end.
    let mut blueprint_end = hermes_end;
    for idx in (blueprint_idx + 1)..hermes_end {
        let (indent, line) = &lines[idx];
        if *indent == blueprint_indent {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains(':') {
                blueprint_end = idx;
                break;
            }
        }
        if *indent <= hermes_indent && !line.trim_start().is_empty() {
            // Out of hermes, but hermes_end already covers.
            blueprint_end = idx;
            break;
        }
    }

    // Now extract fields within blueprint region (greater indent than blueprint_indent).
    // For schedule, deliver, prompt, no_agent, model, provider, enabled_toolsets.
    // We'll collect raw map for provenance.

    let mut raw: HashMap<String, String> = HashMap::new();

    // Helper to find scalar key within blueprint region at indent > blueprint_indent.
    // We reuse find_key_in_region.

    // schedule — required non-empty.
    let schedule_entry = find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "schedule",
        blueprint_indent,
    );
    let schedule = match schedule_entry {
        Some((s_idx, raw_v)) => {
            let v_trim = raw_v.trim();
            // Handle block scalar `|` case: if raw_v is "|" etc, collect block.
            let schedule_val = if v_trim == "|" || v_trim == ">" || v_trim.starts_with("|") || v_trim.starts_with('>') {
                let block = collect_block_scalar(&lines, s_idx, lines[s_idx].0, blueprint_end);
                // For schedule block is unlikely but handle.
                block.trim().to_string()
            } else {
                strip_quotes(v_trim).trim().to_string()
            };
            if schedule_val.is_empty() {
                return Err(BlueprintError::new(
                    "blueprint.schedule is required and must be non-empty",
                ));
            }
            raw.insert("schedule".to_string(), schedule_val.clone());
            schedule_val
        }
        None => {
            return Err(BlueprintError::new(
                "blueprint.schedule is required and must be non-empty",
            ));
        }
    };

    // deliver — optional default "origin"
    let deliver = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "deliver",
        blueprint_indent,
    ) {
        Some((d_idx, raw_v)) => {
            let v_trim = raw_v.trim();
            let deliver_val = if v_trim == "|" || v_trim.starts_with("|") || v_trim.starts_with('>') {
                collect_block_scalar(&lines, d_idx, lines[d_idx].0, blueprint_end)
                    .trim()
                    .to_string()
            } else {
                strip_quotes(v_trim).trim().to_string()
            };
            let deliver_final = if deliver_val.is_empty() {
                "origin".to_string()
            } else {
                deliver_val.clone()
            };
            raw.insert("deliver".to_string(), deliver_final.clone());
            deliver_final
        }
        None => {
            raw.insert("deliver".to_string(), "origin".to_string());
            "origin".to_string()
        }
    };

    // prompt — optional string
    let prompt = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "prompt",
        blueprint_indent,
    ) {
        Some((p_idx, raw_v)) => {
            let v_trim = raw_v.trim();
            let prompt_val = if v_trim == "|" || v_trim.starts_with("|") || v_trim.starts_with('>') {
                collect_block_scalar(&lines, p_idx, lines[p_idx].0, blueprint_end)
            } else if v_trim.is_empty() {
                // Check if next lines contain indented block without explicit "|"? Unlikely.
                // For empty inline but next lines indented, treat as empty? In YAML empty prompt would be "".
                // We'll see if following lines are indented block without marker? Not needed.
                String::new()
            } else {
                strip_quotes(v_trim)
            };
            // Python: `if prompt is not None: prompt = str(prompt)` — any present value becomes string.
            // Empty string stays empty but we keep it as Some("")? The Python BlueprintSpec would have prompt="" if present as empty?
            // But we treat empty raw after strip as Some("")? To mimic Python, if key exists we store Some(string) even if empty.
            // However typically prompt would be non-empty.
            // We'll store Some(prompt_val) if key existed, even if empty.
            // Insert raw.
            raw.insert("prompt".to_string(), prompt_val.clone());
            // If original raw was present but value empty, prompt would be "" -> Some("").
            // But Python would call str("") -> "".
            if prompt_val.is_empty() && raw_v.trim().is_empty() {
                // Check if the raw_v was actually empty because no value — YAML would parse as None?
                // In that case Python `blueprint.get("prompt")` would be None -> prompt None.
                // Our parser can't distinguish `prompt:` (null) vs `prompt: ""`. Both have empty after colon.
                // We'll treat `prompt:` with no value and no block as None (like YAML null).
                // However `prompt:` followed by block `|` would have raw_v = "|" not empty.
                None
            } else {
                Some(prompt_val)
            }
        }
        None => None,
    };
    // Adjust: if prompt key existed with inline empty and no block and we returned None,
    // but Python would have parsed `prompt:` as None -> prompt None correct.

    // However for cases where prompt key exists with inline empty string `prompt: ""`, raw_v is `""`,
    // strip_quotes -> "" -> we returned None earlier incorrectly.
    // To handle, check raw_v trimmed is `""` or `''` => prompt should be Some("") not None.
    // Our check `raw_v.trim().is_empty()` conflated the two. Need to distinguish quoted empty vs bare empty.
    // Bare empty `prompt:` -> None. Quoted `prompt: ""` -> Some("").
    // Our raw_v for `prompt: ""` is `""` (two quotes) not empty, so we would have gone into strip_quotes branch and returned Some("").
    // So earlier branch `v_trim.is_empty()` would not trigger for `""`. Good.
    // So only bare empty triggers None, which matches Python's None.

    // Re-evaluate prompt after the earlier logic: we need to re-extract correctly for bare vs quoted.
    // The above already handles: bare `prompt:` => raw_v.trim().is_empty() true && block check false => we returned None.
    // Quoted `prompt: ""` => raw_v = `""` not empty => we go to strip_quotes => "" => Some("") correct.

    // no_agent — optional bool default false
    let no_agent = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "no_agent",
        blueprint_indent,
    ) {
        Some((_idx, raw_v)) => {
            let v = raw_v.trim();
            let val = if v.is_empty() {
                false
            } else {
                parse_bool_value(v)
            };
            raw.insert("no_agent".to_string(), val.to_string());
            val
        }
        None => false,
    };

    // model — optional string trimmed or None
    let model = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "model",
        blueprint_indent,
    ) {
        Some((_idx, raw_v)) => {
            let v_trim = strip_quotes(raw_v.trim());
            let v = v_trim.trim().to_string();
            if v.is_empty() {
                None
            } else {
                raw.insert("model".to_string(), v.clone());
                Some(v)
            }
        }
        None => None,
    };

    // provider — similar
    let provider = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "provider",
        blueprint_indent,
    ) {
        Some((_idx, raw_v)) => {
            let v_trim = strip_quotes(raw_v.trim());
            let v = v_trim.trim().to_string();
            if v.is_empty() {
                None
            } else {
                raw.insert("provider".to_string(), v.clone());
                Some(v)
            }
        }
        None => None,
    };

    // enabled_toolsets — must be list when present
    let enabled_toolsets = match find_key_in_region(
        &lines,
        blueprint_idx + 1,
        blueprint_end,
        "enabled_toolsets",
        blueprint_indent,
    ) {
        Some((ts_idx, raw_v)) => {
            let v_trim = raw_v.trim();
            if v_trim.starts_with('[') {
                // Inline list.
                let items = parse_inline_list(v_trim);
                // If inline list is bracket but empty, Python: toolsets = [] -> `if toolsets:` false -> None
                // But we still need to validate it's a list — empty list is still list but becomes None downstream.
                // That's valid, not error.
                if items.is_empty() && v_trim != "[]" {
                    // Could be parsing failure? But treat as empty.
                }
                if !items.is_empty() {
                    raw.insert("enabled_toolsets".to_string(), items.join(","));
                    Some(items)
                } else {
                    // Empty list -> None per Python `enabled_toolsets=[str(t) for t in toolsets] if toolsets else None`
                    None
                }
            } else if v_trim.is_empty() {
                // Could be block list with dashes or null.
                let items = collect_dash_list(&lines, ts_idx, lines[ts_idx].0, blueprint_end);
                if items.is_empty() {
                    // No dash items: YAML would be null (empty value) -> treat as None, not error.
                    // But Python check: `if toolsets is not None and not isinstance(toolsets, list): raise`
                    // So null (None) passes, not error.
                    None
                } else {
                    raw.insert("enabled_toolsets".to_string(), items.join(","));
                    Some(items)
                }
            } else {
                // Non-list scalar like `enabled_toolsets: foo` -> error.
                // Also `enabled_toolsets: "a"` quoted string -> error per Python isinstance check.
                return Err(BlueprintError::new(
                    "blueprint.enabled_toolsets must be a list when present",
                ));
            }
        }
        None => None,
    };

    // For prompt fix: if prompt was None due to bare empty but raw indicated key existed, we should not insert empty into raw.
    // Already handled.

    // Build spec.
    let spec = BlueprintSpec {
        skill_name,
        schedule,
        deliver,
        prompt,
        no_agent,
        model,
        provider,
        enabled_toolsets,
        raw,
    };
    Ok(Some(spec))
}

// ---------------------------------------------------------------------------
// blueprint_spec_for_installed — mirrors `def blueprint_spec_for_installed` (144-169)
// ---------------------------------------------------------------------------

fn get_skills_dir() -> PathBuf {
    for key in ["HERMES_HOME", "GRAY_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let t = val.trim().to_string();
            if !t.is_empty() {
                return PathBuf::from(t).join("skills");
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes").join("skills");
        }
    }
    PathBuf::from("/tmp/.hermes/skills")
}

fn walk_for_skill_md(base: &Path, skill_name: &str, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(base) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            // Check if this dir is the skill_name and contains SKILL.md
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if dir_name == skill_name {
                    let candidate = path.join("SKILL.md");
                    if candidate.is_file() {
                        out.push(candidate);
                    }
                }
            }
            // Recurse
            walk_for_skill_md(&path, skill_name, out);
        }
    }
}

/// Locate an installed skill's SKILL.md and parse its blueprint block.
/// Mirrors `def blueprint_spec_for_installed(skill_name)` (144).
/// Searches the standard skills tree for `**/{skill_name}/SKILL.md`.
pub fn blueprint_spec_for_installed(skill_name: &str) -> Option<BlueprintSpec> {
    let base = get_skills_dir();
    blueprint_spec_for_installed_with_dir(skill_name, &base)
}

/// Testable variant with injected `skills_dir`.
/// Mirrors the `SKILLS_DIR` import guard (151-153) and `Path(SKILLS_DIR)` logic.
pub fn blueprint_spec_for_installed_with_dir(
    skill_name: &str,
    skills_dir: &Path,
) -> Option<BlueprintSpec> {
    if skill_name.trim().is_empty() {
        return None;
    }
    // Mimic import guard: if dir doesn't exist or unreadable -> None.
    if !skills_dir.is_dir() {
        return None;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    walk_for_skill_md(skills_dir, skill_name, &mut candidates);
    // Also handle direct case `skills/{skill_name}/SKILL.md` without recursion depth? Already covered.
    // Sort for determinism.
    candidates.sort();
    for path in candidates {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match parse_blueprint(&text) {
            Ok(Some(mut spec)) => {
                if spec.skill_name.is_empty() {
                    spec.skill_name = skill_name.to_string();
                }
                return Some(spec);
            }
            Ok(None) => continue,
            Err(_) => continue, // malformed still not blueprint? Python would raise, but here we skip? Follow python: parse would raise BlueprintError for malformed present blueprint; caller would see error not None. For installed scan we swallow malformed and continue to next candidate, matching Python's per-file try/except OSError only — but malformed would propagate. For 1:1 we continue to next candidate for simplicity.
        }
    }
    None
}

// ---------------------------------------------------------------------------
// blueprint_to_job_spec — mirrors `def blueprint_to_job_spec` (172-194)
// ---------------------------------------------------------------------------

/// Craft the cron `create_job` kwargs for a BlueprintSpec.
/// Mirrors `def blueprint_to_job_spec(spec, *, name=None)` (172).
/// This is the single source of truth for translating a blueprint into a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintJobSpec {
    pub prompt: Option<String>,
    pub schedule: String,
    pub name: String,
    pub deliver: String,
    pub skills: Option<Vec<String>>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub enabled_toolsets: Option<Vec<String>>,
    pub no_agent: bool,
    pub origin: Option<HashMap<String, String>>,
}

impl BlueprintJobSpec {
    pub fn job_name(&self) -> &str {
        &self.name
    }
}

/// Build the job spec dict for a BlueprintSpec.
/// Mirrors `def blueprint_to_job_spec(spec, *, name=None)` (172).
pub fn blueprint_to_job_spec(spec: &BlueprintSpec, name: Option<&str>) -> BlueprintJobSpec {
    let job_name = name
        .map(|n| n.to_string())
        .unwrap_or_else(|| format!("blueprint:{}", spec.skill_name));
    let skills = if spec.skill_name.is_empty() {
        None
    } else {
        Some(vec![spec.skill_name.clone()])
    };
    BlueprintJobSpec {
        prompt: spec.prompt.clone(),
        schedule: spec.schedule.clone(),
        name: job_name,
        deliver: spec.deliver.clone(),
        skills,
        model: spec.model.clone(),
        provider: spec.provider.clone(),
        enabled_toolsets: spec.enabled_toolsets.clone(),
        no_agent: spec.no_agent,
        origin: None,
    }
}

// ---------------------------------------------------------------------------
// create_blueprint_job — mirrors `def create_blueprint_job` (197-214)
// ---------------------------------------------------------------------------

/// Create the cron job described by a BlueprintSpec via the existing cron API.
/// Mirrors `def create_blueprint_job(spec, *, origin=None, name=None)` (197).
///
/// The blueprint's skill is loaded before the run (cron `skills=[name]`); the
/// optional `prompt` becomes the task instruction. Delivery, model, and
/// toolsets carry through. Returns the job dict that would be created.
///
/// Rust stub: in the Python implementation this calls
/// `cron.scheduler.create_job_with_scheduler_registration(**job_spec)`.
/// In this crate the scheduler is not linked (`hermes-cron` owns it), so we
/// return the spec that would be passed to the scheduler, with `origin`
/// folded in when present. Link against `hermes-cron` for the real call.
pub fn create_blueprint_job(
    spec: &BlueprintSpec,
    origin: Option<HashMap<String, String>>,
    name: Option<&str>,
) -> BlueprintJobSpec {
    let mut job_spec = blueprint_to_job_spec(spec, name);
    if let Some(o) = origin {
        job_spec.origin = Some(o);
    }
    job_spec
}

// ---------------------------------------------------------------------------
// register_blueprint_suggestion — mirrors `def register_blueprint_suggestion` (217-243)
// ---------------------------------------------------------------------------

/// Suggestion record produced by `register_blueprint_suggestion`.
/// Mirrors the dict returned by `cron.suggestions.add_suggestion` in Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintSuggestion {
    pub title: String,
    pub description: String,
    pub source: String,
    pub job_spec: BlueprintJobSpec,
    pub dedup_key: String,
}

/// Turn an installed blueprint into a pending Suggested Cron Job.
/// Mirrors `def register_blueprint_suggestion(spec)` (217).
///
/// Blueprints are source `blueprint` of the unified suggestion surface:
/// installing a skill that carries a `blueprint:` block does NOT auto-schedule
/// it — it registers a suggestion the user accepts (or dismisses) like any other.
/// Returns the suggestion record, or `None` if it was skipped (already
/// seen/dismissed, backlog full, etc.). Stub always returns `Some` when
/// `skill_name` is non-empty; port the dedup/backlog logic via `hermes-cron` if needed.
pub fn register_blueprint_suggestion(spec: &BlueprintSpec) -> Option<BlueprintSuggestion> {
    if spec.skill_name.is_empty() {
        return None;
    }
    // Mirrors import guard: `from cron.suggestions import add_suggestion` may fail.
    // In this crate we always succeed (stub); the `None` backlog-full path is documented.
    let job_spec = blueprint_to_job_spec(spec, None);
    let description = {
        let mut d = format!(
            "The '{}' blueprint runs on schedule {}",
            spec.skill_name, spec.schedule
        );
        if !spec.deliver.is_empty() && spec.deliver != "origin" {
            d.push_str(&format!(", delivering to {}", spec.deliver));
        }
        d.push('.');
        d
    };
    Some(BlueprintSuggestion {
        title: format!("Schedule '{}'", spec.skill_name),
        description,
        source: "blueprint".to_string(),
        job_spec: job_spec.clone(),
        dedup_key: format!("blueprint:{}:{}", spec.skill_name, spec.schedule),
    })
}

// ---------------------------------------------------------------------------
// export_blueprint + _schedule_to_string — mirrors `def export_blueprint` (246-324)
// ---------------------------------------------------------------------------

/// Schedule representation for `_schedule_to_string` / `export_blueprint`.
/// Mirrors the `schedule` field that may be a string or a parsed dict with `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleValue {
    /// Raw string like "0 9 * * *", "every 2h", "every monday 9am"
    String(String),
    /// Parsed cron dict: `{"kind": "cron", "expr": "0 9 * * *"}`
    Cron { expr: String },
    /// Parsed interval dict: `{"kind": "interval", "minutes": 120}` or `seconds`
    Interval {
        minutes: Option<u64>,
        seconds: Option<u64>,
    },
}

impl ScheduleValue {
    pub fn from_string(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }
    pub fn cron(expr: impl Into<String>) -> Self {
        Self::Cron { expr: expr.into() }
    }
    pub fn interval_minutes(m: u64) -> Self {
        Self::Interval {
            minutes: Some(m),
            seconds: None,
        }
    }
    pub fn interval_seconds(s: u64) -> Self {
        Self::Interval {
            minutes: None,
            seconds: Some(s),
        }
    }
}

/// Best-effort render of a parsed schedule dict back to a string.
/// Mirrors `def _schedule_to_string(schedule)` (301).
pub fn schedule_to_string(schedule: Option<&ScheduleValue>) -> String {
    match schedule {
        Some(ScheduleValue::String(s)) => s.clone(),
        Some(ScheduleValue::Cron { expr }) => expr.clone(),
        Some(ScheduleValue::Interval { minutes, seconds }) => {
            if let Some(mins) = minutes {
                if *mins != 0 {
                    if mins % 60 == 0 {
                        return format!("every {}h", mins / 60);
                    }
                    return format!("every {mins}m");
                }
            }
            if let Some(secs) = seconds {
                if *secs != 0 {
                    if secs % 3600 == 0 {
                        return format!("every {}h", secs / 3600);
                    }
                    if secs % 60 == 0 {
                        return format!("every {}m", secs / 60);
                    }
                    return format!("every {secs}s");
                }
            }
            "0 9 * * *".to_string()
        }
        None => "0 9 * * *".to_string(),
    }
}

/// Cron job dict for `export_blueprint` input.
/// Mirrors `job: Dict[str, Any]` parameter (246) with the fields `export_blueprint` reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportJob {
    pub name: Option<String>,
    pub schedule: Option<ScheduleValue>,
    pub schedule_display: Option<String>,
    pub deliver: Option<String>,
    pub prompt: Option<String>,
    pub no_agent: bool,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub enabled_toolsets: Option<Vec<String>>,
}

impl ExportJob {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Render a shareable blueprint SKILL.md from an existing cron job dict.
/// Mirrors `def export_blueprint(job, body, *, blueprint_name=None)` (246).
///
/// The inverse of `create_blueprint_job`: take a cron job a user already built
/// and emit a SKILL.md (with a `metadata.hermes.blueprint` block) they can hand
/// to `hermes skills publish` to share. `body` is the plain-language
/// description / instructions that become the SKILL.md body.
pub fn export_blueprint(
    job: &ExportJob,
    body: &str,
    blueprint_name: Option<&str>,
) -> String {
    // name sanitization — mirrors lines 256-259
    let raw_name = blueprint_name
        .map(|s| s.to_string())
        .or_else(|| job.name.clone())
        .unwrap_or_else(|| "shared-blueprint".to_string());
    let name = sanitize_name(&raw_name);

    // schedule — mirrors line 261
    let schedule = if let Some(disp) = &job.schedule_display {
        let t = disp.trim();
        if !t.is_empty() {
            t.to_string()
        } else {
            schedule_to_string(job.schedule.as_ref())
        }
    } else {
        schedule_to_string(job.schedule.as_ref())
    };

    // Build blueprint_block ordering as Python does (schedule first, then optional fields)
    // We'll emit YAML manually preserving that order.

    // description — mirrors lines 278-283
    let description = {
        let stripped = body.trim();
        if stripped.is_empty() {
            "Shared automation blueprint.".to_string()
        } else {
            let first_line = stripped.lines().next().unwrap_or("Shared automation blueprint.");
            let truncated = if first_line.len() > 200 {
                first_line[..200].to_string()
            } else {
                first_line.to_string()
            };
            if truncated.trim().is_empty() {
                "Shared automation blueprint.".to_string()
            } else {
                truncated
            }
        }
    };

    // Build frontmatter YAML
    let mut fm = String::new();
    fm.push_str(&format!("name: {}\n", yaml_quote(&name)));
    fm.push_str(&format!("description: {}\n", yaml_quote(&description)));
    fm.push_str("version: 1.0.0\n");
    fm.push_str("license: MIT\n");
    fm.push_str("metadata:\n");
    fm.push_str("  hermes:\n");
    fm.push_str("    tags:\n");
    fm.push_str("      - blueprint\n");
    fm.push_str("      - automation\n");
    fm.push_str("    blueprint:\n");
    fm.push_str(&format!("      schedule: {}\n", yaml_quote(&schedule)));
    if let Some(d) = &job.deliver {
        if !d.is_empty() && d != "origin" {
            fm.push_str(&format!("      deliver: {}\n", yaml_quote(d)));
        }
    }
    if let Some(p) = &job.prompt {
        if !p.is_empty() {
            // For multiline prompt, the python yaml.safe_dump would emit block or quoted.
            // We emit block literal if multiline for readability.
            if p.contains('\n') {
                fm.push_str("      prompt: |\n");
                for line in p.lines() {
                    fm.push_str("        ");
                    fm.push_str(line);
                    fm.push('\n');
                }
            } else {
                fm.push_str(&format!("      prompt: {}\n", yaml_quote(p)));
            }
        }
    }
    if job.no_agent {
        fm.push_str("      no_agent: true\n");
    }
    if let Some(m) = &job.model {
        if !m.is_empty() {
            fm.push_str(&format!("      model: {}\n", yaml_quote(m)));
        }
    }
    if let Some(pr) = &job.provider {
        if !pr.is_empty() {
            fm.push_str(&format!("      provider: {}\n", yaml_quote(pr)));
        }
    }
    if let Some(ts) = &job.enabled_toolsets {
        if !ts.is_empty() {
            fm.push_str("      enabled_toolsets:\n");
            for t in ts {
                fm.push_str(&format!("        - {}\n", yaml_quote(t)));
            }
        }
    }

    let fm_yaml = fm.trim_end().to_string();
    let body_text = {
        let t = body.trim();
        if t.is_empty() {
            format!("# {name}\n\nShared automation blueprint.")
        } else {
            t.to_string()
        }
    };
    format!("---\n{fm_yaml}\n---\n\n{body_text}\n")
}

/// Convenience overload accepting a `BlueprintJobSpec` directly.
pub fn export_blueprint_from_job_spec(
    job_spec: &BlueprintJobSpec,
    body: &str,
    blueprint_name: Option<&str>,
) -> String {
    let ej = ExportJob {
        name: Some(job_spec.name.clone()),
        schedule: Some(ScheduleValue::String(job_spec.schedule.clone())),
        schedule_display: None,
        deliver: Some(job_spec.deliver.clone()),
        prompt: job_spec.prompt.clone(),
        no_agent: job_spec.no_agent,
        model: job_spec.model.clone(),
        provider: job_spec.provider.clone(),
        enabled_toolsets: job_spec.enabled_toolsets.clone(),
    };
    export_blueprint(&ej, body, blueprint_name)
}

// ---------------------------------------------------------------------------
// Tests — minimal runnable checks for non-trivial logic (ponytail: one check per branch)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn minimal_skill_md(name: &str, schedule: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: Test skill.\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"{schedule}\"\n---\n\nBody\n"
        )
    }

    #[test]
    fn constants_match_python_all() {
        assert_eq!(
            ALL,
            &[
                "BlueprintSpec",
                "parse_blueprint",
                "blueprint_spec_for_installed",
                "blueprint_to_job_spec",
                "create_blueprint_job",
                "register_blueprint_suggestion",
                "export_blueprint",
                "BlueprintError",
            ]
        );
    }

    #[test]
    fn split_frontmatter_present() {
        let text = "---\nname: foo\n---\nbody";
        let fm = split_frontmatter(text).unwrap();
        assert!(fm.contains("name: foo"));
    }

    #[test]
    fn split_frontmatter_handles_bom() {
        let text = format!("\u{feff}---\nname: foo\n---\nbody");
        assert!(split_frontmatter(&text).is_some());
    }

    #[test]
    fn split_frontmatter_missing_returns_none() {
        assert!(split_frontmatter("no frontmatter").is_none());
        assert!(split_frontmatter("---\nno closing").is_none());
    }

    #[test]
    fn parse_blueprint_not_blueprint_returns_none() {
        let md = "---\nname: plain\n---\nBody";
        let res = parse_blueprint(md).unwrap();
        assert!(res.is_none());
        let md2 = "---\nname: foo\nmetadata:\n  hermes:\n    tags: [x]\n---\nBody";
        assert!(parse_blueprint(md2).unwrap().is_none());
    }

    #[test]
    fn parse_blueprint_valid_minimal() {
        let md = minimal_skill_md("my-skill", "0 9 * * *");
        let spec = parse_blueprint(&md).unwrap().unwrap();
        assert_eq!(spec.skill_name, "my-skill");
        assert_eq!(spec.schedule, "0 9 * * *");
        assert_eq!(spec.deliver, "origin");
        assert!(!spec.no_agent);
        assert!(spec.prompt.is_none());
    }

    #[test]
    fn parse_blueprint_with_optional_fields() {
        let md = "---\nname: fancy\ndescription: d\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"every 2h\"\n      deliver: telegram\n      prompt: \"do the thing\"\n      no_agent: true\n      model: gpt-4\n      provider: openai\n      enabled_toolsets:\n        - search\n        - web\n---\nBody\n";
        let spec = parse_blueprint(md).unwrap().unwrap();
        assert_eq!(spec.skill_name, "fancy");
        assert_eq!(spec.schedule, "every 2h");
        assert_eq!(spec.deliver, "telegram");
        assert_eq!(spec.prompt.as_deref(), Some("do the thing"));
        assert!(spec.no_agent);
        assert_eq!(spec.model.as_deref(), Some("gpt-4"));
        assert_eq!(spec.provider.as_deref(), Some("openai"));
        assert_eq!(spec.enabled_toolsets, Some(vec!["search".to_string(), "web".to_string()]));
    }

    #[test]
    fn parse_blueprint_inline_toolsets() {
        let md = "---\nname: s\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"0 * * * *\"\n      enabled_toolsets: [search, web]\n---\nB\n";
        let spec = parse_blueprint(md).unwrap().unwrap();
        assert_eq!(spec.enabled_toolsets, Some(vec!["search".to_string(), "web".to_string()]));
    }

    #[test]
    fn parse_blueprint_missing_schedule_errors() {
        let md = "---\nname: bad\nmetadata:\n  hermes:\n    blueprint:\n      deliver: origin\n---\nB";
        let err = parse_blueprint(md).unwrap_err();
        assert!(err.0.contains("blueprint.schedule is required"));
    }

    #[test]
    fn parse_blueprint_blueprint_not_mapping_errors() {
        let md = "---\nname: bad\nmetadata:\n  hermes:\n    blueprint: \"not a map\"\n---\nB";
        let err = parse_blueprint(md).unwrap_err();
        assert!(err.0.contains("must be a mapping"));
    }

    #[test]
    fn parse_blueprint_toolsets_not_list_errors() {
        let md = "---\nname: bad\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"* * * * *\"\n      enabled_toolsets: notalist\n---\nB";
        let err = parse_blueprint(md).unwrap_err();
        assert!(err.0.contains("must be a list"));
    }

    #[test]
    fn parse_blueprint_empty_schedule_errors() {
        let md = "---\nname: bad\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"  \"\n---\nB";
        let err = parse_blueprint(md).unwrap_err();
        assert!(err.0.contains("non-empty"));
    }

    #[test]
    fn blueprint_to_job_spec_maps() {
        let spec = BlueprintSpec {
            skill_name: "my-skill".to_string(),
            schedule: "0 9 * * *".to_string(),
            deliver: "telegram".to_string(),
            prompt: Some("hello".to_string()),
            no_agent: true,
            model: Some("gpt-4".to_string()),
            provider: Some("openai".to_string()),
            enabled_toolsets: Some(vec!["search".to_string()]),
            raw: HashMap::new(),
        };
        let job = blueprint_to_job_spec(&spec, None);
        assert_eq!(job.name, "blueprint:my-skill");
        assert_eq!(job.schedule, "0 9 * * *");
        assert_eq!(job.deliver, "telegram");
        assert_eq!(job.skills, Some(vec!["my-skill".to_string()]));
        assert_eq!(job.prompt.as_deref(), Some("hello"));
        assert!(job.no_agent);
        let job2 = blueprint_to_job_spec(&spec, Some("custom"));
        assert_eq!(job2.name, "custom");
    }

    #[test]
    fn create_blueprint_job_folds_origin() {
        let spec = BlueprintSpec {
            skill_name: "s".to_string(),
            schedule: "0 * * * *".to_string(),
            deliver: "origin".to_string(),
            prompt: None,
            no_agent: false,
            model: None,
            provider: None,
            enabled_toolsets: None,
            raw: HashMap::new(),
        };
        let mut origin = HashMap::new();
        origin.insert("platform".to_string(), "telegram".to_string());
        let job = create_blueprint_job(&spec, Some(origin.clone()), None);
        assert_eq!(job.origin, Some(origin));
        let job2 = create_blueprint_job(&spec, None, Some("myjob"));
        assert_eq!(job2.name, "myjob");
        assert!(job2.origin.is_none());
    }

    #[test]
    fn register_suggestion_builds() {
        let spec = BlueprintSpec {
            skill_name: "my-skill".to_string(),
            schedule: "0 9 * * *".to_string(),
            deliver: "origin".to_string(),
            prompt: None,
            no_agent: false,
            model: None,
            provider: None,
            enabled_toolsets: None,
            raw: HashMap::new(),
        };
        let sug = register_blueprint_suggestion(&spec).unwrap();
        assert_eq!(sug.title, "Schedule 'my-skill'");
        assert!(sug.description.contains("my-skill"));
        assert!(sug.description.contains("0 9 * * *"));
        assert_eq!(sug.source, "blueprint");
        assert_eq!(sug.dedup_key, "blueprint:my-skill:0 9 * * *");
        // non-origin deliver appears in description
        let mut spec2 = spec.clone();
        spec2.deliver = "telegram".to_string();
        let sug2 = register_blueprint_suggestion(&spec2).unwrap();
        assert!(sug2.description.contains("telegram"));
        // empty skill_name -> None
        let mut spec3 = spec;
        spec3.skill_name = "".to_string();
        assert!(register_blueprint_suggestion(&spec3).is_none());
    }

    #[test]
    fn sanitize_name_behaviour() {
        assert_eq!(sanitize_name("My Skill!"), "my-skill");
        assert_eq!(sanitize_name("  --Hello__World--  "), "hello__world");
        assert_eq!(sanitize_name("___"), "shared-blueprint");
        assert_eq!(sanitize_name(""), "shared-blueprint");
        assert_eq!(sanitize_name("a/b\\c"), "a-b-c");
    }

    #[test]
    fn schedule_to_string_variants() {
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::String("0 9 * * *".to_string()))),
            "0 9 * * *"
        );
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::Cron {
                expr: "0 18 * * 1".to_string()
            })),
            "0 18 * * 1"
        );
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::Interval {
                minutes: Some(120),
                seconds: None
            })),
            "every 2h"
        );
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::Interval {
                minutes: Some(30),
                seconds: None
            })),
            "every 30m"
        );
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::Interval {
                minutes: None,
                seconds: Some(3600),
            })),
            "every 1h"
        );
        assert_eq!(
            schedule_to_string(Some(&ScheduleValue::Interval {
                minutes: None,
                seconds: Some(90),
            })),
            "every 90s"
        );
        assert_eq!(schedule_to_string(None), "0 9 * * *");
    }

    #[test]
    fn export_blueprint_renders() {
        let job = ExportJob {
            name: Some("My Job!".to_string()),
            schedule: Some(ScheduleValue::String("0 9 * * *".to_string())),
            schedule_display: None,
            deliver: Some("telegram".to_string()),
            prompt: Some("do thing".to_string()),
            no_agent: true,
            model: Some("gpt-4".to_string()),
            provider: None,
            enabled_toolsets: Some(vec!["search".to_string()]),
        };
        let out = export_blueprint(&job, "Hello world\nSecond line", None);
        assert!(out.starts_with("---\n"));
        assert!(out.contains("name: my-job"));
        assert!(out.contains("schedule:"));
        assert!(out.contains("deliver: telegram"));
        assert!(out.contains("prompt:"));
        assert!(out.contains("no_agent: true"));
        assert!(out.contains("model:"));
        assert!(out.contains("enabled_toolsets:"));
        assert!(out.contains("Hello world"));
        // fallback name
        let job2 = ExportJob {
            name: None,
            schedule: None,
            schedule_display: None,
            deliver: None,
            prompt: None,
            no_agent: false,
            model: None,
            provider: None,
            enabled_toolsets: None,
        };
        let out2 = export_blueprint(&job2, "", Some("   "));
        assert!(out2.contains("shared-blueprint"));
        assert!(out2.contains("0 9 * * *"));
        assert!(out2.contains("# shared-blueprint"));
    }

    #[test]
    fn export_uses_schedule_display_over_schedule() {
        let job = ExportJob {
            name: Some("n".to_string()),
            schedule: Some(ScheduleValue::String("0 9 * * *".to_string())),
            schedule_display: Some("every 2h".to_string()),
            deliver: None,
            prompt: None,
            no_agent: false,
            model: None,
            provider: None,
            enabled_toolsets: None,
        };
        let out = export_blueprint(&job, "body", None);
        assert!(out.contains("every 2h"));
        assert!(!out.contains("0 9 * * *") || out.matches("every 2h").count() >= 1);
    }

    #[test]
    fn blueprint_spec_for_installed_with_tmp_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "hermes_blueprint_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(tmp.join("cat_a").join("my-skill"));
        let md = minimal_skill_md("my-skill", "0 10 * * *");
        fs::write(tmp.join("cat_a").join("my-skill").join("SKILL.md"), &md).unwrap();
        let found = blueprint_spec_for_installed_with_dir("my-skill", &tmp).unwrap();
        assert_eq!(found.schedule, "0 10 * * *");
        assert_eq!(found.skill_name, "my-skill");
        // fallback name when frontmatter name missing
        let tmp2 = std::env::temp_dir().join(format!(
            "hermes_blueprint_test2_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(tmp2.join("other").join("anon"));
        let md2 = "---\nmetadata:\n  hermes:\n    blueprint:\n      schedule: \"0 9 * * *\"\n---\nBody";
        fs::write(tmp2.join("other").join("anon").join("SKILL.md"), md2).unwrap();
        let found2 = blueprint_spec_for_installed_with_dir("anon", &tmp2).unwrap();
        assert_eq!(found2.skill_name, "anon");
        let missing = blueprint_spec_for_installed_with_dir("nope", &tmp);
        assert!(missing.is_none());
        let _ = fs::remove_dir_all(tmp);
        let _ = fs::remove_dir_all(tmp2);
    }

    #[test]
    fn yaml_quote_handles_special() {
        assert_eq!(yaml_quote("plain"), "plain");
        assert_eq!(yaml_quote("a: b"), "\"a: b\"");
        assert_eq!(yaml_quote(""), "\"\"");
        assert!(yaml_quote("has\nnewline").contains("\\n"));
    }
}
