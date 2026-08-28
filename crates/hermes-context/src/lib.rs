//! hermes-context — context window compression.
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 1/11 (lines 1-800).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 2/11 (lines 800-1600).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 3/11 (lines 1600-2400).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py` (4465 LOC).
//! T0015 — slice 1/6 (lines 1-800).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py` (4465 LOC).
//! T0015 — slice 2/6 (lines 800-1600).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_engine.py` (489 LOC).
//! T0017 — full file (lines 1-489).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py` (4465 LOC).
//! T0015 — slice 3/6 (lines 1600-2400).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py` (4465 LOC).
//! T0015 — slice 4/6 (lines 2400-3200).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 10/11 (lines 7200-8000).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — slice 11/11 (lines 8000-8211) (last).
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py` (8211 LOC).
//! T0014 — extra (lines 8211 tail — EOF sentinel, no new Python lines).
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! `cargo` is not invoked by this slice; the file is verified by line-level
//! audit against the Python source.

pub mod compressor_slice1;
pub mod compressor_slice2;
pub mod compressor_slice3;
pub mod compressor_slice4;
pub mod compressor_slice5;
pub mod compressor_slice6;
pub mod compressor_slice7;
pub mod compressor_slice8;
pub mod compressor_slice9;
pub mod compressor_slice10;
pub mod compressor_slice11;
pub mod compressor_extra;
pub mod context_breakdown;
pub mod context_engine;
pub mod context_references;
pub mod conversation_slice1;
pub mod conversation_slice2;
pub mod conversation_slice3;
pub mod conversation_slice4;
pub mod manual_compression_feedback;
pub mod partial_compress;
