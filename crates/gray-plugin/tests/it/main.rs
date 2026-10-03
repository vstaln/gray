//! One integration-test binary for the plugin suites. Filter with the
//! module path, e.g. `cargo test -p gray-plugin --test it sidecar::`.
//!
//! `builder_enabled` stays a separate `tests/*.rs`: it mutates process-global
//! GRAY_HOME and cwd, which would race these suites in a shared process.

mod lifecycle;
mod lock;
mod manifest;
mod profile;
mod sidecar;
