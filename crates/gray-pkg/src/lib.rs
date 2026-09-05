//! `gray-pkg`: plugin package management — index client, fetch, verify.
//!
//! Networking lives here, never in `gray-plugin` (protocol only).
//! Ecosystem adapters land in Task 2.3.

pub mod adapter;
pub mod fetch;
pub mod index;
pub mod ops;

use std::path::PathBuf;

pub(crate) fn gray_home() -> PathBuf {
    std::env::var("GRAY_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".gray"))
                .unwrap_or_else(|_| PathBuf::from(".gray"))
        })
}

pub(crate) fn plugins_dir() -> PathBuf {
    gray_home().join("plugins")
}
