use gray_plugin::profile::{load_entries, load_profile, PluginEntry};

#[test]
fn load_profile_with_sidecar() {
    let entries = load_entries("testdata/gray_sidecar.yml").unwrap();
    assert!(matches!(entries[1], PluginEntry::Sidecar(_)));
}

#[test]
fn load_profile_returns_ordered_names() {
    let names = load_profile("testdata/gray.yml").unwrap();
    assert_eq!(names, vec!["tools-basic", "tools-search"]);
}

#[test]
fn load_profile_missing_file_is_err() {
    assert!(load_profile("testdata/nonexistent.yml").is_err());
}

#[test]
fn load_profile_garbage_is_err() {
    let dir = std::env::temp_dir();
    let path = dir.join("gray-profile-garbage-test.yml");
    std::fs::write(&path, "::: not yaml :::\n\t\x00garbage").unwrap();
    assert!(load_profile(path.to_str().unwrap()).is_err());
}
