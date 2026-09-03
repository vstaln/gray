use gray_plugin::{Manifest, merge_manifests};

fn manifest_a() -> Manifest {
    Manifest {
        name: "plugin-a".to_string(),
        version: "0.1.0".to_string(),
        tools: vec!["read".to_string()],
        provider: None,
    }
}

fn manifest_b() -> Manifest {
    Manifest {
        name: "plugin-b".to_string(),
        version: "0.1.0".to_string(),
        tools: vec!["read".to_string()],
        provider: None,
    }
}

#[test]
fn later_entry_wins_on_name_conflict() {
    let merged = merge_manifests(vec![manifest_a(), manifest_b()]);
    assert_eq!(merged["read"], "plugin-b");
}
