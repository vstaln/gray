use super::*;
use crate::setup::registry::{FieldKind, SetupField};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const DECL: SetupDecl = SetupDecl {
    config_path: ".config/test-app/config.json",
    fields: &[
        SetupField {
            key: "token",
            kind: FieldKind::Required,
            description: "t",
            url: None,
            secret: true,
            picker: None,
        },
        SetupField {
            key: "gray_bin",
            kind: FieldKind::Derived,
            description: "g",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "workdir",
            kind: FieldKind::Derived,
            description: "w",
            url: None,
            secret: false,
            picker: None,
        },
    ],
    verify: &["test-app", "doctor"],
    post_steps: &[],
    service: None,
};

fn supplied_token(value: &str) -> Supplied {
    let mut s = Supplied::default();
    s.insert("token", value.to_string(), true);
    s
}

#[test]
fn writes_privately_and_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(DECL.config_path);
    let gray_home = tmp.path().join(".gray");
    write_config(
        &path,
        &DECL,
        &supplied_token("sk-abc"),
        &gray_home,
        tmp.path(),
    )
    .unwrap();

    let data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["token"], "sk-abc");
    // Derived fields landed as absolute paths.
    assert!(Path::new(data["gray_bin"].as_str().unwrap()).is_absolute());
    assert!(Path::new(data["workdir"].as_str().unwrap()).is_absolute());

    #[cfg(unix)]
    {
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let dir = fs::metadata(path.parent().unwrap()).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o700);
    }
    // No temp file left behind.
    assert!(!tmp.path().join(".config/test-app/config.tmp").exists());
}

#[test]
fn unknown_keys_survive_a_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let gray_home = tmp.path().join(".gray");
    let path = tmp.path().join(DECL.config_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, r#"{"future_key": 42, "token": "old"}"#).unwrap();
    write_config(
        &path,
        &DECL,
        &supplied_token("sk-new"),
        &gray_home,
        tmp.path(),
    )
    .unwrap();
    let data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["future_key"], 42);
    assert_eq!(data["token"], "sk-new");
}

#[test]
fn an_unparseable_config_is_replaced_not_corrupted() {
    let tmp = tempfile::tempdir().unwrap();
    let gray_home = tmp.path().join(".gray");
    let path = tmp.path().join(DECL.config_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{not json").unwrap();
    write_config(
        &path,
        &DECL,
        &supplied_token("sk-x"),
        &gray_home,
        tmp.path(),
    )
    .unwrap();
    let data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["token"], "sk-x");
}

#[test]
fn a_secret_never_renders_in_debug_or_errors() {
    let supplied = supplied_token("sk-DEADBEEF");
    let shown = format!("{supplied:?}");
    assert!(shown.contains("<secret>"), "{shown}");
    assert!(!shown.contains("DEADBEEF"), "{shown}");

    // A failed write reports the path problem, never the value.
    // Unix-only half: chmod read-only has no Windows equivalent (ACLs), so
    // without the gate the write succeeds and the expect below panics.
    #[cfg(unix)]
    {
        let tmp = tempfile::tempdir().unwrap();
        let gray_home = tmp.path().join(".gray");
        let blocked = tmp.path().join("blocked");
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).unwrap();
        let path = blocked.join(".config/test-app/config.json");
        let err = write_config(&path, &DECL, &supplied, &gray_home, tmp.path())
            .err()
            .expect("a read-only directory must fail");
        let text = format!("{err:#}");
        assert!(!text.contains("DEADBEEF"), "{text}");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn empty_supplied_still_writes_derived_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let gray_home = tmp.path().join(".gray");
    let path = tmp.path().join(DECL.config_path);
    write_config(&path, &DECL, &Supplied::default(), &gray_home, tmp.path()).unwrap();
    let data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(data.get("gray_bin").is_some());
    assert!(data.get("workdir").is_some());
    assert!(data.get("token").is_none());
}
