//! hermes-cron — cron subsystem (scheduler, jobs, executions).
//!
//! T0375: 1:1 port of `cron/scheduler_provider.py` (703) → `scheduler_provider.rs`.
//! T0376: 1:1 port of `cron/executions.py` → `executions2.rs`.
//! `executions.rs` is intentionally untouched to avoid name clash;
//! this crate exposes the new ledger as `executions2`.
//! T0377: 1:1 port of `cron/suggestions.py` → `suggestions.rs`.
//! T0378: 1:1 port of `cron/monitor.py` → `monitor.rs`.
//! T0379: 1:1 port of `cron/notepad.py` → `notepad.rs`.
//! T0380: 1:1 port of `cron/suggestion_catalog.py` → `suggestion_catalog.rs`.
//! T0381: 1:1 port of `cron/__init__.py` (42) → `init.rs`.

pub mod blueprint_catalog;
pub mod executions2;
pub mod init;
pub mod monitor;
pub mod notepad;
pub mod scheduler_provider;
pub mod suggestion_catalog;
pub mod suggestions;
