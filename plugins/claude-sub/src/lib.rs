//! Library target for the `claude-sub` sidecar. The binary uses these
//! modules over the sidecar wire; the host tests import the exact same
//! provider declaration so validation is version-locked by tests.

pub mod catalog;
pub mod chat;
pub mod manifest;
pub mod models;
pub mod relay;
pub mod setup;
