use gray_plugin::profile::{load_entries, PluginEntry};

#[test]
fn load_profile_with_sidecar() {
    let entries = load_entries("testdata/gray_sidecar.yml").unwrap();
    assert!(matches!(entries[1], PluginEntry::Sidecar(_)));
}
