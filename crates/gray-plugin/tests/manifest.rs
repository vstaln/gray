use gray_core::message::ToolDef;
use gray_plugin::{Manifest, merge_manifests};

fn tool_def(name: &str) -> ToolDef {
    ToolDef::new(name, format!("sidecar tool {name}"), serde_json::json!({}))
}

fn manifest_a() -> Manifest {
    Manifest {
        name: "plugin-a".to_string(),
        version: "0.1.0".to_string(),
        tools: vec![tool_def("read")],
        commands: vec![],
        hooks: vec![],
        provider: None,
        protocol: None,
        capabilities: vec![],
        subcommands: vec![],
    }
}

fn manifest_b() -> Manifest {
    Manifest {
        name: "plugin-b".to_string(),
        version: "0.1.0".to_string(),
        tools: vec![tool_def("read")],
        commands: vec![],
        hooks: vec![],
        provider: None,
        protocol: None,
        capabilities: vec![],
        subcommands: vec![],
    }
}

#[test]
fn later_entry_wins_on_name_conflict() {
    let merged = merge_manifests(vec![manifest_a(), manifest_b()]);
    assert_eq!(merged["read"], "plugin-b");
}

#[test]
fn manifest_result_with_tool_schema_round_trips_def() {
    let v = serde_json::json!({
        "name": "echo",
        "version": "0.1.0",
        "tools": [{"name": "echo", "description": "Echo text back",
                   "parameters": {"type": "object"},
                   "snippet": "echo <text> — echo text back"}],
        "commands": ["/echo"],
        "hooks": ["prompt/context", "tool/before", "turn/end"],
    });
    let m = Manifest::from_result(&v);
    assert_eq!(m.name, "echo");
    assert_eq!(m.commands, vec!["/echo".to_string()]);
    assert_eq!(m.hooks.len(), 3);
    assert_eq!(m.tools.len(), 1);
    let def = &m.tools[0];
    assert_eq!(def.name, "echo");
    assert_eq!(def.description, "Echo text back");
    assert_eq!(def.parameters, serde_json::json!({"type": "object"}));
}

#[test]
fn legacy_string_tool_entries_still_parse() {
    // Pre-v1 sidecars send `"tools": ["echo"]` — keep working.
    let v = serde_json::json!({"name": "echo", "version": "0.1.0", "tools": ["echo"]});
    let m = Manifest::from_result(&v);
    assert_eq!(m.tools.len(), 1);
    assert_eq!(m.tools[0].name, "echo");
    assert!(m.commands.is_empty() && m.hooks.is_empty());
}

#[test]
fn capabilities_and_subcommands_parse_lenient() {
    let v = serde_json::json!({
        "name": "cron", "version": "0.1.0", "tools": [],
        "capabilities": ["session", "bogus-cap"],
        "subcommands": ["/cron"],
    });
    let m = Manifest::from_result(&v);
    assert_eq!(m.capabilities, vec!["session".to_string(), "bogus-cap".to_string()]);
    assert_eq!(m.subcommands, vec!["/cron".to_string()]);
    // Absent → empty (pre-v1 sidecars keep working).
    let m2 = Manifest::from_result(&serde_json::json!({"name": "x", "tools": []}));
    assert!(m2.capabilities.is_empty() && m2.subcommands.is_empty());
}
