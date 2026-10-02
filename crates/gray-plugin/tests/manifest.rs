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
        ..Manifest::default()
    }
}

fn manifest_b() -> Manifest {
    Manifest {
        name: "plugin-b".to_string(),
        version: "0.1.0".to_string(),
        tools: vec![tool_def("read")],
        ..Manifest::default()
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
fn subcommands_parse_lenient() {
    let v = serde_json::json!({
        "name": "cron", "version": "0.1.0", "tools": [],
        "subcommands": ["/cron"],
    });
    let m = Manifest::from_result(&v);
    assert_eq!(m.subcommands, vec!["/cron".to_string()]);
    // Absent → empty (pre-v1 sidecars keep working).
    let m2 = Manifest::from_result(&serde_json::json!({"name": "x", "tools": []}));
    assert!(m2.subcommands.is_empty());
}

#[test]
fn tool_label_parses_and_stays_model_invisible() {
    // A manifest `label` names the transcript headline; providers project
    // name/description/parameters only, so it never reaches the model.
    let v = serde_json::json!({
        "name": "x", "version": "0.1.0",
        "tools": [{"name": "notify_owner", "description": "d",
                   "parameters": {"type": "object"}, "label": "Notify Owner"}],
    });
    let m = Manifest::from_result(&v);
    assert_eq!(m.tools[0].label.as_deref(), Some("Notify Owner"));
    // Blank labels do not stick.
    let v2 = serde_json::json!({
        "name": "x", "version": "0.1.0",
        "tools": [{"name": "t", "description": "d",
                   "parameters": {"type": "object"}, "label": "  "}],
    });
    assert!(Manifest::from_result(&v2).tools[0].label.is_none());
    // `None` skips serialization: persisted payloads stay byte-stable.
    let def = gray_core::message::ToolDef::new("t", "d", serde_json::json!({}));
    assert!(
        !serde_json::to_value(&def)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("label")
    );
}
