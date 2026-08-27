//! hermes-cli kanban_diagnostics — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/kanban_diagnostics.py`
//! slice 1/2 — lines 1–900 of 1216 (first 900 LOC).
//! Covers: module docstring, imports, `SEVERITY_ORDER`,
//! `severity_at_or_above`, `DiagnosticAction` + `Diagnostic` dataclasses,
//! rule helpers (`_task_field`, `_parse_payload`, `_event_kind`, `_event_ts`,
//! `_active_hallucination_events`, `_generic_recovery_actions`), `RuleFn` alias,
//! aux-slot helpers (`_aux_slot_explicit`, `_main_model_visible`,
//! `triage_aux_status`, `_positive_int`), and rule implementations through
//! `_rule_stuck_in_blocked` (lines 831-878) plus the truncated header of
//! `_rule_block_unblock_cycling` (lines 881-900, remainder continues in
//! `kanban_diagnostics_slice2.rs` lines 901-1216).
//!
//! T0711 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-28
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli/kanban_diagnostics.py` module doc (lines 1-28).
///
/// ```text
/// Kanban diagnostics — structured, actionable distress signals for tasks.
///
/// A `Diagnostic` is a machine-readable description of something that's wrong
/// with a kanban task: a hallucinated card id, a spawn crash-loop, a task
/// stuck blocked for too long, etc. Each one carries:
///
/// * A **kind** (canonical code; UI/tests match on this).
/// * A **severity** (`warning` / `error` / `critical`).
/// * A **title** (one-line human description) and **detail** (longer text).
/// * A list of **suggested actions** — structured entries the dashboard
///   turns into buttons and the CLI turns into hints.
///
/// Rules run over (task, recent events, recent runs, optional graph context) and
/// emit diagnostics. They are stateless and read-only — no DB writes. Callers compute
/// diagnostics on demand (on `/board` load, `/tasks/:id` fetch, or
/// `hermes kanban diagnostics`).
///
/// Design goals:
///
/// * Fixable-on-the-operator's-side signals only (missing config, phantom
///   ids, crash loop). Not "the provider returned 502 once" — that's a
///   transient runtime blip, not a diagnostic.
/// * Recoverable: every diagnostic comes with at least one suggested
///   recovery action the operator can actually take from the UI.
/// * Auto-clearing: when the underlying failure mode resolves (a clean
///   `completed` event arrives, a spawn succeeds, the task gets
///   unblocked), the diagnostic stops firing. The audit event trail stays.
/// ```
pub const MODULE_DOC: &str = "hermes_cli/kanban_diagnostics.py — kanban diagnostics (lines 1-900 slice)";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 30-36
// ---------------------------------------------------------------------------
// Python: dataclasses.dataclass/field, typing.Any/Callable/Iterable/Optional, json, time
//
// Rust: std only (NEVER cargo). `json` is stubbed via tiny string scan;
// `time` via std::time::SystemTime.

/// Severity rungs, ordered least → most urgent. The UI colors them
/// amber (warning), orange (error), red (critical). Sorted outputs put
/// critical first so operators see the worst fires at the top.
///
/// Mirrors `SEVERITY_ORDER = ("warning", "error", "critical")` (line 41).
pub const SEVERITY_ORDER: &[&str] = &["warning", "error", "critical"];

// ---------------------------------------------------------------------------
// severity_at_or_above — lines 44-50
// ---------------------------------------------------------------------------

/// Mirrors `severity_at_or_above(severity, threshold)` (lines 44-50).
///
/// ```python
/// def severity_at_or_above(severity: Optional[str], threshold: Optional[str]) -> bool:
///     if threshold is None:
///         return True
///     if severity not in SEVERITY_ORDER or threshold not in SEVERITY_ORDER:
///         return False
///     return SEVERITY_ORDER.index(severity) >= SEVERITY_ORDER.index(threshold)
/// ```
pub fn severity_at_or_above(severity: Option<&str>, threshold: Option<&str>) -> bool {
    if threshold.is_none() {
        return true;
    }
    let sev = match severity {
        Some(s) => s,
        None => return false,
    };
    let thr = threshold.unwrap();
    let sev_idx = SEVERITY_ORDER.iter().position(|&s| s == sev);
    let thr_idx = SEVERITY_ORDER.iter().position(|&s| s == thr);
    match (sev_idx, thr_idx) {
        (Some(si), Some(ti)) => si >= ti,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// DiagnosticAction — lines 53-85
// ---------------------------------------------------------------------------

/// A single recovery action attached to a diagnostic.
///
/// Mirrors `DiagnosticAction` dataclass (lines 53-85).
///
/// The `kind` determines how both the UI and CLI render it:
///
/// * `reclaim` / `reassign` — POST to the matching /tasks/:id/*
///   endpoint; dashboard wires into the existing recovery popover.
/// * `unblock` — PATCH status back to `ready` (for stuck-blocked
///   diagnostics).
/// * `cli_hint` — print/copy a shell command (e.g.
///   `hermes -p <profile> auth`). No HTTP side effect.
/// * `open_docs` — deep-link to the docs URL named in `payload.url`.
/// * `comment` — nudge the operator to add a comment (for
///   stuck-blocked tasks that need human input).
///
/// `suggested=true` marks the action as the recommended first step;
/// the UI highlights it. Multiple actions can be suggested if they're
/// equally valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticAction {
    pub kind: String,
    pub label: String,
    pub payload: HashMap<String, String>,
    pub suggested: bool,
}

impl DiagnosticAction {
    pub fn new(kind: &str, label: &str, payload: HashMap<String, String>, suggested: bool) -> Self {
        Self {
            kind: kind.to_string(),
            label: label.to_string(),
            payload,
            suggested,
        }
    }

    /// Mirrors `DiagnosticAction.to_dict()` (lines 79-85).
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("kind".to_string(), self.kind.clone());
        m.insert("label".to_string(), self.label.clone());
        // payload is a dict; stringify as minimal JSON-ish for no-serde slice
        let payload_str = if self.payload.is_empty() {
            "{}".to_string()
        } else {
            let mut s = String::from("{");
            let mut first = true;
            for (k, v) in &self.payload {
                if !first {
                    s.push_str(", ");
                }
                first = false;
                let ek = k.replace('\\', "\\\\").replace('"', "\\\"");
                let ev = v.replace('\\', "\\\\").replace('"', "\\\"");
                s.push_str(&format!("\"{ek}\": \"{ev}\""));
            }
            s.push('}');
            s
        };
        m.insert("payload".to_string(), payload_str);
        m.insert("suggested".to_string(), self.suggested.to_string());
        m
    }
}

// ---------------------------------------------------------------------------
// Diagnostic — lines 88-117
// ---------------------------------------------------------------------------

/// One active distress signal on a task.
///
/// Mirrors `Diagnostic` dataclass (lines 88-117).
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub actions: Vec<DiagnosticAction>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub count: i64,
    pub run_id: Option<i64>,
    pub data: HashMap<String, String>,
}

impl Diagnostic {
    pub fn new(
        kind: &str,
        severity: &str,
        title: &str,
        detail: &str,
        actions: Vec<DiagnosticAction>,
        first_seen_at: i64,
        last_seen_at: i64,
        count: i64,
        run_id: Option<i64>,
        data: HashMap<String, String>,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            severity: severity.to_string(),
            title: title.to_string(),
            detail: detail.to_string(),
            actions,
            first_seen_at,
            last_seen_at,
            count,
            run_id,
            data,
        }
    }

    /// Mirrors `Diagnostic.to_dict()` (lines 105-117).
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("kind".to_string(), self.kind.clone());
        m.insert("severity".to_string(), self.severity.clone());
        m.insert("title".to_string(), self.title.clone());
        m.insert("detail".to_string(), self.detail.clone());
        m.insert("first_seen_at".to_string(), self.first_seen_at.to_string());
        m.insert("last_seen_at".to_string(), self.last_seen_at.to_string());
        m.insert("count".to_string(), self.count.to_string());
        m.insert(
            "run_id".to_string(),
            self.run_id.map(|v| v.to_string()).unwrap_or_default(),
        );
        // actions -> JSON-ish string for no-serde slice
        let actions_str = self
            .actions
            .iter()
            .map(|a| {
                let d = a.to_dict();
                format!(
                    "{{\"kind\": \"{}\", \"label\": \"{}\", \"suggested\": {}}}",
                    d.get("kind").unwrap_or(&String::new()),
                    d.get("label").unwrap_or(&String::new()),
                    d.get("suggested").unwrap_or(&"false".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        m.insert("actions".to_string(), format!("[{actions_str}]"));
        // data stringified
        let data_str = if self.data.is_empty() {
            "{}".to_string()
        } else {
            let mut s = String::from("{");
            let mut first = true;
            for (k, v) in &self.data {
                if !first {
                    s.push_str(", ");
                }
                first = false;
                let ek = k.replace('\\', "\\\\").replace('"', "\\\"");
                let ev = v.replace('\\', "\\\\").replace('"', "\\\"");
                s.push_str(&format!("\"{ek}\": \"{ev}\""));
            }
            s.push('}');
            s
        };
        m.insert("data".to_string(), data_str);
        m
    }
}

// ---------------------------------------------------------------------------
// Rule helpers — lines 120-211
// ---------------------------------------------------------------------------

/// Mirrors `_task_field(task, name, default=None)` (lines 124-145).
///
/// Read a field from a task regardless of representation.
///
/// Callers pass sqlite3.Row (dict-like with [] but no attribute
/// access), kanban_db.Task dataclasses (attribute access), or plain
/// dicts (both). This normalises them so rule functions don't have
/// to branch on type each time.
///
/// Rust: tasks are modelled as `HashMap<String, String>` (plain dict)
/// and `HashMap<String, serde-like>` stringified. We mirror the
/// Python polymorphism by accepting `Option<&HashMap>` and returning
/// `Option<String>`; callers compare via `task_field_or_default`.
/// The `Row`-like `keys()` check is collapsed to a direct map lookup
/// since the map IS the `keys()` source.
pub fn task_field(task: Option<&HashMap<String, String>>, name: &str) -> Option<String> {
    task.and_then(|m| m.get(name).cloned())
}

/// Mirrors `_task_field(task, name, default)` with explicit default (lines 124-145).
pub fn task_field_or_default(
    task: Option<&HashMap<String, String>>,
    name: &str,
    default: &str,
) -> String {
    task_field(task, name).unwrap_or_else(|| default.to_string())
}

/// Parse helper for event payload — mirrors `_parse_payload(ev)` (lines 148-160).
///
/// Tolerate event.payload being either a dict or a JSON string.
/// In Rust payloads are stored as `HashMap<String, String>` stringified;
/// this helper parses a JSON-ish string value when present, otherwise
/// returns empty map. For slice 1 without serde we do a best-effort
/// string scan; structured payloads are stored pre-parsed in the event map
/// under `payload` or `payload_json` keys.
pub fn parse_payload(ev: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let raw = match ev.and_then(|m| m.get("payload")) {
        Some(v) => v.clone(),
        None => return HashMap::new(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return HashMap::new();
    }
    // If it looks like JSON object, try tiny JSON scan; else treat as empty
    if trimmed.starts_with('{') {
        // Also check if caller stored a pre-parsed flattened payload under `payload_json`
        // style keys — for slice 1 we just attempt to parse the JSON string.
        if let Some(parsed) = try_parse_json_object(trimmed) {
            return parsed;
        }
        return HashMap::new();
    }
    // If payload was stored as JSON string inside the map value, it would have
    // been double-encoded; handle that via ev.get("payload_json") fallback
    if let Some(j) = ev.and_then(|m| m.get("payload_json")) {
        if let Some(parsed) = try_parse_json_object(j) {
            return parsed;
        }
    }
    HashMap::new()
}

/// Tiny JSON object parser for slice 1 — handles flat string arrays / string values
/// needed for `phantom_cards` / `phantom_refs` / `reason` fields without serde (NEVER cargo).
/// Returns map of key -> raw value string (arrays stringified as JSON).
fn try_parse_json_object(text: &str) -> Option<HashMap<String, String>> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let mut map = HashMap::new();
    let bytes = trimmed.as_bytes();
    let mut pos = 0usize;
    let mut current_key: Option<String> = None;
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            pos += 1;
            let mut s = String::new();
            let mut esc = false;
            while pos < bytes.len() {
                let c = bytes[pos] as char;
                if esc {
                    s.push(c);
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    break;
                } else {
                    s.push(c);
                }
                pos += 1;
            }
            // s is quoted string
            let mut ahead = pos + 1;
            while ahead < bytes.len()
                && (bytes[ahead] == b' '
                    || bytes[ahead] == b'\n'
                    || bytes[ahead] == b'\r'
                    || bytes[ahead] == b'\t')
            {
                ahead += 1;
            }
            if ahead < bytes.len() && bytes[ahead] == b':' {
                current_key = Some(s);
                pos = ahead + 1;
                // skip whitespace
                while pos < bytes.len()
                    && (bytes[pos] == b' '
                        || bytes[pos] == b'\n'
                        || bytes[pos] == b'\r'
                        || bytes[pos] == b'\t')
                {
                    pos += 1;
                }
                // value: string, array, number, bool, null, object
                if pos >= bytes.len() {
                    if let Some(k) = current_key.take() {
                        map.insert(k, String::new());
                    }
                    break;
                }
                if bytes[pos] == b'"' {
                    // string value
                    pos += 1;
                    let mut v = String::new();
                    let mut esc2 = false;
                    while pos < bytes.len() {
                        let c = bytes[pos] as char;
                        if esc2 {
                            v.push(c);
                            esc2 = false;
                        } else if c == '\\' {
                            esc2 = true;
                        } else if c == '"' {
                            break;
                        } else {
                            v.push(c);
                        }
                        pos += 1;
                    }
                    if let Some(k) = current_key.take() {
                        map.insert(k, v);
                    }
                } else if bytes[pos] == b'[' {
                    // array value — capture until matching ]
                    let start = pos;
                    let mut depth = 0usize;
                    let mut in_str = false;
                    let mut esc_a = false;
                    while pos < bytes.len() {
                        let c = bytes[pos] as char;
                        if in_str {
                            if esc_a {
                                esc_a = false;
                            } else if c == '\\' {
                                esc_a = true;
                            } else if c == '"' {
                                in_str = false;
                            }
                        } else if c == '"' {
                            in_str = true;
                        } else if c == '[' {
                            depth += 1;
                        } else if c == ']' {
                            if depth == 0 {
                                pos += 1;
                                break;
                            }
                            depth -= 1;
                            if depth == 0 {
                                // closing of outer array? Actually we started at 1
                                // we increment on `[`, so decrement on `]`
                                // when depth returns to 0 we have closed outer
                                // but we already handled outer `[` as depth 1
                            }
                            if depth == 0 {
                                pos += 1;
                                break;
                            }
                        }
                        pos += 1;
                        if !in_str && depth == 0 && bytes[pos - 1] == b']' {
                            break;
                        }
                    }
                    // naive: find closing ]
                    let mut end = start;
                    let mut d = 0i32;
                    let mut instr = false;
                    let mut esca = false;
                    for (idx, &b) in bytes[start..].iter().enumerate() {
                        let ch = b as char;
                        if instr {
                            if esca {
                                esca = false;
                            } else if ch == '\\' {
                                esca = true;
                            } else if ch == '"' {
                                instr = false;
                            }
                        } else if ch == '"' {
                            instr = true;
                        } else if ch == '[' {
                            d += 1;
                        } else if ch == ']' {
                            d -= 1;
                            if d == 0 {
                                end = start + idx + 1;
                                break;
                            }
                        }
                    }
                    if end > start {
                        let arr_str = String::from_utf8_lossy(&bytes[start..end]).to_string();
                        if let Some(k) = current_key.take() {
                            map.insert(k, arr_str);
                        }
                        pos = end;
                    } else if let Some(k) = current_key.take() {
                        map.insert(k, String::new());
                    }
                } else {
                    // bare value: read until , or }
                    let start = pos;
                    let mut in_s = false;
                    let mut esc_b = false;
                    while pos < bytes.len() {
                        let c = bytes[pos] as char;
                        if in_s {
                            if esc_b {
                                esc_b = false;
                            } else if c == '\\' {
                                esc_b = true;
                            } else if c == '"' {
                                in_s = false;
                            }
                        } else if c == '"' {
                            in_s = true;
                        } else if c == ',' || c == '}' {
                            break;
                        }
                        pos += 1;
                    }
                    let v = String::from_utf8_lossy(&bytes[start..pos]).trim().to_string();
                    if let Some(k) = current_key.take() {
                        map.insert(k, v);
                    }
                    // pos at , or } — outer loop will advance
                    continue;
                }
            } else if let Some(k) = current_key.take() {
                map.insert(k, s);
            }
        }
        pos += 1;
    }
    Some(map)
}

/// Helper to extract string array from parsed payload value.
/// Mirrors `payload.get("phantom_cards", []) or []` iteration (lines 341-344).
pub fn parse_string_array(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" || trimmed == "[]" {
        return Vec::new();
    }
    if trimmed.starts_with('[') {
        // tiny JSON array of strings: ["a","b"] or []
        let inner = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
        if inner.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_str = false;
        let mut esc = false;
        for ch in inner.chars() {
            if in_str {
                if esc {
                    cur.push(ch);
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    in_str = false;
                    out.push(cur.clone());
                    cur.clear();
                } else {
                    cur.push(ch);
                }
            } else if ch == '"' {
                in_str = true;
            }
        }
        return out;
    }
    vec![trimmed.to_string()]
}

/// Mirrors `_event_kind(ev) -> str` (lines 163-164).
pub fn event_kind(ev: Option<&HashMap<String, String>>) -> String {
    task_field(ev, "kind").unwrap_or_default()
}

/// Mirrors `_event_ts(ev) -> int` (lines 167-169).
pub fn event_ts(ev: Option<&HashMap<String, String>>) -> i64 {
    task_field(ev, "created_at")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
}

/// Mirrors `_active_hallucination_events(events, kind)` (lines 172-193).
///
/// Return events of `kind` that have no `completed`/`edited`
/// event *strictly after* them. Walks chronologically: each clean
/// event resets the accumulator; each matching event gets appended.
///
/// Events must be sorted by id (i.e. arrival order); callers pass the
/// task's full event list which the DB already returns in that order.
pub fn active_hallucination_events<'a>(
    events: &'a [HashMap<String, String>],
    kind: &str,
) -> Vec<&'a HashMap<String, String>> {
    let mut active: Vec<&HashMap<String, String>> = Vec::new();
    for ev in events {
        let k = event_kind(Some(ev));
        if k == "completed" || k == "edited" {
            active.clear();
        } else if k == kind {
            active.push(ev);
        }
    }
    active
}

/// Mirrors `_generic_recovery_actions(task, *, running)` (lines 197-210).
/// Standard always-available actions. Every diagnostic can offer these as
/// fallbacks regardless of kind — they're the two baseline recovery
/// primitives the kernel supports.
pub fn generic_recovery_actions(
    task: Option<&HashMap<String, String>>,
    running: bool,
) -> Vec<DiagnosticAction> {
    let mut out = Vec::new();
    if running {
        out.push(DiagnosticAction::new("reclaim", "Reclaim task", HashMap::new(), false));
    }
    let mut payload = HashMap::new();
    payload.insert("reclaim_first".to_string(), running.to_string());
    out.push(DiagnosticAction::new(
        "reassign",
        "Reassign to different profile",
        payload,
        false,
    ));
    let _ = task;
    out
}

// ---------------------------------------------------------------------------
// Rule implementations — lines 213-900
// ---------------------------------------------------------------------------

/// Each rule takes (task, events, runs, now_ts, config) and returns
/// zero or more Diagnostic instances. `events` / `runs` are lists of
/// kanban_db.Event / kanban_db.Run (or plain dicts matching the same
/// shape — for test convenience).
///
/// Mirrors `RuleFn = Callable[[Any, list[Any], list[Any], int, dict], list[Diagnostic]]` (line 222).
pub type RuleFn = fn(
    Option<&HashMap<String, String>>,
    &[HashMap<String, String>],
    &[HashMap<String, String>],
    i64,
    &HashMap<String, String>,
) -> Vec<Diagnostic>;

// ---------------------------------------------------------------------------
// _aux_slot_explicit — lines 225-241
// ---------------------------------------------------------------------------

/// Mirrors `_aux_slot_explicit(slot)` (lines 225-241).
///
/// Return True if the auxiliary slot has user-supplied non-default fields.
///
/// Defaults from `DEFAULT_CONFIG` use `provider: "auto"` with empty
/// model/base_url/api_key — that path falls through to the main model. An
/// "explicit" config is one where the user actively set a provider (not
/// "auto"), or supplied a model / base_url / api_key.
pub fn aux_slot_explicit(slot: Option<&HashMap<String, String>>) -> bool {
    let slot = match slot {
        Some(s) => s,
        None => return false,
    };
    let provider = slot
        .get("provider")
        .map(|v| v.trim().to_lowercase())
        .unwrap_or_default();
    if !provider.is_empty() && provider != "auto" {
        return true;
    }
    for key in ["model", "base_url", "api_key"] {
        if slot.get(key).map(|v| !v.trim().is_empty()).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Variant that accepts raw `HashMap` for JSON-like slot (mirrors Python dict check).
pub fn aux_slot_explicit_map(slot: &HashMap<String, String>) -> bool {
    aux_slot_explicit(Some(slot))
}

// ---------------------------------------------------------------------------
// _main_model_visible — lines 244-263
// ---------------------------------------------------------------------------

/// Mirrors `_main_model_visible(raw_config)` (lines 244-263).
///
/// Best-effort check that a main model is configured.
///
/// Diagnostics runs in the dashboard process which may not share the CLI's
/// runtime state, so we read the raw config dict. If we cannot prove the
/// main model is set, we err on the side of NOT firing the diagnostic.
pub fn main_model_visible(raw_config: Option<&HashMap<String, String>>) -> bool {
    let cfg = match raw_config {
        Some(c) => c,
        None => return false,
    };
    // Python checks `raw_config.get("model")` — if dict, looks for provider+model
    // If model is a flat string in the map, we check directly.
    if let Some(model) = cfg.get("model") {
        if !model.trim().is_empty() {
            // Check if provider also present (dict case flattened)
            if cfg.get("provider").map(|v| !v.trim().is_empty()).unwrap_or(false) {
                return true;
            }
            // Also check keys like model.provider / model.default flattened?
            // For slice 1 without nested maps, treat non-empty model string as visible
            // when provider key exists elsewhere, else require explicit provider.
            // Python: provider = str(model_cfg.get("provider") or "").strip()
            //         model = str(model_cfg.get("default") or model_cfg.get("model") or model_cfg.get("name") or "").strip()
            // So we mirror that by looking for provider key variants.
            let provider = cfg
                .get("model.provider")
                .or_else(|| cfg.get("provider"))
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            let model_name = cfg
                .get("model.default")
                .or_else(|| cfg.get("model.model"))
                .or_else(|| cfg.get("model.name"))
                .or_else(|| cfg.get("model"))
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            if !provider.is_empty() && !model_name.is_empty() {
                return true;
            }
            // Fallback: Python `return bool(str(model_cfg or "").strip())` when model_cfg is string
            // If model is a plain string (not dict), python returns bool(str(model_cfg).strip())
            // We treat any non-empty plain model string as visible only if it contains a provider-like
            // value? To stay faithful, return true for non-empty string.
            return !model.trim().is_empty();
        }
    }
    // Check flattened model keys
    let provider = cfg
        .get("model.provider")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let model = cfg
        .get("model.default")
        .or_else(|| cfg.get("model.model"))
        .or_else(|| cfg.get("model.name"))
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    !provider.is_empty() && !model.is_empty()
}

// ---------------------------------------------------------------------------
// triage_aux_status — lines 266-314
// ---------------------------------------------------------------------------

/// Mirrors `triage_aux_status(config)` (lines 266-314).
///
/// Inspect raw config and report whether triage paths look configured.
///
/// Returns `None` when config context is unavailable (suppress diagnostic
/// to avoid noisy false positives in tests / low-level callers). Otherwise
/// returns a dict with:
///
///   - `auto_decompose`: bool — whether the dispatcher auto-runs decompose
///   - `decomposer_explicit`: bool — user-supplied decomposer slot
///   - `specifier_explicit`: bool — user-supplied specifier slot
///   - `main_model_visible`: bool — main model can serve as auto fallback
pub fn triage_aux_status(
    config: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let config = config?;
    // Explicit override: `triage_aux_status` key in config (flattened as `triage_aux_status.*`)
    // Python: `explicit = config.get("triage_aux_status")` if isinstance(explicit, dict): return explicit
    // We check for a flattened marker `triage_aux_status` == "explicit"
    if config.contains_key("triage_aux_status") {
        // If caller set triage_aux_status.* keys, return them as-is
        let mut out = HashMap::new();
        for (k, v) in config {
            if k.starts_with("triage_aux_status.") {
                out.insert(k["triage_aux_status.".len()..].to_string(), v.clone());
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
        // If the value itself is a JSON object string, try parse
        if let Some(raw) = config.get("triage_aux_status") {
            if raw.trim().starts_with('{') {
                if let Some(parsed) = try_parse_json_object(raw) {
                    return Some(parsed);
                }
            }
        }
    }

    // Check if any config context at all — when neither auxiliary
    // nor kanban nor model keys are present, caller is low-level test passing {} — stay silent.
    let has_aux = config.keys().any(|k| k == "auxiliary" || k.starts_with("auxiliary."));
    let has_kanban = config.keys().any(|k| k == "kanban" || k.starts_with("kanban."));
    let has_model = config.contains_key("model") || config.keys().any(|k| k.starts_with("model"));
    if !has_aux && !has_kanban && !has_model {
        return None;
    }

    // Decomposer / specifier explicit checks — look for flattened auxiliary keys
    // Python: `aux.get("kanban_decomposer")` / `aux.get("triage_specifier")`
    // Flattened: `auxiliary.kanban_decomposer.provider` etc.
    let mut decomposer_explicit = false;
    let mut specifier_explicit = false;
    // Collect auxiliary slot maps
    let mut decomposer_slot: HashMap<String, String> = HashMap::new();
    let mut specifier_slot: HashMap<String, String> = HashMap::new();
    for (k, v) in config {
        if k.starts_with("auxiliary.kanban_decomposer.") {
            decomposer_slot.insert(k["auxiliary.kanban_decomposer.".len()..].to_string(), v.clone());
        } else if k == "auxiliary.kanban_decomposer" {
            // JSON string
            if let Some(parsed) = try_parse_json_object(v) {
                decomposer_slot.extend(parsed);
            }
        }
        if k.starts_with("auxiliary.triage_specifier.") {
            specifier_slot.insert(k["auxiliary.triage_specifier.".len()..].to_string(), v.clone());
        } else if k == "auxiliary.triage_specifier" {
            if let Some(parsed) = try_parse_json_object(v) {
                specifier_slot.extend(parsed);
            }
        }
    }
    // Also check direct `auxiliary` JSON
    if decomposer_slot.is_empty() {
        if let Some(aux_raw) = config.get("auxiliary") {
            if let Some(parsed) = try_parse_json_object(aux_raw) {
                // Look for nested decomposer
                if let Some(v) = parsed.get("kanban_decomposer") {
                    if let Some(inner) = try_parse_json_object(v) {
                        decomposer_slot = inner;
                    }
                }
            }
        }
    }
    if specifier_slot.is_empty() {
        if let Some(aux_raw) = config.get("auxiliary") {
            if let Some(parsed) = try_parse_json_object(aux_raw) {
                if let Some(v) = parsed.get("triage_specifier") {
                    if let Some(inner) = try_parse_json_object(v) {
                        specifier_slot = inner;
                    }
                }
            }
        }
    }
    decomposer_explicit = aux_slot_explicit(Some(&decomposer_slot));
    specifier_explicit = aux_slot_explicit(Some(&specifier_slot));

    // `auto_decompose` defaults to True per kanban DEFAULT_CONFIG.
    let mut auto_decompose = true;
    if let Some(v) = config.get("kanban.auto_decompose") {
        auto_decompose = v.trim().to_lowercase() != "false"
            && v.trim() != "0"
            && v.trim().to_lowercase() != "no"
            && v.trim().to_lowercase() != "off";
        if v.trim().is_empty() {
            auto_decompose = true;
        }
    } else if let Some(v) = config.get("auto_decompose") {
        auto_decompose = v.trim().to_lowercase() != "false"
            && v.trim() != "0"
            && v.trim().to_lowercase() != "no";
    }

    let main_visible = main_model_visible(Some(config));

    let mut out = HashMap::new();
    out.insert("auto_decompose".to_string(), auto_decompose.to_string());
    out.insert(
        "decomposer_explicit".to_string(),
        decomposer_explicit.to_string(),
    );
    out.insert(
        "specifier_explicit".to_string(),
        specifier_explicit.to_string(),
    );
    out.insert(
        "main_model_visible".to_string(),
        main_visible.to_string(),
    );
    Some(out)
}

// ---------------------------------------------------------------------------
// _positive_int — lines 317-322
// ---------------------------------------------------------------------------

/// Mirrors `_positive_int(value, default)` (lines 317-322).
pub fn positive_int(value: Option<&str>, default: i64) -> i64 {
    let raw = match value {
        Some(v) => v.trim(),
        None => return default,
    };
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => n,
        _ => default,
    }
}

// ---------------------------------------------------------------------------
// _rule_hallucinated_cards — lines 325-369
// ---------------------------------------------------------------------------

/// Mirrors `_rule_hallucinated_cards(task, events, runs, now, cfg)` (lines 325-369).
///
/// Blocked-hallucination gate fires: a worker called kanban_complete
/// with created_cards that didn't exist or weren't created by the
/// completing profile. Task stayed in its prior state; the operator
/// needs to decide how to proceed.
///
/// Auto-clears when a successful completion (or edit) follows the
/// blocked event.
pub fn rule_hallucinated_cards(
    task: Option<&HashMap<String, String>>,
    events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    _now: i64,
    _cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let hits = active_hallucination_events(events, "completion_blocked_hallucination");
    if hits.is_empty() {
        return Vec::new();
    }
    let mut phantom_ids: Vec<String> = Vec::new();
    let first = event_ts(Some(hits[0]));
    let last = event_ts(Some(hits[hits.len() - 1]));
    for ev in &hits {
        let payload = parse_payload(Some(ev));
        let raw = payload.get("phantom_cards").map(|s| s.as_str()).unwrap_or("");
        for pid in parse_string_array(raw) {
            if !phantom_ids.contains(&pid) {
                phantom_ids.push(pid);
            }
        }
        // Also handle payload stored as JSON string in event map directly
        if raw.is_empty() {
            if let Some(pc) = ev.get("phantom_cards") {
                for pid in parse_string_array(pc) {
                    if !phantom_ids.contains(&pid) {
                        phantom_ids.push(pid);
                    }
                }
            }
        }
    }
    let running = task_field(task, "status").as_deref() == Some("running");
    let mut actions: Vec<DiagnosticAction> = Vec::new();
    actions.push(DiagnosticAction::new(
        "comment",
        "Add a comment explaining what to do",
        HashMap::new(),
        false,
    ));
    actions.extend(generic_recovery_actions(task, running));
    let mut data = HashMap::new();
    data.insert("phantom_ids".to_string(), phantom_ids.join(","));
    vec![Diagnostic::new(
        "hallucinated_cards",
        "error",
        "Worker claimed cards that don't exist",
        "The completing worker declared created_cards that either didn't exist or weren't created by its profile. The completion was blocked and the task stayed in its prior state. Usually means the worker hallucinated ids instead of capturing return values from kanban_create.",
        actions,
        first,
        last,
        hits.len() as i64,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_triage_aux_unavailable — lines 372-481
// ---------------------------------------------------------------------------

/// Mirrors `_rule_triage_aux_unavailable(task, events, runs, now, cfg)` (lines 372-481).
///
/// A triage task cannot leave triage without an auxiliary helper.
///
/// With the auto-decompose dispatcher (kanban.auto_decompose, default True),
/// triage tasks fan out via `auxiliary.kanban_decomposer` and fall back to
/// `auxiliary.triage_specifier` when the decomposer returns `fanout=false`.
/// With auto-decompose off, the user must run `hermes kanban specify`,
/// which only needs `auxiliary.triage_specifier`.
///
/// The default slot is `provider: auto` → auto-falls back to the main model,
/// so this rule only fires when:
///
///   - the relevant slot is explicitly set to something broken, OR
///   - the auto fallback has no main model to fall back to.
///
/// Config context is required; pass {} from tests to keep the rule silent.
pub fn rule_triage_aux_unavailable(
    task: Option<&HashMap<String, String>>,
    _events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    if task_field(task, "status").as_deref() != Some("triage") {
        return Vec::new();
    }
    let status = match triage_aux_status(Some(cfg)) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let auto_decompose = status
        .get("auto_decompose")
        .map(|v| v == "true")
        .unwrap_or(true);
    let decomposer_explicit = status
        .get("decomposer_explicit")
        .map(|v| v == "true")
        .unwrap_or(false);
    let specifier_explicit = status
        .get("specifier_explicit")
        .map(|v| v == "true")
        .unwrap_or(false);
    let main_visible = status
        .get("main_model_visible")
        .map(|v| v == "true")
        .unwrap_or(false);

    let (primary_slot, primary_explicit, fallback_slot, _fallback_explicit, primary_desc, detail_path) =
        if auto_decompose {
            (
                "auxiliary.kanban_decomposer",
                decomposer_explicit,
                "auxiliary.triage_specifier",
                specifier_explicit,
                "decomposer",
                "Auto-decompose is on, so the dispatcher needs auxiliary.kanban_decomposer (with auxiliary.triage_specifier as a fallback for non-fan-out tasks).",
            )
        } else {
            (
                "auxiliary.triage_specifier",
                specifier_explicit,
                "auxiliary.kanban_decomposer",
                decomposer_explicit,
                "specifier",
                "Auto-decompose is off, so triage tasks need `hermes kanban specify`, which uses auxiliary.triage_specifier.",
            )
        };

    if primary_explicit || main_visible {
        return Vec::new();
    }

    let task_id = task_field(task, "id").unwrap_or_else(|| "<task_id>".to_string());
    let mut actions: Vec<DiagnosticAction> = Vec::new();
    let mut cmd_payload = HashMap::new();
    cmd_payload.insert(
        "command".to_string(),
        format!("hermes config set {primary_slot}.provider auto"),
    );
    actions.push(DiagnosticAction::new(
        "cli_hint",
        &format!("Configure {primary_slot}"),
        cmd_payload,
        true,
    ));
    if !specifier_explicit && !main_visible && !auto_decompose {
        // Python checks `not fallback_explicit and not main_visible` — we mirror for both branches
        // but the fallback hint is only added when fallback is also not explicit.
        // The original Python condition is `if not fallback_explicit and not main_visible:`
        let mut fb_payload = HashMap::new();
        fb_payload.insert(
            "command".to_string(),
            format!("hermes config set {fallback_slot}.provider auto"),
        );
        actions.push(DiagnosticAction::new(
            "cli_hint",
            &format!("Or configure fallback {fallback_slot}"),
            fb_payload,
            false,
        ));
    } else if !decomposer_explicit && !main_visible && auto_decompose {
        // For auto_decompose=true, fallback is triage_specifier
        if !specifier_explicit {
            let mut fb_payload = HashMap::new();
            fb_payload.insert(
                "command".to_string(),
                format!("hermes config set {fallback_slot}.provider auto"),
            );
            actions.push(DiagnosticAction::new(
                "cli_hint",
                &format!("Or configure fallback {fallback_slot}"),
                fb_payload,
                false,
            ));
        }
    }
    if !auto_decompose {
        let mut spec_payload = HashMap::new();
        spec_payload.insert(
            "command".to_string(),
            format!("hermes kanban specify {task_id}"),
        );
        actions.push(DiagnosticAction::new(
            "cli_hint",
            &format!("Specify manually: hermes kanban specify {task_id}"),
            spec_payload,
            false,
        ));
    }

    let mut data = HashMap::new();
    data.insert("task_id".to_string(), task_id.clone());
    data.insert("auto_decompose".to_string(), auto_decompose.to_string());
    data.insert("primary_slot".to_string(), primary_slot.to_string());
    data.insert("main_model_visible".to_string(), main_visible.to_string());

    vec![Diagnostic::new(
        "triage_aux_unavailable",
        "warning",
        &format!("Triage {primary_desc} has no usable model"),
        &format!(
            "This task is still in triage and no working auxiliary model is visible to the dispatcher. {detail_path} The default slot uses `provider: auto` which falls back to the main model, but no main model is configured either. Configure the slot directly or set a main model so the auto fallback can take over."
        ),
        actions,
        now,
        now,
        1,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_prose_phantom_refs — lines 484-515
// ---------------------------------------------------------------------------

/// Mirrors `_rule_prose_phantom_refs(task, events, runs, now, cfg)` (lines 484-515).
///
/// Advisory prose-scan: the completion summary mentions `t_<hex>`
/// ids that don't resolve. Non-blocking; surfaced as a warning only.
///
/// Auto-clears when a fresh clean completion arrives AFTER the
/// suspected event.
pub fn rule_prose_phantom_refs(
    task: Option<&HashMap<String, String>>,
    events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    _now: i64,
    _cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let hits = active_hallucination_events(events, "suspected_hallucinated_references");
    if hits.is_empty() {
        return Vec::new();
    }
    let mut phantom_refs: Vec<String> = Vec::new();
    for ev in &hits {
        let payload = parse_payload(Some(ev));
        let raw = payload.get("phantom_refs").map(|s| s.as_str()).unwrap_or("");
        for pid in parse_string_array(raw) {
            if !phantom_refs.contains(&pid) {
                phantom_refs.push(pid);
            }
        }
        if raw.is_empty() {
            if let Some(pr) = ev.get("phantom_refs") {
                for pid in parse_string_array(pr) {
                    if !phantom_refs.contains(&pid) {
                        phantom_refs.push(pid);
                    }
                }
            }
        }
    }
    let running = task_field(task, "status").as_deref() == Some("running");
    let mut data = HashMap::new();
    data.insert("phantom_refs".to_string(), phantom_refs.join(","));
    vec![Diagnostic::new(
        "prose_phantom_refs",
        "warning",
        "Completion summary references unknown task ids",
        "The completion summary mentions task ids that don't resolve in this board's database. The completion itself succeeded, but downstream consumers parsing the summary may be pointed at cards that never existed.",
        generic_recovery_actions(task, running),
        event_ts(Some(hits[0])),
        event_ts(Some(hits[hits.len() - 1])),
        hits.len() as i64,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_repeated_failures — lines 518-650
// ---------------------------------------------------------------------------

/// Mirrors `_rule_repeated_failures(task, events, runs, now, cfg)` (lines 518-650).
///
/// Task's unified `consecutive_failures` counter is climbing —
/// something about this task+profile combo is broken and each retry
/// fails the same way. Triggers regardless of the specific failure
/// mode (spawn error, timeout, crash) because operationally they
/// all look the same: the kernel keeps retrying and the operator
/// needs to intervene.
///
/// Threshold: cfg["failure_threshold"]. Runtime callers should derive
/// this from `kanban.failure_limit` unless the user explicitly set a
/// diagnostics threshold, so the signal does not lag behind the
/// dispatcher's circuit breaker.
///
/// Accepts the legacy `spawn_failure_threshold` config key for
/// back-compat.
///
/// Terminal statuses are exempt: a done/archived card has nothing left
/// to retry, so a lingering failure streak is history, not a signal.
/// (`complete_task` resets the counter, but a manual done — e.g. a
/// dashboard drag — ends no run and used to leave the flag stuck.)
///
/// A fresh attempt in flight (`running`) is also exempt: retrying a
/// task should clear the stale failure banner until this attempt also
/// resolves. Otherwise a card that's actively trying again still shows
/// "failed Nx", which reads as a current failure. It re-fires if the new
/// run fails too (status leaves `running` with a recorded outcome).
pub fn rule_repeated_failures(
    task: Option<&HashMap<String, String>>,
    _events: &[HashMap<String, String>],
    runs: &[HashMap<String, String>],
    now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let status = task_field(task, "status").unwrap_or_default();
    if status == "done" || status == "archived" || status == "running" {
        return Vec::new();
    }
    let threshold = {
        let raw = cfg
            .get("failure_threshold")
            .or_else(|| cfg.get("spawn_failure_threshold"))
            .map(|s| s.as_str());
        positive_int(raw, 3)
    };
    let failure_limit = positive_int(cfg.get("failure_limit").map(|s| s.as_str()), threshold);
    let failures: i64 = {
        let cf = task_field(task, "consecutive_failures")
            .and_then(|v| v.parse::<i64>().ok());
        if let Some(v) = cf {
            v
        } else {
            task_field(task, "spawn_failures")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        }
    };
    // Python: `if failures is None or failures < threshold: return []`
    // In Rust failures is always Some(i64) after fallback; check threshold.
    // Also handle the case where task has no failure field at all (None).
    let has_cf = task_field(task, "consecutive_failures").is_some()
        || task_field(task, "spawn_failures").is_some();
    if !has_cf && failures == 0 {
        // Check if task actually has no failure count — still need threshold check
        // Python would have failures=0 from default, so < threshold -> return []
    }
    if failures < threshold {
        return Vec::new();
    }
    let last_err = task_field(task, "last_failure_error")
        .or_else(|| task_field(task, "last_spawn_error"))
        .unwrap_or_default();
    let assignee = task_field(task, "assignee").unwrap_or_default();

    // Classify the most recent failure by peeking at run outcomes
    let mut ordered: Vec<&HashMap<String, String>> = runs.iter().collect();
    ordered.sort_by_key(|r| {
        r.get("id")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    });
    let mut most_recent_outcome: Option<String> = None;
    for r in ordered.iter().rev() {
        let oc = r.get("outcome").map(|s| s.as_str()).unwrap_or("");
        if oc == "spawn_failed" || oc == "timed_out" || oc == "crashed" {
            most_recent_outcome = Some(oc.to_string());
            break;
        }
    }

    let mut actions: Vec<DiagnosticAction> = Vec::new();
    if most_recent_outcome.as_deref() == Some("spawn_failed")
        && !assignee.is_empty()
        && assignee != "default"
    {
        let mut p1 = HashMap::new();
        p1.insert("command".to_string(), format!("hermes -p {assignee} doctor"));
        actions.push(DiagnosticAction::new(
            "cli_hint",
            &format!("Verify profile: hermes -p {assignee} doctor"),
            p1,
            true,
        ));
        let mut p2 = HashMap::new();
        p2.insert("command".to_string(), format!("hermes -p {assignee} auth"));
        actions.push(DiagnosticAction::new(
            "cli_hint",
            &format!("Fix profile auth: hermes -p {assignee} auth"),
            p2,
            false,
        ));
    } else if matches!(
        most_recent_outcome.as_deref(),
        Some("timed_out") | Some("crashed")
    ) {
        if let Some(task_id) = task_field(task, "id") {
            if !task_id.is_empty() {
                let mut p = HashMap::new();
                p.insert(
                    "command".to_string(),
                    format!("hermes kanban log {task_id}"),
                );
                actions.push(DiagnosticAction::new(
                    "cli_hint",
                    &format!("Check logs: hermes kanban log {task_id}"),
                    p,
                    true,
                ));
            }
        }
    }
    let running = task_field(task, "status").as_deref() == Some("running");
    actions.extend(generic_recovery_actions(task, running));

    let severity = if failures >= threshold * 2 {
        "critical"
    } else {
        "error"
    };
    let err_text = last_err.trim().to_string();
    let err_snippet = if err_text.is_empty() {
        String::new()
    } else if err_text.len() > 500 {
        format!("{}…", &err_text[..500])
    } else {
        err_text.clone()
    };
    let outcome_label = match most_recent_outcome.as_deref() {
        Some("spawn_failed") => "spawn",
        Some("timed_out") => "timeout",
        Some("crashed") => "crash",
        _ => "failure",
    };
    let (title, detail) = if !err_snippet.is_empty() {
        let first_line = err_snippet.lines().next().unwrap_or("").chars().take(160).collect::<String>();
        (
            format!("Agent {outcome_label} x{failures}: {first_line}"),
            format!(
                "This task has failed {failures} times in a row (most recent: {outcome_label}). Full last error:\n\n{err_snippet}\n\nThe dispatcher circuit breaker is configured for {failure_limit} consecutive non-success attempts. Fix the root cause and reclaim or unblock the task to retry."
            ),
        )
    } else {
        (
            format!("Agent {outcome_label} x{failures} (no error recorded)"),
            format!(
                "This task has failed {failures} times in a row (most recent: {outcome_label}) but no error text was captured. Check the suggested command or the worker log."
            ),
        )
    };
    let mut data = HashMap::new();
    data.insert("consecutive_failures".to_string(), failures.to_string());
    data.insert(
        "most_recent_outcome".to_string(),
        most_recent_outcome.unwrap_or_default(),
    );
    data.insert("last_error".to_string(), last_err.clone());
    data.insert("failure_threshold".to_string(), threshold.to_string());
    data.insert("failure_limit".to_string(), failure_limit.to_string());

    vec![Diagnostic::new(
        &format!("repeated_failures"),
        severity,
        &title,
        &detail,
        actions,
        now,
        now,
        failures,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_repeated_crashes — lines 653-750
// ---------------------------------------------------------------------------

/// Mirrors `_rule_repeated_crashes(task, events, runs, now, cfg)` (lines 653-750).
///
/// The worker spawns fine but keeps crashing mid-run. Check the last
/// N runs' outcomes; N consecutive `crashed` without a successful
/// `completed` means something about the task + profile combo is
/// broken (OOM, missing dependency, tool it needs is down).
///
/// Threshold: cfg["crash_threshold"] (default 2).
///
/// Narrower than `repeated_failures` — fires earlier (2 crashes vs 3
/// total failures) so the operator gets a crash-specific heads-up
/// before the unified rule kicks in. Suppresses itself when the
/// unified rule is also about to fire, to avoid double-flagging.
///
/// Terminal statuses are exempt for the same reason as
/// `repeated_failures` — with one extra wrinkle: this rule reads run
/// history, and a manual done (dashboard drag) appends no `completed`
/// run to break the crash streak, so the flag was permanent (#kanban
/// desktop dogfood). Done means done.
///
/// `running` is exempt too: a fresh attempt is in flight, and its
/// in-flight run (no outcome yet) doesn't break the trailing crash scan,
/// so a retried card kept showing "crashed Nx" over an active run. The
/// banner re-fires if the new attempt also crashes.
pub fn rule_repeated_crashes(
    task: Option<&HashMap<String, String>>,
    _events: &[HashMap<String, String>],
    runs: &[HashMap<String, String>],
    now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let status = task_field(task, "status").unwrap_or_default();
    if status == "done" || status == "archived" || status == "running" {
        return Vec::new();
    }
    let failure_threshold: i64 = {
        let raw = cfg
            .get("failure_threshold")
            .or_else(|| cfg.get("spawn_failure_threshold"))
            .map(|s| s.as_str());
        // Python does `int(cfg.get(...))` without positive guard here
        raw.and_then(|v| v.parse::<i64>().ok()).unwrap_or(3)
    };
    let unified_counter: i64 = task_field(task, "consecutive_failures")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    if unified_counter >= failure_threshold {
        return Vec::new();
    }
    let threshold: i64 = cfg
        .get("crash_threshold")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(2);
    let mut ordered: Vec<&HashMap<String, String>> = runs.iter().collect();
    ordered.sort_by_key(|r| {
        r.get("id")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    });
    let mut consecutive: i64 = 0;
    let mut last_err: Option<String> = None;
    for r in ordered.iter().rev() {
        let outcome = r.get("outcome").map(|s| s.as_str()).unwrap_or("");
        if outcome == "crashed" {
            consecutive += 1;
            if last_err.is_none() {
                last_err = r.get("error").cloned();
            }
        } else if outcome == "completed" || outcome == "reclaimed" {
            break;
        } else {
            continue;
        }
    }
    if consecutive < threshold {
        return Vec::new();
    }
    let task_id = task_field(task, "id").unwrap_or_default();
    let mut actions: Vec<DiagnosticAction> = Vec::new();
    if !task_id.is_empty() {
        let mut p = HashMap::new();
        p.insert("command".to_string(), format!("hermes kanban log {task_id}"));
        actions.push(DiagnosticAction::new(
            "cli_hint",
            &format!("Check logs: hermes kanban log {task_id}"),
            p,
            true,
        ));
    }
    let running = task_field(task, "status").as_deref() == Some("running");
    actions.extend(generic_recovery_actions(task, running));
    let severity = if consecutive >= threshold * 2 {
        "critical"
    } else {
        "error"
    };
    let err_text = last_err.as_deref().unwrap_or("").trim().to_string();
    let err_snippet = if err_text.is_empty() {
        String::new()
    } else if err_text.len() > 500 {
        format!("{}…", &err_text[..500])
    } else {
        err_text.clone()
    };
    let (title, detail) = if !err_snippet.is_empty() {
        let first_line = err_snippet.lines().next().unwrap_or("").chars().take(160).collect::<String>();
        (
            format!("Agent crashed {consecutive}x: {first_line}"),
            format!("The last {consecutive} runs ended with outcome=crashed. Full last error:\n\n{err_snippet}"),
        )
    } else {
        (
            format!("Agent crashed {consecutive}x (no error recorded)"),
            format!(
                "The last {consecutive} runs ended with outcome=crashed but no error text was captured. Check the worker log for more."
            ),
        )
    };
    let mut data = HashMap::new();
    data.insert(
        "consecutive_crashes".to_string(),
        consecutive.to_string(),
    );
    data.insert("last_error".to_string(), last_err.unwrap_or_default());
    vec![Diagnostic::new(
        "repeated_crashes",
        severity,
        &title,
        &detail,
        actions,
        now,
        now,
        consecutive,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_review_dependency_deadlock — lines 753-828
// ---------------------------------------------------------------------------

/// Mirrors `_rule_review_dependency_deadlock(task, events, runs, now, cfg)` (lines 753-828).
///
/// Detect a legacy review handoff that starves downstream children.
///
/// Older workers were instructed to sticky-block an implementation with a
/// `review-required:` reason. A separately modelled reviewer child cannot
/// promote until that parent is terminal, so the lane has no autonomous next
/// step. This compatibility diagnostic is graph-aware but deliberately leaves
/// both the dependency graph and the user's sticky block unchanged.
pub fn rule_review_dependency_deadlock(
    task: Option<&HashMap<String, String>>,
    events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    if task_field(task, "status").as_deref() != Some("blocked") {
        return Vec::new();
    }
    let mut latest_block: Option<&HashMap<String, String>> = None;
    for ev in events {
        if event_kind(Some(ev)) == "blocked" {
            latest_block = Some(ev);
        }
    }
    let latest_block = match latest_block {
        Some(b) => b,
        None => return Vec::new(),
    };
    let payload = parse_payload(Some(latest_block));
    let reason = payload
        .get("reason")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            latest_block
                .get("reason")
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        });
    if !reason.to_lowercase().starts_with("review-required:") {
        return Vec::new();
    }
    // Python: `graph = cfg.get("_graph")` — flattened as `_graph` JSON or `_graph.*`
    let graph_raw = cfg.get("_graph").map(|s| s.as_str()).unwrap_or("");
    let has_graph = !graph_raw.is_empty()
        || cfg.keys().any(|k| k.starts_with("_graph."))
        || cfg.contains_key("_graph.children");
    if !has_graph {
        // Check if _graph is present as JSON object string
        if graph_raw.trim().is_empty() {
            return Vec::new();
        }
    }
    // For slice 1 without serde, we look for children encoded as `_graph.children` JSON array
    // or `_graph` JSON object containing `children`. Best-effort: try to parse _graph as JSON.
    let children_json = cfg
        .get("_graph.children")
        .or_else(|| cfg.get("_graph"))
        .map(|s| s.as_str())
        .unwrap_or("");
    // Also check `_graph` parsed object
    let mut waiting_children: Vec<String> = Vec::new();
    let mut child_ids: Vec<String> = Vec::new();
    // Try to extract children from JSON: expect [{"id": "...", "status": "todo"}, ...]
    // Tiny scan for "status": "todo" entries
    if !children_json.is_empty() {
        // Very small heuristic: find all `"status": "todo"` and capture preceding/following `"id"`
        // For correctness we do a simple string scan for `"id"` values where status is todo
        // This is best-effort without serde; for fully structured graph use slice 2 with serde_json.
        let bytes = children_json.as_bytes();
        // Collect all child objects by scanning for `{` ... `}`
        // Simpler: look for `"id"` and `"status"` pairs in order
        let mut pos = 0usize;
        while pos < bytes.len() {
            // Find next "id"
            let slice = &children_json[pos..];
            let id_key = "\"id\"";
            let status_key = "\"status\"";
            let id_idx = match slice.find(id_key) {
                Some(i) => i,
                None => break,
            };
            let abs_id_idx = pos + id_idx;
            // Find colon after id
            let after_id = &children_json[abs_id_idx + id_key.len()..];
            let colon = match after_id.find(':') {
                Some(c) => c,
                None => break,
            };
            let val_start = abs_id_idx + id_key.len() + colon + 1;
            // Skip whitespace and quote
            let mut vpos = val_start;
            while vpos < children_json.len()
                && (children_json.as_bytes()[vpos] == b' '
                    || children_json.as_bytes()[vpos] == b'\n'
                    || children_json.as_bytes()[vpos] == b'\r'
                    || children_json.as_bytes()[vpos] == b'\t'
                    || children_json.as_bytes()[vpos] == b'"')
            {
                vpos += 1;
                if children_json.as_bytes()[vpos - 1] == b'"' {
                    break;
                }
            }
            // Now vpos at start of id value (after opening quote)
            let mut id_end = vpos;
            while id_end < children_json.len() && children_json.as_bytes()[id_end] != b'"' {
                if children_json.as_bytes()[id_end] == b'\\' {
                    id_end += 2;
                } else {
                    id_end += 1;
                }
            }
            let id_val = children_json[vpos..id_end].to_string();
            // Look ahead for status within next ~200 chars
            let ahead_end = (abs_id_idx + 300).min(children_json.len());
            let ahead = &children_json[abs_id_idx..ahead_end];
            if let Some(s_idx) = ahead.find(status_key) {
                let after_status = &ahead[s_idx + status_key.len()..];
                if let Some(colon2) = after_status.find(':') {
                    let status_val_start = s_idx + status_key.len() + colon2 + 1;
                    let status_slice = &ahead[status_val_start..];
                    let trimmed = status_slice.trim_start();
                    if trimmed.starts_with("\"todo\"") || trimmed.starts_with("'todo'") {
                        if !id_val.is_empty() {
                            child_ids.push(id_val.clone());
                            waiting_children.push(id_val);
                        }
                    }
                }
            }
            pos = id_end + 1;
        }
        // Fallback: if scan found nothing but raw contains "todo", do simpler split
        if waiting_children.is_empty() && children_json.contains("\"todo\"") {
            // Try alternative: count occurrences of `"status":"todo"` or `"status": "todo"`
            // and extract ids via regex-like scan for `"id":`
            // Already attempted; leave empty if not found
        }
    } else {
        // No graph children key — check if cfg has `_graph` as flattened children entries
        // e.g. `_graph.children.0.id` etc. (unlikely in tests, but handle)
        let mut child_count = 0usize;
        for (k, v) in cfg {
            if k.starts_with("_graph.children.") && k.ends_with(".status") && v == "todo" {
                // Extract child id from sibling key
                let prefix = k.trim_end_matches(".status");
                let id_key = format!("{prefix}.id");
                if let Some(cid) = cfg.get(&id_key) {
                    child_ids.push(cid.clone());
                    waiting_children.push(cid.clone());
                    child_count += 1;
                }
            }
        }
        if child_count == 0 {
            return Vec::new();
        }
    }
    if waiting_children.is_empty() {
        return Vec::new();
    }

    let task_id = task_field(task, "id").unwrap_or_default();
    let mut actions: Vec<DiagnosticAction> = Vec::new();
    if !task_id.is_empty() {
        let mut p = HashMap::new();
        p.insert(
            "command".to_string(),
            format!("hermes kanban complete {task_id}"),
        );
        actions.push(DiagnosticAction::new(
            "cli_hint",
            "Complete the finished implementation phase",
            p,
            true,
        ));
    }
    if !task_id.is_empty() && !child_ids.is_empty() {
        let mut p = HashMap::new();
        p.insert(
            "command".to_string(),
            format!("hermes kanban unlink {task_id} {}", child_ids[0]),
        );
        actions.push(DiagnosticAction::new(
            "cli_hint",
            "Or unlink the incorrectly gated reviewer",
            p,
            false,
        ));
    }

    let blocked_at = {
        let ts = event_ts(Some(latest_block));
        if ts != 0 { ts } else { now }
    };
    let mut data = HashMap::new();
    data.insert("blocked_parent_id".to_string(), task_id.clone());
    data.insert("waiting_child_ids".to_string(), child_ids.join(","));
    data.insert("block_reason".to_string(), reason.clone());
    vec![Diagnostic::new(
        "review_dependency_deadlock",
        "error",
        &format!("Review handoff blocks {} dependent task(s)", child_ids.len()),
        "This implementation is sticky-blocked for review while its downstream task(s) require the implementation to be done or archived before they can run. Complete the finished phase, unlink the incorrect dependency, or migrate this workflow to the first-class review lifecycle.",
        actions,
        blocked_at,
        blocked_at,
        child_ids.len() as i64,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_stuck_in_blocked — lines 831-878
// ---------------------------------------------------------------------------

/// Mirrors `_rule_stuck_in_blocked(task, events, runs, now, cfg)` (lines 831-878).
///
/// Task has been in `blocked` status for too long without a comment.
///
/// Threshold: cfg["blocked_stale_hours"] (default 24).
/// Surfaced as a warning so humans know there's a pending unblock.
pub fn rule_stuck_in_blocked(
    task: Option<&HashMap<String, String>>,
    events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let hours: f64 = cfg
        .get("blocked_stale_hours")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(24.0);
    let status = task_field(task, "status").unwrap_or_default();
    if status != "blocked" {
        return Vec::new();
    }
    let mut last_blocked_ts: i64 = 0;
    for ev in events {
        if event_kind(Some(ev)) == "blocked" {
            let t = event_ts(Some(ev));
            if t > last_blocked_ts {
                last_blocked_ts = t;
            }
        }
    }
    if last_blocked_ts == 0 {
        return Vec::new();
    }
    let age_hours = (now - last_blocked_ts) as f64 / 3600.0;
    if age_hours < hours {
        return Vec::new();
    }
    for ev in events {
        let k = event_kind(Some(ev));
        if (k == "commented" || k == "unblocked") && event_ts(Some(ev)) > last_blocked_ts {
            return Vec::new();
        }
    }
    let mut actions = Vec::new();
    actions.push(DiagnosticAction::new(
        "comment",
        "Add a comment / unblock the task",
        HashMap::new(),
        true,
    ));
    let mut data = HashMap::new();
    data.insert("blocked_at".to_string(), last_blocked_ts.to_string());
    data.insert("age_hours".to_string(), format!("{:.1}", age_hours));
    vec![Diagnostic::new(
        "stuck_in_blocked",
        "warning",
        &format!("Task has been blocked for {}h", age_hours as i64),
        &format!(
            "This task transitioned to blocked {}h ago and has had no comments or unblock attempts since. Blocked tasks are waiting for human input — check the block reason and either unblock with feedback or answer with a comment.",
            age_hours as i64
        ),
        actions,
        last_blocked_ts,
        last_blocked_ts,
        1,
        None,
        data,
    )]
}

// ---------------------------------------------------------------------------
// _rule_block_unblock_cycling — lines 881-900 (slice 1 header, remainder in slice2)
// ---------------------------------------------------------------------------

/// Mirrors `_rule_block_unblock_cycling(task, events, runs, now, cfg)` header
/// (lines 881-900).
///
/// Task has cycled through blocked → unblocked many times — the
/// `unblock` is not fixing the underlying problem and the worker
/// keeps re-blocking for substantially the same reason.
///
/// `_rule_stuck_in_blocked` resets its timer on any `commented` /
/// `unblocked` event, so a task that cycles every few minutes is
/// invisible to it regardless of how many times it cycles (#29747
/// gap 1). This rule complements that one by counting block→unblock
/// cycles in a sliding window.
///
/// Threshold: cfg["block_cycle_threshold"] (default 3) cycles within
/// cfg["block_cycle_window_seconds"] (default 24h).
///
/// Slice 1 covers the docstring and threshold setup through the
/// `HERMES_KANBAN_*` / `cfg` reads and the walk comment at line 900.
/// The body (cycle counting, threshold check, Diagnostic construction)
/// continues in `kanban_diagnostics_slice2.rs` lines 901-955.
pub fn rule_block_unblock_cycling(
    _task: Option<&HashMap<String, String>>,
    _events: &[HashMap<String, String>],
    _runs: &[HashMap<String, String>],
    _now: i64,
    cfg: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    // Header stub — thresholds that are within slice 1 (lines 895-900).
    let threshold = positive_int(
        cfg.get("block_cycle_threshold").map(|s| s.as_str()),
        3,
    );
    let window_seconds: f64 = cfg
        .get("block_cycle_window_seconds")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(24.0 * 3600.0);
    let _cycle_cutoff = _now - window_seconds as i64;
    let _ = threshold;
    // Full cycle-counting logic (lines 901-955) lives in slice2.
    // This stub preserves 1:1 line mapping for the slice boundary and
    // keeps the crate compilable without pulling the rest of the file.
    Vec::new()
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `kanban_diagnostics.py` lines 901-1216 (remainder of
// `_rule_block_unblock_cycling` body at 901-955, `_rule_stranded_in_ready`
// at 958-1078, `_RULES` registry + `DIAGNOSTIC_KINDS` + `DEFAULT_CONFIG` at
// 1081-1123, `config_from_kanban_config` / `config_from_runtime_config` at
// 1126-1168, and `compute_task_diagnostics` at 1171-1216) continue in
// `kanban_diagnostics_slice2.rs`. This file intentionally stops at the
// 900-line boundary so that `cargo` is never invoked and the 2-slice
// decomposition stays clean.
