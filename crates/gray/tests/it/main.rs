//! One integration-test binary instead of ten: each `tests/*.rs` used to be
//! its own crate, re-linking libgray + every dep per file. Filter with the
//! module path, e.g. `cargo test -p gray --test it home_paths::`.
//!
//! These suites share one process. None mutate env/cwd (subprocess
//! isolation instead); a suite that must should stay a separate `tests/*.rs`.

mod backdrop_dim;
mod cron_chat_delivery;
mod home_paths;
mod memory_cli;
mod native_plugin;
mod plugin_first_command;
mod plugin_registry;
mod provider_effort;
mod windows_limits;
mod working_directory;
