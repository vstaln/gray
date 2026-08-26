//! hermes-cli — CLI entry point and subcommand dispatch.
//!
//! T0681: 1:1 port of `hermes_cli/main.py` slice 1/10.
//! Crate root re-exports the sliced modules; each `main_sliceN.rs`
//! covers ~1/10 of the 14 268-line Python source.

pub mod main_slice1;
