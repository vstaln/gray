//! One integration-test binary for the shell/read suites. Filter with the
//! module path, e.g. `cargo test -p gray-tools --test it shell_contract::`.
//!
//! `index_bench` and `tool_bench` stay separate `tests/*.rs`: they time wall
//! clock and must not share a process with concurrently running suites.

mod background_jobs;
mod platform_shell;
mod read_zoo;
mod shell_contract;
