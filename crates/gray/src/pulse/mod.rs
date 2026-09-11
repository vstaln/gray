//! Pulse: a standing goal run on a schedule by the gateway cron ticker.
pub mod config;
pub mod goal;
pub mod job;
pub mod plugin;

pub use job::enable;
