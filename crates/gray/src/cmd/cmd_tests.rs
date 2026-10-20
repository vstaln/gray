use super::*;
use clap::Parser as _;
use gray_plugin::{ModelSlot, ModelVariant, SlotOption};

fn test_config() -> Config {
    let cli = crate::Cli::try_parse_from(["gray"]).expect("cli");
    let mut config = Config::resolve_with(&cli, |_| None).expect("config");
    config.base_url = "http://cmd-test.invalid/v1".to_string();
    config.api_key = None;
    config.provider_id = String::new();
    config.model = Some("m0".to_string());
    config.thinking_effort = Some("medium".to_string());
    config.fast_mode = Some(false);
    config
}

fn fake_models() -> Vec<(String, String)> {
    vec![
        ("m0".to_string(), "Model Zero".to_string()),
        ("m1".to_string(), String::new()),
        ("m2-max".to_string(), "Model Two Max".to_string()),
    ]
}

fn picker_rec(models: &[(String, String)]) -> store::PendingDoc {
    store::PendingDoc {
        kind: "model_picker".to_string(),
        created_at: crate::util::now_secs(),
        user: None,
        draft: store::ModelDraft {
            model: Some("m0".to_string()),
            effort: Some("medium".to_string()),
            fast: false,
            parts: Default::default(),
        },
        models: models.to_vec(),
        shapes: Default::default(),
        page: 0,
    }
}

fn composite_models() -> Vec<(String, String)> {
    vec![
        ("m0".to_string(), "Model Zero".to_string()),
        ("fusion-x".to_string(), "Fusion".to_string()),
    ]
}

fn composite_shape() -> RowShape {
    let slot = |key: &str, label: &str, opts: &[(&str, &str)]| ModelSlot {
        key: key.to_string(),
        label: label.to_string(),
        options: opts
            .iter()
            .map(|(id, name)| SlotOption {
                id: id.to_string(),
                name: name.to_string(),
            })
            .collect(),
    };
    let variant = |id: &str, lead: &str, kick: &str| ModelVariant {
        id: id.to_string(),
        effort: None,
        fast: false,
        parts: BTreeMap::from([
            ("lead".to_string(), lead.to_string()),
            ("sidekick".to_string(), kick.to_string()),
        ]),
    };
    RowShape {
        variants: vec![
            variant("fusion-x-opus-a", "opus", "a"),
            variant("fusion-x-swe-b", "swe", "b"),
        ],
        slots: vec![
            slot("lead", "Lead", &[("opus", "Opus 5.5"), ("swe", "SWE-2")]),
            slot("sidekick", "Sidekick", &[("a", "Alpha"), ("b", "Beta")]),
        ],
    }
}

fn composite_shapes() -> BTreeMap<String, RowShape> {
    BTreeMap::from([("fusion-x".to_string(), composite_shape())])
}

fn composite_rec() -> store::PendingDoc {
    let mut rec = picker_rec(&composite_models());
    rec.shapes = composite_shapes();
    rec.draft.model = Some("fusion-x".to_string());
    rec.draft.parts = BTreeMap::from([
        ("lead".to_string(), "opus".to_string()),
        ("sidekick".to_string(), "a".to_string()),
    ]);
    rec
}

#[test]
fn list_shape_matches_contract() {
    let list = registry::list_json();
    let commands = list["commands"].as_array().expect("commands array");
    assert!(
        commands.len() >= crate::repl::REGISTRY.len(),
        "every builtin command must be served; installed plugins may add more"
    );
    let by_name = |n: &str| commands.iter().find(|c| c["name"] == n).expect(n);
    let model = by_name("models");
    assert_eq!(model["surface"], "interactive");
    assert_eq!(model["scope"], "conversation");
    assert!(
        model["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "connect")
    );
    assert_eq!(model["args"][0]["name"], "args");
    assert_eq!(model["args"][0]["required"], false);
    assert_eq!(by_name("help")["surface"], "text");
    assert_eq!(by_name("help")["scope"], "global");
    assert_eq!(by_name("theme")["surface"], "local");
    assert_eq!(by_name("sessions")["surface"], "text");
    assert_eq!(by_name("usage")["args"], json!([]));

    for d in crate::repl::REGISTRY {
        assert_eq!(by_name(d.name)["description"], d.desc);
    }
}

#[test]
fn ui_doc_serde_wire_shape() {
    let doc = ui::UiDoc::message(
        "model-picker",
        ui::Visibility::Ephemeral,
        ui::Lifecycle::Editable,
        vec![
            ui::MessageNode::Text {
                content: "hi".to_string(),
            },
            ui::MessageNode::Separator,
            ui::MessageNode::ActionRow {
                children: vec![
                    ui::MessageNode::StringSelect {
                        logical_id: "model".to_string(),
                        placeholder: Some("model".to_string()),
                        options: vec![ui::SelectOption {
                            label: "Model Zero".to_string(),
                            value: "m0".to_string(),
                            description: Some("m0".to_string()),
                            default: true,
                        }],
                        min_values: None,
                        max_values: None,
                    },
                    ui::MessageNode::Button {
                        logical_id: "apply".to_string(),
                        label: "Apply".to_string(),
                        style: ui::ButtonStyle::Primary,
                        url: None,
                        disabled: false,
                    },
                ],
            },
        ],
    );
    let v = serde_json::to_value(&doc).unwrap();
    assert_eq!(v["protocol"], "gray.ui");
    assert_eq!(v["version"], 1);
    assert_eq!(v["surface"], "message");
    assert_eq!(v["document_id"], "model-picker");
    assert_eq!(v["visibility"], "ephemeral");
    assert_eq!(v["lifecycle"], "editable");
    assert_eq!(v["components"][0]["type"], "text");
    assert_eq!(v["components"][0]["content"], "hi");
    assert_eq!(v["components"][1]["type"], "separator");
    let select = &v["components"][2]["children"][0];
    assert_eq!(select["type"], "string_select");
    assert_eq!(select["logical_id"], "model");
    assert_eq!(select["options"][0]["default"], true);
    let button = &v["components"][2]["children"][1];
    assert_eq!(button["type"], "button");
    assert_eq!(button["style"], "primary");

    let back: ui::UiDoc = serde_json::from_value(v).unwrap();
    assert_eq!(back, doc);
}

#[test]
fn model_picker_doc_from_draft() {
    let models = fake_models();
    let draft = store::ModelDraft {
        model: Some("m0".to_string()),
        effort: Some("medium".to_string()),
        fast: false,
        parts: Default::default(),
    };
    let v = serde_json::to_value(model_picker_doc(&draft, &models, &BTreeMap::new(), 0)).unwrap();
    assert_eq!(v["document_id"], "model-picker");
    assert_eq!(v["lifecycle"], "editable");
    assert_eq!(
        v["components"][0]["content"],
        "**Model** — `m0` · effort `medium` · fast `off`"
    );
    let select = &v["components"][1]["children"][0];
    assert_eq!(select["logical_id"], "model");
    let options = select["options"].as_array().unwrap();
    assert_eq!(options.len(), 3);
    assert_eq!(options[0]["value"], "m0");
    assert_eq!(options[0]["default"], true);
    assert!(options[1]["default"].is_null());
    let effort = &v["components"][2]["children"][0];
    assert_eq!(effort["logical_id"], "effort");
    let eopts = effort["options"].as_array().unwrap();
    assert_eq!(eopts.len(), 4);
    assert_eq!(eopts[1]["value"], "medium");
    assert_eq!(eopts[1]["default"], true);
    let buttons = v["components"][3]["children"].as_array().unwrap();
    assert_eq!(buttons[0]["logical_id"], "apply");
    assert_eq!(buttons[0]["style"], "primary");
    assert_eq!(buttons[1]["logical_id"], "fast");
    assert_eq!(buttons[1]["label"], "⚡ fast: off");
    assert_eq!(buttons[2]["logical_id"], "cancel");
}

#[tokio::test]
async fn interact_values_and_fast_revise_the_doc() {
    let home = tempfile::tempdir().unwrap();
    let models = fake_models();
    let rec = picker_rec(&models);
    let token = store::put(home.path(), &rec).unwrap();
    let mut config = test_config();

    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"model":["m1"],"effort":["low"]}}"#),
    )
    .unwrap();
    let CommandResult::Ui {
        token: t2,
        document,
        ..
    } = res
    else {
        panic!("expected revised ui doc");
    };
    assert_eq!(t2, token);
    let v = serde_json::to_value(&document).unwrap();
    assert_eq!(
        v["components"][0]["content"],
        "**Model** — `m1` · effort `low` · fast `off`"
    );
    let saved = store::get(home.path(), &token, crate::util::now_secs()).unwrap();
    assert_eq!(saved.draft.model.as_deref(), Some("m1"));
    assert_eq!(saved.draft.effort.as_deref(), Some("low"));

    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"submit":"fast"}"#),
    )
    .unwrap();
    let CommandResult::Ui { document, .. } = res else {
        panic!("expected revised ui doc");
    };
    let v = serde_json::to_value(&document).unwrap();
    assert_eq!(v["components"][3]["children"][1]["label"], "⚡ fast: on");
    assert!(
        v["components"][0]["content"]
            .as_str()
            .unwrap()
            .contains("fast `on`")
    );
    let saved = store::get(home.path(), &token, crate::util::now_secs()).unwrap();
    assert!(saved.draft.fast);
}

#[tokio::test]
async fn interact_apply_persists_and_closes() {
    let home = tempfile::tempdir().unwrap();
    let models = fake_models();
    let mut rec = picker_rec(&models);
    rec.draft.model = Some("m1".to_string());
    rec.draft.effort = Some("low".to_string());
    rec.draft.fast = true;
    let token = store::put(home.path(), &rec).unwrap();
    let mut config = test_config();

    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"submit":"apply"}"#),
    )
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(close);
    assert!(text.contains("Model set to m1"), "text: {text}");

    assert!(store::get(home.path(), &token, crate::util::now_secs()).is_err());
    assert_eq!(config.model.as_deref(), Some("m1"));
    assert_eq!(config.thinking_effort.as_deref(), Some("low"));
    assert_eq!(config.fast_mode, Some(true));
    let saved = crate::setup::load_saved_config_at(&home.path().join("config.json"));
    assert_eq!(saved.model.as_deref(), Some("m1"));
    assert_eq!(saved.fast_mode, Some(true));
}

#[tokio::test]
async fn interact_cancel_closes() {
    let home = tempfile::tempdir().unwrap();
    let token = store::put(home.path(), &picker_rec(&fake_models())).unwrap();
    let mut config = test_config();
    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"submit":"cancel"}"#),
    )
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert_eq!(text, "Cancelled.");
    assert!(close);
    assert!(store::get(home.path(), &token, crate::util::now_secs()).is_err());
}

#[tokio::test]
async fn interact_expired_and_bad_actions() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();

    let err = interact(&mut config, home.path(), "nope", None).unwrap_err();
    assert_eq!(err.0, "expired");

    let mut rec = picker_rec(&fake_models());
    rec.created_at = crate::util::now_secs() - store::PENDING_TTL_SECS - 1;
    let token = store::put(home.path(), &rec).unwrap();
    let err = interact(&mut config, home.path(), &token, None).unwrap_err();
    assert_eq!(err.0, "expired");
    assert!(
        !home
            .path()
            .join("cmd_ui")
            .join(format!("{token}.json"))
            .exists()
    );

    let token = store::put(home.path(), &picker_rec(&fake_models())).unwrap();
    let err = interact(&mut config, home.path(), &token, Some("{")).unwrap_err();
    assert_eq!(err.0, "invalid_action");

    for (action, want) in [
        (r#"{"values":{"bogus":["x"]}}"#, "unknown control"),
        (r#"{"submit":"bogus"}"#, "unknown submit"),
        (r#"{"values":{"model":["nope"]}}"#, "unknown model option"),
        (r#"{"values":{"effort":["ludicrous"]}}"#, "unknown effort"),
    ] {
        let err = interact(&mut config, home.path(), &token, Some(action)).unwrap_err();
        assert_eq!(err.0, "invalid_action");
        assert!(err.1.contains(want), "{} vs {}", err.1, want);
    }

    let mut owned = picker_rec(&fake_models());
    owned.user = Some("u1".to_string());
    let token = store::put(home.path(), &owned).unwrap();
    let err = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"user":"u2","submit":"fast"}"#),
    )
    .unwrap_err();
    assert_eq!(err.0, "not_owner");
}

#[tokio::test]
async fn run_rejects_local_and_unknown() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    let err = run_command(&mut config, home.path(), "nosuch", &[])
        .await
        .unwrap_err();
    assert_eq!(err.0, "unknown_command");
    for name in ["theme", "quit", "copy", "hotkeys", "restart", "hehe"] {
        let err = run_command(&mut config, home.path(), name, &[])
            .await
            .unwrap_err();
        assert_eq!(err.0, "local_only", "{name}");
        assert!(err.1.contains(name));
    }

    let err = run_command(&mut config, home.path(), "reboot", &[])
        .await
        .unwrap_err();
    assert_eq!(err.0, "local_only");
    let err = run_command(&mut config, home.path(), "exit", &[])
        .await
        .unwrap_err();
    assert_eq!(err.0, "local_only");

    let err = run_command(&mut config, home.path(), "skills", &["paste".to_string()])
        .await
        .unwrap_err();
    assert_eq!(err.0, "invalid_args");
    let err = run_command(&mut config, home.path(), "sessions", &["--all".to_string()])
        .await
        .unwrap_err();
    assert_eq!(err.0, "invalid_args");
}

#[tokio::test]
async fn run_model_direct_set() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    let res = run_command(&mut config, home.path(), "models", &["m1".to_string()])
        .await
        .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(close);
    assert_eq!(text, "✓ Model set to m1");
    assert_eq!(config.model.as_deref(), Some("m1"));
    let saved = crate::setup::load_saved_config_at(&home.path().join("config.json"));
    assert_eq!(saved.model.as_deref(), Some("m1"));
}

#[tokio::test]
async fn run_help_and_skills_text() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    let res = run_command(&mut config, home.path(), "help", &[])
        .await
        .unwrap();
    let CommandResult::Text { text, .. } = res else {
        panic!("expected text");
    };
    assert!(text.contains("/model"));
    assert!(text.contains("/help"));

    let skills_dir = home.path().join("skills").join("demo-skill");
    std::fs::create_dir_all(&skills_dir).unwrap();
    std::fs::write(
        skills_dir.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: does demo things\n---\nbody\n",
    )
    .unwrap();
    let res = run_command(&mut config, home.path(), "skills", &[])
        .await
        .unwrap();
    let CommandResult::Text { text, .. } = res else {
        panic!("expected text");
    };

    assert!(!text.is_empty());
}

#[tokio::test]
async fn new_resets_session_pointer() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("session.json"),
        r#"{"session_id":"abc12345-dead-4ead-8ead-abc123456789","generation":"g1","last_activity":1}"#,
    )
    .unwrap();
    new_result(home.path()).await.unwrap();
    let state: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("session.json")).unwrap()).unwrap();
    assert!(state["session_id"].is_null());
    assert_ne!(state["generation"], "g1");
}

#[test]
fn model_picker_doc_composite_row_gets_slot_selects() {
    let models = composite_models();
    let shapes = composite_shapes();
    let draft = store::ModelDraft {
        model: Some("fusion-x".to_string()),
        effort: Some("medium".to_string()),
        fast: false,
        parts: BTreeMap::from([
            ("lead".to_string(), "opus".to_string()),
            ("sidekick".to_string(), "a".to_string()),
        ]),
    };
    let v = serde_json::to_value(model_picker_doc(&draft, &models, &shapes, 0)).unwrap();
    let comps = v["components"].as_array().unwrap();

    assert_eq!(comps.len(), 6);
    let lead = &comps[2]["children"][0];
    assert_eq!(lead["type"], "string_select");
    assert_eq!(lead["logical_id"], "slot:lead");
    assert_eq!(lead["placeholder"], "Lead");
    let opts = lead["options"].as_array().unwrap();
    assert_eq!(opts.len(), 2);
    assert_eq!(opts[0]["value"], "opus");
    assert_eq!(opts[0]["label"], "Opus 5.5");
    assert_eq!(opts[0]["default"], true);
    assert!(opts[1]["default"].is_null());
    let kick = &comps[3]["children"][0];
    assert_eq!(kick["logical_id"], "slot:sidekick");
    assert_eq!(kick["placeholder"], "Sidekick");
    assert_eq!(kick["options"][0]["value"], "a");
    assert_eq!(kick["options"][0]["default"], true);
    assert_eq!(comps[4]["children"][0]["logical_id"], "effort");
    assert_eq!(comps[5]["children"][0]["logical_id"], "apply");
}

#[test]
fn model_picker_fallback_lists_slot_options() {
    let models = composite_models();
    let shapes = composite_shapes();
    let draft = store::ModelDraft {
        model: Some("fusion-x".to_string()),
        effort: Some("medium".to_string()),
        fast: false,
        parts: Default::default(),
    };
    let text = model_picker_fallback(&draft, &models, &shapes);
    assert!(text.contains("lead:"), "{text}");
    assert!(text.contains("1. Opus 5.5 (opus)"), "{text}");
    assert!(text.contains("sidekick:"), "{text}");
    assert!(text.contains("2. Beta (b)"), "{text}");
    assert!(
        text.contains("model <id> [lead=<opt>] [sidekick=<opt>]"),
        "{text}"
    );
}

#[tokio::test]
async fn interact_slot_values_update_draft_parts() {
    let home = tempfile::tempdir().unwrap();
    let token = store::put(home.path(), &composite_rec()).unwrap();
    let mut config = test_config();

    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"slot:lead":["swe"]}}"#),
    )
    .unwrap();
    let CommandResult::Ui { document, .. } = res else {
        panic!("expected revised ui doc");
    };
    let v = serde_json::to_value(&document).unwrap();
    let comps = v["components"].as_array().unwrap();
    assert_eq!(comps[2]["children"][0]["options"][1]["value"], "swe");
    assert_eq!(comps[2]["children"][0]["options"][1]["default"], true);
    assert_eq!(comps[3]["children"][0]["options"][1]["value"], "b");
    assert_eq!(comps[3]["children"][0]["options"][1]["default"], true);
    let saved = store::get(home.path(), &token, crate::util::now_secs()).unwrap();
    assert_eq!(saved.draft.parts["lead"], "swe");
    assert_eq!(saved.draft.parts["sidekick"], "b");

    interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"model":["m0"]}}"#),
    )
    .unwrap();
    let saved = store::get(home.path(), &token, crate::util::now_secs()).unwrap();
    assert!(saved.draft.parts.is_empty());
    interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"model":["fusion-x"]}}"#),
    )
    .unwrap();
    let saved = store::get(home.path(), &token, crate::util::now_secs()).unwrap();
    assert_eq!(saved.draft.parts["lead"], "opus");
    assert_eq!(saved.draft.parts["sidekick"], "a");

    for action in [
        r#"{"values":{"slot:nope":["x"]}}"#,
        r#"{"values":{"slot:lead":["zzz"]}}"#,
    ] {
        let err = interact(&mut config, home.path(), &token, Some(action)).unwrap_err();
        assert_eq!(err.0, "invalid_action");
    }

    interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"model":["m0"]}}"#),
    )
    .unwrap();
    let err = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"values":{"slot:lead":["swe"]}}"#),
    )
    .unwrap_err();
    assert_eq!(err.0, "invalid_action");
    assert!(err.1.contains("unknown control"), "{}", err.1);
}

#[tokio::test]
async fn interact_apply_composite_persists_parts() {
    let home = tempfile::tempdir().unwrap();
    let mut rec = composite_rec();
    rec.draft.parts = BTreeMap::from([
        ("lead".to_string(), "swe".to_string()),
        ("sidekick".to_string(), "b".to_string()),
    ]);
    let token = store::put(home.path(), &rec).unwrap();
    let mut config = test_config();

    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"submit":"apply"}"#),
    )
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(close);
    assert!(text.contains("Fusion"), "text: {text}");
    assert!(text.contains("SWE-2"), "text: {text}");
    assert!(text.contains("Beta"), "text: {text}");
    assert_eq!(config.model.as_deref(), Some("fusion-x"));
    assert_eq!(config.model_parts["lead"], "swe");
    assert_eq!(config.model_parts["sidekick"], "b");
    let saved = crate::setup::load_saved_config_at(&home.path().join("config.json"));
    assert_eq!(saved.model.as_deref(), Some("fusion-x"));
    assert_eq!(saved.model_parts["lead"], "swe");
    assert_eq!(saved.model_parts["sidekick"], "b");
    let key = crate::setup::effort_memory_key(&config.provider_id, &config.base_url, "fusion-x");
    assert_eq!(saved.parts_memory[&key]["sidekick"], "b");
}

#[tokio::test]
async fn interact_apply_unserved_combo_bounces() {
    let home = tempfile::tempdir().unwrap();
    let mut rec = composite_rec();

    rec.draft.parts = BTreeMap::from([
        ("lead".to_string(), "opus".to_string()),
        ("sidekick".to_string(), "b".to_string()),
    ]);
    let token = store::put(home.path(), &rec).unwrap();
    let mut config = test_config();
    let res = interact(
        &mut config,
        home.path(),
        &token,
        Some(r#"{"submit":"apply"}"#),
    )
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(!close);
    assert!(text.contains("no fusion-x variant serves"), "text: {text}");

    assert!(store::get(home.path(), &token, crate::util::now_secs()).is_ok());
    assert_ne!(config.model.as_deref(), Some("fusion-x"));
}

#[tokio::test]
async fn run_model_direct_composite_args() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    let shape = composite_shape();
    crate::setup::variants::cache_row_shape(
        "fusion-x",
        shape.variants.clone(),
        shape.slots.clone(),
    );

    let res = run_command(
        &mut config,
        home.path(),
        "models",
        &[
            "fusion-x".to_string(),
            "lead=swe".to_string(),
            "sidekick=b".to_string(),
        ],
    )
    .await
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(close);
    assert!(text.contains("SWE-2"), "text: {text}");
    assert_eq!(config.model.as_deref(), Some("fusion-x"));
    assert_eq!(config.model_parts["lead"], "swe");
    assert_eq!(config.model_parts["sidekick"], "b");
    let saved = crate::setup::load_saved_config_at(&home.path().join("config.json"));
    assert_eq!(saved.model.as_deref(), Some("fusion-x"));
    assert_eq!(saved.model_parts["lead"], "swe");

    let res = run_command(
        &mut config,
        home.path(),
        "models",
        &["fusion-x".to_string(), "lead=opus".to_string()],
    )
    .await
    .unwrap();
    let CommandResult::Text { .. } = res else {
        panic!("expected text result");
    };
    assert_eq!(config.model_parts["lead"], "opus");
    assert_eq!(config.model_parts["sidekick"], "a");

    for (args, want) in [
        (vec!["fusion-x", "bogus=x"], "unknown slot 'bogus'"),
        (vec!["fusion-x", "lead=zzz"], "unknown lead option 'zzz'"),
        (
            vec!["fusion-x", "lead=opus", "sidekick=b"],
            "no fusion-x variant serves",
        ),
        (vec!["fusion-x", "dangling"], "bad token 'dangling'"),
    ] {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let err = run_command(&mut config, home.path(), "models", &args)
            .await
            .unwrap_err();
        assert_eq!(err.0, "invalid_args");
        assert!(err.1.contains(want), "{} vs {}", err.1, want);
    }
}

#[tokio::test]
async fn run_model_direct_composite_bare_lands_default_parts() {
    let home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    let shape = composite_shape();
    crate::setup::variants::cache_row_shape(
        "fusion-x",
        shape.variants.clone(),
        shape.slots.clone(),
    );
    let res = run_command(
        &mut config,
        home.path(),
        "models",
        &["fusion-x".to_string()],
    )
    .await
    .unwrap();
    let CommandResult::Text { text, close } = res else {
        panic!("expected text result");
    };
    assert!(close);
    assert!(text.contains("Opus 5.5"), "text: {text}");
    assert!(text.contains("Alpha"), "text: {text}");
    assert_eq!(config.model.as_deref(), Some("fusion-x"));
    assert_eq!(config.model_parts["lead"], "opus");
    assert_eq!(config.model_parts["sidekick"], "a");
    let saved = crate::setup::load_saved_config_at(&home.path().join("config.json"));
    assert_eq!(saved.model_parts["lead"], "opus");
}
