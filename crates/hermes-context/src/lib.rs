//! hermes-context — context window compression.
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 1/11 (lines 1-800).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py` (4465 LOC).
//! T0015 — slice 1/6 (lines 1-800).
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! `cargo` is not invoked by this slice; the file is verified by line-level
//! audit against the Python source.

pub mod compressor_slice1;
pub mod conversation_slice1;
