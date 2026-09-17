//! `gray-pkg`: plugin package management — index client, fetch, verify.
//!
//! Networking lives here, never in `gray-plugin` (protocol only).

pub mod errors;
pub mod fetch;
pub mod index;
pub mod ops;
pub mod skills_ops;
pub mod sources;

use std::path::PathBuf;

pub(crate) fn gray_home() -> PathBuf {
    gray_core::paths::gray_home().unwrap_or_else(|| PathBuf::from(".gray"))
}

pub(crate) fn plugins_dir() -> PathBuf {
    gray_home().join("plugins")
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
