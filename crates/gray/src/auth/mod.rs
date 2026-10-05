//! Host-owned credential storage and refresh coordination.

pub mod broker;
pub mod store;

pub use broker::{PluginCredentialSource, shared_plugin_source, shared_plugin_source_with_model};
pub use store::{
    AuthLock, CredentialStore, StoredCredential, load_plugin_credential, remove_plugin_owner,
    save_plugin_credential,
};
