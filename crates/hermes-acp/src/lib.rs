//! hermes-acp — ACP adapter crate.
//!
//! T0410/T0411: 1:1 port of `acp_adapter/server.py` slices 1-3/4.
//! Crate root re-exports the sliced modules; each `server_sliceN.rs`
//! covers ~660 lines of the 2640-line Python source.

pub mod server_slice1;
pub mod server_slice2;
pub mod server_slice3;
