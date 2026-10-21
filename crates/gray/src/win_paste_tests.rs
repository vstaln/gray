use super::*;

fn obj(json: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(json)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn split_model_repoints_default_paste_ctrl_v() {
    // Same shape as a real WT settings.json (actions + keybindings id refs).
    let mut o = obj(r#"{
        "actions": [],
        "keybindings": [
            {"id": null, "keys": "ctrl+v"},
            {"id": "Terminal.PasteFromClipboard", "keys": "ctrl+shift+v"},
            {"id": "Terminal.CopyToClipboard", "keys": "ctrl+c"}
        ]
    }"#);
    assert!(ensure_unbound(&mut o));
    let s = serde_json::to_string(&o).unwrap();
    assert!(s.contains(UNBOUND_ID));
    let bindings = o["keybindings"].as_array().unwrap();
    let ctrl_v = bindings
        .iter()
        .find(|b| keys_include(&b["keys"], "ctrl+v"))
        .unwrap();
    assert_eq!(ctrl_v["id"].as_str().unwrap(), UNBOUND_ID);
    // ctrl+shift+v paste survives for terminal-level text paste.
    let csv = bindings
        .iter()
        .find(|b| keys_include(&b["keys"], "ctrl+shift+v"))
        .unwrap();
    assert_eq!(csv["id"].as_str().unwrap(), "Terminal.PasteFromClipboard");
}

#[test]
fn split_model_is_idempotent() {
    let mut o = obj(r#"{"actions": [], "keybindings": [{"id": null, "keys": "ctrl+v"}]}"#);
    assert!(ensure_unbound(&mut o));
    assert!(!ensure_unbound(&mut o));
}

#[test]
fn split_model_adds_binding_when_ctrl_v_not_listed() {
    let mut o = obj(r#"{"actions": [], "keybindings": []}"#);
    assert!(ensure_unbound(&mut o));
    let bindings = o["keybindings"].as_array().unwrap();
    assert!(
        bindings
            .iter()
            .any(|b| b["id"].as_str() == Some(UNBOUND_ID) && keys_include(&b["keys"], "ctrl+v"))
    );
}

#[test]
fn split_model_leaves_custom_ctrl_v_binding_and_appends_ours() {
    let mut o = obj(r#"{
        "actions": [{"command": "find", "id": "User.find"}],
        "keybindings": [{"id": "User.find", "keys": "ctrl+v"}]
    }"#);
    assert!(ensure_unbound(&mut o));
    let bindings = o["keybindings"].as_array().unwrap();
    // User's binding untouched; ours appended after (last wins in WT).
    assert_eq!(bindings[0]["id"].as_str().unwrap(), "User.find");
    assert_eq!(bindings.last().unwrap()["id"].as_str().unwrap(), UNBOUND_ID);
}

#[test]
fn legacy_model_repoints_paste_and_appends_unbound() {
    let mut o = obj(r#"{
        "actions": [
            {"command": "paste", "keys": "ctrl+v"},
            {"command": "copy", "keys": "ctrl+c"}
        ]
    }"#);
    assert!(ensure_unbound(&mut o));
    let actions = o["actions"].as_array().unwrap();
    let unbound = actions
        .iter()
        .filter(|a| a["command"].as_str() == Some("unbound"))
        .count();
    assert!(unbound >= 1);
    assert!(actions.iter().any(|a| a["command"].as_str() == Some("unbound")
        && keys_include(&a["keys"], "ctrl+v")));
}

#[test]
fn legacy_model_strips_ctrl_v_from_multi_key_paste() {
    let mut o = obj(r#"{
        "actions": [{"command": "paste", "keys": ["ctrl+v", "ctrl+shift+v"]}]
    }"#);
    assert!(ensure_unbound(&mut o));
    let actions = o["actions"].as_array().unwrap();
    let paste = actions
        .iter()
        .find(|a| a["command"].as_str() == Some("paste"))
        .unwrap();
    assert_eq!(
        paste["keys"].as_array().unwrap()[0].as_str().unwrap(),
        "ctrl+shift+v"
    );
    assert!(actions
        .iter()
        .any(|a| a["command"].as_str() == Some("unbound") && keys_include(&a["keys"], "ctrl+v")));
}

#[test]
fn keys_include_handles_string_and_array() {
    assert!(keys_include(&serde_json::json!("ctrl+v"), "ctrl+v"));
    assert!(keys_include(
        &serde_json::json!(["ctrl+v", "ctrl+x"]),
        "ctrl+v"
    ));
    assert!(!keys_include(&serde_json::json!("ctrl+shift+v"), "ctrl+v"));
    assert!(!keys_include(&serde_json::Value::Null, "ctrl+v"));
}
