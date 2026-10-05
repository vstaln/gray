//! Plugin capability declarations and operator consent.
//!
//! A plugin declares the privileged host surfaces it wants in its
//! `plugin/manifest` reply (`"capabilities": [...]`). Declaring is not
//! consent: the operator grants them once, the grant is recorded next to
//! the lock entry, and an update that declares something new is asked
//! for again. Anything undeclared or ungranted is off — a plugin must
//! probe and degrade.
//!
//! Ported from Hermes' `plugin_capabilities.py` (one id per existing
//! enforcing surface; ids without a gate are deliberately never minted).
//! Consent is an audit and audit-trail layer, not a sandbox: a plugin
//! binary still runs as the user.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::lock::LockEntry;

/// Spawn a host-owned agent turn (`host/run`) with the user's model,
/// credentials and budget.
pub const HOST_TURN: &str = "host.turn";
/// Ask the user a blocking question (`host/ask`) from inside a tool call.
pub const HOST_ASK: &str = "host.ask";
/// Print a line into the conversation (`host/say`).
pub const HOST_SAY: &str = "host.say";
/// Register a tool that shadows a built-in one of the same name.
pub const TOOL_OVERRIDE: &str = "tool.override";
/// Own the above-editor widget slot.
pub const WIDGET_OVERRIDE: &str = "widget.override";
/// Contribute `/connect` providers and receive credential material for auth RPCs.
pub const PROVIDER_CREDENTIALS: &str = "provider.credentials";

/// One declarable capability and the host surface it opens.
pub struct CapabilitySpec {
    /// Capability id, as it appears in the manifest and the consent record.
    pub id: &'static str,
    /// The `host/*` method this gates, when the surface is a host request.
    /// `None` for surfaces enforced elsewhere (tool/widget registration).
    pub host_method: Option<&'static str>,
    /// One-line risk text shown on the consent screen.
    pub description: &'static str,
}

/// The canonical registry. Only capabilities with an existing enforcing
/// surface appear here: an id nothing checks is a promise, not a gate.
const SPECS: &[CapabilitySpec] = &[
    CapabilitySpec {
        id: HOST_TURN,
        host_method: Some("host/run"),
        description: "run an agent turn as you (your model, your credentials, your budget)",
    },
    CapabilitySpec {
        id: HOST_ASK,
        host_method: Some("host/ask"),
        description: "ask you a blocking question in the middle of a tool call",
    },
    CapabilitySpec {
        id: HOST_SAY,
        host_method: Some("host/say"),
        description: "print a line into the conversation on its own",
    },
    CapabilitySpec {
        id: TOOL_OVERRIDE,
        host_method: None,
        description: "replace a built-in tool (an override intercepts everything routed through it)",
    },
    CapabilitySpec {
        id: WIDGET_OVERRIDE,
        host_method: None,
        description: "own the one above-editor widget slot",
    },
    CapabilitySpec {
        id: PROVIDER_CREDENTIALS,
        host_method: None,
        description: "contribute /connect providers and receive stored credentials for auth RPCs",
    },
];

/// Every known capability, in registry order.
pub fn all() -> &'static [CapabilitySpec] {
    SPECS
}

/// Look one up by id.
pub fn spec(id: &str) -> Option<&'static CapabilitySpec> {
    SPECS.iter().find(|s| s.id == id)
}

/// The capability a `host/*` method needs, when it needs one.
pub fn capability_for_host_method(method: &str) -> Option<&'static str> {
    SPECS
        .iter()
        .find(|s| s.host_method == Some(method))
        .map(|s| s.id)
}

/// Normalize a manifest `capabilities` list into known ids, in manifest
/// order. Unknown ids are dropped with a warning: they can never be
/// granted by this build, so keeping them off the consent screen is the
/// fail-closed choice (the plugin sees them as ungranted).
pub fn parse_declared(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in raw {
        let id = id.trim();
        if spec(id).is_none() {
            let known: Vec<&str> = SPECS.iter().map(|s| s.id).collect();
            log::warn!(
                "plugin declares unknown capability {id:?} (known: {}) — ignoring",
                known.join(", ")
            );
            continue;
        }
        if !out.iter().any(|k| k == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Stable digest of a declared list. Recorded at consent time so a later
/// manifest that declares something different is detectable and re-asked.
/// Digest of the sorted list: declaration order is not semantic.
pub fn consent_hash(declared: &[String]) -> String {
    let mut sorted: Vec<&str> = declared.iter().map(|s| s.as_str()).collect();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    for id in sorted {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    // 16 hex chars is plenty for change detection, and it is not a secret.
    digest.iter().take(8).fold(String::new(), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Which of `declared` a lock entry actually gets.
///
/// A pre-consent entry (`capabilities_hash: None`, written before consent
/// existed) is grandfathered to everything it declares: it ran with
/// those powers before, and it would otherwise stop working on upgrade.
/// A consented entry gets exactly the intersection of what it declares and
/// what was granted — a manifest that stops declaring a capability cannot
/// keep using it.
pub fn granted_for(entry: &LockEntry, declared: &[String]) -> BTreeSet<String> {
    if entry.capabilities_hash.is_none() {
        return declared.iter().cloned().collect();
    }
    entry
        .granted_capabilities
        .iter()
        .filter(|g| declared.iter().any(|d| d == *g))
        .cloned()
        .collect()
}

/// Declared capabilities that have no grant yet — the list an install or
/// update must ask about. Empty when nothing new is requested.
pub fn pending_consent(entry: &LockEntry, declared: &[String]) -> Vec<String> {
    let granted = granted_for(entry, declared);
    declared
        .iter()
        .filter(|d| !granted.contains(d.as_str()))
        .cloned()
        .collect()
}

/// True when an already-consented entry's manifest changed what it
/// declares, so the operator has to look again (update re-consent).
pub fn needs_reconsent(entry: &LockEntry, declared: &[String]) -> bool {
    match &entry.capabilities_hash {
        None => false, // grandfathered, nothing was promised
        Some(hash) => *hash != consent_hash(declared),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(granted: Vec<&str>, hash: Option<&str>) -> LockEntry {
        LockEntry {
            runtime_role: None,
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            hash: String::new(),
            source: String::new(),
            argv: vec![],
            adapter_version: "1".into(),
            installed_at: String::new(),
            scope: "user".into(),
            enabled: true,
            granted_capabilities: granted.into_iter().map(str::to_string).collect(),
            capabilities_hash: hash.map(str::to_string),
            cli_argv: None,
        }
    }

    #[test]
    fn unknown_ids_are_dropped_not_minted() {
        let raw = vec![
            "host.ask".to_string(),
            "turn".to_string(),                     // truncated
            "gateway.platform_actions".to_string(), // hermes-only id
        ];
        assert_eq!(parse_declared(&raw), vec!["host.ask".to_string()]);
    }

    #[test]
    fn host_methods_map_to_their_capability() {
        assert_eq!(capability_for_host_method("host/ask"), Some(HOST_ASK));
        assert_eq!(capability_for_host_method("host/run"), Some(HOST_TURN));
        assert_eq!(capability_for_host_method("host/say"), Some(HOST_SAY));
        assert_eq!(capability_for_host_method("tool/call"), None);
    }

    #[test]
    fn legacy_entries_keep_everything_they_declare() {
        // Written before consent existed: nothing to honour, nothing to break.
        let e = entry(vec![], None);
        assert!(granted_for(&e, &["host.ask".into(), "host.turn".into()]).contains("host.ask"));
        assert!(pending_consent(&e, &["host.ask".into()]).is_empty());
        assert!(!needs_reconsent(&e, &["host.ask".into()]));
    }

    #[test]
    fn a_consented_entry_gets_only_the_intersection() {
        let declared = vec!["host.ask".to_string(), "host.turn".to_string()];
        // Consented (hash present) but the operator granted only host.ask.
        let e = entry(vec!["host.ask"], Some(consent_hash(&declared).as_str()));
        let granted = granted_for(&e, &declared);
        assert!(granted.contains("host.ask"));
        assert!(!granted.contains("host.turn"));
        assert_eq!(
            pending_consent(&e, &declared),
            vec!["host.turn".to_string()]
        );
    }

    #[test]
    fn reconsent_when_the_declared_set_moves() {
        let declared = vec!["host.ask".to_string()];
        let e = entry(vec!["host.ask"], Some(consent_hash(&declared).as_str()));
        assert!(!needs_reconsent(&e, &declared));
        // Same ids, different order: not a semantic change.
        let reordered = vec!["host.ask".to_string()];
        assert!(!needs_reconsent(&e, &reordered));
        // A new capability in the manifest re-opens the question.
        let wider = vec!["host.ask".to_string(), "host.turn".to_string()];
        assert!(needs_reconsent(&e, &wider));
        assert_eq!(pending_consent(&e, &wider), vec!["host.turn".to_string()]);
    }

    #[test]
    fn dropping_a_declaration_removes_the_power() {
        let granted_at_install = vec!["host.ask".to_string(), "host.turn".to_string()];
        let hash = consent_hash(&granted_at_install);
        let e = entry(vec!["host.ask", "host.turn"], Some(hash.as_str()));
        // Manifest no longer declares host.turn: it cannot use it.
        let declared = vec!["host.ask".to_string()];
        assert!(!granted_for(&e, &declared).contains("host.turn"));
        // And the drift is reported so the operator can re-consent.
        assert!(needs_reconsent(&e, &declared));
    }
}
