//! hermes-acp — ACP adapter crate.
//!
//! T0410: 1:1 port of `acp_adapter/server.py` slice 1/4.
//! Crate root re-exports the sliced modules; each `server_sliceN.rs`
//! covers ~660 lines of the 2640-line Python source.

pub mod server_slice1;
