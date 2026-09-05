//! The `bash` tool: re-export only (V2 Phase 1D).
//!
//! Implementation lives in `shell::tools::bash`, the guard in
//! `shell::guard`. This module stays so the `crate::bash::BashTool` path
//! (used by `plugin.rs`/downstream crates) keeps working; the registry for
//! Phase 2 will add tools without touching this file.

pub use crate::shell::tools::bash::{BASH_GUIDELINES, BASH_SNIPPET, BashTool};
