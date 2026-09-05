use std::sync::Arc;

use gray_tools::Registry;
use gray_tools::plugin::{ToolsBasicPlugin, ToolsSearchPlugin};

#[test]
fn registry_from_plugins_collects_in_order() {
    let plugins: Vec<Arc<dyn gray_plugin::Plugin>> =
        vec![Arc::new(ToolsBasicPlugin), Arc::new(ToolsSearchPlugin)];
    let reg = Registry::from_plugins(&plugins);
    assert!(reg.get("read").is_some());
    assert!(reg.get("grep").is_some());
}
