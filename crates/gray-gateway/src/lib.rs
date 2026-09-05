//! Gateway crate — stubs by default, real adapters behind feature flags.
//!
//! Real deps are optional so `cargo check -p gray-gateway` passes offline:
//! - `telegram` feature → `teloxide` long-polling (Telegram MAX 4096 utf16)
//! - `discord`  feature → `twilight-gateway` (Discord MAX 2000)
//! - `slack`    feature → `slack-morphism` Socket Mode (Slack MAX 39000)
//!
//! Enable: `cargo check -p gray-gateway --features telegram,slack`
//! All:    `cargo check -p gray-gateway --features all-platforms`
//!
//! Architecture (hermes-agent gateway + OpenClaw control plane):
//! - [`platform`]  — `BasePlatformAdapter` (the one adapter trait) + chunking helpers
//! - [`session`]   — `SessionSource` → `build_session_key` (never hand-build keys)
//! - [`authz`]     — deny-by-default authorization + gateway tool policy
//! - [`pairing`]   — DM pairing codes approved via `gray gateway pairing approve`
//! - [`delivery`]  — origin / home-channel / explicit targets, `gray send`
//! - [`daemon`]    — `GatewayRunner`: inbound pipeline, streaming, cron delivery

pub mod authz;
pub mod config;
pub mod daemon;
pub mod delivery;
pub mod pairing;
pub mod platform;
pub mod progress;
pub mod session;
pub mod status;
pub mod systemd;
pub mod telegram;
pub mod discord;
pub mod slack;
