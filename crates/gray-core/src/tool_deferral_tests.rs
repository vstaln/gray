use super::*;

fn def(name: &str, desc: &str) -> ToolDef {
    ToolDef::new(name, desc, serde_json::json!({"type": "object"}))
}

fn names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

fn call(name: &str, args: serde_json::Value) -> Message {
    Message {
        role: Role::Assistant,
        injected: false,
        content: vec![ContentBlock::ToolUse {
            id: "c1".into(),
            name: name.into(),
            args,
            thought_signature: None,
        }],
    }
}

fn all() -> Vec<ToolDef> {
    vec![
        def("bash", "Run a shell command."),
        def(
            "slack_send",
            "Send a Slack message, e.g. to a channel. More text.",
        ),
        def("mcp__rea__trace", "Trace a feature."),
        def("mcp__rea__diff", "Diff two builds."),
    ]
}

#[test]
fn only_always_tools_and_the_loader_are_sent_until_something_is_loaded() {
    let always: HashSet<String> = ["bash".to_string()].into();
    let sent = request_tools(&all(), &always, &[]);
    assert_eq!(names(&sent), ["bash", LOAD_TOOLS]);
    let index = &sent[1].description;
    assert!(index.contains("- slack_send: Send a Slack message, e.g. to a channel\n"));
    assert!(index.contains("mcp__rea__* (2 tools): trace, diff"));
}

#[test]
fn loaded_and_called_tools_stay_live_from_history() {
    let always: HashSet<String> = ["bash".to_string()].into();
    let history = vec![
        call(LOAD_TOOLS, serde_json::json!({"names": ["mcp__rea__*"]})),
        call("slack_send", serde_json::json!({})),
    ];
    let sent = request_tools(&all(), &always, &history);
    assert_eq!(
        names(&sent),
        ["bash", "slack_send", "mcp__rea__trace", "mcp__rea__diff"]
    );
}

#[test]
fn the_loader_reports_loads_misses_and_query_hits() {
    let out = run_loader(
        &all(),
        &serde_json::json!({"names": ["mcp__rea__*", "nope"], "query": "slack"}),
    );
    assert!(!out.is_error);
    assert!(
        out.content
            .contains("Loaded 2 tool(s); call them directly now: mcp__rea__trace, mcp__rea__diff")
    );
    assert!(out.content.contains("No tool matches: nope"));
    assert!(out.content.contains("- slack_send:"));
    assert!(run_loader(&all(), &serde_json::json!({})).is_error);
}
