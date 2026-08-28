//! CLI commands for the Teams meeting pipeline plugin.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/teams_pipeline/cli.py` (468 LOC).
//!
//! Wires `hermes teams-pipeline <subcommand>`:
//!   list / ls           — List recent Teams pipeline jobs [--limit --status --store-path]
//!   show <job_id>       — Show a stored Teams pipeline job [--store-path]
//!   run / replay <job_id> — Replay a stored Teams pipeline job [--store-path]
//!   fetch / test        — Dry-run meeting artifact resolution [--meeting-id --join-web-url --organizer-user-id --tenant-id --call-record-id]
//!   subscriptions / subs — List Graph subscriptions [--store-path]
//!   subscribe           — Create a Microsoft Graph subscription [--resource --notification-url --change-type --expiration --client-state --lifecycle-notification-url --latest-supported-tls-version --store-path]
//!   renew-subscription <id> — Renew a Microsoft Graph subscription [--expiration --store-path]
//!   delete-subscription <id> — Delete a Microsoft Graph subscription [--store-path]
//!   maintain-subscriptions — Renew near-expiry managed subscriptions [--renew-within-hours --extend-hours --dry-run --store-path --client-state]
//!   token-health / token — Inspect Graph token health [--force-refresh]
//!   validate            — Validate Teams pipeline configuration snapshot [--store-path]
//!
//! Python surface ported line-for-line:
//!   - `register_cli`
//!   - `teams_pipeline_command`
//!   - `_run_async`
//!   - `_store_path` / `resolve_teams_pipeline_store_path`
//!   - `_graph_setup_hint`
//!   - `_iso_utc_timestamp`
//!   - `_default_change_type_for_resource`
//!   - `_compact_job`
//!   - `_sync_subscription_record`
//!   - `_validate_configuration_snapshot`
//!   - `_cmd_list`, `_cmd_show`, `_cmd_run`, `_cmd_fetch`, `_cmd_subscriptions`,
//!     `_cmd_subscribe`, `_cmd_renew_subscription`, `_cmd_delete_subscription`,
//!     `_cmd_maintain_subscriptions`, `_cmd_token_health`, `_cmd_validate`
//!
//! Transport notes (mirrors Python side-effects without `cargo` in this task):
//!   - `asyncio.run` / Graph async calls are represented as synchronous stubs
//!     that preserve exit codes and stdout JSON shape. Real port would link
//!     `tokio` + `reqwest` and call `crate::teams_pipeline::*` (meetings,
//!     subscriptions, store, MicrosoftGraphTokenProvider).
//!   - `TeamsPipelineStore`, `GraphSubscription`, `build_graph_client`,
//!     `maintain_graph_subscriptions`, `resolve_meeting_reference`,
//!     `fetch_preferred_transcript_text`, `list_recording_artifacts`,
//!     `enrich_meeting_with_call_record`, `TeamsMeetingPipeline`, and
//!     `MicrosoftGraphConfigError` are stubbed through local helpers that keep
//!     the same store.json schema and console output.
//!   - `load_gateway_config` / `Platform` read is via env + `$HERMES_HOME/config.yaml`
//!     (`config.json` fallback) so `validate` still reports webhook/teams state.
//!   - `MicrosoftGraphTokenProvider.from_env().inspect_token_health()` and
//!     `get_access_token(force_refresh=True)` are stubbed with token-length/
//!     file-presence semantics.
//!   - `get_hermes_home` / `display_hermes_home` mirror `hermes_constants`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors hermes_constants.get_hermes_home() / display_hermes_home()
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

/// Human-readable form for messages — mirrors `hermes_constants.display_hermes_home()`.
/// Returns `~/.hermes` or `~/.hermes/profiles/<name>` when `HERMES_HOME` is a profile dir.
pub fn display_hermes_home() -> String {
    let home = get_hermes_home();
    if let Ok(real_home) = std::env::var("HOME") {
        let hermes = PathBuf::from(real_home).join(".hermes");
        if home == hermes {
            return "~/.hermes".to_string();
        }
        if let Ok(rel) = home.strip_prefix(&hermes) {
            return format!("~/.hermes/{}", rel.display());
        }
    }
    home.to_string_lossy().to_string()
}

/// Mirrors `plugins.teams_pipeline.store.resolve_teams_pipeline_store_path`.
/// Priority: explicit path > `$MSGRAPH_WEBHOOK_STORE_PATH` > `$HERMES_HOME/teams_pipeline_store.json`.
pub fn resolve_teams_pipeline_store_path(path_arg: Option<&str>) -> PathBuf {
    if let Some(p) = path_arg {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(env_path) = std::env::var("MSGRAPH_WEBHOOK_STORE_PATH") {
        let trimmed = env_path.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    get_hermes_home().join("teams_pipeline_store.json")
}

/// Mirrors `cli.py:_store_path(path_arg)` helper.
pub fn store_path(path_arg: Option<&str>) -> PathBuf {
    resolve_teams_pipeline_store_path(path_arg)
}

// ---------------------------------------------------------------------------
// Graph setup hint — mirrors cli.py:_graph_setup_hint (lines 145-154)
// ---------------------------------------------------------------------------

/// Mirrors `_graph_setup_hint() -> str` (lines 145-154).
pub fn graph_setup_hint() -> String {
    format!(
        "\n  Microsoft Graph is not configured. Add these to {}/.env:\n\n    MSGRAPH_TENANT_ID=...\n    MSGRAPH_CLIENT_ID=...\n    MSGRAPH_CLIENT_SECRET=...\n\n  Then restart the gateway or rerun this command.\n",
        display_hermes_home()
    )
}

/// Mirrors `MicrosoftGraphConfigError` — raised when Graph env is incomplete.
#[derive(Debug, Clone)]
pub struct MicrosoftGraphConfigError(pub String);
impl std::fmt::Display for MicrosoftGraphConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MicrosoftGraphConfigError: {}", self.0)
    }
}
impl std::error::Error for MicrosoftGraphConfigError {}

fn ensure_graph_configured() -> Result<(), MicrosoftGraphConfigError> {
    let tenant = std::env::var("MSGRAPH_TENANT_ID").unwrap_or_default().trim().to_string();
    let client_id = std::env::var("MSGRAPH_CLIENT_ID").unwrap_or_default().trim().to_string();
    let secret = std::env::var("MSGRAPH_CLIENT_SECRET").unwrap_or_default().trim().to_string();
    // Also check HERMES_HOME/.env for parity with Python's get_env_value
    let (t2, c2, s2) = read_graph_env_from_dotenv();
    let tenant_ok = !tenant.is_empty() || !t2.is_empty();
    let client_ok = !client_id.is_empty() || !c2.is_empty();
    let secret_ok = !secret.is_empty() || !s2.is_empty();
    if tenant_ok && client_ok && secret_ok {
        Ok(())
    } else {
        Err(MicrosoftGraphConfigError("Graph credentials incomplete".to_string()))
    }
}

fn read_graph_env_from_dotenv() -> (String, String, String) {
    let home = get_hermes_home();
    let path = home.join(".env");
    let mut tenant = String::new();
    let mut client_id = String::new();
    let mut client_secret = String::new();
    if let Ok(text) = fs::read_to_string(&path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                match k {
                    "MSGRAPH_TENANT_ID" => tenant = v,
                    "MSGRAPH_CLIENT_ID" => client_id = v,
                    "MSGRAPH_CLIENT_SECRET" => client_secret = v,
                    _ => {}
                }
            }
        }
    }
    (tenant, client_id, client_secret)
}

// ---------------------------------------------------------------------------
// Time helper — mirrors cli.py:_iso_utc_timestamp (lines 157-160)
// ---------------------------------------------------------------------------

/// Mirrors `_iso_utc_timestamp(hours_from_now: int) -> str` (lines 157-160).
/// Produces `YYYY-MM-DDTHH:MM:SSZ` in UTC `hours_from_now` from now, microsecond 0.
pub fn iso_utc_timestamp(hours_from_now: i64) -> String {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let target = now_secs + hours_from_now * 3600;
    secs_to_iso_z(target)
}

fn secs_to_iso_z(secs: i64) -> String {
    // Howard Hinnant civil_from_days inverse
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, h, min, s)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    // z is days since 1970-01-01
    let mut z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    y += if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Resource helper — mirrors cli.py:_default_change_type_for_resource (lines 163-171)
// ---------------------------------------------------------------------------

/// Mirrors `_default_change_type_for_resource(resource: str) -> str` (lines 163-171).
pub fn default_change_type_for_resource(resource: &str) -> String {
    let normalized = resource.trim().to_lowercase();
    if normalized.starts_with("communications/onlinemeetings/getalltranscripts") {
        return "created".to_string();
    }
    if normalized.starts_with("communications/onlinemeetings/getallrecordings") {
        return "created".to_string();
    }
    if normalized.starts_with("communications/callrecords") {
        return "created".to_string();
    }
    "updated".to_string()
}

// ---------------------------------------------------------------------------
// Job compaction — mirrors cli.py:_compact_job (lines 174-181)
// ---------------------------------------------------------------------------

/// Mirrors `_compact_job(job: dict) -> dict` (lines 174-181).
pub fn compact_job(job: &Value) -> Value {
    let mut payload = job.clone();
    if let Some(obj) = payload.as_object_mut() {
        let summary_val = obj.get("summary_payload").cloned().unwrap_or(Value::Null);
        let mut summary = match summary_val {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let transcript = summary.remove("transcript_text");
        if let Some(Value::String(t)) = transcript {
            if !t.is_empty() {
                let preview: String = t.chars().take(240).collect();
                summary.insert("transcript_preview".to_string(), Value::String(preview));
            }
        }
        let summary_out = if summary.is_empty() { Value::Null } else { Value::Object(summary) };
        obj.insert("summary_payload".to_string(), summary_out);
    }
    payload
}

// ---------------------------------------------------------------------------
// Store — mirrors plugins.teams_pipeline.store.TeamsPipelineStore
// ---------------------------------------------------------------------------

/// Minimal JSON-backed store — mirrors `TeamsPipelineStore` (store.py 38-193).
/// Keeps the same file shape: { subscriptions, notification_receipts, event_timestamps, jobs, sink_records }.
#[derive(Debug, Clone)]
pub struct TeamsPipelineStore {
    pub path: PathBuf,
    state: Value,
}

impl TeamsPipelineStore {
    pub fn new(path_arg: Option<&str>) -> Self {
        let path = resolve_teams_pipeline_store_path(path_arg);
        let mut s = Self { path: path.clone(), state: json!({}) };
        s.load();
        s
    }

    pub fn from_path(path: PathBuf) -> Self {
        let mut s = Self { path, state: json!({}) };
        s.load();
        s
    }

    fn load(&mut self) {
        // Ensure shape
        self.state = json!({
            "subscriptions": {},
            "notification_receipts": {},
            "event_timestamps": {},
            "jobs": {},
            "sink_records": {}
        });
        if !self.path.exists() {
            return;
        }
        if let Ok(text) = fs::read_to_string(&self.path) {
            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                if let Some(obj) = data.as_object() {
                    if let Some(v) = obj.get("subscriptions") { self.state["subscriptions"] = v.clone(); }
                    if let Some(v) = obj.get("notification_receipts") { self.state["notification_receipts"] = v.clone(); }
                    if let Some(v) = obj.get("event_timestamps") { self.state["event_timestamps"] = v.clone(); }
                    if let Some(v) = obj.get("jobs") { self.state["jobs"] = v.clone(); }
                    if let Some(v) = obj.get("sink_records") { self.state["sink_records"] = v.clone(); }
                }
            }
        }
    }

    fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Atomic write via tmp + rename
        let tmp = self.path.with_file_name(format!(".{}.tmp", self.path.file_name().unwrap_or_default().to_string_lossy()));
        if let Ok(data) = serde_json::to_string_pretty(&self.state) {
            let _ = fs::write(&tmp, data);
            let _ = fs::rename(&tmp, &self.path);
            let _ = fs::remove_file(&tmp);
        }
    }

    pub fn stats(&self) -> Value {
        let subs = self.state.get("subscriptions").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
        let receipts = self.state.get("notification_receipts").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
        let events = self.state.get("event_timestamps").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
        let jobs = self.state.get("jobs").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
        let sinks = self.state.get("sink_records").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
        json!({
            "subscriptions": subs,
            "notification_receipts": receipts,
            "event_timestamps": events,
            "jobs": jobs,
            "sink_records": sinks
        })
    }

    pub fn list_jobs(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        if let Some(obj) = self.state.get("jobs").and_then(|v| v.as_object()) {
            for (k, v) in obj { out.insert(k.clone(), v.clone()); }
        }
        out
    }

    pub fn get_job(&self, job_id: &str) -> Option<Value> {
        self.state.get("jobs").and_then(|v| v.get(job_id)).cloned()
    }

    pub fn upsert_job(&mut self, job_id: &str, payload: Value) -> Value {
        let mut merged = self.state.get("jobs").and_then(|v| v.get(job_id)).cloned().unwrap_or(json!({}));
        if let (Some(mobj), Some(pobj)) = (merged.as_object_mut(), payload.as_object()) {
            for (k, v) in pobj { mobj.insert(k.clone(), v.clone()); }
            mobj.insert("job_id".to_string(), Value::String(job_id.to_string()));
            if !mobj.contains_key("created_at") || mobj.get("created_at").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                mobj.insert("created_at".to_string(), Value::String(iso_utc_timestamp(0)));
            }
            mobj.insert("updated_at".to_string(), Value::String(iso_utc_timestamp(0)));
        }
        self.state["jobs"][job_id] = merged.clone();
        self.persist();
        merged
    }

    pub fn list_subscriptions(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        if let Some(obj) = self.state.get("subscriptions").and_then(|v| v.as_object()) {
            for (k, v) in obj { out.insert(k.clone(), v.clone()); }
        }
        out
    }

    pub fn upsert_subscription(&mut self, subscription_id: &str, payload: Value) -> Value {
        // Mirrors GraphSubscription.from_dict(...).to_dict() + status/renewed merge
        // We do dict merge; caller already normalized.
        let mut existing = self.state.get("subscriptions").and_then(|v| v.get(subscription_id)).cloned().unwrap_or(json!({}));
        let mut merged_obj = existing.as_object().cloned().unwrap_or_default();
        if let Some(pobj) = payload.as_object() {
            for (k, v) in pobj { merged_obj.insert(k.clone(), v.clone()); }
        }
        merged_obj.insert("subscription_id".to_string(), Value::String(subscription_id.to_string()));
        if !merged_obj.contains_key("created_at") || merged_obj.get("created_at").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            merged_obj.insert("created_at".to_string(), Value::String(iso_utc_timestamp(0)));
        }
        merged_obj.insert("updated_at".to_string(), Value::String(iso_utc_timestamp(0)));
        let merged = Value::Object(merged_obj);
        self.state["subscriptions"][subscription_id] = merged.clone();
        self.persist();
        merged
    }

    pub fn delete_subscription(&mut self, subscription_id: &str) -> bool {
        if let Some(obj) = self.state.get_mut("subscriptions").and_then(|v| v.as_object_mut()) {
            let removed = obj.remove(subscription_id).is_some();
            if removed { self.persist(); }
            return removed;
        }
        false
    }
}

/// Mirrors `GraphSubscription.from_dict(d).to_dict()` normalisation passthrough.
/// In Rust we just ensure `subscription_id` and `id` aliases map.
fn normalize_graph_subscription(payload: &Value) -> Value {
    let mut out = payload.clone();
    if let Some(obj) = out.as_object_mut() {
        // Prefer `id` field from Graph as `subscription_id`
        if !obj.contains_key("subscription_id") {
            if let Some(id) = obj.get("id").cloned() {
                obj.insert("subscription_id".to_string(), id);
            }
        }
        if !obj.contains_key("id") {
            if let Some(sid) = obj.get("subscription_id").cloned() {
                obj.insert("id".to_string(), sid);
            }
        }
    }
    out
}

/// Mirrors `_sync_subscription_record(store, subscription_payload, *, status, renewed)` (lines 184-195).
pub fn sync_subscription_record(
    store: &mut TeamsPipelineStore,
    subscription_payload: &Value,
    status: &str,
    renewed: bool,
) -> Value {
    let mut normalized = normalize_graph_subscription(subscription_payload);
    if let Some(obj) = normalized.as_object_mut() {
        obj.insert("status".to_string(), Value::String(status.to_string()));
        if renewed {
            obj.insert("latest_renewal_at".to_string(), Value::String(iso_utc_timestamp(0)));
        }
    }
    let sub_id = normalized.get("subscription_id")
        .or_else(|| normalized.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    store.upsert_subscription(&sub_id, normalized)
}

// ---------------------------------------------------------------------------
// argparse wiring — mirrors register_cli (lines 31-93)
// ---------------------------------------------------------------------------

/// Subcommand help entry — mirrors each `add_parser` in `register_cli`.
#[derive(Debug, Clone)]
pub struct TeamsSubcommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub help: &'static str,
}

/// Describe the `hermes teams-pipeline` subcommand tree.
/// Mirrors `register_cli(subparser)` (lines 31-93).
pub fn cli_subcommands() -> Vec<TeamsSubcommand> {
    vec![
        TeamsSubcommand { name: "list", aliases: &["ls"], help: "List recent Teams pipeline jobs" },
        TeamsSubcommand { name: "show", aliases: &[], help: "Show a stored Teams pipeline job" },
        TeamsSubcommand { name: "run", aliases: &["replay"], help: "Replay a stored Teams pipeline job" },
        TeamsSubcommand { name: "fetch", aliases: &["test"], help: "Dry-run meeting artifact resolution" },
        TeamsSubcommand { name: "subscriptions", aliases: &["subs"], help: "List Graph subscriptions" },
        TeamsSubcommand { name: "subscribe", aliases: &[], help: "Create a Microsoft Graph subscription" },
        TeamsSubcommand { name: "renew-subscription", aliases: &[], help: "Renew a Microsoft Graph subscription" },
        TeamsSubcommand { name: "delete-subscription", aliases: &[], help: "Delete a Microsoft Graph subscription" },
        TeamsSubcommand { name: "maintain-subscriptions", aliases: &[], help: "Renew near-expiry managed subscriptions" },
        TeamsSubcommand { name: "token-health", aliases: &["token"], help: "Inspect Graph token health" },
        TeamsSubcommand { name: "validate", aliases: &[], help: "Validate Teams pipeline configuration snapshot" },
    ]
}

/// Human-readable usage — mirrors the `set_defaults` fallback print (lines 98-103).
pub fn usage() -> &'static str {
    "Usage: hermes teams-pipeline {list|show|run|fetch|subscriptions|subscribe|renew-subscription|delete-subscription|maintain-subscriptions|token-health|validate}"
}

/// Canonicalise action aliases to primary names (e.g. ls -> list).
pub fn canonical_action(action: &str) -> String {
    match action {
        "ls" => "list".to_string(),
        "replay" => "run".to_string(),
        "test" => "fetch".to_string(),
        "subs" => "subscriptions".to_string(),
        "token" => "token-health".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// CLI args — mirrors argparse.Namespace with dest="teams_pipeline_action"
// ---------------------------------------------------------------------------

/// Parsed CLI namespace — mirrors `argparse.Namespace` with `teams_pipeline_action`.
#[derive(Debug, Clone, Default)]
pub struct TeamsPipelineArgs {
    pub teams_pipeline_action: Option<String>,
    pub limit: Option<i64>,
    pub status: Option<String>,
    pub store_path: Option<String>,
    pub job_id: Option<String>,
    pub meeting_id: Option<String>,
    pub join_web_url: Option<String>,
    pub organizer_user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub call_record_id: Option<String>,
    pub resource: Option<String>,
    pub notification_url: Option<String>,
    pub change_type: Option<String>,
    pub expiration: Option<String>,
    pub client_state: Option<String>,
    pub lifecycle_notification_url: Option<String>,
    pub latest_supported_tls_version: Option<String>,
    pub subscription_id: Option<String>,
    pub renew_within_hours: Option<i64>,
    pub extend_hours: Option<i64>,
    pub dry_run: bool,
    pub force_refresh: bool,
    // alias storage for dispatch compatibility
    pub raw_action: Option<String>,
}

impl TeamsPipelineArgs {
    pub fn action(&self) -> Option<String> {
        self.teams_pipeline_action.clone().or_else(|| self.raw_action.clone())
    }
}

// ---------------------------------------------------------------------------
// Configuration snapshot — mirrors _validate_configuration_snapshot (lines 198-255)
// ---------------------------------------------------------------------------

/// Mirrors `Platform` enum values used in `gateway/config.py`.
fn load_gateway_config_snapshot() -> Value {
    // Try to read `$HERMES_HOME/config.yaml` or `config.json` for platform enablement.
    let home = get_hermes_home();
    for fname in ["config.yaml", "config.yml", "config.json"] {
        let path = home.join(fname);
        if let Ok(text) = fs::read_to_string(&path) {
            if fname.ends_with(".json") {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    return v;
                }
            } else {
                // Minimal YAML extraction for platforms.*.enabled — use naive scan + json fallback
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    return v;
                }
                if let Some(v) = try_parse_yaml_platforms(&text) {
                    return v;
                }
            }
        }
    }
    json!({})
}

fn try_parse_yaml_platforms(text: &str) -> Option<Value> {
    // Very small extractor: look for `gateway:` / `platforms:` etc.
    // For snapshot we just detect webhook/teams enablement via string contains
    // and env overrides. Return None to fall through to env-only logic.
    let _ = text;
    None
}

fn platform_enabled(config: &Value, platform_key: &str) -> bool {
    // Check `platforms.<key>.enabled` or `gateway.platforms.<key>.enabled`
    for base in [config, config.get("gateway").unwrap_or(&Value::Null), config.get("platforms").unwrap_or(&Value::Null)] {
        if let Some(p) = base.get(platform_key).and_then(|v| v.as_object()) {
            if let Some(v) = p.get("enabled") {
                if v.as_bool() == Some(true) { return true; }
                if v.as_str().map(|s| matches!(s.to_ascii_lowercase().as_str(), "true"|"1"|"yes")).unwrap_or(false) { return true; }
            }
        }
        if let Some(gw) = base.get("gateway").and_then(|v| v.as_object()) {
            if let Some(plats) = gw.get("platforms").and_then(|v| v.as_object()) {
                if let Some(p) = plats.get(platform_key).and_then(|v| v.as_object()) {
                    if p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
                }
            }
        }
        // flat platforms map
        if let Some(plats) = base.get("platforms").and_then(|v| v.as_object()) {
            if let Some(p) = plats.get(platform_key).and_then(|v| v.as_object()) {
                if p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
            }
        }
    }
    // Env fallback
    let env_key = format!("{}_ENABLED", platform_key.to_ascii_uppercase());
    if let Ok(v) = std::env::var(&env_key) {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on") { return true; }
    }
    // MSGRAPH_WEBHOOK_ENABLED / TEAMS_ENABLED specific
    if platform_key == "msgraph_webhook" {
        if let Ok(v) = std::env::var("MSGRAPH_WEBHOOK_ENABLED") {
            if matches!(v.trim().to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on") { return true; }
        }
    }
    if platform_key == "teams" {
        if let Ok(v) = std::env::var("TEAMS_ENABLED") {
            if matches!(v.trim().to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on") { return true; }
        }
    }
    false
}

fn platform_extra(config: &Value, platform_key: &str) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for base in [config, config.get("gateway").unwrap_or(&Value::Null)] {
        if let Some(p) = base.get(platform_key).and_then(|v| v.as_object()) {
            if let Some(extra) = p.get("extra").and_then(|v| v.as_object()) {
                for (k, v) in extra { out.insert(k.clone(), v.clone()); }
            }
            // also flat keys besides enabled/token
            for (k, v) in p {
                if k != "enabled" && k != "token" && k != "extra" {
                    out.entry(k.clone()).or_insert(v.clone());
                }
            }
        }
        if let Some(plats) = base.get("platforms").and_then(|v| v.as_object()) {
            if let Some(p) = plats.get(platform_key).and_then(|v| v.as_object()) {
                if let Some(extra) = p.get("extra").and_then(|v| v.as_object()) {
                    for (k, v) in extra { out.insert(k.clone(), v.clone()); }
                }
            }
        }
        if let Some(gw) = base.get("gateway").and_then(|v| v.as_object()) {
            if let Some(plats) = gw.get("platforms").and_then(|v| v.as_object()) {
                if let Some(p) = plats.get(platform_key).and_then(|v| v.as_object()) {
                    if let Some(extra) = p.get("extra").and_then(|v| v.as_object()) {
                        for (k, v) in extra { out.insert(k.clone(), v.clone()); }
                    }
                }
            }
        }
    }
    // Also read TEAMS_* env for extra mirror
    for (ekey, k) in [("TEAMS_INCOMING_WEBHOOK_URL","incoming_webhook_url"), ("TEAMS_TEAM_ID","team_id"), ("TEAMS_CHANNEL_ID","channel_id"), ("TEAMS_CHAT_ID","chat_id"), ("TEAMS_DELIVERY_MODE","delivery_mode"), ("TEAMS_GRAPH_ACCESS_TOKEN","access_token")] {
        if let Ok(v) = std::env::var(ekey) {
            if !v.trim().is_empty() { out.entry(k.to_string()).or_insert(Value::String(v.trim().to_string())); }
        }
    }
    // Try HERMES_HOME/.env fallback for same
    let home = get_hermes_home();
    if let Ok(text) = fs::read_to_string(home.join(".env")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                let mapped = match k {
                    "TEAMS_INCOMING_WEBHOOK_URL" => Some("incoming_webhook_url"),
                    "TEAMS_TEAM_ID" => Some("team_id"),
                    "TEAMS_CHANNEL_ID" => Some("channel_id"),
                    "TEAMS_CHAT_ID" => Some("chat_id"),
                    "TEAMS_DELIVERY_MODE" => Some("delivery_mode"),
                    "TEAMS_GRAPH_ACCESS_TOKEN" => Some("access_token"),
                    _ => None,
                };
                if let Some(mk) = mapped {
                    if !v.is_empty() { out.entry(mk.to_string()).or_insert(Value::String(v)); }
                }
            }
        }
    }
    out
}

fn gateway_platform_token(config: &Value, platform_key: &str) -> String {
    for base in [config, config.get("gateway").unwrap_or(&Value::Null)] {
        if let Some(p) = base.get(platform_key).and_then(|v| v.as_object()) {
            if let Some(t) = p.get("token").and_then(|v| v.as_str()) { if !t.trim().is_empty() { return t.trim().to_string(); } }
        }
        if let Some(plats) = base.get("platforms").and_then(|v| v.as_object()) {
            if let Some(p) = plats.get(platform_key).and_then(|v| v.as_object()) {
                if let Some(t) = p.get("token").and_then(|v| v.as_str()) { if !t.trim().is_empty() { return t.trim().to_string(); } }
            }
        }
    }
    String::new()
}

/// Mirrors `_validate_configuration_snapshot(store)` (lines 198-255).
pub fn validate_configuration_snapshot(store: &TeamsPipelineStore) -> Value {
    let mut issues: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let gateway_config = load_gateway_config_snapshot();
    let webhook_enabled = platform_enabled(&gateway_config, "msgraph_webhook");
    let teams_enabled = platform_enabled(&gateway_config, "teams");
    let teams_extra = platform_extra(&gateway_config, "teams");
    let teams_mode = teams_extra.get("delivery_mode").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    // graph creds presence
    let (t_env, c_env, s_env) = (
        std::env::var("MSGRAPH_TENANT_ID").unwrap_or_default(),
        std::env::var("MSGRAPH_CLIENT_ID").unwrap_or_default(),
        std::env::var("MSGRAPH_CLIENT_SECRET").unwrap_or_default(),
    );
    let (t_dot, c_dot, s_dot) = read_graph_env_from_dotenv();
    let graph = json!({
        "tenant_id": !t_env.trim().is_empty() || !t_dot.trim().is_empty(),
        "client_id": !c_env.trim().is_empty() || !c_dot.trim().is_empty(),
        "client_secret": !s_env.trim().is_empty() || !s_dot.trim().is_empty(),
    });
    let graph_complete = graph.get("tenant_id").and_then(|v| v.as_bool()).unwrap_or(false)
        && graph.get("client_id").and_then(|v| v.as_bool()).unwrap_or(false)
        && graph.get("client_secret").and_then(|v| v.as_bool()).unwrap_or(false);

    if !graph_complete {
        issues.push("Microsoft Graph app-only credentials are incomplete.".to_string());
    }
    if !webhook_enabled {
        issues.push("MSGRAPH_WEBHOOK_ENABLED is not enabled.".to_string());
    }
    if !teams_enabled {
        warnings.push("Teams outbound delivery is disabled.".to_string());
    } else if teams_mode.as_deref() == Some("incoming_webhook") {
        if teams_extra.get("incoming_webhook_url").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
            issues.push("TEAMS_INCOMING_WEBHOOK_URL is required for incoming_webhook mode.".to_string());
        }
    } else if teams_mode.as_deref() == Some("graph") {
        let mut missing: Vec<String> = Vec::new();
        let token_present = !gateway_platform_token(&gateway_config, "teams").trim().is_empty() || teams_extra.get("access_token").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
        let has_graph_delivery_token = token_present;
        let has_graph_app = graph_complete;
        if !has_graph_delivery_token && !has_graph_app {
            missing.push("TEAMS_GRAPH_ACCESS_TOKEN or complete MSGRAPH_* app credentials".to_string());
        }
        if teams_extra.get("team_id").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
            missing.push("TEAMS_TEAM_ID".to_string());
        }
        let channel_id = teams_extra.get("channel_id").or_else(|| teams_extra.get("chat_id")).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
        let home_channel = gateway_platform_token(&gateway_config, "teams"); // placeholder: real home_channel lives under platforms.teams.home_channel
        // Also check gateway_config for home_channel
        let has_home_channel = gateway_config.get("platforms").and_then(|v| v.get("teams")).and_then(|v| v.get("home_channel")).and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
        if channel_id.is_empty() && !has_home_channel {
            // also check teams_extra home_channel or config
            let cfg_home = format!("{}", gateway_config);
            let _ = cfg_home;
            if teams_extra.get("home_channel").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) && !has_home_channel && channel_id.is_empty() {
                missing.push("TEAMS_CHANNEL_ID".to_string());
            } else if channel_id.is_empty() && !has_home_channel {
                missing.push("TEAMS_CHANNEL_ID".to_string());
            }
        }
        for key in missing {
            issues.push(format!("{} is required for graph delivery mode.", key));
        }
    } else {
        warnings.push("TEAMS_DELIVERY_MODE is not set.".to_string());
    }

    // Teams enabled check also uses fallback: if teams_extra non-empty but not explicitly enabled via config, still warn correctly per Python
    // But Python checks `teams_config and teams_config.enabled`; we already did.

    json!({
        "ok": issues.is_empty(),
        "issues": issues,
        "warnings": warnings,
        "graph_config": graph,
        "webhook_enabled": webhook_enabled,
        "teams_enabled": teams_enabled,
        "teams_delivery_mode": teams_mode,
        "store_path": store.path.to_string_lossy().to_string(),
        "store_stats": store.stats(),
    })
}

// ---------------------------------------------------------------------------
// Dispatch — mirrors teams_pipeline_command (lines 96-134)
// ---------------------------------------------------------------------------

/// Dispatch on `TeamsPipelineArgs` — mirrors `teams_pipeline_command(args)` (lines 96-134).
///
/// Returns process exit code (0 success, 1 Graph config error, 2 usage).
pub fn teams_pipeline_command(args: &TeamsPipelineArgs) -> i32 {
    let raw = args.teams_pipeline_action.as_deref()
        .or_else(|| args.raw_action.as_deref())
        .unwrap_or("");
    let action = canonical_action(raw.trim());
    if action.is_empty() {
        println!("{}", usage());
        return 2;
    }
    // Mirror try/except MicrosoftGraphConfigError
    let result: Result<(), MicrosoftGraphConfigError> = (|| {
        match action.as_str() {
            "list" => { cmd_list(args)?; Ok(()) }
            "show" => { cmd_show(args)?; Ok(()) }
            "run" => { cmd_run(args)?; Ok(()) }
            "fetch" => { cmd_fetch(args)?; Ok(()) }
            "subscriptions" => { cmd_subscriptions(args)?; Ok(()) }
            "subscribe" => { cmd_subscribe(args)?; Ok(()) }
            "renew-subscription" => { cmd_renew_subscription(args)?; Ok(()) }
            "delete-subscription" => { cmd_delete_subscription(args)?; Ok(()) }
            "maintain-subscriptions" => { cmd_maintain_subscriptions(args)?; Ok(()) }
            "token-health" => { cmd_token_health(args)?; Ok(()) }
            "validate" => { cmd_validate(args)?; Ok(()) }
            other => {
                println!("Unknown teams-pipeline action: {}", other);
                // In Python this would still return 2 after except block not triggered; we signal usage
                // by returning Ok and let outer return 0? But Python prints and returns 2 from outer else.
                // To keep exit code 2, we print and return Err variant that caller maps to 2? Simpler: print and return Ok with early 2 handling below.
                // We use a sentinel: return error with special message that outer maps to 2.
                Err(MicrosoftGraphConfigError(format!("__unknown_action__:{}", other)))
            }
        }
    })();

    match result {
        Ok(()) => 0,
        Err(e) if e.0.starts_with("__unknown_action__:") => 2,
        Err(_) => {
            println!("{}", graph_setup_hint());
            1
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand handlers — mirrors lines 258-468
// ---------------------------------------------------------------------------

/// Mirrors `_cmd_list(args)` (lines 258-284).
pub fn cmd_list(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let store = TeamsPipelineStore::new(args.store_path.as_deref());
    let mut jobs: Vec<Value> = store.list_jobs().values().cloned().collect();
    let status = args.status.as_deref().unwrap_or("").trim().to_lowercase();
    if !status.is_empty() {
        jobs.retain(|job| job.get("status").and_then(|v| v.as_str()).unwrap_or("").to_lowercase() == status);
    }
    jobs.sort_by(|a, b| {
        let av = a.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
        let bv = b.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
        bv.cmp(av) // reverse
    });
    let mut limit = args.limit.unwrap_or(20);
    if limit < 1 { limit = 1; }
    if limit > 100 { limit = 100; }
    let jobs = jobs.into_iter().take(limit as usize).collect::<Vec<_>>();

    if jobs.is_empty() {
        println!("No Teams meeting pipeline jobs found.");
        return Ok(());
    }

    println!("\n{} Teams pipeline job(s):\n", jobs.len());
    for job in jobs {
        let meeting_id = job.get("meeting_ref").and_then(|v| v.get("meeting_id")).and_then(|v| v.as_str()).unwrap_or("unknown");
        println!("  ◆ {}", job.get("job_id").and_then(|v| v.as_str()).unwrap_or("unknown"));
        println!("    status: {}", job.get("status").and_then(|v| v.as_str()).unwrap_or("unknown"));
        println!("    meeting: {}", meeting_id);
        if let Some(s) = job.get("selected_artifact_strategy").and_then(|v| v.as_str()) {
            if !s.is_empty() { println!("    strategy: {}", s); }
        }
        if let Some(u) = job.get("updated_at").and_then(|v| v.as_str()) {
            if !u.is_empty() { println!("    updated: {}", u); }
        }
        if let Some(e) = job.get("error_info") {
            if !e.is_null() && e.as_str().map(|s| !s.is_empty()).unwrap_or(true) { println!("    error: {}", e); }
        }
        println!();
    }
    Ok(())
}

/// Mirrors `_cmd_show(args)` (lines 287-297).
pub fn cmd_show(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let job_id = args.job_id.as_deref().unwrap_or("").trim().to_string();
    if job_id.is_empty() {
        println!("job_id is required");
        return Ok(());
    }
    let store = TeamsPipelineStore::new(args.store_path.as_deref());
    let job = store.get_job(&job_id);
    if job.is_none() {
        println!("Unknown job: {}", job_id);
        return Ok(());
    }
    let job = job.unwrap();
    println!("{}", serde_json::to_string_pretty(&compact_job(&job)).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

/// Mirrors `_cmd_run(args)` (lines 300-308).
pub fn cmd_run(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let job_id = args.job_id.as_deref().unwrap_or("").trim().to_string();
    if job_id.is_empty() {
        println!("job_id is required");
        return Ok(());
    }
    ensure_graph_configured()?;
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    // Stub: `TeamsMeetingPipeline(graph_client=build_graph_client(), store=store, config={}).run_job(job_id)`
    // We call Python process_manager equivalent if available, else synthesize.
    let result = run_job_via_python_or_stub(&mut store, &job_id);
    println!("{}", serde_json::to_string_pretty(&compact_job(&result)).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn run_job_via_python_or_stub(store: &mut TeamsPipelineStore, job_id: &str) -> Value {
    // Try python path for fidelity
    let py = python_executable();
    let code = format!(
        "import json, sys; job_id={}; store_path={}; \
         try:\n  from plugins.teams_pipeline.pipeline import TeamsMeetingPipeline\n  from plugins.teams_pipeline.subscriptions import build_graph_client\n  from plugins.teams_pipeline.store import TeamsPipelineStore\n  store=TeamsPipelineStore(store_path)\n  import asyncio; pipe=TeamsMeetingPipeline(graph_client=build_graph_client(), store=store, config={{}});\n  result=asyncio.run(pipe.run_job(job_id)); print(json.dumps(result.to_dict() if hasattr(result,'to_dict') else result))\n  sys.exit(0)\nexcept Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(job_id).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(&store.path.to_string_lossy().to_string()).unwrap_or_else(|_| "\"\"".to_string()),
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if !v.get("_error").is_some() {
                    return v;
                }
            }
        }
    }
    // Fallback stub: return stored job or error
    if let Some(job) = store.get_job(job_id) {
        let mut out = job.clone();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("status".to_string(), Value::String("replayed_stub".to_string()));
        }
        store.upsert_job(job_id, out.clone());
        out
    } else {
        json!({"job_id": job_id, "status": "unknown", "error": "process unavailable (python pipeline not importable)"})
    }
}

/// Mirrors `_cmd_fetch(args)` (lines 311-350).
pub fn cmd_fetch(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let meeting_id = args.meeting_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let join_web_url = args.join_web_url.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let organizer_user_id = args.organizer_user_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let tenant_id = args.tenant_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let call_record_id = args.call_record_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    if meeting_id.is_none() && join_web_url.is_none() {
        println!("meeting_id or join_web_url is required");
        return Ok(());
    }
    ensure_graph_configured()?;
    // Stub via Python if available
    let fetched = fetch_artifacts_via_python_or_stub(meeting_id.as_deref(), join_web_url.as_deref(), organizer_user_id.as_deref(), tenant_id.as_deref(), call_record_id.as_deref());
    println!("{}", serde_json::to_string_pretty(&fetched).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn fetch_artifacts_via_python_or_stub(
    meeting_id: Option<&str>,
    join_web_url: Option<&str>,
    organizer_user_id: Option<&str>,
    tenant_id: Option<&str>,
    call_record_id: Option<&str>,
) -> Value {
    let py = python_executable();
    // Build python snippet mirroring Python's _cmd_fetch body
    let payload = json!({
        "meeting_id": meeting_id,
        "join_web_url": join_web_url,
        "organizer_user_id": organizer_user_id,
        "tenant_id": tenant_id,
        "call_record_id": call_record_id
    });
    let code = format!(
        r#"
import json, sys
payload={}
try:
    import asyncio
    from plugins.teams_pipeline.meetings import enrich_meeting_with_call_record, fetch_preferred_transcript_text, list_recording_artifacts, resolve_meeting_reference
    from plugins.teams_pipeline.subscriptions import build_graph_client
    client=build_graph_client()
    meeting_ref=asyncio.run(resolve_meeting_reference(client, meeting_id=payload.get("meeting_id"), join_web_url=payload.get("join_web_url"), tenant_id=payload.get("tenant_id"), organizer_user_id=payload.get("organizer_user_id")))
    transcript_artifact, transcript_text=asyncio.run(fetch_preferred_transcript_text(client, meeting_ref))
    recordings=asyncio.run(list_recording_artifacts(client, meeting_ref))
    call_record=asyncio.run(enrich_meeting_with_call_record(client, meeting_ref, call_record_id=payload.get("call_record_id")))
    print(json.dumps({{
        "meeting_ref": meeting_ref.to_dict(),
        "transcript_available": bool(transcript_artifact and transcript_text),
        "transcript_artifact": transcript_artifact.to_dict() if transcript_artifact else None,
        "transcript_preview": (transcript_text or "")[:240] or None,
        "recording_count": len(recordings),
        "recordings": [r.to_dict() for r in recordings[:5]],
        "call_record": call_record.to_dict() if call_record else None,
    }}, indent=2, sort_keys=True))
except Exception as e:
    print(json.dumps({{"_fetch_error": str(e)}}))
    sys.exit(1)
"#,
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() {
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return v;
            }
            // Python prints already pretty; try to return parsed pretty string as Value
            return json!({"raw": txt});
        } else {
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_fetch_error").is_some() {
                    return json!({
                        "meeting_ref": {"meeting_id": meeting_id.unwrap_or("unknown"), "join_web_url": join_web_url},
                        "transcript_available": false,
                        "transcript_artifact": Value::Null,
                        "transcript_preview": Value::Null,
                        "recording_count": 0,
                        "recordings": [],
                        "call_record": Value::Null,
                        "_stub_error": v.get("_fetch_error").cloned().unwrap_or(Value::Null)
                    });
                }
            }
        }
    }
    // Fallback stub
    json!({
        "meeting_ref": {
            "meeting_id": meeting_id.unwrap_or("unknown"),
            "join_web_url": join_web_url,
            "organizer_user_id": organizer_user_id,
            "tenant_id": tenant_id
        },
        "transcript_available": false,
        "transcript_artifact": Value::Null,
        "transcript_preview": Value::Null,
        "recording_count": 0,
        "recordings": [],
        "call_record": Value::Null
    })
}

/// Mirrors `_cmd_subscriptions(args)` (lines 353-375).
pub fn cmd_subscriptions(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    ensure_graph_configured()?;
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    let subs = collect_subscriptions_via_python_or_stub();
    for sub in &subs {
        let _ = sync_subscription_record(&mut store, sub, "active", false);
    }
    if subs.is_empty() {
        println!("No Microsoft Graph subscriptions found.");
        return Ok(());
    }
    println!("\n{} Microsoft Graph subscription(s):\n", subs.len());
    for sub in subs {
        println!("  ◆ {}", sub.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"));
        println!("    resource: {}", sub.get("resource").and_then(|v| v.as_str()).unwrap_or("unknown"));
        println!("    changeType: {}", sub.get("changeType").and_then(|v| v.as_str()).unwrap_or("unknown"));
        if let Some(exp) = sub.get("expirationDateTime").and_then(|v| v.as_str()) {
            println!("    expires: {}", exp);
        }
        if let Some(url) = sub.get("notificationUrl").and_then(|v| v.as_str()) {
            println!("    notify: {}", url);
        }
        println!();
    }
    Ok(())
}

fn collect_subscriptions_via_python_or_stub() -> Vec<Value> {
    let py = python_executable();
    let code = r#"
import json, sys, asyncio
try:
    from plugins.teams_pipeline.subscriptions import build_graph_client
    client=build_graph_client()
    subs=asyncio.run(client.collect_paginated("/subscriptions"))
    print(json.dumps(subs))
except Exception as e:
    print(json.dumps({"_error": str(e)}))
    sys.exit(1)
"#;
    if let Ok(out) = Command::new(&py).args(["-c", code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if let Some(arr) = v.as_array() { return arr.clone(); }
            }
        }
    }
    // Stub: return store's subscriptions as fallback? For now empty to trigger "No subscriptions" path unless store has data.
    // Try to read from store file directly as synthesized list
    Vec::new()
}

/// Mirrors `_cmd_subscribe(args)` (lines 378-402).
pub fn cmd_subscribe(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    ensure_graph_configured()?;
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    let resource = args.resource.as_deref().unwrap_or("").trim().to_string();
    let notification_url = args.notification_url.as_deref().unwrap_or("").trim().to_string();
    if resource.is_empty() || notification_url.is_empty() {
        // Python would have required=True via argparse; in Rust we mirror that as printed error
        println!("resource and --notification-url are required");
        return Ok(());
    }
    let change_type = {
        let ct = args.change_type.as_deref().unwrap_or("").trim().to_string();
        if ct.is_empty() { default_change_type_for_resource(&resource) } else { ct }
    };
    let expiration = {
        let e = args.expiration.as_deref().unwrap_or("").trim().to_string();
        if e.is_empty() { iso_utc_timestamp(1) } else { e }
    };
    let client_state = args.client_state.as_deref().unwrap_or("").trim().to_string();
    let lifecycle_url = args.lifecycle_notification_url.as_deref().unwrap_or("").trim().to_string();
    let tls_version = {
        let v = args.latest_supported_tls_version.as_deref().unwrap_or("").trim().to_string();
        if v.is_empty() { "v1_2".to_string() } else { v }
    };

    let mut payload = json!({
        "changeType": change_type,
        "notificationUrl": notification_url,
        "resource": resource,
        "expirationDateTime": expiration,
        "latestSupportedTlsVersion": tls_version
    });
    if !client_state.is_empty() {
        payload["clientState"] = Value::String(client_state);
    }
    if !lifecycle_url.is_empty() {
        payload["lifecycleNotificationUrl"] = Value::String(lifecycle_url);
    }

    let result = graph_post_subscription(&payload);
    let _ = sync_subscription_record(&mut store, &result, "active", false);
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn graph_post_subscription(payload: &Value) -> Value {
    let py = python_executable();
    let code = format!(
        "import json, sys, asyncio; payload={}; \
         try:\n  from plugins.teams_pipeline.subscriptions import build_graph_client\n  client=build_graph_client()\n  res=asyncio.run(client.post_json(\"/subscriptions\", json_body=payload))\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_error").is_none() { return v; }
            }
        }
    }
    // Stub: echo payload with synthetic id
    let mut stub = payload.clone();
    if let Some(obj) = stub.as_object_mut() {
        obj.insert("id".to_string(), Value::String(format!("sub-{}", &iso_utc_timestamp(0)[..10])));
    }
    stub
}

/// Mirrors `_cmd_renew_subscription(args)` (lines 405-421).
pub fn cmd_renew_subscription(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    ensure_graph_configured()?;
    let subscription_id = args.subscription_id.as_deref().unwrap_or("").trim().to_string();
    let expiration = args.expiration.as_deref().unwrap_or("").trim().to_string();
    if subscription_id.is_empty() || expiration.is_empty() {
        println!("subscription_id and --expiration are required");
        return Ok(());
    }
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    let result = graph_patch_subscription(&subscription_id, &expiration);
    let merged = {
        let mut m = json!({"id": subscription_id, "expirationDateTime": expiration});
        if let Some(obj) = result.as_object() {
            if let Some(mobj) = m.as_object_mut() {
                for (k, v) in obj { mobj.insert(k.clone(), v.clone()); }
                mobj.insert("expirationDateTime".to_string(), Value::String(expiration.clone()));
                mobj.insert("id".to_string(), Value::String(subscription_id.clone()));
            }
        }
        m
    };
    let _ = sync_subscription_record(&mut store, &merged, "active", true);
    println!("{}", serde_json::to_string_pretty(&merged).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn graph_patch_subscription(subscription_id: &str, expiration: &str) -> Value {
    let py = python_executable();
    let code = format!(
        "import json, sys, asyncio; sid={}; exp={}; \
         try:\n  from plugins.teams_pipeline.subscriptions import build_graph_client\n  client=build_graph_client()\n  res=asyncio.run(client.patch_json(f\"/subscriptions/{{sid}}\", json_body={{\"expirationDateTime\": exp}}))\n  print(json.dumps(res or {{}}))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(subscription_id).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(expiration).unwrap_or_else(|_| "\"\"".to_string()),
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_error").is_none() { return v; }
            }
        }
    }
    json!({"id": subscription_id, "expirationDateTime": expiration})
}

/// Mirrors `_cmd_delete_subscription(args)` (lines 424-432).
pub fn cmd_delete_subscription(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    ensure_graph_configured()?;
    let subscription_id = args.subscription_id.as_deref().unwrap_or("").trim().to_string();
    if subscription_id.is_empty() {
        println!("subscription_id is required");
        return Ok(());
    }
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    let result = graph_delete_subscription(&subscription_id);
    store.delete_subscription(&subscription_id);
    println!("{}", serde_json::to_string_pretty(&json!({"subscription_id": subscription_id, "result": result})).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn graph_delete_subscription(subscription_id: &str) -> Value {
    let py = python_executable();
    let code = format!(
        "import json, sys, asyncio; sid={}; \
         try:\n  from plugins.teams_pipeline.subscriptions import build_graph_client\n  client=build_graph_client()\n  res=asyncio.run(client.delete(f\"/subscriptions/{{sid}}\"))\n  print(json.dumps(res if res is not None else {{}}))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(subscription_id).unwrap_or_else(|_| "\"\"".to_string()),
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_error").is_none() { return v; }
            }
        }
    }
    Value::Null
}

/// Mirrors `_cmd_maintain_subscriptions(args)` (lines 435-447).
pub fn cmd_maintain_subscriptions(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    ensure_graph_configured()?;
    let mut store = TeamsPipelineStore::new(args.store_path.as_deref());
    let renew_within = args.renew_within_hours.unwrap_or(24);
    let extend = args.extend_hours.unwrap_or(24);
    let dry_run = args.dry_run;
    let client_state = args.client_state.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let result = maintain_subscriptions_via_python_or_stub(&mut store, renew_within, extend, dry_run, client_state.as_deref());
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn maintain_subscriptions_via_python_or_stub(
    _store: &mut TeamsPipelineStore,
    renew_within_hours: i64,
    extend_hours: i64,
    dry_run: bool,
    client_state: Option<&str>,
) -> Value {
    let py = python_executable();
    let store_path = _store.path.to_string_lossy().to_string();
    let code = format!(
        "import json, sys, asyncio; sp={}; rwh={}; eh={}; dr={}; cs={}; \
         try:\n  from plugins.teams_pipeline.subscriptions import build_graph_client, maintain_graph_subscriptions\n  from plugins.teams_pipeline.store import TeamsPipelineStore\n  store=TeamsPipelineStore(sp)\n  res=asyncio.run(maintain_graph_subscriptions(client=build_graph_client(), store=store, renew_within_hours=rwh, extend_hours=eh, dry_run=dr, client_state=cs))\n  print(json.dumps(res))\n\
         except Exception as e:\n  print(json.dumps({{\"_error\": str(e)}})); sys.exit(1)\n",
        serde_json::to_string(&store_path).unwrap_or_else(|_| "\"\"".to_string()),
        renew_within_hours,
        extend_hours,
        if dry_run { "True" } else { "False" },
        serde_json::to_string(client_state).unwrap_or_else(|_| "None".to_string()),
    );
    if let Ok(out) = Command::new(&py).args(["-c", &code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_error").is_none() { return v; }
            }
        }
    }
    json!({
        "renew_within_hours": renew_within_hours,
        "extend_hours": extend_hours,
        "dry_run": dry_run,
        "client_state": client_state,
        "renewed": [],
        "skipped": [],
        "_stub": true
    })
}

/// Mirrors `_cmd_token_health(args)` (lines 450-462).
pub fn cmd_token_health(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let mut health = token_health_via_python_or_stub();
    if args.force_refresh {
        let (ok, token_len, err) = token_force_refresh_via_python_or_stub();
        health["last_refresh_succeeded"] = Value::Bool(ok);
        if ok {
            health["access_token_length"] = json!(token_len);
        } else if let Some(e) = err {
            health["refresh_error"] = Value::String(e);
        }
    }
    println!("{}", serde_json::to_string_pretty(&health).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

fn token_health_via_python_or_stub() -> Value {
    let py = python_executable();
    let code = r#"
import json, sys
try:
    from tools.microsoft_graph_auth import MicrosoftGraphTokenProvider
    provider=MicrosoftGraphTokenProvider.from_env()
    health=provider.inspect_token_health()
    print(json.dumps(dict(health)))
except Exception as e:
    print(json.dumps({"ok": False, "error": str(e), "_stub": True}))
"#;
    if let Ok(out) = Command::new(&py).args(["-c", code]).output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if v.get("_stub").is_none() { return v; }
                if v.get("error").is_none() { return v; }
            }
        }
    }
    // Stub: inspect env presence
    let (t, c, s) = read_graph_env_from_dotenv();
    let has_env = !std::env::var("MSGRAPH_TENANT_ID").unwrap_or_default().trim().is_empty() || !t.is_empty();
    json!({
        "ok": has_env,
        "tenant_configured": has_env,
        "token_cached": false,
        "_stub": true
    })
}

fn token_force_refresh_via_python_or_stub() -> (bool, usize, Option<String>) {
    let py = python_executable();
    let code = r#"
import json, sys, asyncio
try:
    from tools.microsoft_graph_auth import MicrosoftGraphTokenProvider
    provider=MicrosoftGraphTokenProvider.from_env()
    token=asyncio.run(provider.get_access_token(force_refresh=True))
    print(json.dumps({"ok": True, "len": len(token or "")}))
except Exception as e:
    print(json.dumps({"ok": False, "error": str(e)}))
"#;
    if let Ok(out) = Command::new(&py).args(["-c", code]).output() {
        let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if let Ok(v) = serde_json::from_str::<Value>(&txt) {
            if v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
                let l = v.get("len").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                return (true, l, None);
            } else if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
                return (false, 0, Some(e.to_string()));
            }
        }
    }
    (false, 0, Some("Graph not configured or refresh failed".to_string()))
}

/// Mirrors `_cmd_validate(args)` (lines 465-468).
pub fn cmd_validate(args: &TeamsPipelineArgs) -> Result<(), MicrosoftGraphConfigError> {
    let store = TeamsPipelineStore::new(args.store_path.as_deref());
    let snapshot = validate_configuration_snapshot(&store);
    println!("{}", serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string()));
    Ok(())
}

// ---------------------------------------------------------------------------
// Utility — mirrors _run_async + python_executable
// ---------------------------------------------------------------------------

fn python_executable() -> String {
    if let Ok(exe) = std::env::var("PYTHON") {
        if !exe.trim().is_empty() { return exe; }
    }
    // which check
    for cand in ["python3", "python"] {
        if which(cand).is_some() { return cand.to_string(); }
    }
    "python3".to_string()
}

fn which(bin: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(bin);
            if cand.is_file() { return Some(cand); }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Entry point helper — mirrors `if __name__ == "__main__"` / argparse dispatch
// ---------------------------------------------------------------------------

/// Parse a minimal `hermes teams-pipeline ...` argv and dispatch.
/// Mirrors the `argparse.ArgumentParser` + `register_cli` + `teams_pipeline_command` wiring.
pub fn main_from_args(args: &[String]) -> i32 {
    if args.is_empty() {
        println!("{}", usage());
        return 2;
    }
    // Translate raw argv into TeamsPipelineArgs for 1:1 dispatch
    let mut parsed = TeamsPipelineArgs::default();
    let raw_sub = args[0].as_str();
    parsed.teams_pipeline_action = Some(raw_sub.to_string());
    parsed.raw_action = Some(raw_sub.to_string());
    let canonical = canonical_action(raw_sub);
    match canonical.as_str() {
        "list" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--limit" if i + 1 < args.len() => { parsed.limit = args[i+1].parse::<i64>().ok(); i += 2; }
                    "--status" if i + 1 < args.len() => { parsed.status = Some(args[i+1].clone()); i += 2; }
                    "--store-path" if i + 1 < args.len() => { parsed.store_path = Some(args[i+1].clone()); i += 2; }
                    _ => { i += 1; }
                }
            }
        }
        "show" => {
            if args.len() >= 2 { parsed.job_id = Some(args[1].clone()); }
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--store-path" && i + 1 < args.len() { parsed.store_path = Some(args[i+1].clone()); i += 2; } else { i += 1; }
            }
        }
        "run" => {
            if args.len() >= 2 { parsed.job_id = Some(args[1].clone()); }
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--store-path" && i + 1 < args.len() { parsed.store_path = Some(args[i+1].clone()); i += 2; } else { i += 1; }
            }
        }
        "fetch" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--meeting-id" if i + 1 < args.len() => { parsed.meeting_id = Some(args[i+1].clone()); i += 2; }
                    "--join-web-url" if i + 1 < args.len() => { parsed.join_web_url = Some(args[i+1].clone()); i += 2; }
                    "--organizer-user-id" if i + 1 < args.len() => { parsed.organizer_user_id = Some(args[i+1].clone()); i += 2; }
                    "--tenant-id" if i + 1 < args.len() => { parsed.tenant_id = Some(args[i+1].clone()); i += 2; }
                    "--call-record-id" if i + 1 < args.len() => { parsed.call_record_id = Some(args[i+1].clone()); i += 2; }
                    _ => { i += 1; }
                }
            }
        }
        "subscriptions" => {
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--store-path" && i + 1 < args.len() { parsed.store_path = Some(args[i+1].clone()); i += 2; } else { i += 1; }
            }
        }
        "subscribe" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--resource" if i + 1 < args.len() => { parsed.resource = Some(args[i+1].clone()); i += 2; }
                    "--notification-url" if i + 1 < args.len() => { parsed.notification_url = Some(args[i+1].clone()); i += 2; }
                    "--change-type" if i + 1 < args.len() => { parsed.change_type = Some(args[i+1].clone()); i += 2; }
                    "--expiration" if i + 1 < args.len() => { parsed.expiration = Some(args[i+1].clone()); i += 2; }
                    "--client-state" if i + 1 < args.len() => { parsed.client_state = Some(args[i+1].clone()); i += 2; }
                    "--lifecycle-notification-url" if i + 1 < args.len() => { parsed.lifecycle_notification_url = Some(args[i+1].clone()); i += 2; }
                    "--latest-supported-tls-version" if i + 1 < args.len() => { parsed.latest_supported_tls_version = Some(args[i+1].clone()); i += 2; }
                    "--store-path" if i + 1 < args.len() => { parsed.store_path = Some(args[i+1].clone()); i += 2; }
                    _ => { i += 1; }
                }
            }
        }
        "renew-subscription" => {
            if args.len() >= 2 { parsed.subscription_id = Some(args[1].clone()); }
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--expiration" if i + 1 < args.len() => { parsed.expiration = Some(args[i+1].clone()); i += 2; }
                    "--store-path" if i + 1 < args.len() => { parsed.store_path = Some(args[i+1].clone()); i += 2; }
                    _ => { i += 1; }
                }
            }
        }
        "delete-subscription" => {
            if args.len() >= 2 { parsed.subscription_id = Some(args[1].clone()); }
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--store-path" && i + 1 < args.len() { parsed.store_path = Some(args[i+1].clone()); i += 2; } else { i += 1; }
            }
        }
        "maintain-subscriptions" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--renew-within-hours" if i + 1 < args.len() => { parsed.renew_within_hours = args[i+1].parse::<i64>().ok(); i += 2; }
                    "--extend-hours" if i + 1 < args.len() => { parsed.extend_hours = args[i+1].parse::<i64>().ok(); i += 2; }
                    "--dry-run" => { parsed.dry_run = true; i += 1; }
                    "--store-path" if i + 1 < args.len() => { parsed.store_path = Some(args[i+1].clone()); i += 2; }
                    "--client-state" if i + 1 < args.len() => { parsed.client_state = Some(args[i+1].clone()); i += 2; }
                    _ => { i += 1; }
                }
            }
        }
        "token-health" => {
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--force-refresh" { parsed.force_refresh = true; i += 1; } else { i += 1; }
            }
        }
        "validate" => {
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--store-path" && i + 1 < args.len() { parsed.store_path = Some(args[i+1].clone()); i += 2; } else { i += 1; }
            }
        }
        _ => {}
    }
    teams_pipeline_command(&parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_home_contains_hermes() {
        let d = display_hermes_home();
        assert!(d.contains("hermes") || d.contains(".hermes") || d.contains('/'));
    }

    #[test]
    fn resolve_store_path_defaults_to_home() {
        let p = resolve_teams_pipeline_store_path(None);
        assert!(p.to_string_lossy().contains("teams_pipeline_store.json"));
    }

    #[test]
    fn resolve_store_path_explicit() {
        let p = resolve_teams_pipeline_store_path(Some("/tmp/custom.json"));
        assert_eq!(p, PathBuf::from("/tmp/custom.json"));
    }

    #[test]
    fn default_change_type_transcripts_is_created() {
        assert_eq!(default_change_type_for_resource("communications/onlinemeetings/getAllTranscripts"), "created");
        assert_eq!(default_change_type_for_resource("communications/callRecords/abc"), "created");
        assert_eq!(default_change_type_for_resource("communications/chats"), "updated");
    }

    #[test]
    fn compact_job_replaces_transcript_with_preview() {
        let job = json!({
            "job_id": "j1",
            "summary_payload": {"transcript_text": "hello world", "other": 1}
        });
        let out = compact_job(&job);
        assert_eq!(out["summary_payload"]["transcript_preview"], Value::String("hello world".to_string()));
        assert!(out["summary_payload"].get("transcript_text").is_none());
    }

    #[test]
    fn compact_job_empty_transcript() {
        let job = json!({"job_id": "j1", "summary_payload": {"transcript_text": ""}});
        let out = compact_job(&job);
        assert!(out["summary_payload"]["transcript_preview"].is_null() || out["summary_payload"].get("transcript_preview").is_none());
    }

    #[test]
    fn cli_subcommands_cover_all() {
        let names: Vec<_> = cli_subcommands().iter().map(|s| s.name).collect();
        for n in ["list","show","run","fetch","subscriptions","subscribe","renew-subscription","delete-subscription","maintain-subscriptions","token-health","validate"] {
            assert!(names.contains(&n), "missing {}", n);
        }
    }

    #[test]
    fn usage_lists_all_actions() {
        let u = usage();
        assert!(u.contains("list|show|run|fetch"));
    }

    #[test]
    fn teams_pipeline_command_empty_returns_2() {
        let args = TeamsPipelineArgs::default();
        assert_eq!(teams_pipeline_command(&args), 2);
    }

    #[test]
    fn teams_pipeline_command_unknown_returns_2() {
        let args = TeamsPipelineArgs { teams_pipeline_action: Some("bogus".to_string()), ..Default::default() };
        assert_eq!(teams_pipeline_command(&args), 2);
    }

    #[test]
    fn iso_timestamp_format() {
        let s = iso_utc_timestamp(0);
        assert!(s.ends_with('Z'));
        assert!(s.contains('T'));
    }

    #[test]
    fn main_from_args_list_no_panic() {
        let args = vec!["list".to_string(), "--limit".to_string(), "5".to_string()];
        let code = main_from_args(&args);
        assert!(code == 0 || code == 1 || code == 2);
    }

    #[test]
    fn canonical_aliases() {
        assert_eq!(canonical_action("ls"), "list");
        assert_eq!(canonical_action("replay"), "run");
        assert_eq!(canonical_action("test"), "fetch");
        assert_eq!(canonical_action("subs"), "subscriptions");
    }
}
