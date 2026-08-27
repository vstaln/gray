//! Honcho declared config surface — rendered by the generic desktop panel.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/memory/honcho/config_schema.py` (324 LOC).
//! Pure declaration: the fields, their types, secrets, and select options.
//! Mirrors `plugins.memory.config_schema` types + `plugins.memory.honcho.config_schema` values.
//!
//! Python surface ported line-for-line:
//!   - `KIND_*` / `STORAGE_*` constants (config_schema.py:30-41)
//!   - `ProviderFieldOption` / `ProviderField` / `ProviderConfigSchema` dataclasses (43-107)
//!   - `_REASONING_LEVELS` + `CONFIG_SCHEMA` with all 28 fields (17-324)
//!   - `get_provider_config_schema(name)` cache (109-144) — cache is global memo keyed on path in Python;
//!     Rust equivalent is pure `honcho_config_schema()` + `get_provider_config_schema(name)` stub.
//!
//! Storage note: `STORAGE_HONCHO_HOST_BLOCK` dispatches in `web_server` to the
//! `hosts.<host>` block of `honcho.json` (see `reference/.../honcho/client.py:_host_block`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Kind / storage constants — mirrors config_schema.py:30-41
// ---------------------------------------------------------------------------

pub const KIND_TEXT: &str = "text";
pub const KIND_SELECT: &str = "select";
pub const KIND_SECRET: &str = "secret";
pub const KIND_BOOL: &str = "bool";
pub const KIND_NUMBER: &str = "number";
pub const KIND_JSON: &str = "json";

pub const STORAGE_FLAT_JSON: &str = "flat_json";
pub const STORAGE_HONCHO_HOST_BLOCK: &str = "honcho_host_block";

// ---------------------------------------------------------------------------
// ProviderFieldOption — mirrors @dataclass(frozen=True) ProviderFieldOption (43-49)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFieldOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

impl ProviderFieldOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), description: String::new() }
    }
    pub fn with_description(value: impl Into<String>, label: impl Into<String>, description: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), description: description.into() }
    }
}

// ---------------------------------------------------------------------------
// ProviderField — mirrors @dataclass(frozen=True) ProviderField (52-92)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderField {
    pub key: String,
    pub label: String,
    #[serde(default = "default_kind_text")]
    pub kind: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub options: Vec<ProviderFieldOption>,
    #[serde(default)]
    pub env_key: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub env_fallbacks: Vec<String>,
    #[serde(default)]
    pub inline: bool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub info: String,
    #[serde(default = "default_scope_host")]
    pub scope: String,
}

fn default_kind_text() -> String { KIND_TEXT.to_string() }
fn default_scope_host() -> String { "host".to_string() }

impl ProviderField {
    /// Mirrors `ProviderField.is_secret` (87-88): `kind == KIND_SECRET`.
    pub fn is_secret(&self) -> bool {
        self.kind == KIND_SECRET
    }
    /// Mirrors `ProviderField.allowed_values()` (90-92).
    pub fn allowed_values(&self) -> std::collections::HashSet<String> {
        self.options.iter().map(|o| o.value.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// ProviderConfigSchema — mirrors @dataclass(frozen=True) ProviderConfigSchema (94-107)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigSchema {
    pub name: String,
    pub label: String,
    #[serde(default = "default_storage_flat")]
    pub storage: String,
    #[serde(default)]
    pub docs_url: String,
    #[serde(default)]
    pub fields: Vec<ProviderField>,
}

fn default_storage_flat() -> String { STORAGE_FLAT_JSON.to_string() }

impl ProviderConfigSchema {
    /// Mirrors `inline_fields()` (105-107).
    pub fn inline_fields(&self) -> Vec<&ProviderField> {
        self.fields.iter().filter(|f| f.inline).collect()
    }
}

// ---------------------------------------------------------------------------
// _REASONING_LEVELS — mirrors config_schema.py:17-24
// ---------------------------------------------------------------------------

pub fn reasoning_levels() -> Vec<ProviderFieldOption> {
    vec![
        ProviderFieldOption::new("minimal", "Minimal"),
        ProviderFieldOption::new("low", "Low"),
        ProviderFieldOption::new("medium", "Medium"),
        ProviderFieldOption::new("high", "High"),
        ProviderFieldOption::new("max", "Max"),
    ]
}

// ---------------------------------------------------------------------------
// CONFIG_SCHEMA — mirrors config_schema.py:27-324
// ---------------------------------------------------------------------------

/// Mirrors `CONFIG_SCHEMA = ProviderConfigSchema(name="honcho", ...)` lines 27-324.
///
/// Returns the owned schema; call once and clone as needed (mirrors Python module singleton).
pub fn honcho_config_schema() -> ProviderConfigSchema {
    let rl = reasoning_levels();
    ProviderConfigSchema {
        name: "honcho".to_string(),
        label: "Honcho".to_string(),
        storage: STORAGE_HONCHO_HOST_BLOCK.to_string(),
        docs_url: "https://docs.honcho.dev/v3/guides/integrations/hermes".to_string(),
        fields: vec![
            // — Connection —
            ProviderField {
                key: "apiKey".to_string(),
                label: "API key".to_string(),
                kind: KIND_SECRET.to_string(),
                default: String::new(),
                description: "Authenticate with Honcho Cloud. Not needed for a self-hosted base URL.".to_string(),
                placeholder: "Enter Honcho API key".to_string(),
                options: vec![],
                env_key: Some("HONCHO_API_KEY".to_string()),
                aliases: vec![],
                env_fallbacks: vec![],
                inline: true,
                group: "Connection".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "baseUrl".to_string(),
                label: "Base URL".to_string(),
                kind: KIND_TEXT.to_string(),
                default: String::new(),
                description: "Self-hosted Honcho URL. Overrides the environment when set.".to_string(),
                placeholder: "https://… (self-hosted)".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec!["base_url".to_string()],
                env_fallbacks: vec!["HONCHO_BASE_URL".to_string()],
                inline: true,
                group: "Connection".to_string(),
                info: String::new(),
                scope: "root".to_string(),
            },
            ProviderField {
                key: "environment".to_string(),
                label: "Environment".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "production".to_string(),
                description: "Honcho environment. Ignored when a base URL is set.".to_string(),
                placeholder: String::new(),
                options: vec![
                    ProviderFieldOption::new("production", "Cloud"),
                    ProviderFieldOption::new("local", "Local"),
                ],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec!["HONCHO_ENVIRONMENT".to_string()],
                inline: true,
                group: "Connection".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "workspace".to_string(),
                label: "Workspace".to_string(),
                kind: KIND_TEXT.to_string(),
                default: String::new(),
                description: "Honcho workspace ID. Defaults to the profile host.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: true,
                group: "Connection".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Identity —
            ProviderField {
                key: "peerName".to_string(),
                label: "Peer name".to_string(),
                kind: KIND_TEXT.to_string(),
                default: String::new(),
                description: "Your stable user peer. Unifies memory across platforms for single-user setups.".to_string(),
                placeholder: "e.g. eri".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: true,
                group: "Identity".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "aiPeer".to_string(),
                label: "AI peer".to_string(),
                kind: KIND_TEXT.to_string(),
                default: String::new(),
                description: "The AI-side peer name. Defaults to the profile host.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: true,
                group: "Identity".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Session —
            ProviderField {
                key: "sessionStrategy".to_string(),
                label: "Session strategy".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "per-directory".to_string(),
                description: "How conversations map to Honcho sessions.".to_string(),
                placeholder: String::new(),
                options: vec![
                    ProviderFieldOption::new("per-session", "Per session"),
                    ProviderFieldOption::new("per-directory", "Per directory"),
                    ProviderFieldOption::new("per-repo", "Per repo"),
                    ProviderFieldOption::new("global", "Global"),
                ],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: true,
                group: "Session".to_string(),
                info: "Per session: every conversation gets its own Honcho session. Per directory: conversations from the same working directory share one. Per repo: conversations from the same git repo share one. Global: everything shares a single session.".to_string(),
                scope: "host".to_string(),
            },
            // —————— Full-config-only fields below (inline=False) ——————
            // — Connection —
            ProviderField {
                key: "timeout".to_string(),
                label: "Request timeout".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Request timeout in seconds for Honcho HTTP calls. Blank uses the default.".to_string(),
                placeholder: "30".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec!["requestTimeout".to_string()],
                env_fallbacks: vec!["HONCHO_TIMEOUT".to_string()],
                inline: false,
                group: "Connection".to_string(),
                info: String::new(),
                scope: "root".to_string(),
            },
            // — Identity —
            ProviderField {
                key: "pinUserPeer".to_string(),
                label: "Pin user peer".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "false".to_string(),
                description: "Pin the user peer to the peer name, ignoring gateway runtime identity. Unifies memory for single-user setups.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec!["pinPeerName".to_string()],
                env_fallbacks: vec![],
                inline: false,
                group: "Identity".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "runtimePeerPrefix".to_string(),
                label: "Runtime peer prefix".to_string(),
                kind: KIND_TEXT.to_string(),
                default: String::new(),
                description: "Prefix applied to unknown gateway runtime user IDs.".to_string(),
                placeholder: "e.g. telegram_".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Identity".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "userPeerAliases".to_string(),
                label: "User peer aliases".to_string(),
                kind: KIND_JSON.to_string(),
                default: String::new(),
                description: "Map gateway runtime user IDs to stable Honcho peers.".to_string(),
                placeholder: "{\"telegram_123\": \"eri\"}".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Identity".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Session —
            ProviderField {
                key: "sessionPeerPrefix".to_string(),
                label: "Session peer prefix".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "false".to_string(),
                description: "Prefix session peer names with the host.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Session".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "sessions".to_string(),
                label: "Session overrides".to_string(),
                kind: KIND_JSON.to_string(),
                default: String::new(),
                description: "Explicit session ID overrides keyed by resolver.".to_string(),
                placeholder: "{\"key\": \"session-id\"}".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Session".to_string(),
                info: String::new(),
                scope: "root".to_string(),
            },
            // — Message writing —
            ProviderField {
                key: "saveMessages".to_string(),
                label: "Save messages".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "true".to_string(),
                description: "Persist conversation messages to Honcho.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Message writing".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "writeFrequency".to_string(),
                label: "Write frequency".to_string(),
                kind: KIND_TEXT.to_string(),
                default: "async".to_string(),
                description: "When to flush messages: async, turn, session, or every N turns.".to_string(),
                placeholder: "async | turn | session | N".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Message writing".to_string(),
                info: "async: write in the background as messages arrive. turn: flush after each turn. session: flush when the session ends. A number N flushes every N turns.".to_string(),
                scope: "host".to_string(),
            },
            // — Dialectic —
            ProviderField {
                key: "dialecticReasoningLevel".to_string(),
                label: "Reasoning level".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "low".to_string(),
                description: "Reasoning effort for dialectic (peer.chat) calls.".to_string(),
                placeholder: String::new(),
                options: rl.clone(),
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "dialecticDynamic".to_string(),
                label: "Dynamic reasoning".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "true".to_string(),
                description: "Let the model override the reasoning level per call.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "dialecticMaxChars".to_string(),
                label: "Max result chars".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Max chars of dialectic result injected into the system prompt.".to_string(),
                placeholder: "1200".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "dialecticDepth".to_string(),
                label: "Depth".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Dialectic passes per cycle (1–3).".to_string(),
                placeholder: "1".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "dialecticDepthLevels".to_string(),
                label: "Per-pass levels".to_string(),
                kind: KIND_JSON.to_string(),
                default: String::new(),
                description: "Reasoning level per pass; array length matches depth.".to_string(),
                placeholder: "[\"low\", \"medium\"]".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "dialecticMaxInputChars".to_string(),
                label: "Max input chars".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Max chars of query input sent to peer.chat().".to_string(),
                placeholder: "10000".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Dialectic".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Reasoning —
            ProviderField {
                key: "reasoningHeuristic".to_string(),
                label: "Reasoning heuristic".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "true".to_string(),
                description: "Scale the reasoning level up on longer queries.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Reasoning".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "reasoningLevelCap".to_string(),
                label: "Reasoning level cap".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "high".to_string(),
                description: "Ceiling for the heuristic-selected reasoning level.".to_string(),
                placeholder: String::new(),
                options: rl.clone(),
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Reasoning".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Recall —
            ProviderField {
                key: "recallMode".to_string(),
                label: "Recall mode".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "hybrid".to_string(),
                description: "How memory retrieval works: hybrid, context-only, or tools-only.".to_string(),
                placeholder: String::new(),
                options: vec![
                    ProviderFieldOption::new("hybrid", "Hybrid"),
                    ProviderFieldOption::new("context", "Context only"),
                    ProviderFieldOption::new("tools", "Tools only"),
                ],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Recall".to_string(),
                info: "Hybrid: auto-injected context plus on-demand memory tools. Context only: injection without tools. Tools only: the model queries memory explicitly, nothing is injected.".to_string(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "contextTokens".to_string(),
                label: "Context token cap".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Cap on auto-injected context tokens. Blank leaves it uncapped.".to_string(),
                placeholder: "(uncapped)".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Recall".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            ProviderField {
                key: "initOnSessionStart".to_string(),
                label: "Eager init".to_string(),
                kind: KIND_BOOL.to_string(),
                default: "false".to_string(),
                description: "Initialize the session eagerly in tools mode instead of on first tool call.".to_string(),
                placeholder: String::new(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Recall".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Limits —
            ProviderField {
                key: "messageMaxChars".to_string(),
                label: "Message max chars".to_string(),
                kind: KIND_NUMBER.to_string(),
                default: String::new(),
                description: "Max chars per message sent to Honcho.".to_string(),
                placeholder: "25000".to_string(),
                options: vec![],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Limits".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
            // — Observation —
            ProviderField {
                key: "observationMode".to_string(),
                label: "Observation mode".to_string(),
                kind: KIND_SELECT.to_string(),
                default: "directional".to_string(),
                description: "Per-peer observation preset. Directional observes all directions; unified shares one view.".to_string(),
                placeholder: String::new(),
                options: vec![
                    ProviderFieldOption::new("directional", "Directional"),
                    ProviderFieldOption::new("unified", "Unified"),
                ],
                env_key: None,
                aliases: vec![],
                env_fallbacks: vec![],
                inline: false,
                group: "Observation".to_string(),
                info: String::new(),
                scope: "host".to_string(),
            },
        ],
    }
}

/// Mirrors Python's `CONFIG_SCHEMA` module singleton.
pub fn config_schema() -> ProviderConfigSchema {
    honcho_config_schema()
}

/// Mirrors `get_provider_config_schema(name)` (112-144) — returns honcho schema for "honcho", else None.
/// Python caches on `str(path)`; Rust is pure and has no filesystem load.
pub fn get_provider_config_schema(name: &str) -> Option<ProviderConfigSchema> {
    if name == "honcho" {
        Some(honcho_config_schema())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_and_storage() {
        let s = honcho_config_schema();
        assert_eq!(s.name, "honcho");
        assert_eq!(s.label, "Honcho");
        assert_eq!(s.storage, STORAGE_HONCHO_HOST_BLOCK);
        assert_eq!(s.docs_url, "https://docs.honcho.dev/v3/guides/integrations/hermes");
    }

    #[test]
    fn field_count_matches_python() {
        let s = honcho_config_schema();
        assert_eq!(s.fields.len(), 28);
    }

    #[test]
    fn inline_fields_are_seven() {
        let s = honcho_config_schema();
        let inline = s.inline_fields();
        assert_eq!(inline.len(), 7);
        let keys: Vec<&str> = inline.iter().map(|f| f.key.as_str()).collect();
        assert!(keys.contains(&"apiKey"));
        assert!(keys.contains(&"baseUrl"));
        assert!(keys.contains(&"environment"));
        assert!(keys.contains(&"workspace"));
        assert!(keys.contains(&"peerName"));
        assert!(keys.contains(&"aiPeer"));
        assert!(keys.contains(&"sessionStrategy"));
    }

    #[test]
    fn reasoning_levels_shape() {
        let levels = reasoning_levels();
        assert_eq!(levels.len(), 5);
        assert_eq!(levels[0].value, "minimal");
        assert_eq!(levels[4].value, "max");
    }

    #[test]
    fn secret_detection() {
        let s = honcho_config_schema();
        let api = s.fields.iter().find(|f| f.key == "apiKey").unwrap();
        assert!(api.is_secret());
        assert_eq!(api.env_key.as_deref(), Some("HONCHO_API_KEY"));
        let base = s.fields.iter().find(|f| f.key == "baseUrl").unwrap();
        assert!(!base.is_secret());
        assert_eq!(base.aliases, vec!["base_url"]);
        assert_eq!(base.env_fallbacks, vec!["HONCHO_BASE_URL"]);
        assert_eq!(base.scope, "root");
    }

    #[test]
    fn select_options_and_defaults() {
        let s = honcho_config_schema();
        let env = s.fields.iter().find(|f| f.key == "environment").unwrap();
        assert_eq!(env.default, "production");
        assert_eq!(env.allowed_values(), ["production", "local"].into_iter().map(String::from).collect());
        let recall = s.fields.iter().find(|f| f.key == "recallMode").unwrap();
        assert_eq!(recall.default, "hybrid");
        assert!(recall.allowed_values().contains("tools"));
        let strat = s.fields.iter().find(|f| f.key == "sessionStrategy").unwrap();
        assert_eq!(strat.default, "per-directory");
        assert!(strat.info.contains("Per session"));
    }

    #[test]
    fn get_provider_config_schema_gate() {
        assert!(get_provider_config_schema("honcho").is_some());
        assert!(get_provider_config_schema("mem0").is_none());
        assert!(get_provider_config_schema("").is_none());
    }
}
