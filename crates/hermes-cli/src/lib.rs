//! hermes-cli — CLI entry point and subcommand dispatch.
//!
//! T0681: 1:1 port of `hermes_cli/main.py` slice 1/10.
//! Crate root re-exports the sliced modules; each `main_sliceN.rs`
//! covers ~1/10 of the 14 268-line Python source.

pub mod main_slice1;
pub mod kanban_db_slice1;
pub mod auth_slice1;
pub mod update_cmd_slice1;
pub mod gateway_slice1;
