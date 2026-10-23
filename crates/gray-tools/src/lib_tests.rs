use super::*;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

struct StubTool {
    name: &'static str,
    marker: &'static str,
}

#[async_trait::async_trait]
impl Tool for StubTool {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn def(&self) -> ToolDef {
        ToolDef::new(self.name, "stub", json!({"type": "object"}))
    }
    async fn execute(&self, _ctx: &ToolContext, _args: Value) -> ToolOutput {
        ToolOutput::ok(self.marker)
    }
}

#[tokio::test]
async fn new_later_tool_wins_on_conflict() {
    let a: Arc<dyn Tool> = Arc::new(StubTool {
        name: "dup",
        marker: "from-a",
    });
    let b: Arc<dyn Tool> = Arc::new(StubTool {
        name: "dup",
        marker: "from-b",
    });
    let reg = Registry::new(vec![a, b]);
    assert_eq!(reg.defs().len(), 1);
    let out = reg
        .lookup("dup")
        .unwrap()
        .execute(&ToolContext::default(), json!({}))
        .await;
    assert!(format!("{out:?}").contains("from-b"), "{out:?}");
}

fn scalar_def() -> ToolDef {
    ToolDef::new(
        "probe",
        "probe",
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer" },
                "ratio": { "type": "number" },
                "verbose": { "type": "boolean" },
                "path": { "type": "string" }
            },
            "required": ["path"]
        }),
    )
}

#[test]
fn coerce_string_scalars_to_typed_values() {
    let out = coerce_args(
        &scalar_def(),
        json!({"limit": "10", "ratio": "2.5", "verbose": "true", "path": "/tmp/x"}),
    );
    assert_eq!(out.get("limit"), Some(&json!(10)));
    assert_eq!(out.get("ratio"), Some(&json!(2.5)));
    assert_eq!(out.get("verbose"), Some(&json!(true)));
}

#[test]
fn coerce_json_string_and_bare_scalar_to_array() {
    let def = ToolDef::new(
        "probe",
        "probe",
        json!({"type": "object", "properties": {"edits": {"type": "array"}}, "required": []}),
    );
    let out = coerce_args(
        &def,
        json!({"edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"}),
    );
    let arr = out
        .get("edits")
        .and_then(|v| v.as_array())
        .expect("edits should coerce to array");
    assert_eq!(arr.len(), 1);
    let out2 = coerce_args(&def, json!({"edits": {"oldText": "a", "newText": "b"}}));
    let arr2 = out2
        .get("edits")
        .and_then(|v| v.as_array())
        .expect("bare object should wrap to array");
    assert_eq!(arr2.len(), 1);
}

#[test]
fn coerce_null_dropped_only_when_optional() {
    let out = coerce_args(&scalar_def(), json!({"path": null, "limit": null}));
    assert!(
        out.get("path").is_some(),
        "required null must be kept so the tool errors"
    );
    assert!(
        out.get("limit").is_none(),
        "optional null must drop to None"
    );

    let out2 = coerce_args(&scalar_def(), json!({"path": "/tmp/x", "limit": "null"}));
    assert_eq!(
        out2.get("limit"),
        Some(&json!("null")),
        "string 'null' must be kept, got {out2}"
    );
}

#[test]
fn aliases_respect_schema_and_floats_stay_lossless() {
    let text_def = ToolDef::new(
        "probe",
        "probe",
        json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": []}),
    );
    let out = coerce_args(&text_def, json!({"text": "hi"}));
    assert_eq!(out.get("text"), Some(&json!("hi")));
    assert!(out.get("content").is_none());

    let out = coerce_args(&scalar_def(), json!({"path": "x", "limit": "2.5"}));
    assert_eq!(out.get("limit"), Some(&json!("2.5")));
    let out = coerce_args(
        &scalar_def(),
        json!({"path": "x", "limit": "99999999999999999999999"}),
    );
    assert_eq!(out.get("limit"), Some(&json!("99999999999999999999999")));
    let out = coerce_args(&scalar_def(), json!({"path": "x", "limit": "NaN"}));
    assert_eq!(out.get("limit"), Some(&json!("NaN")));

    let out = coerce_args(&scalar_def(), json!({"path": "x", "limit": "2.0"}));
    assert_eq!(out.get("limit"), Some(&json!(2)));
}

#[test]
fn aliases_rename_legacy_arg_names() {
    assert!(
        ALIASES.contains(&("file_path", "path")),
        "ALIASES must map legacy names"
    );
    let out = coerce_args(&scalar_def(), json!({"file_path": "/tmp/x", "limit": "3"}));
    assert_eq!(out.get("path"), Some(&json!("/tmp/x")));
    assert!(out.get("file_path").is_none());
    assert_eq!(out.get("limit"), Some(&json!(3)));
}

#[test]
fn strip_framing_unwraps_code_fences() {
    let raw = "```json\n{\"path\":\"/tmp/x\"}\n```";
    let stripped = strip_framing(raw);
    let v: Value = serde_json::from_str(stripped.trim()).expect("framing strip must yield JSON");
    assert_eq!(v.get("path"), Some(&json!("/tmp/x")));
}

#[tokio::test]
async fn registry_execute_applies_aliases_and_coercion() {
    struct Probe {
        seen: Arc<std::sync::Mutex<Option<Value>>>,
    }
    #[async_trait::async_trait]
    impl Tool for Probe {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn def(&self) -> ToolDef {
            ToolDef::new(
                "probe",
                "probe",
                json!({
                    "type": "object",
                    "properties": {"limit": {"type": "integer"}, "path": {"type": "string"}},
                    "required": ["path"]
                }),
            )
        }
        async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
            *self.seen.lock().unwrap() = Some(args);
            ToolOutput::ok("ok")
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(None));
    let probe: Arc<dyn Tool> = Arc::new(Probe { seen: seen.clone() });
    let reg = Registry::new(vec![probe]);
    let out = ToolExecutor::execute(
        &reg,
        &ToolContext::default(),
        "probe",
        json!({"file_path": "/tmp/x", "limit": "7"}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    let args = seen.lock().unwrap().clone().expect("tool should see args");
    assert_eq!(args.get("path"), Some(&json!("/tmp/x")), "{args}");
    assert_eq!(args.get("limit"), Some(&json!(7)), "{args}");
}

#[test]
fn coerce_multiple_stringified_edits_preserves_array() {
    let def = ToolDef::new(
        "probe",
        "probe",
        json!({"type":"object","properties":{"edits":{"type":"array"}}}),
    );
    let edits = json!([{"oldText":"a","newText":"b"},{"oldText":"c","newText":"d"}]);
    for input in [edits.to_string(), format!("```json\n{edits}\n```")] {
        assert_eq!(coerce_args(&def, json!({"edits":input}))["edits"], edits);
    }
}
