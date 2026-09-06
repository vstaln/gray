//! gray-extras: out-of-default-build surface (proxy, OAuth signin).
//!
//! Phase 1 scope cut: these modules left the default `gray` build so the
//! default tree carries no axum/OAuth-signin weight. They still build
//! (and test) under `--workspace`.

pub mod oauth;
pub mod proxy;
