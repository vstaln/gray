use std::sync::Arc;

use gray::profile::{ToolsBasicPlugin, ToolsSearchPlugin, from_plugins};

#[test]
fn registry_from_plugins_collects_in_order() {
    let plugins: Vec<Arc<dyn gray_plugin::Plugin>> =
        vec![Arc::new(ToolsBasicPlugin), Arc::new(ToolsSearchPlugin)];
    let (reg, manifests) = from_plugins(&plugins);
    assert!(reg.get("read").is_some());
    assert!(reg.get("grep").is_some());
    // Manifests travel with the registry so --dump-manifest can't drift.
    assert_eq!(manifests.len(), 2);
    assert_eq!(manifests[0].name, "tools-basic");
    assert_eq!(manifests[1].name, "tools-search");
}
