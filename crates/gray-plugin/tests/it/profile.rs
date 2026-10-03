use gray_plugin::profile::{PluginEntry, load_entries};

#[test]
fn load_profile_with_sidecar() {
    let entries = load_entries("testdata/gray_sidecar.yml").unwrap();
    assert!(matches!(entries[1], PluginEntry::Sidecar(_)));
}
