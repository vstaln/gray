//! hermes-cron — cron subsystem (scheduler, jobs, executions).
//!
//! T0376: 1:1 port of `cron/executions.py` → `executions2.rs`.
//! `executions.rs` is intentionally untouched to avoid name clash;
//! this crate exposes the new ledger as `executions2`.

pub mod executions2;
