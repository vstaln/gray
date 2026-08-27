//! hermes-state — SessionDB persistence crate.
//! Wave 1 — mirrors `hermes_state.py` + `hermes_state_common.py` / `_schema` /
//! `_search` / `_portability` (T0013).
//!
//! 1:1 port layout: each Python mixin becomes a Rust module on the same
//! `StateStore` type. This crate is the single owner of the `state.db` SQLite
//! store (rusqlite, WAL). See `docs/port/root-modules.md` §4.

pub mod common;
pub mod portability;
pub mod search_slice1;
