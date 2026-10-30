use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::lock::LockEntry;

pub const HOST_TURN: &str = "host.turn";

pub const HOST_ASK: &str = "host.ask";

pub const HOST_SAY: &str = "host.say";

pub const HOST_RESTART: &str = "host.restart";

pub const HOST_SESSIONS: &str = "host.sessions";

pub const TOOL_OVERRIDE: &str = "tool.override";

pub const WIDGET_OVERRIDE: &str = "widget.override";

pub const PROVIDER_CREDENTIALS: &str = "provider.credentials";

pub const SHELL_ENV: &str = "shell.env";

pub struct CapabilitySpec {
    pub id: &'static str,

    pub host_method: Option<&'static str>,

    pub description: &'static str,
}

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
        id: HOST_RESTART,
        host_method: Some("host/restart"),
        description: "restart gray on the binary on disk (resuming this conversation)",
    },
    CapabilitySpec {
        id: HOST_SESSIONS,
        host_method: Some("host/sessions"),
        description: "see which gray sessions are running (ids and pids)",
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
    CapabilitySpec {
        id: SHELL_ENV,
        host_method: None,
        description: "contribute environment variables and PATH prepends to the bash tool's shell",
    },
];

pub fn all() -> &'static [CapabilitySpec] {
    SPECS
}

pub fn spec(id: &str) -> Option<&'static CapabilitySpec> {
    SPECS.iter().find(|s| s.id == id)
}

pub fn capability_for_host_method(method: &str) -> Option<&'static str> {
    SPECS
        .iter()
        .find(|s| s.host_method == Some(method))
        .map(|s| s.id)
}

pub fn parse_declared(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in raw {
        let id = id.trim();
        if spec(id).is_none() {
            // Manifests are parsed more than once per process; say it once.
            static WARNED: std::sync::Mutex<BTreeSet<String>> =
                std::sync::Mutex::new(BTreeSet::new());
            if !WARNED.lock().is_ok_and(|mut w| w.insert(id.to_string())) {
                continue;
            }
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

pub fn consent_hash(declared: &[String]) -> String {
    let mut sorted: Vec<&str> = declared.iter().map(|s| s.as_str()).collect();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    for id in sorted {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();

    digest.iter().take(8).fold(String::new(), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Everything a manifest declares is granted: the declaration in the
/// manifest is the consent surface (shown at install/update). A plugin can
/// still not reach a host method it did not declare — those calls keep
/// failing `capability_not_granted`. The lock entry only feeds the
/// `plugin capabilities` status display now.
pub fn granted_for(_entry: &LockEntry, declared: &[String]) -> BTreeSet<String> {
    declared.iter().cloned().collect()
}

pub fn needs_reconsent(entry: &LockEntry, declared: &[String]) -> bool {
    match &entry.capabilities_hash {
        None => false,
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
            global: false,
            extra: Default::default(),
        }
    }

    #[test]
    fn unknown_ids_are_dropped_not_minted() {
        let raw = vec![
            "host.ask".to_string(),
            "turn".to_string(),
            "gateway.platform_actions".to_string(),
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
    fn declared_capabilities_are_all_granted() {
        let declared = vec!["host.ask".to_string(), "host.turn".to_string()];
        // Whatever the lock recorded — old grants, a mismatched hash, none —
        // the manifest declaration is the consent surface.
        for e in [
            entry(vec![], None),
            entry(vec!["host.ask"], Some(consent_hash(&declared).as_str())),
            entry(vec![], Some("stale")),
        ] {
            let granted = granted_for(&e, &declared);
            assert!(granted.contains("host.ask"));
            assert!(granted.contains("host.turn"));
        }
        // Undeclared powers stay out.
        assert!(!granted_for(&entry(vec![], None), &[]).contains("host.ask"));
    }

    #[test]
    fn needs_reconsent_tracks_drift_for_the_status_display() {
        let declared = vec!["host.ask".to_string()];
        let e = entry(vec!["host.ask"], Some(consent_hash(&declared).as_str()));
        assert!(!needs_reconsent(&e, &declared));
        assert!(!needs_reconsent(&e, &["host.ask".to_string()]));

        let wider = vec!["host.ask".to_string(), "host.turn".to_string()];
        assert!(needs_reconsent(&e, &wider));

        let e = entry(vec![], None);
        assert!(!needs_reconsent(&e, &declared));
    }
}
