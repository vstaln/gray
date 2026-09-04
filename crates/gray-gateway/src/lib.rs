//! Gateway crate — Discord adapter only.
//!
//! An earlier revision carried Telegram/Slack adapters that validated tokens
//! and logged sends but never delivered a message, behind `telegram`/`slack`
//! features the shipped binary never enabled. Deleted (ponytail-audit #2);
//! re-add a platform when it can actually send. `Platform` keeps all three
//! variants so persisted session keys (`gray:main:telegram:…`) still parse.

pub mod config;
pub mod daemon;
pub mod platform;
pub mod session;
pub mod systemd;
pub mod discord;
