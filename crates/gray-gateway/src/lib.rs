//! Gateway crate — stubs by default, real adapters behind feature flags.
//!
//! Real deps are optional so `cargo check -p gray-gateway` passes offline:
//! - `telegram` feature → `teloxide` polling (Telegram MAX 4096 utf16)
//! - `discord`  feature → `twilight-gateway` (Discord MAX 2000)
//! - `slack`    feature → `slack-morphism` Socket Mode (Slack MAX 39000)
//!
//! Enable: `cargo check -p gray-gateway --features telegram,slack`
//! All:    `cargo check -p gray-gateway --features all-platforms`

pub mod config;
pub mod daemon;
pub mod platform;
pub mod session;
pub mod systemd;
pub mod telegram;
pub mod discord;
pub mod slack;
