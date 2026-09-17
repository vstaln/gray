use std::sync::Arc;

use gray::profile::{ToolsBasicPlugin, ToolsSearchPlugin, from_plugins};

#[test]
fn registry_from_plugins_collects_in_order() {
    let plugins: Vec<Arc<dyn gray_plugin::Plugin>> =
        vec![Arc::new(ToolsBasicPlugin), Arc::new(ToolsSearchPlugin)];
    let (reg, manifests) = from_plugins(&plugins);
    let names = reg.tool_names();
    assert!(names.iter().any(|n| n == "read"));
    assert!(names.iter().any(|n| n == "grep"));
    // Manifests travel with the registry so --dump-manifest can't drift.
    assert_eq!(manifests.len(), 2);
    assert_eq!(manifests[0].name, "tools-basic");
    assert_eq!(manifests[1].name, "tools-search");
}
