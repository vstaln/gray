//! gray-extras: out-of-default-build surface (proxy, OAuth signin, cron CLI).
//!
//! Phase 1 scope cut: these modules left the default `gray` build so the
//! default tree carries no axum/cron/OAuth-signin weight. They still build
//! (and test) under `--workspace`; Task 3 re-exposes cron/gateway via the
//! plugin protocol (`command/run`).

pub mod cron_cli;
pub mod oauth;
pub mod proxy;
