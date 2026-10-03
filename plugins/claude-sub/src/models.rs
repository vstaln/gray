//! Pinned model catalog: no HTTP `/models` endpoint exists, so the pinned
//! route table IS the catalog. The CLI's own picker (the `initialize`
//! handshake) is the live list on Hermes; here discovery degrades to the
//! pinned table when the CLI is missing or logged out.

use gray_plugin::{ProviderModel, ProviderModelCatalog};

use crate::catalog;

/// Effort tiers native `--effort` accepts (gray's `off` = flag omitted).
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The pinned catalog: every routable id with its window and efforts.
pub fn catalog() -> ProviderModelCatalog {
    let models = catalog::all_ids()
        .into_iter()
        .map(|id| ProviderModel {
            name: catalog::display_name(&id),
            context_window: catalog::context_window(&id),
            reasoning_efforts: EFFORTS.iter().map(|s| s.to_string()).collect(),
            id,
        })
        .collect();
    ProviderModelCatalog { models }
}

#[path = "models_tests.rs"]
#[cfg(test)]
mod tests;
