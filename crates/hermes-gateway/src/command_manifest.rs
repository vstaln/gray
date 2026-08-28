//! Gateway-declared slash-command manifest for the relay lane (Phase 4).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/relay/command_manifest.py` (145 LOC).
//!
//! The native Discord adapter registers its slash commands directly on the
//! Discord command tree (`_register_slash_commands`,
//! plugins/platforms/discord/adapter.py) — it holds the bot token. Over the
//! relay the CONNECTOR holds the token, so the gateway DECLARES the same
//! command set on its `hello` frame (`command_manifest`) and the connector
//! reconciles Discord's global application-command registration against it
//! (gateway-gateway `DiscordCommandRegistrar`: GET → diff → bulk PUT,
//! idempotent, best-effort).
//!
//! This module is that declaration: the single source of truth for what the
//! relay lane advertises. It MIRRORS the native tree — same names, same
//! descriptions — so a user moving between a native-Discord deployment and a
//! hosted/relay one sees the same command palette. Interactions come back over
//! the passthrough plane and are normalized by
//! RelayAdapter._discord_interaction_to_event into the same "/name args"
//! COMMAND events the dispatcher already routes, so declaring a command here
//! requires NO new handler — the dispatcher's existing slash surface is the
//! handler.
//!
//! Wire shape (per entry): {name, description, options?} where options rows are
//! Discord option objects passed through verbatim. Names must satisfy
//! Discord's CHAT_INPUT rules ([a-z0-9_-]{1,32}); the connector drops invalid
//! entries (fail-open per entry, never the whole manifest).
//!
//! Python source docstring (preserved):
//! ```text
//! Gateway-declared slash-command manifest for the relay lane (Phase 4).
//!
//! The native Discord adapter registers its slash commands directly on the
//! Discord command tree (`_register_slash_commands`,
//! plugins/platforms/discord/adapter.py) — it holds the bot token. Over the
//! relay the CONNECTOR holds the token, so the gateway DECLARES the same
//! command set on its `hello` frame (`command_manifest`) and the connector
//! reconciles Discord's global application-command registration against it
//! (gateway-gateway `DiscordCommandRegistrar`: GET → diff → bulk PUT,
//! idempotent, best-effort).
//!
//! This module is that declaration: the single source of truth for what the
//! relay lane advertises. It MIRRORS the native tree — same names, same
//! descriptions — so a user moving between a native-Discord deployment and a
//! hosted/relay one sees the same command palette. Interactions come back over
//! the passthrough plane and are normalized by
//! RelayAdapter._discord_interaction_to_event into the same "/name args"
//! COMMAND events the dispatcher already routes, so declaring a command here
//! requires NO new handler — the dispatcher's existing slash surface is the
//! handler.
//!
//! Wire shape (per entry): {name, description, options?} where options rows are
//! Discord option objects passed through verbatim. Names must satisfy
//! Discord's CHAT_INPUT rules ([a-z0-9_-]{1,32}); the connector drops invalid
//! entries (fail-open per entry, never the whole manifest).
//! ```

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Discord option type 3 = STRING. Mirrors `_STR = 3`.
pub const _STR: i32 = 3;

/// Alias for readability.
pub const DISCORD_OPTION_TYPE_STRING: i32 = _STR;

// ---------------------------------------------------------------------------
// Wire types — mirrors `Dict[str, Any]` shapes in Python
// ---------------------------------------------------------------------------

/// A single choice entry inside a STRING option. Mirrors `{"name": c, "value": c}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandChoice {
    pub name: String,
    pub value: String,
}

/// A Discord application-command option. Mirrors the dict returned by `_opt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOption {
    /// Discord option type. Always `3` (STRING) in this manifest.
    #[serde(rename = "type")]
    pub r#type: i32,
    pub name: String,
    pub description: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<CommandChoice>>,
}

/// A single slash-command declaration. Mirrors `{name, description, options?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandManifestEntry {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<CommandOption>>,
}

// ---------------------------------------------------------------------------
// Internal helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

/// Build a STRING option row. Mirrors `_opt(name, description, *, choices) -> Dict[str, Any]`.
pub fn _opt(name: &str, description: &str, choices: Option<&[&str]>) -> CommandOption {
    CommandOption {
        r#type: _STR,
        name: name.to_string(),
        description: description.to_string(),
        required: false,
        choices: choices.map(|cs| {
            cs.iter()
                .map(|c| CommandChoice {
                    name: (*c).to_string(),
                    value: (*c).to_string(),
                })
                .collect()
        }),
    }
}

// Private alias for traceability (mirrors Python's `_opt` exactly)
#[allow(dead_code)]
fn _opt_choice(name: &str, value: &str) -> CommandChoice {
    CommandChoice {
        name: name.to_string(),
        value: value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions
// ---------------------------------------------------------------------------

/// The relay lane's Discord slash-command manifest (native-tree mirror).
///
/// Mirrors `build_relay_command_manifest() -> List[Dict[str, Any]]`.
/// Returns the 27 commands the gateway declares on its `hello` frame.
pub fn build_relay_command_manifest() -> Vec<CommandManifestEntry> {
    vec![
        CommandManifestEntry {
            name: "new".to_string(),
            description: "Start a new conversation".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "reset".to_string(),
            description: "Reset your Hermes session".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "model".to_string(),
            description: "Show or change the model".to_string(),
            options: Some(vec![_opt(
                "name",
                "Model name. Leave empty to see current.",
                None,
            )]),
        },
        CommandManifestEntry {
            name: "reasoning".to_string(),
            description: "Show/change reasoning effort, or toggle showing it".to_string(),
            options: Some(vec![_opt(
                "effort",
                "Level, reset, or show/hide. Leave empty to see current.",
                Some(&[
                    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "reset",
                    "show", "hide",
                ]),
            )]),
        },
        CommandManifestEntry {
            name: "personality".to_string(),
            description: "Set a personality".to_string(),
            options: Some(vec![_opt(
                "name",
                "Personality name. Leave empty to list.",
                None,
            )]),
        },
        CommandManifestEntry {
            name: "retry".to_string(),
            description: "Retry your last message".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "undo".to_string(),
            description: "Remove the last exchange".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "status".to_string(),
            description: "Show Hermes session status".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "sethome".to_string(),
            description: "Set this chat as the home channel".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "stop".to_string(),
            description: "Stop the running Hermes agent".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "steer".to_string(),
            description: "Inject a message after the next tool call (no interrupt)".to_string(),
            options: Some(vec![_opt("text", "What to tell the agent", None)]),
        },
        CommandManifestEntry {
            name: "compress".to_string(),
            description: "Compress conversation context".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "title".to_string(),
            description: "Set or show the session title".to_string(),
            options: Some(vec![_opt("text", "New title. Leave empty to show.", None)]),
        },
        CommandManifestEntry {
            name: "resume".to_string(),
            description: "Resume a previously-named session".to_string(),
            options: Some(vec![_opt("name", "Session title or id", None)]),
        },
        CommandManifestEntry {
            name: "usage".to_string(),
            description: "Show token usage for this session".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "help".to_string(),
            description: "Show available commands".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "insights".to_string(),
            description: "Show usage insights and analytics".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "reload-mcp".to_string(),
            description: "Reload MCP servers from config".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "reload-skills".to_string(),
            description: "Re-scan skills for new or removed entries".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "voice".to_string(),
            description: "Toggle voice reply mode".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "update".to_string(),
            description: "Update Hermes Agent to the latest version".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "restart".to_string(),
            description: "Gracefully restart the Hermes gateway".to_string(),
            options: None,
        },
        CommandManifestEntry {
            name: "approve".to_string(),
            description: "Approve a pending dangerous command".to_string(),
            options: Some(vec![_opt(
                "scope",
                "Approval scope",
                Some(&["once", "session", "always", "all"]),
            )]),
        },
        CommandManifestEntry {
            name: "deny".to_string(),
            description: "Deny a pending dangerous command".to_string(),
            options: Some(vec![_opt("reason", "Why (relayed to the agent)", None)]),
        },
        CommandManifestEntry {
            name: "thread".to_string(),
            description: "Create a new thread and start a Hermes session in it".to_string(),
            options: Some(vec![_opt("name", "Thread name", None)]),
        },
        CommandManifestEntry {
            name: "queue".to_string(),
            description: "Queue a prompt for the next turn (doesn't interrupt)".to_string(),
            options: Some(vec![_opt("text", "The prompt to queue", None)]),
        },
        CommandManifestEntry {
            name: "background".to_string(),
            description: "Run a prompt in the background".to_string(),
            options: Some(vec![_opt("text", "The prompt to run", None)]),
        },
    ]
}

/// `serde_json::Value` variant — returns the wire JSON array directly.
///
/// Convenience for callers that need `Vec<Value>` (e.g. hello-frame serialization)
/// without an extra `serde_json::to_value` step. Mirrors Python's `List[Dict[str, Any]]`.
pub fn build_relay_command_manifest_value() -> Vec<serde_json::Value> {
    build_relay_command_manifest()
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
        .collect()
}
