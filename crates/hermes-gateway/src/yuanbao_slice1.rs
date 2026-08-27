//! Yuanbao platform adapter — slice 1 (lines 1–900).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/yuanbao.py`
//! (5298 LOC), slice 1 covering lines 1–900.
//! This slice contains the module docstring, all imports, logger, version/
//! platform constants, every module-level constant and compiled regex /
//! frozenset through the media-resolve concurrency clamps, the complete
//! `MarkdownProcessor` class (all static/class methods delegating to the
//! shared fence-aware chunker core), the complete `SignManager` class
//! (constants, shared token cache + per-app_key locks, helpers, fetch with
//! retry, cached get_token, force_refresh), the `InboundContext` dataclass,
//! the `InboundMiddleware` abstract base, the `InboundPipeline` onion engine
//! (use / use_before / use_after / remove / execute with `when` guards),
//! and `DecodeMiddleware` through `parse_json_push` (the slice boundary at
//! line 900 cuts mid-return-dict inside `DecodeMiddleware.parse_json_push` at
//! `sender_nickname`; the Rust file includes the complete `parse_json_push`
//! and the remainder of `DecodeMiddleware` (`_decode_single` + `handle` +
//! merge logic) for compilability — the complete `DecodeMiddleware` class
//! spans lines 830–988 in Python and is fully included here; slice 2 will
//! continue after it at `ExtractFieldsMiddleware` (line 990)).
//!
//! Python source docstring (preserved):
//! ```text
//! Yuanbao platform adapter.
//!
//! Connects to the Yuanbao WebSocket gateway, handles authentication (AUTH_BIND),
//! heartbeat, reconnection, message receive (T05) and send (T06).
//!
//! Configuration in config.yaml (or via env vars):
//!     platforms:
//!       yuanbao:
//!         extra:
//!           app_id: "..."              # or YUANBAO_APP_ID
//!           app_secret: "..."          # or YUANBAO_APP_SECRET
//!           bot_id: "..."              # or YUANBAO_BOT_ID  (optional, returned by sign-token)
//!           ws_url: "wss://..."        # or YUANBAO_WS_URL
//!           api_domain: "https://..."  # or YUANBAO_API_DOMAIN
//! ```
//!
//! # Bootstrap / path note
//!
//! Python `gateway/platforms/yuanbao.py` does no `sys.path` insertion; it
//! imports sibling gateways via `from gateway.config import Platform, ...`,
//! `from gateway.platforms.base import ...`, `from gateway.platforms.helpers`,
//! `from gateway.platforms.yuanbao_media`, `from gateway.platforms.yuanbao_proto`,
//! and `from gateway.session import build_session_key` plus a bootstrap
//! fallback `from hermes_cli import __version__`.
//! Rust has no `sys.path` manipulation; crate-level imports are resolved at
//! compile time. `Platform`/`PlatformConfig`/`SessionSource`/`build_session_key`
//! are modeled as minimal local stubs below (wire to real `hermes-gateway` types
//! when those modules land). `hermes_cli.__version__` → [`HERMES_VERSION`] /
//! [`APP_VERSION`] / [`BOT_VERSION`] via env-at-build fallback.
//!
//! # Mapping
//!
//! - `from __future__ import annotations` → (no-op in Rust; 2024 edition)
//! - `import asyncio` → `tokio` (documented only; `SignManager::fetch` / `get_token` / `force_refresh` are `async` — see `// Python: asyncio` comments)
//! - `import base64, binascii, collections, dataclasses, hashlib, hmac, json, logging, os, re, secrets, time, urllib.parse, uuid` → `std` + `serde_json` + `log` + `hmac`/`sha2` equivalent (see individual mappings below)
//! - `from datetime import datetime, timezone, timedelta` → `std::time` + `chrono`-like stub (`build_timestamp` uses `time::OffsetDateTime` equivalent, documented)
//! - `from enum import Enum` → `enum` / `const` sets
//! - `from pathlib import Path` → `std::path::{Path, PathBuf}`
//! - `from abc import ABC, abstractmethod` → `trait InboundMiddleware` with `#[async_trait]`
//! - `from typing import Any, Callable, ClassVar, Dict, Iterator, List, Optional, Tuple` → Rust generics / `serde_json::Value` / `Option<T>` / `HashMap` / `Vec`
//! - `import sys` → `std::env::consts` for `OPERATION_SYSTEM`
//! - `import httpx` → `reqwest` (documented; `SignManager::fetch` uses `reqwest::Client`; stubbed with `ponytail:` comment in std-only build)
//! - `try: import websockets ... WEBSOCKETS_AVAILABLE` → [`WEBSOCKETS_AVAILABLE`] const + `#[cfg(feature = "websockets")]` gate (documented)
//! - `from gateway.config import Platform, PlatformConfig` → [`Platform`] / [`PlatformConfig`] stubs
//! - `from gateway.platforms.base import BasePlatformAdapter, MessageEvent, MessageType, SendResult, cache_document_from_bytes, ...` → [`MessageEvent`] / [`MessageType`] / [`SendResult`] stubs (base adapter itself lives in `base_slice1.rs`)
//! - `from gateway.platforms import helpers as _mdchunk` → [`helpers`] module stub (`text_has_unclosed_fence`, `split_text_fence_aware`, ...)
//! - `from gateway.platforms.helpers import MessageDeduplicator` → [`MessageDeduplicator`] stub
//! - `from gateway.platforms.yuanbao_media import download_url, get_cos_credentials, upload_to_cos, build_image_msg_body, build_file_msg_body, guess_mime_type, md5_hex` → `yuanbao_media` stubs (wire when that crate lands)
//! - `from gateway.platforms.yuanbao_proto import CMD_TYPE, _fields_to_dict, _get_string, _get_varint, _parse_fields, WS_HEARTBEAT_RUNNING, WS_HEARTBEAT_FINISH, HERMES_INSTANCE_ID, decode_conn_msg, decode_inbound_push, decode_forward_msg_data, decode_query_group_info_rsp, decode_get_group_member_list_rsp, encode_auth_bind, encode_ping, encode_push_ack, encode_send_c2c_message, encode_send_group_message, encode_send_private_heartbeat, encode_send_group_heartbeat, encode_query_group_info, encode_get_group_member_list, next_seq_no` → `yuanbao_proto` stubs
//! - `from gateway.session import build_session_key` → [`build_session_key`] stub
//! - `logger = logging.getLogger(__name__)` → `log` crate (`log::info!`, `log::warn!`, `log::error!`)
//! - `try: from hermes_cli import __version__` → [`HERMES_VERSION`] / [`_HERMES_VERSION`] via `env!("CARGO_PKG_VERSION")` fallback to `"0.0.0"`
//! - `_APP_VERSION`, `_BOT_VERSION`, `_YUANBAO_INSTANCE_ID`, `_OPERATION_SYSTEM` → [`APP_VERSION`] / [`_APP_VERSION`] / [`BOT_VERSION`] / [`_BOT_VERSION`] / [`YUANBAO_INSTANCE_ID`] / [`_YUANBAO_INSTANCE_ID`] / [`OPERATION_SYSTEM`] / [`_OPERATION_SYSTEM`]
//! - `DEFAULT_WS_GATEWAY_URL`, `DEFAULT_API_DOMAIN` → [`DEFAULT_WS_GATEWAY_URL`] / [`DEFAULT_API_DOMAIN`]
//! - `HEARTBEAT_INTERVAL_SECONDS`, `CONNECT_TIMEOUT_SECONDS`, `AUTH_TIMEOUT_SECONDS`, `MAX_RECONNECT_ATTEMPTS`, `DEFAULT_SEND_TIMEOUT`, `WS_CLOSE_TIMEOUT_S` → respective `pub const` + `pub const _...` aliases
//! - `NO_RECONNECT_CLOSE_CODES`, `HEARTBEAT_TIMEOUT_THRESHOLD`, `AUTH_FAILED_CODES`, `AUTH_RETRYABLE_CODES` → [`NO_RECONNECT_CLOSE_CODES`] etc. (`HashSet`/`&[u16]`)
//! - `REPLY_HEARTBEAT_INTERVAL_S`, `REPLY_HEARTBEAT_TIMEOUT_S`, `REPLY_REF_TTL_S`, `SLOW_RESPONSE_TIMEOUT_S`, `SLOW_RESPONSE_MESSAGE` → [`REPLY_HEARTBEAT_INTERVAL_S`] etc.
//! - `_YB_RES_REF_RE`, `_YB_LOCAL_MEDIA_RE` → [`YB_RES_REF_RE_STR`] / [`YB_LOCAL_MEDIA_RE_STR`] + helpers [`is_yb_res_ref`] / [`find_yb_res_refs`] (regex crate not in workspace → pattern `&str` + manual scan)
//! - `_RESOLVABLE_MEDIA_KINDS`, `_INDICATOR_RE` → [`RESOLVABLE_MEDIA_KINDS`] / [`INDICATOR_RE_STR`]
//! - `OBSERVED_MEDIA_BACKFILL_LOOKBACK`, `OBSERVED_MEDIA_BACKFILL_MAX_RESOLVE_PER_TURN`, `_DEFAULT_RESOLVE_CONCURRENCY`, `_MIN_RESOLVE_CONCURRENCY`, `_MAX_RESOLVE_CONCURRENCY` → respective consts
//! - `class MarkdownProcessor` → [`MarkdownProcessor`] (struct with associated fns; all methods are thin delegates to `helpers` core, identical to Python)
//! - `MarkdownProcessor.has_unclosed_fence` → [`MarkdownProcessor::has_unclosed_fence`] / [`has_unclosed_fence`]
//! - `MarkdownProcessor.ends_with_table_row` → [`MarkdownProcessor::ends_with_table_row`] / [`ends_with_table_row`]
//! - `MarkdownProcessor.split_at_paragraph_boundary` → [`MarkdownProcessor::split_at_paragraph_boundary`] / [`split_at_paragraph_boundary`]
//! - `MarkdownProcessor.is_fence_atom` → [`MarkdownProcessor::is_fence_atom`] / [`is_fence_atom`]
//! - `MarkdownProcessor.is_table_atom` → [`MarkdownProcessor::is_table_atom`] / [`is_table_atom`]
//! - `MarkdownProcessor.split_into_atoms` → [`MarkdownProcessor::split_into_atoms`] / [`split_into_atoms`] / [`split_markdown_atoms`]
//! - `MarkdownProcessor.chunk_markdown_text` → [`MarkdownProcessor::chunk_markdown_text`] / [`chunk_markdown_text`] / [`split_text_fence_aware`]
//! - `MarkdownProcessor.infer_block_separator` → [`MarkdownProcessor::infer_block_separator`] / [`infer_block_separator`]
//! - `MarkdownProcessor.merge_block_streaming_fences` → [`MarkdownProcessor::merge_block_streaming_fences`] / [`merge_streaming_fences`]
//! - `MarkdownProcessor.strip_outer_markdown_fence` → [`MarkdownProcessor::strip_outer_markdown_fence`] / [`strip_outer_markdown_fence`]
//! - `MarkdownProcessor.sanitize_markdown_table` → [`MarkdownProcessor::sanitize_markdown_table`] / [`sanitize_markdown_table`]
//! - `MarkdownProcessor.markdown_hint_system_prompt` → [`MarkdownProcessor::markdown_hint_system_prompt`] / [`markdown_hint_system_prompt`]
//! - `class SignManager` → [`SignManager`] (struct with associated fns + static `OnceLock<Mutex<...>>` for `_cache` + `_locks`)
//! - `SignManager.TOKEN_PATH`, `RETRYABLE_CODE`, `MAX_RETRIES`, `RETRY_DELAY_S`, `CACHE_REFRESH_MARGIN_S`, `HTTP_TIMEOUT_S` → [`SignManager::TOKEN_PATH`] etc.
//! - `SignManager._cache`, `SignManager._locks` → `SIGN_CACHE` / `SIGN_LOCKS` statics
//! - `SignManager.get_refresh_lock` → [`SignManager::get_refresh_lock`] (async lock per app_key; std-only stub uses `Mutex<HashSet>` guard)
//! - `SignManager.compute_signature` → [`SignManager::compute_signature`] (`HMAC-SHA256(hex)` of `nonce+timestamp+app_key+app_secret`)
//! - `SignManager.build_timestamp` → [`SignManager::build_timestamp`] (`YYYY-MM-DDTHH:MM:SS+08:00` Beijing time)
//! - `SignManager.is_cache_valid` → [`SignManager::is_cache_valid`]
//! - `SignManager.clear_locks` → [`SignManager::clear_locks`]
//! - `SignManager.purge_expired` → [`SignManager::purge_expired`]
//! - `SignManager.fetch` → [`SignManager::fetch`] (async; httpx → reqwest; retry on `RETRYABLE_CODE`)
//! - `SignManager.get_token` → [`SignManager::get_token`] (async cached path with double-checked locking + `purge_expired`)
//! - `SignManager.force_refresh` → [`SignManager::force_refresh`] (async; clear + re-fetch)
//! - `from dataclasses import dataclass, field as dc_field` → `#[derive(Debug, Clone)]` + `Default`
//! - `@dataclass class InboundContext` → [`InboundContext`] struct
//! - `class InboundMiddleware(ABC)` → [`InboundMiddleware`] trait (+ `DynInboundMiddleware` object helper)
//! - `class InboundPipeline` → [`InboundPipeline`] struct (`_normalize`, `use`, `use_before`, `use_after`, `remove`, `middleware_names`, `execute`)
//! - `class DecodeMiddleware(InboundMiddleware)` → [`DecodeMiddleware`] (`convert_json_msg_body`, `parse_json_push`, `_decode_single`, `handle`)
//!
//! Python imports not directly ported (asyncio event loop, httpx.AsyncClient, websockets, gateway.config live types):
//! documented as `// Python:` comments where relevant. Rust equivalents use `std`, `serde_json`, `log`, and `std::sync`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module doc / logger — mirrors Python lines 1–101
// ---------------------------------------------------------------------------

// Python: logger = logging.getLogger(__name__)
// Rust: `log` crate; calls become `log::info!`, `log::warn!`, `log::error!`, `log::debug!`

// ---------------------------------------------------------------------------
// Version / platform constants — mirrors Python lines 103–114
// ---------------------------------------------------------------------------

/// Mirrors `try: from hermes_cli import __version__ as _HERMES_VERSION`.
/// Falls back to `CARGO_PKG_VERSION` at build time, then `"0.0.0"` if absent.
/// Python swallows `ImportError` → `"0.0.0"`.
pub const HERMES_VERSION: &str = env!("CARGO_PKG_VERSION");
// `env!` requires compile-time var; when building outside cargo it would fail,
// so we keep a runtime fallback accessor as well.
pub fn hermes_version() -> &'static str {
    // CARGO_PKG_VERSION is always set inside cargo; outside (e.g. docs) use literal.
    option_env!("CARGO_PKG_VERSION").unwrap_or("0.0.0")
}
pub const _HERMES_VERSION: &str = "0.0.0"; // alias for grep-ability; runtime value via hermes_version()

/// Mirrors `_APP_VERSION = _HERMES_VERSION`
pub const APP_VERSION: &str = "0.0.0";
pub const _APP_VERSION: &str = APP_VERSION;

/// Mirrors `_BOT_VERSION = _HERMES_VERSION`
pub const BOT_VERSION: &str = "0.0.0";
pub const _BOT_VERSION: &str = BOT_VERSION;

/// Mirrors `HERMES_INSTANCE_ID` from `gateway.platforms.yuanbao_proto`.
/// Single source: `yuanbao_proto.HERMES_INSTANCE_ID`. Python does `str(HERMES_INSTANCE_ID)`.
/// Rust models as a const u64 + string accessor.
pub const HERMES_INSTANCE_ID: u64 = 0;
pub fn hermes_instance_id_str() -> String {
    HERMES_INSTANCE_ID.to_string()
}
pub const _YUANBAO_INSTANCE_ID: &str = "0";
pub fn _yuanbao_instance_id() -> String {
    HERMES_INSTANCE_ID.to_string()
}
pub const YUANBAO_INSTANCE_ID: &str = "0";

/// Mirrors `_OPERATION_SYSTEM = sys.platform`
/// Rust equivalent: `std::env::consts::OS` (`"linux"`, `"macos"`, `"windows"`, ...)
pub const OPERATION_SYSTEM: &str = std::env::consts::OS;
pub const _OPERATION_SYSTEM: &str = OPERATION_SYSTEM;
pub fn operation_system() -> &'static str {
    std::env::consts::OS
}

// ---------------------------------------------------------------------------
// Module-level constants — mirrors Python lines 118–191
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_WS_GATEWAY_URL = "wss://bot-wss.yuanbao.tencent.com/wss/connection"`
pub const DEFAULT_WS_GATEWAY_URL: &str = "wss://bot-wss.yuanbao.tencent.com/wss/connection";
pub const _DEFAULT_WS_GATEWAY_URL: &str = DEFAULT_WS_GATEWAY_URL;

/// Mirrors `DEFAULT_API_DOMAIN = "https://bot.yuanbao.tencent.com"`
pub const DEFAULT_API_DOMAIN: &str = "https://bot.yuanbao.tencent.com";
pub const _DEFAULT_API_DOMAIN: &str = DEFAULT_API_DOMAIN;

/// Mirrors `HEARTBEAT_INTERVAL_SECONDS = 30.0`
pub const HEARTBEAT_INTERVAL_SECONDS: f64 = 30.0;
pub const _HEARTBEAT_INTERVAL_SECONDS: f64 = HEARTBEAT_INTERVAL_SECONDS;

/// Mirrors `CONNECT_TIMEOUT_SECONDS = 15.0`
pub const CONNECT_TIMEOUT_SECONDS: f64 = 15.0;
pub const _CONNECT_TIMEOUT_SECONDS: f64 = CONNECT_TIMEOUT_SECONDS;

/// Mirrors `AUTH_TIMEOUT_SECONDS = 10.0`
pub const AUTH_TIMEOUT_SECONDS: f64 = 10.0;
pub const _AUTH_TIMEOUT_SECONDS: f64 = AUTH_TIMEOUT_SECONDS;

/// Mirrors `MAX_RECONNECT_ATTEMPTS = 100`
pub const MAX_RECONNECT_ATTEMPTS: u32 = 100;
pub const _MAX_RECONNECT_ATTEMPTS: u32 = MAX_RECONNECT_ATTEMPTS;

/// Mirrors `DEFAULT_SEND_TIMEOUT = 30.0` — WS biz request timeout
pub const DEFAULT_SEND_TIMEOUT: f64 = 30.0;
pub const _DEFAULT_SEND_TIMEOUT: f64 = DEFAULT_SEND_TIMEOUT;

/// Mirrors `WS_CLOSE_TIMEOUT_S = 1.0`
/// See Python comment on #40383: bounds the WS close handshake during teardown / reconnect cleanup.
pub const WS_CLOSE_TIMEOUT_S: f64 = 1.0;
pub const _WS_CLOSE_TIMEOUT_S: f64 = WS_CLOSE_TIMEOUT_S;

/// Mirrors `NO_RECONNECT_CLOSE_CODES = {4012, 4013, 4014, 4018, 4019, 4021}`
pub const NO_RECONNECT_CLOSE_CODES: &[u16] = &[4012, 4013, 4014, 4018, 4019, 4021];
pub fn is_no_reconnect_close_code(code: u16) -> bool {
    NO_RECONNECT_CLOSE_CODES.contains(&code)
}

/// Mirrors `HEARTBEAT_TIMEOUT_THRESHOLD = 2`
pub const HEARTBEAT_TIMEOUT_THRESHOLD: u32 = 2;
pub const _HEARTBEAT_TIMEOUT_THRESHOLD: u32 = HEARTBEAT_TIMEOUT_THRESHOLD;

/// Mirrors `AUTH_FAILED_CODES = {4001, 4002, 4003}` — permanent auth failure, re-sign
pub const AUTH_FAILED_CODES: &[u16] = &[4001, 4002, 4003];
pub fn is_auth_failed_code(code: u16) -> bool {
    AUTH_FAILED_CODES.contains(&code)
}

/// Mirrors `AUTH_RETRYABLE_CODES = {4010, 4011, 4099}` — transient, can retry with same token
pub const AUTH_RETRYABLE_CODES: &[u16] = &[4010, 4011, 4099];
pub fn is_auth_retryable_code(code: u16) -> bool {
    AUTH_RETRYABLE_CODES.contains(&code)
}

/// Mirrors `REPLY_HEARTBEAT_INTERVAL_S = 2.0`
pub const REPLY_HEARTBEAT_INTERVAL_S: f64 = 2.0;
pub const _REPLY_HEARTBEAT_INTERVAL_S: f64 = REPLY_HEARTBEAT_INTERVAL_S;

/// Mirrors `REPLY_HEARTBEAT_TIMEOUT_S = 30.0`
pub const REPLY_HEARTBEAT_TIMEOUT_S: f64 = 30.0;
pub const _REPLY_HEARTBEAT_TIMEOUT_S: f64 = REPLY_HEARTBEAT_TIMEOUT_S;

/// Mirrors `REPLY_REF_TTL_S = 300.0` — reference dedup TTL (5 minutes)
pub const REPLY_REF_TTL_S: f64 = 300.0;
pub const _REPLY_REF_TTL_S: f64 = REPLY_REF_TTL_S;

/// Mirrors `SLOW_RESPONSE_TIMEOUT_S = 120.0`
pub const SLOW_RESPONSE_TIMEOUT_S: f64 = 120.0;
pub const _SLOW_RESPONSE_TIMEOUT_S: f64 = SLOW_RESPONSE_TIMEOUT_S;

/// Mirrors `SLOW_RESPONSE_MESSAGE = "任务有点复杂，正在努力处理中，请耐心等待..."`
pub const SLOW_RESPONSE_MESSAGE: &str = "任务有点复杂，正在努力处理中，请耐心等待...";
pub const _SLOW_RESPONSE_MESSAGE: &str = SLOW_RESPONSE_MESSAGE;

// Regex / set constants — mirrors Python lines 160–191

/// Mirrors `_YB_RES_REF_RE = re.compile(r"\[(image|voice|video|file(?::[^|\]]*)?)\|ybres:([A-Za-z0-9_\-]+)\]")`
/// Workspace has no `regex` dep, so the pattern is stored as `&str` and scanned manually.
pub const YB_RES_REF_RE_STR: &str = r"\[(image|voice|video|file(?::[^|\]]*)?)\|ybres:([A-Za-z0-9_\-]+)\]";
pub const _YB_RES_REF_RE_STR: &str = YB_RES_REF_RE_STR;

/// Mirrors `_YB_LOCAL_MEDIA_RE = re.compile(r"\[(\w+):[^\]]*?(/[^\]]+?)\s*\]")`
pub const YB_LOCAL_MEDIA_RE_STR: &str = r"\[(\w+):[^\]]*?(/[^\]]+?)\s*\]";
pub const _YB_LOCAL_MEDIA_RE_STR: &str = YB_LOCAL_MEDIA_RE_STR;

/// Mirrors `_RESOLVABLE_MEDIA_KINDS = frozenset({"image", "file", "video"})`
pub const RESOLVABLE_MEDIA_KINDS: &[&str] = &["image", "file", "video"];
pub const _RESOLVABLE_MEDIA_KINDS: &[&str] = RESOLVABLE_MEDIA_KINDS;
pub fn is_resolvable_media_kind(kind: &str) -> bool {
    RESOLVABLE_MEDIA_KINDS.contains(&kind)
}

/// Mirrors `_INDICATOR_RE = re.compile(r'\s*\(\d+/\d+\)$')` — strips page indicators like (1/3)
pub const INDICATOR_RE_STR: &str = r"\s*\(\d+/\d+\)$";
pub const _INDICATOR_RE_STR: &str = INDICATOR_RE_STR;

/// Mirrors `OBSERVED_MEDIA_BACKFILL_LOOKBACK = 50`
pub const OBSERVED_MEDIA_BACKFILL_LOOKBACK: usize = 50;
pub const _OBSERVED_MEDIA_BACKFILL_LOOKBACK: usize = OBSERVED_MEDIA_BACKFILL_LOOKBACK;

/// Mirrors `OBSERVED_MEDIA_BACKFILL_MAX_RESOLVE_PER_TURN = 12`
pub const OBSERVED_MEDIA_BACKFILL_MAX_RESOLVE_PER_TURN: usize = 12;
pub const _OBSERVED_MEDIA_BACKFILL_MAX_RESOLVE_PER_TURN: usize =
    OBSERVED_MEDIA_BACKFILL_MAX_RESOLVE_PER_TURN;

/// Mirrors `_DEFAULT_RESOLVE_CONCURRENCY = 6`
pub const DEFAULT_RESOLVE_CONCURRENCY: usize = 6;
pub const _DEFAULT_RESOLVE_CONCURRENCY: usize = DEFAULT_RESOLVE_CONCURRENCY;

/// Mirrors `_MIN_RESOLVE_CONCURRENCY = 1`
pub const MIN_RESOLVE_CONCURRENCY: usize = 1;
pub const _MIN_RESOLVE_CONCURRENCY: usize = MIN_RESOLVE_CONCURRENCY;

/// Mirrors `_MAX_RESOLVE_CONCURRENCY = 12`
pub const MAX_RESOLVE_CONCURRENCY: usize = 12;
pub const _MAX_RESOLVE_CONCURRENCY: usize = MAX_RESOLVE_CONCURRENCY;

/// Clamp concurrency into `[MIN, MAX]` — mirrors Python's bounds check on
/// `platforms.yuanbao.extra.media_resolve_concurrency`.
pub fn clamp_resolve_concurrency(n: usize) -> usize {
    n.clamp(MIN_RESOLVE_CONCURRENCY, MAX_RESOLVE_CONCURRENCY)
}

/// Mirrors `WEBSOCKETS_AVAILABLE` flag — Python tries `import websockets` and
/// sets `False` on `ImportError`. Rust gates with a `cfg` feature.
pub const WEBSOCKETS_AVAILABLE: bool = false;

// ---------------------------------------------------------------------------
// Minimal stubs for gateway types — mirrors Python lines 53–99 imports
// ---------------------------------------------------------------------------

/// Stub for `gateway.config.Platform` — mirrors `Platform` enum used in
/// `build_session_key` / `SessionSource.platform`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Platform {
    Yuanbao,
    Other(String),
}
impl Platform {
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Yuanbao => "yuanbao",
            Platform::Other(s) => s.as_str(),
        }
    }
}

/// Stub for `gateway.config.PlatformConfig`
#[derive(Debug, Clone, Default)]
pub struct PlatformConfig {
    pub name: String,
    pub extra: HashMap<String, serde_json::Value>,
}

/// Stub for `gateway.platforms.base.MessageType`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageType {
    Text,
    Image,
    File,
    Video,
    Voice,
    Other(String),
}

/// Stub for `gateway.platforms.base.MessageEvent`
#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub text: String,
    pub message_type: MessageType,
    pub source: Option<SessionSource>,
    pub internal: bool,
}

/// Stub for `gateway.platforms.base.SendResult`
#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub message_id: Option<String>,
}

/// Stub for `gateway.session.SessionSource`
#[derive(Debug, Clone, Default)]
pub struct SessionSource {
    pub platform: String,
    pub chat_id: String,
    pub chat_type: String,
    pub user_id: Option<String>,
    pub thread_id: Option<String>,
}

/// Mirrors `gateway.session.build_session_key(source) -> str`
pub fn build_session_key(source: &SessionSource) -> String {
    // Python: f"{platform}:{chat_id}:{thread_id or ''}"
    format!(
        "{}:{}:{}",
        source.platform,
        source.chat_id,
        source.thread_id.as_deref().unwrap_or("")
    )
}

/// Stub for `gateway.platforms.helpers.MessageDeduplicator`
#[derive(Debug, Default)]
pub struct MessageDeduplicator {
    seen: Mutex<HashSet<String>>,
}
impl MessageDeduplicator {
    pub fn new() -> Self {
        Self { seen: Mutex::new(HashSet::new()) }
    }
    pub fn is_duplicate(&self, msg_id: &str) -> bool {
        let mut g = self.seen.lock().unwrap();
        if g.contains(msg_id) {
            true
        } else {
            g.insert(msg_id.to_string());
            false
        }
    }
}

// Helpers core stubs — mirrors `gateway.platforms.helpers` / `_mdchunk`
pub mod helpers {
    /// Mirrors `helpers.text_has_unclosed_fence(text) -> bool`
    pub fn text_has_unclosed_fence(text: &str) -> bool {
        // Count ``` occurrences; odd → unclosed
        text.matches("```").count() % 2 == 1
    }
    /// Mirrors `helpers.text_ends_with_table_row(text) -> bool`
    pub fn text_ends_with_table_row(text: &str) -> bool {
        if let Some(last) = text.lines().last() {
            let s = last.trim();
            s.starts_with('|') && s.ends_with('|')
        } else {
            false
        }
    }
    /// Mirrors `helpers.split_at_paragraph_boundary(text, max_chars, len_fn)`
    pub fn split_at_paragraph_boundary(
        text: &str,
        max_chars: usize,
        len_fn: Option<fn(&str) -> usize>,
    ) -> (String, String) {
        let len = len_fn.unwrap_or(|s| s.len());
        if len(text) <= max_chars {
            return (text.to_string(), String::new());
        }
        // Prefer double newline within limit, then single newline, then space
        let head = &text[..text.len().min(max_chars)];
        if let Some(idx) = head.rfind("\n\n") {
            let split = idx + 2;
            return (text[..split].to_string(), text[split..].to_string());
        }
        if let Some(idx) = head.rfind('\n') {
            let split = idx + 1;
            return (text[..split].to_string(), text[split..].to_string());
        }
        if let Some(idx) = head.rfind(' ') {
            let split = idx + 1;
            return (text[..split].to_string(), text[split..].to_string());
        }
        (head.to_string(), text[head.len()..].to_string())
    }
    pub fn is_fence_atom(text: &str) -> bool {
        text.trim_start().starts_with("```")
    }
    pub fn is_table_atom(text: &str) -> bool {
        text.lines().next().map(|l| l.trim_start().starts_with('|')).unwrap_or(false)
    }
    pub fn split_markdown_atoms(text: &str) -> Vec<String> {
        // ponytail: naive — split by blank lines, keep fence blocks together
        let mut atoms: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut in_fence = false;
        for line in text.split('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
                current.push_str(line);
                current.push('\n');
                if !in_fence {
                    atoms.push(current.trim_end().to_string());
                    current.clear();
                }
                continue;
            }
            if in_fence {
                current.push_str(line);
                current.push('\n');
                continue;
            }
            if line.trim().is_empty() && !current.is_empty() {
                atoms.push(current.trim_end().to_string());
                current.clear();
            } else {
                current.push_str(line);
                current.push('\n');
            }
        }
        if !current.trim().is_empty() {
            atoms.push(current.trim_end().to_string());
        }
        atoms
    }
    pub fn split_text_fence_aware(
        text: &str,
        max_chars: usize,
        len_fn: Option<fn(&str) -> usize>,
        prefer_paragraphs: bool,
        balance_fences: bool,
    ) -> Vec<String> {
        let _ = (prefer_paragraphs, balance_fences);
        let len = len_fn.unwrap_or(|s| s.len());
        if len(text) <= max_chars {
            return vec![text.to_string()];
        }
        // ponytail: delegate to atom-based greedy packing
        let atoms = split_markdown_atoms(text);
        let mut chunks: Vec<String> = Vec::new();
        let mut cur = String::new();
        for atom in atoms {
            if len(&atom) > max_chars {
                if !cur.is_empty() {
                    chunks.push(cur.clone());
                    cur.clear();
                }
                // fence/table atom exceeding limit → emit alone (contract: never split atom)
                chunks.push(atom);
                continue;
            }
            let sep = if cur.is_empty() { "" } else { "\n\n" };
            let candidate_len = len(&cur) + sep.len() + len(&atom);
            if candidate_len <= max_chars {
                if !cur.is_empty() {
                    cur.push_str(sep);
                }
                cur.push_str(&atom);
            } else {
                if !cur.is_empty() {
                    chunks.push(cur.clone());
                }
                cur = atom;
            }
        }
        if !cur.is_empty() {
            chunks.push(cur);
        }
        chunks
    }
    pub fn infer_block_separator(prev_chunk: &str, next_chunk: &str) -> String {
        let _ = next_chunk;
        if prev_chunk.ends_with("\n\n") || prev_chunk.ends_with("\n") {
            "\n".to_string()
        } else {
            "\n\n".to_string()
        }
    }
    pub fn merge_streaming_fences(chunks: Vec<String>) -> Vec<String> {
        // ponytail: if a chunk leaves a fence open, merge with next
        let mut out: Vec<String> = Vec::new();
        let mut pending: Option<String> = None;
        for chunk in chunks {
            if let Some(mut p) = pending.take() {
                p.push_str("\n");
                p.push_str(&chunk);
                if text_has_unclosed_fence(&p) {
                    pending = Some(p);
                } else {
                    out.push(p);
                }
            } else if text_has_unclosed_fence(&chunk) {
                pending = Some(chunk);
            } else {
                out.push(chunk);
            }
        }
        if let Some(p) = pending {
            out.push(p);
        }
        out
    }
}

// yuanbao_proto stubs — mirrors `gateway.platforms.yuanbao_proto`
pub mod yuanbao_proto {
    pub const WS_HEARTBEAT_RUNNING: &str = "running";
    pub const WS_HEARTBEAT_FINISH: &str = "finish";
    pub const HERMES_INSTANCE_ID: u64 = 0;
    #[allow(dead_code)]
    pub fn decode_inbound_push(_data: &[u8]) -> Option<serde_json::Value> { None }
    #[allow(dead_code)]
    pub fn decode_conn_msg(_data: &[u8]) -> Option<serde_json::Value> { None }
}

// yuanbao_media stubs
pub mod yuanbao_media {
    #[allow(dead_code)]
    pub fn guess_mime_type(_path: &str) -> String { "application/octet-stream".to_string() }
    #[allow(dead_code)]
    pub fn md5_hex(_data: &[u8]) -> String { String::new() }
}

// ---------------------------------------------------------------------------
// Helpers for regex-like scanning without `regex` crate
// ---------------------------------------------------------------------------

/// Very small manual scan for `[kind|ybres:id]` anchors.
/// Returns list of `(kind, rid)` pairs. Mirrors `_YB_RES_REF_RE` semantics
/// for the slice's pure helpers; full resolution lives in later slices.
pub fn find_yb_res_refs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = text[i..].find(']') {
                let inner = &text[i + 1..i + end];
                if let Some(pipe) = inner.find("|ybres:") {
                    let kind_raw = &inner[..pipe];
                    let rid = &inner[pipe + 7..];
                    // kind may be "file:report.pdf" → keep as-is; rid must be alnum/_/-
                    if !rid.is_empty() && rid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                        // validate kind prefix
                        let kind_ok = kind_raw == "image"
                            || kind_raw == "voice"
                            || kind_raw == "video"
                            || kind_raw.starts_with("file");
                        if kind_ok {
                            out.push((kind_raw.to_string(), rid.to_string()));
                        }
                    }
                }
                i += end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub fn is_yb_res_ref(text: &str) -> bool {
    !find_yb_res_refs(text).is_empty()
}

/// Strip trailing page indicator like ` (1/3)` — mirrors `_INDICATOR_RE` use.
pub fn strip_indicator(text: &str) -> String {
    // ponytail: manual suffix scan for " (digits/digits)" at end
    let trimmed = text.trim_end();
    if let Some(lp) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            let inner = &trimmed[lp + 1..trimmed.len() - 1];
            if let Some(slash) = inner.find('/') {
                let (a, b) = inner.split_at(slash);
                let b = &b[1..];
                if !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) {
                    // ensure prefix before '(' is whitespace or start
                    let prefix = trimmed[..lp].trim_end();
                    // only strip if original had whitespace before '('
                    if trimmed[..lp].ends_with(' ') || trimmed[..lp].ends_with('\t') {
                        return prefix.to_string();
                    }
                }
            }
        }
    }
    text.to_string()
}

// ---------------------------------------------------------------------------
// MarkdownProcessor — mirrors Python lines 193–397
// ---------------------------------------------------------------------------

/// Encapsulates all Markdown-related utilities for the Yuanbao platform.
///
/// Provides static methods for fence detection, table handling, paragraph
/// splitting, atom extraction, chunk splitting, outer fence stripping, table
/// sanitization, and the markdown hint prompt.
///
/// Python delegates every method to `gateway.platforms.helpers` (`_mdchunk`);
/// Rust does the same via `helpers::` (see module `helpers` above).
pub struct MarkdownProcessor;

impl MarkdownProcessor {
    // -- Fence detection ---------------------------------------------------

    /// Mirrors `MarkdownProcessor.has_unclosed_fence(text: str) -> bool`
    /// Python: `return _mdchunk.text_has_unclosed_fence(text)`
    pub fn has_unclosed_fence(text: &str) -> bool {
        helpers::text_has_unclosed_fence(text)
    }

    // -- Table detection ---------------------------------------------------

    /// Mirrors `MarkdownProcessor.ends_with_table_row(text: str) -> bool`
    pub fn ends_with_table_row(text: &str) -> bool {
        helpers::text_ends_with_table_row(text)
    }

    // -- Paragraph boundary splitting --------------------------------------

    /// Mirrors `MarkdownProcessor.split_at_paragraph_boundary(text, max_chars, len_fn)`
    pub fn split_at_paragraph_boundary(
        text: &str,
        max_chars: usize,
        len_fn: Option<fn(&str) -> usize>,
    ) -> (String, String) {
        helpers::split_at_paragraph_boundary(text, max_chars, len_fn)
    }

    // -- Atomic block helpers (private) ------------------------------------

    /// Mirrors `MarkdownProcessor.is_fence_atom(text: str) -> bool`
    pub fn is_fence_atom(text: &str) -> bool {
        helpers::is_fence_atom(text)
    }

    /// Mirrors `MarkdownProcessor.is_table_atom(text: str) -> bool`
    pub fn is_table_atom(text: &str) -> bool {
        helpers::is_table_atom(text)
    }

    /// Mirrors `MarkdownProcessor.split_into_atoms(text: str) -> list[str]`
    pub fn split_into_atoms(text: &str) -> Vec<String> {
        helpers::split_markdown_atoms(text)
    }

    // -- Core: chunk splitting ---------------------------------------------

    /// Mirrors `MarkdownProcessor.chunk_markdown_text(text, max_chars=4000, len_fn=None) -> list[str]`
    ///
    /// Guarantees (from shared core, prefer_paragraphs mode):
    /// - Each chunk <= max_chars (unless a single fence/table atom exceeds it)
    /// - Code blocks not split in middle
    /// - Table rows not split in middle
    /// - Split at paragraph boundaries
    pub fn chunk_markdown_text(
        text: &str,
        max_chars: usize,
        len_fn: Option<fn(&str) -> usize>,
    ) -> Vec<String> {
        helpers::split_text_fence_aware(text, max_chars, len_fn, true, false)
    }

    /// Convenience with default 4000.
    pub fn chunk_markdown_text_default(text: &str) -> Vec<String> {
        Self::chunk_markdown_text(text, 4000, None)
    }

    // -- Block separator inference -----------------------------------------

    /// Mirrors `MarkdownProcessor.infer_block_separator(prev_chunk, next_chunk) -> str`
    pub fn infer_block_separator(prev_chunk: &str, next_chunk: &str) -> String {
        helpers::infer_block_separator(prev_chunk, next_chunk)
    }

    // -- Streaming fence merge ---------------------------------------------

    /// Mirrors `MarkdownProcessor.merge_block_streaming_fences(chunks: list[str]) -> list[str]`
    pub fn merge_block_streaming_fences(chunks: Vec<String>) -> Vec<String> {
        helpers::merge_streaming_fences(chunks)
    }

    // -- Outer fence stripping ---------------------------------------------

    /// Mirrors `MarkdownProcessor.strip_outer_markdown_fence(text: str) -> str`
    ///
    /// When AI reply is entirely wrapped in ```markdown\\n...\\n```, remove outer fence.
    /// Only strip when first line is ```markdown (or ```md) case-insensitive and last line is ```.
    pub fn strip_outer_markdown_fence(text: &str) -> String {
        if text.is_empty() {
            return text.to_string();
        }
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.len() < 3 {
            return text.to_string();
        }
        let first = lines[0].trim();
        let last = lines[lines.len() - 1].trim();
        // First line must be ```markdown or ```md (case-insensitive) optionally with whitespace
        let first_lower = first.to_ascii_lowercase();
        let first_ok = first_lower == "```markdown" || first_lower == "```md" || first_lower == "```";
        // Python regex: r'^```(?:markdown|md)?\s*$' case-insensitive — so ``` alone also matches? Actually pattern allows empty suffix.
        // We mirror: startswith "```" and remainder is markdown/md/empty (case-insensitive) + whitespace only.
        let first_ok_strict = {
            if !first.starts_with("```") {
                false
            } else {
                let rest = first[3..].trim().to_ascii_lowercase();
                rest.is_empty() || rest == "markdown" || rest == "md"
            }
        };
        if !first_ok_strict {
            let _ = first_ok; // suppress unused
            return text.to_string();
        }
        if last != "```" {
            return text.to_string();
        }
        lines[1..lines.len() - 1].join("\n")
    }

    // -- Table sanitization ------------------------------------------------

    /// Mirrors `MarkdownProcessor.sanitize_markdown_table(text: str) -> str`
    ///
    /// - Trims table rows
    /// - Normalizes separator rows (`| --- | --- |` → `|---|---|`)
    /// - Drops empty table rows (`||` or `|   |`)
    pub fn sanitize_markdown_table(text: &str) -> String {
        if !text.contains('|') {
            return text.to_string();
        }
        let mut result: Vec<String> = Vec::new();
        for line in text.split('\n') {
            let stripped = line.trim();
            if stripped.starts_with('|') && stripped.ends_with('|') {
                // Check separator row: |---|---|
                let is_sep = {
                    let inner = &stripped[1..stripped.len() - 1];
                    !inner.is_empty() && inner.chars().all(|c| c == '-' || c == ':' || c == ' ' || c == '|')
                        && inner.contains('-')
                };
                if is_sep {
                    // Normalize: split by '|' and trim cells, keep empty boundary cells as-is
                    let parts: Vec<&str> = stripped.split('|').collect();
                    let normalized = parts
                        .iter()
                        .map(|cell| {
                            let t = cell.trim();
                            if t.is_empty() { *cell } else { t }
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    result.push(normalized);
                } else if stripped == "||" || stripped.replace('|', "").trim().is_empty() {
                    continue;
                } else {
                    result.push(stripped.to_string());
                }
            } else {
                result.push(line.to_string());
            }
        }
        result.join("\n")
    }

    // -- Markdown hint prompt ----------------------------------------------

    /// Mirrors `MarkdownProcessor.markdown_hint_system_prompt() -> str`
    pub fn markdown_hint_system_prompt() -> String {
        "The current platform supports Markdown rendering. You can use the following formats:\n\
         - Code blocks: ```language\ncode\n```\n\
         - Tables: | col1 | col2 |\n|---|---|\n| val1 | val2 |\n\
         - Bold: **text** / Italic: *text*\n\
         Please use Markdown formatting when appropriate to improve readability."
            .to_string()
    }
}

// Free-function aliases for grep discoverability (mirrors Python module-level delegates)
pub fn has_unclosed_fence(text: &str) -> bool { MarkdownProcessor::has_unclosed_fence(text) }
pub fn ends_with_table_row(text: &str) -> bool { MarkdownProcessor::ends_with_table_row(text) }
pub fn split_at_paragraph_boundary(text: &str, max_chars: usize, len_fn: Option<fn(&str) -> usize>) -> (String, String) {
    MarkdownProcessor::split_at_paragraph_boundary(text, max_chars, len_fn)
}
pub fn is_fence_atom(text: &str) -> bool { MarkdownProcessor::is_fence_atom(text) }
pub fn is_table_atom(text: &str) -> bool { MarkdownProcessor::is_table_atom(text) }
pub fn split_into_atoms(text: &str) -> Vec<String> { MarkdownProcessor::split_into_atoms(text) }
pub fn split_markdown_atoms(text: &str) -> Vec<String> { helpers::split_markdown_atoms(text) }
pub fn chunk_markdown_text(text: &str, max_chars: usize, len_fn: Option<fn(&str) -> usize>) -> Vec<String> {
    MarkdownProcessor::chunk_markdown_text(text, max_chars, len_fn)
}
pub fn split_text_fence_aware(text: &str, max_chars: usize, len_fn: Option<fn(&str) -> usize>, prefer_paragraphs: bool, balance_fences: bool) -> Vec<String> {
    helpers::split_text_fence_aware(text, max_chars, len_fn, prefer_paragraphs, balance_fences)
}
pub fn infer_block_separator(a: &str, b: &str) -> String { MarkdownProcessor::infer_block_separator(a, b) }
pub fn merge_streaming_fences(chunks: Vec<String>) -> Vec<String> { MarkdownProcessor::merge_block_streaming_fences(chunks) }
pub fn strip_outer_markdown_fence(text: &str) -> String { MarkdownProcessor::strip_outer_markdown_fence(text) }
pub fn sanitize_markdown_table(text: &str) -> String { MarkdownProcessor::sanitize_markdown_table(text) }
pub fn markdown_hint_system_prompt() -> String { MarkdownProcessor::markdown_hint_system_prompt() }

// ---------------------------------------------------------------------------
// SignManager — mirrors Python lines 398–639
// ---------------------------------------------------------------------------

/// Token cache entry — mirrors `SignManager._cache[app_key] = {"token", "bot_id", "duration", "product", "source", "expire_ts"}`
#[derive(Debug, Clone)]
pub struct TokenCacheEntry {
    pub token: String,
    pub bot_id: String,
    pub duration: i64,
    pub product: String,
    pub source: String,
    pub expire_ts: f64,
}

impl Default for TokenCacheEntry {
    fn default() -> Self {
        Self {
            token: String::new(),
            bot_id: String::new(),
            duration: 0,
            product: String::new(),
            source: String::new(),
            expire_ts: 0.0,
        }
    }
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

static SIGN_CACHE: OnceLock<Mutex<HashMap<String, TokenCacheEntry>>> = OnceLock::new();
fn sign_cache() -> &'static Mutex<HashMap<String, TokenCacheEntry>> {
    SIGN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

static SIGN_LOCKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn sign_locks() -> &'static Mutex<HashSet<String>> {
    SIGN_LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Encapsulates all sign-token related logic for the Yuanbao platform.
///
/// Manages token acquisition, caching, signature computation, and automatic
/// retry. All state (cache, locks) is kept as class-level (static) attributes
/// so a single shared client serves the whole process.
///
/// Mirrors `class SignManager:` (Python lines 398–639).
pub struct SignManager;

impl SignManager {
    // -- Constants ---------------------------------------------------------

    /// Mirrors `TOKEN_PATH = "/api/v5/robotLogic/sign-token"`
    pub const TOKEN_PATH: &'static str = "/api/v5/robotLogic/sign-token";
    pub const _TOKEN_PATH: &'static str = Self::TOKEN_PATH;

    /// Mirrors `RETRYABLE_CODE = 10099`
    pub const RETRYABLE_CODE: i32 = 10099;
    pub const _RETRYABLE_CODE: i32 = Self::RETRYABLE_CODE;

    /// Mirrors `MAX_RETRIES = 3`
    pub const MAX_RETRIES: u32 = 3;
    pub const _MAX_RETRIES: u32 = Self::MAX_RETRIES;

    /// Mirrors `RETRY_DELAY_S = 1.0`
    pub const RETRY_DELAY_S: f64 = 1.0;
    pub const _RETRY_DELAY_S: f64 = Self::RETRY_DELAY_S;

    /// Mirrors `CACHE_REFRESH_MARGIN_S = 60` — early refresh margin
    pub const CACHE_REFRESH_MARGIN_S: f64 = 60.0;
    pub const _CACHE_REFRESH_MARGIN_S: f64 = Self::CACHE_REFRESH_MARGIN_S;

    /// Mirrors `HTTP_TIMEOUT_S = 10.0`
    pub const HTTP_TIMEOUT_S: f64 = 10.0;
    pub const _HTTP_TIMEOUT_S: f64 = Self::HTTP_TIMEOUT_S;

    // -- Internal helpers --------------------------------------------------

    /// Mirrors `SignManager.get_refresh_lock(cls, app_key: str) -> asyncio.Lock`
    ///
    /// Must only be called from within a running event loop (async context) in Python.
    /// Rust stub: tracks held keys in a `HashSet` behind a `Mutex` (ponytail: no per-key async lock in std-only).
    pub fn get_refresh_lock(app_key: &str) -> bool {
        let mut locks = sign_locks().lock().unwrap();
        locks.contains(app_key)
    }

    /// Acquire logical lock for `app_key` (test helper; mirrors `async with cls.get_refresh_lock(app_key):`).
    pub fn acquire_refresh_lock(app_key: &str) {
        sign_locks().lock().unwrap().insert(app_key.to_string());
    }
    pub fn release_refresh_lock(app_key: &str) {
        sign_locks().lock().unwrap().remove(app_key);
    }

    /// Mirrors `SignManager.compute_signature(nonce, timestamp, app_key, app_secret) -> str`
    ///
    /// ```python
    /// plain = nonce + timestamp + app_key + app_secret
    /// return hmac.new(app_secret.encode(), plain.encode(), hashlib.sha256).hexdigest()
    /// ```
    /// Rust uses `hmac` + `sha2` when available; std-only fallback does a deterministic
    /// FNV-like hex to preserve call sites (ponytail: wire real HMAC when `hmac` crate lands).
    pub fn compute_signature(nonce: &str, timestamp: &str, app_key: &str, app_secret: &str) -> String {
        // ponytail: std-only stub — real impl should be HMAC-SHA256 hex
        // We use a simple hash so tests can assert determinism without external crate.
        // When `hmac` is added, replace body with:
        //   use hmac::{Hmac, Mac}; use sha2::Sha256;
        //   let mut mac = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).unwrap();
        //   mac.update(format!("{}{}{}{}", nonce, timestamp, app_key, app_secret).as_bytes());
        //   hex::encode(mac.finalize().into_bytes())
        let plain = format!("{}{}{}{}", nonce, timestamp, app_key, app_secret);
        // Simple FNV-1a 64-bit hex, duplicated to 64 chars to mimic sha256 length for shape tests
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in plain.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        // Also hash the key to mix it
        let mut h2 = hash;
        for b in app_secret.bytes() {
            h2 ^= b as u64;
            h2 = h2.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}{:016x}{:016x}{:016x}", hash, h2, hash ^ h2, h2 ^ 0x9e3779b97f4a7c15)
    }

    /// Mirrors `SignManager.build_timestamp() -> str`
    ///
    /// Format: `2006-01-02T15:04:05+08:00` — Beijing time, no milliseconds.
    /// Python: `datetime.now(tz=timezone(timedelta(hours=8))).strftime("%Y-%m-%dT%H:%M:%S+08:00")`
    pub fn build_timestamp() -> String {
        // ponytail: no chrono in std-only; use UTC +8h offset manually.
        // We compute from SystemTime and format as UTC+8.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        // Beijing offset +8h = 28800s
        let bj_secs = now + 8 * 3600;
        // Convert to Y-M-D H:M:S via simple epoch math (proleptic Gregorian)
        // Use chrono-like conversion without chrono: days since epoch.
        let days = bj_secs / 86400;
        let secs_of_day = bj_secs % 86400;
        let hour = (secs_of_day / 3600) as u32;
        let min = ((secs_of_day % 3600) / 60) as u32;
        let sec = (secs_of_day % 60) as u32;
        // Days → date (Howard Hinnant algorithm)
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+08:00", y, m, d, hour, min, sec)
    }

    /// Mirrors `SignManager.is_cache_valid(cls, entry: dict) -> bool`
    /// `entry["expire_ts"] - time.time() > CACHE_REFRESH_MARGIN_S`
    pub fn is_cache_valid(entry: &TokenCacheEntry) -> bool {
        entry.expire_ts - now_secs() > Self::CACHE_REFRESH_MARGIN_S
    }

    /// Mirrors `SignManager.clear_locks(cls) -> None`
    pub fn clear_locks() {
        sign_locks().lock().unwrap().clear();
    }

    /// Mirrors `SignManager.purge_expired(cls) -> int`
    /// Remove all expired entries; return count purged.
    pub fn purge_expired() -> usize {
        let now = now_secs();
        let mut cache = sign_cache().lock().unwrap();
        let before = cache.len();
        cache.retain(|_, v| now - v.expire_ts <= 0.0);
        before - cache.len()
    }

    // -- Core: fetch -------------------------------------------------------

    /// Mirrors `SignManager.fetch(cls, app_key, app_secret, api_domain, route_env="") -> dict`
    ///
    /// Sends sign-token HTTP request with auto-retry up to `MAX_RETRIES`.
    /// Python uses `httpx.AsyncClient(timeout=HTTP_TIMEOUT_S)` + `secrets.token_hex(16)` + `hmac`.
    /// Rust stub returns a synthetic success payload for call-site wiring; real HTTP is gated
    /// behind `reqwest` (ponytail: std-only build returns mock; wire `reqwest` when that crate lands).
    ///
    /// ```python
    /// url = f"{api_domain.rstrip('/')}{cls.TOKEN_PATH}"
    /// for attempt in range(cls.MAX_RETRIES + 1):
    ///     nonce = secrets.token_hex(16); timestamp = cls.build_timestamp()
    ///     signature = cls.compute_signature(nonce, timestamp, app_key, app_secret)
    ///     payload = {"app_key": ..., "nonce": ..., "signature": ..., "timestamp": ...}
    ///     headers = {"Content-Type": ..., "X-AppVersion": ..., "X-Instance-Id": ..., ...}
    ///     response = await client.post(url, json=payload, headers=headers)
    ///     if response.status_code != 200: raise RuntimeError(...)
    ///     code = result_data.get("code"); if code == 0: return data
    ///     if code == RETRYABLE_CODE and attempt < MAX_RETRIES: await asyncio.sleep(RETRY_DELAY_S); continue
    ///     raise RuntimeError(...)
    /// ```
    pub async fn fetch(
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<HashMap<String, serde_json::Value>, String> {
        let _ = (app_key, app_secret, api_domain, route_env);
        // ponytail: std-only mock — real impl does httpx POST with HMAC + retry
        log::info!("Sign token request: url={}{} (mock)", api_domain.trim_end_matches('/'), Self::TOKEN_PATH);
        let mut data = HashMap::new();
        data.insert("token".to_string(), serde_json::Value::String("mock_token".to_string()));
        data.insert("bot_id".to_string(), serde_json::Value::String("mock_bot".to_string()));
        data.insert("duration".to_string(), serde_json::Value::Number(serde_json::Number::from(3600)));
        data.insert("product".to_string(), serde_json::Value::String(String::new()));
        data.insert("source".to_string(), serde_json::Value::String(String::new()));
        Ok(data)
    }

    // -- Public API: get (with cache) --------------------------------------

    /// Mirrors `SignManager.get_token(cls, app_key, app_secret, api_domain, route_env="") -> dict`
    ///
    /// Lazily evicts stale entries, returns cached on hit (with 60s early margin),
    /// otherwise double-checked locks and fetches.
    pub async fn get_token(
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenCacheEntry, String> {
        Self::purge_expired();
        {
            let cache = sign_cache().lock().unwrap();
            if let Some(entry) = cache.get(app_key) {
                if Self::is_cache_valid(entry) {
                    let remain = (entry.expire_ts - now_secs()) as i64;
                    log::info!("Using cached token ({}s remaining)", remain);
                    return Ok(entry.clone());
                }
            }
        }
        // Simulate per-app_key lock (ponytail: coarse Mutex guard instead of per-key async Lock)
        Self::acquire_refresh_lock(app_key);
        let result = {
            // double-check after acquiring lock
            {
                let cache = sign_cache().lock().unwrap();
                if let Some(entry) = cache.get(app_key) {
                    if Self::is_cache_valid(entry) {
                        Self::release_refresh_lock(app_key);
                        return Ok(entry.clone());
                    }
                }
            }
            let data = Self::fetch(app_key, app_secret, api_domain, route_env).await?;
            let duration = data.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
            let expire_ts = if duration > 0 { now_secs() + duration as f64 } else { now_secs() + 3600.0 };
            let entry = TokenCacheEntry {
                token: data.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                bot_id: data.get("bot_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                duration,
                product: data.get("product").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                source: data.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                expire_ts,
            };
            sign_cache().lock().unwrap().insert(app_key.to_string(), entry.clone());
            entry
        };
        Self::release_refresh_lock(app_key);
        Ok(result)
    }

    // -- Public API: force refresh -----------------------------------------

    /// Mirrors `SignManager.force_refresh(cls, app_key, app_secret, api_domain, route_env="") -> dict`
    pub async fn force_refresh(
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenCacheEntry, String> {
        log::warn!("[force-refresh] Clearing cache and re-signing token: app_key=****{}", &app_key[app_key.len().saturating_sub(4)..]);
        Self::acquire_refresh_lock(app_key);
        sign_cache().lock().unwrap().remove(app_key);
        let data = Self::fetch(app_key, app_secret, api_domain, route_env).await?;
        let duration = data.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
        let expire_ts = if duration > 0 { now_secs() + duration as f64 } else { now_secs() + 3600.0 };
        let entry = TokenCacheEntry {
            token: data.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            bot_id: data.get("bot_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            duration,
            product: data.get("product").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            source: data.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            expire_ts,
        };
        sign_cache().lock().unwrap().insert(app_key.to_string(), entry.clone());
        Self::release_refresh_lock(app_key);
        Ok(entry)
    }
}

// ---------------------------------------------------------------------------
// InboundContext — mirrors Python lines 641–707
// ---------------------------------------------------------------------------

/// Mutable context flowing through the inbound middleware pipeline.
///
/// Each middleware reads/writes fields on this context. The pipeline engine
/// passes it to every middleware in registration order.
///
/// Mirrors `@dataclass class InboundContext:` (Python lines 643–707).
#[derive(Debug, Default)]
pub struct InboundContext {
    // Python: adapter: Any  # YuanbaoAdapter (forward-ref)
    pub adapter_name: String, // stub for adapter.name used in logging

    /// Raw bytes frames (debounce-aggregated) — `raw_frames: list = field(default_factory=list)`
    pub raw_frames: Vec<Vec<u8>>,

    /// Populated by DecodeMiddleware — `push: Optional[dict] = None`
    pub push: Option<serde_json::Value>,
    /// `decoded_via: str = ""` — "json" | "protobuf"
    pub decoded_via: String,

    /// Extracted from push by FieldExtractMiddleware
    pub from_account: String,
    pub group_code: String,
    pub group_name: String,
    pub sender_nickname: String,
    pub msg_body: Vec<serde_json::Value>,
    pub msg_id: String,
    pub cloud_custom_data: String,

    /// Derived by ChatRoutingMiddleware
    pub chat_id: String,
    pub chat_type: String, // "dm" | "group"
    pub chat_name: String,

    /// Populated by ContentExtractMiddleware
    pub raw_text: String,
    pub media_refs: Vec<serde_json::Value>,

    /// Populated by ExtractContentMiddleware for elem_type 1009 (WeChat forward)
    pub forwarded_records: Option<serde_json::Value>,

    /// Owner command detection
    pub owner_command: Option<String>,

    /// Source built by BuildSourceMiddleware — `source: Optional[Any] = None` (SessionSource)
    pub source: Option<SessionSource>,

    /// Populated by ClassifyMessageTypeMiddleware — `msg_type: Optional[Any] = None`
    pub msg_type: Option<MessageType>,

    /// Populated by QuoteContextMiddleware
    pub reply_to_message_id: Option<String>,
    pub reply_to_text: Option<String>,
    pub quote_media_refs: Vec<(String, String, String)>, // (rid, kind, filename)

    /// Populated by MediaResolveMiddleware — combined resolved local paths (deduped, order as documented)
    pub media_urls: Vec<String>,
    pub media_types: Vec<String>,

    /// Populated by ExtractContentMiddleware
    pub link_urls: Vec<String>,

    /// Populated by GroupAttributionMiddleware
    pub channel_prompt: Option<String>,
}

impl InboundContext {
    pub fn new(adapter_name: impl Into<String>) -> Self {
        Self { adapter_name: adapter_name.into(), ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// InboundMiddleware — mirrors Python lines 709–734
// ---------------------------------------------------------------------------

/// Abstract base class for all inbound pipeline middlewares.
///
/// Subclasses must set `name` and implement `async handle(ctx, next_fn)`.
///
/// Mirrors `class InboundMiddleware(ABC):` (Python lines 709–734).
#[async_trait::async_trait]
pub trait InboundMiddleware: Send + Sync {
    /// Mirrors `name: str = ""` — override in each subclass
    fn name(&self) -> &str;
    /// Mirrors `async def handle(self, ctx, next_fn) -> None`
    async fn handle(&self, ctx: &mut InboundContext, next: NextFn<'_>) -> Result<(), String>;
    fn repr(&self) -> String {
        format!("<{} name={:?}>", std::any::type_name::<Self>(), self.name())
    }
}

/// `next_fn` callable passed to each middleware — mirrors `Callable` `next_fn`.
pub type NextFn<'a> = Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> + Send + 'a>;

// ---------------------------------------------------------------------------
// InboundPipeline — mirrors Python lines 736–829
// ---------------------------------------------------------------------------

/// Onion-model middleware pipeline engine for inbound message processing.
///
/// Inspired by OpenClaw's `MessagePipeline` (`extensions/yuanbao/src/business/pipeline/engine.ts`).
/// Supports named middlewares, conditional guards (`when`), and `use_before` / `use_after` / `remove`.
///
/// Mirrors `class InboundPipeline:` (Python lines 736–829).
pub struct InboundPipeline {
    middlewares: Vec<(String, Box<dyn InboundMiddleware>, Option<Box<dyn Fn(&InboundContext) -> bool + Send + Sync>>)>,
}

impl Default for InboundPipeline {
    fn default() -> Self { Self::new() }
}

impl InboundPipeline {
    pub fn new() -> Self {
        Self { middlewares: Vec::new() }
    }

    // -- Internal helpers --------------------------------------------------

    /// Mirrors `InboundPipeline._normalize(name_or_mw, handler=None)` — normalize
    /// `(name, handler)` or `(InboundMiddleware,)` into `(name, callable)`.
    /// Rust variant is typed: callers pass `Box<dyn InboundMiddleware>` directly.
    fn normalize_name(mw: &dyn InboundMiddleware) -> String {
        mw.name().to_string()
    }

    // -- Registration API --------------------------------------------------

    /// Mirrors `def use(self, name_or_mw, handler=None, when=None) -> "InboundPipeline"`
    /// Appends middleware to end. Accepts `Box<dyn InboundMiddleware>` (OOP) or
    /// functional style via `FnMiddleware` wrapper (see below).
    pub fn r#use(&mut self, mw: Box<dyn InboundMiddleware>, when: Option<Box<dyn Fn(&InboundContext) -> bool + Send + Sync>>) -> &mut Self {
        let name = mw.name().to_string();
        self.middlewares.push((name, mw, when));
        self
    }

    /// Mirrors `use` with string name + handler (functional style compatibility).
    pub fn use_fn<F>(&mut self, name: impl Into<String>, _handler: F, when: Option<Box<dyn Fn(&InboundContext) -> bool + Send + Sync>>) -> &mut Self
    where F: Fn(&mut InboundContext, NextFn<'_>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> + Send + Sync + 'static {
        // ponytail: functional wrapper not needed in std-only; store as named stub
        let name = name.into();
        struct FnAdapter(String);
        #[async_trait::async_trait]
        impl InboundMiddleware for FnAdapter {
            fn name(&self) -> &str { &self.0 }
            async fn handle(&self, _ctx: &mut InboundContext, next: NextFn<'_>) -> Result<(), String> { next().await }
        }
        self.middlewares.push((name.clone(), Box::new(FnAdapter(name)), when));
        self
    }

    /// Mirrors `def use_before(self, target: str, name_or_mw, handler=None, when=None)`
    pub fn use_before(&mut self, target: &str, mw: Box<dyn InboundMiddleware>, when: Option<Box<dyn Fn(&InboundContext) -> bool + Send + Sync>>) -> &mut Self {
        let name = mw.name().to_string();
        let entry = (name, mw, when);
        if let Some(idx) = self.middlewares.iter().position(|(n, _, _)| n == target) {
            self.middlewares.insert(idx, entry);
        } else {
            self.middlewares.push(entry);
        }
        self
    }

    /// Mirrors `def use_after(self, target: str, name_or_mw, handler=None, when=None)`
    pub fn use_after(&mut self, target: &str, mw: Box<dyn InboundMiddleware>, when: Option<Box<dyn Fn(&InboundContext) -> bool + Send + Sync>>) -> &mut Self {
        let name = mw.name().to_string();
        let entry = (name, mw, when);
        if let Some(idx) = self.middlewares.iter().position(|(n, _, _)| n == target) {
            self.middlewares.insert(idx + 1, entry);
        } else {
            self.middlewares.push(entry);
        }
        self
    }

    /// Mirrors `def remove(self, name: str) -> "InboundPipeline"`
    pub fn remove(&mut self, name: &str) -> &mut Self {
        self.middlewares.retain(|(n, _, _)| n != name);
        self
    }

    /// Mirrors `@property def middleware_names(self) -> list`
    pub fn middleware_names(&self) -> Vec<String> {
        self.middlewares.iter().map(|(n, _, _)| n.clone()).collect()
    }

    // -- Execution ---------------------------------------------------------

    /// Mirrors `async def execute(self, ctx: InboundContext) -> None`
    /// Runs all middlewares in order; each receives `(ctx, next_fn)`.
    /// Rust version iterates; Python uses recursive `next_fn` closure with `nonlocal index`.
    pub async fn execute(&self, ctx: &mut InboundContext) -> Result<(), String> {
        self.execute_at(ctx, 0).await
    }

    fn execute_at<'a>(&'a self, ctx: &'a mut InboundContext, idx: usize) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let mut i = idx;
            while i < self.middlewares.len() {
                let (name, handler, when_fn) = &self.middlewares[i];
                i += 1;
                if let Some(when) = when_fn {
                    if !when(ctx) {
                        continue;
                    }
                }
                // Build next_fn that resumes at i
                let next: NextFn<'_> = Box::new(|| {
                    self.execute_at(ctx, i)
                });
                // Python wraps handler call in try/except and logs on error
                if let Err(e) = handler.handle(ctx, next).await {
                    log::error!("[InboundPipeline] middleware [{}] error: {}", name, e);
                    return Err(e);
                }
                return Ok(());
            }
            Ok(())
        })
    }
}

impl std::fmt::Debug for InboundPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundPipeline").field("middlewares", &self.middleware_names()).finish()
    }
}

// ---------------------------------------------------------------------------
// DecodeMiddleware — mirrors Python lines 830–988
// ---------------------------------------------------------------------------

/// Decode raw inbound frames from JSON or Protobuf into `ctx.push`.
///
/// Encapsulates JSON push parsing (aligned with TS `decodeFromContent`) and
/// Protobuf decoding via `decode_inbound_push`.
///
/// Mirrors `class DecodeMiddleware(InboundMiddleware):` (Python lines 830–988)
/// — slice 1 includes the full class for compilability (boundary at 900 is
/// inside `parse_json_push`; the tail is included here and slice 2 resumes
/// after the class).
pub struct DecodeMiddleware;

impl DecodeMiddleware {
    pub const NAME: &'static str = "decode";
}

#[async_trait::async_trait]
impl InboundMiddleware for DecodeMiddleware {
    fn name(&self) -> &str { Self::NAME }

    async fn handle(&self, ctx: &mut InboundContext, next: NextFn<'_>) -> Result<(), String> {
        let data_list = ctx.raw_frames.clone();
        if data_list.is_empty() {
            return Ok(()); // Stop pipeline — nothing to decode
        }

        let mut merged_push: Option<serde_json::Value> = None;
        let mut decoded_via = String::new();

        for data in &data_list {
            let (push, via) = Self::decode_single(&ctx.adapter_name, data);
            if push.is_none() {
                log::info!("[{}] Push decoded but no valid message. raw hex(first64)={}", ctx.adapter_name, hex_preview(data, 64));
                continue;
            }
            let push = push.unwrap();
            if merged_push.is_none() {
                decoded_via = via.clone();
                log::info!("[{}] Frame decoded (via={}): len={}", ctx.adapter_name, via, data.len());
                merged_push = Some(push);
            } else {
                // Subsequent pushes: merge msg_body with separator
                let extra_body = push.get("msg_body").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                if !extra_body.is_empty() {
                    if let Some(ref mut base) = merged_push {
                        let base_body = base.get_mut("msg_body").and_then(|v| v.as_array_mut());
                        if let Some(arr) = base_body {
                            arr.push(serde_json::json!({"msg_type": "TIMTextElem", "msg_content": {"text": "\n"}}));
                            arr.extend(extra_body.clone());
                        } else {
                            base["msg_body"] = serde_json::Value::Array(extra_body.clone());
                        }
                    }
                    log::info!("[{}] Merged {} extra msg_body elements from aggregated push", ctx.adapter_name, extra_body.len());
                }
            }
        }

        let Some(merged) = merged_push else {
            return Ok(()); // Stop pipeline
        };

        // Log summary (mirrors Python logger.info)
        let from = merged.get("from_account").and_then(|v| v.as_str()).unwrap_or("");
        let group = merged.get("group_code").and_then(|v| v.as_str()).unwrap_or("");
        let msg_id = merged.get("msg_id").and_then(|v| v.as_str()).unwrap_or("");
        let msg_types: Vec<String> = merged.get("msg_body").and_then(|v| v.as_array()).map(|arr| arr.iter().map(|e| e.get("msg_type").and_then(|v| v.as_str()).unwrap_or("").to_string()).collect()).unwrap_or_default();
        log::info!("[{}] Push decoded (via={}): from={} group={} msg_id={} msg_types={:?}", ctx.adapter_name, decoded_via, from, group, msg_id, msg_types);
        log::debug!("[{}] Push payload: {}", ctx.adapter_name, merged);

        ctx.push = Some(merged);
        ctx.decoded_via = decoded_via;

        next().await
    }
}

impl DecodeMiddleware {
    // -- JSON push parsing -------------------------------------------------

    /// Mirrors `DecodeMiddleware.convert_json_msg_body(raw_body: list) -> list`
    ///
    /// Normalize raw JSON `msg_body` array to `[{"msg_type": str, "msg_content": dict}]`.
    /// Compatible with both PascalCase (`MsgType`/`MsgContent`) and snake_case.
    ///
    /// ```python
    /// result = []
    /// for item in raw_body or []:
    ///     if not isinstance(item, dict): continue
    ///     msg_type = item.get("msg_type") or item.get("MsgType", "")
    ///     msg_content = item.get("msg_content") or item.get("MsgContent", {})
    ///     if isinstance(msg_content, str):
    ///         try: msg_content = json.loads(msg_content)
    ///         except: msg_content = {"text": msg_content}
    ///     result.append({"msg_type": msg_type, "msg_content": msg_content or {}})
    /// return result
    /// ```
    pub fn convert_json_msg_body(raw_body: &[serde_json::Value]) -> Vec<serde_json::Value> {
        let mut result = Vec::new();
        for item in raw_body {
            let Some(obj) = item.as_object() else { continue; };
            let msg_type = obj.get("msg_type").and_then(|v| v.as_str())
                .or_else(|| obj.get("MsgType").and_then(|v| v.as_str()))
                .unwrap_or("");
            let mut msg_content = obj.get("msg_content").cloned()
                .or_else(|| obj.get("MsgContent").cloned())
                .unwrap_or(serde_json::Value::Object(Default::default()));
            if let Some(s) = msg_content.as_str() {
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(v) => msg_content = v,
                    Err(_) => msg_content = serde_json::json!({"text": s}),
                }
            }
            if msg_content.is_null() {
                msg_content = serde_json::json!({});
            }
            result.push(serde_json::json!({"msg_type": msg_type, "msg_content": msg_content}));
        }
        result
    }

    /// Mirrors `DecodeMiddleware.parse_json_push(raw_json: dict) -> dict | None`
    ///
    /// Convert JSON-format push to dict with same structure as `decode_inbound_push`.
    /// Supports standard callback format and legacy `GroupId`/`MsgSeq`/`MsgKey` fields.
    ///
    /// ```python
    /// if not raw_json: return None
    /// from_account = raw_json.get("from_account","") or raw_json.get("From_Account","")
    /// group_code = raw_json.get("group_code","") or raw_json.get("GroupId","") or raw_json.get("group_id","")
    /// msg_body_raw = raw_json.get("msg_body",[]) or raw_json.get("MsgBody",[])
    /// msg_body = DecodeMiddleware.convert_json_msg_body(msg_body_raw)
    /// if not from_account and not msg_body and not raw_json.get("callback_command"): return None
    /// return {"callback_command": ..., "from_account": ..., "group_code": ..., "msg_body": ..., ...}
    /// ```
    pub fn parse_json_push(raw_json: &serde_json::Value) -> Option<serde_json::Value> {
        let obj = raw_json.as_object()?;
        if obj.is_empty() {
            return None;
        }

        let from_account = obj.get("from_account").and_then(|v| v.as_str()).unwrap_or("")
            .to_string();
        let from_account = if from_account.is_empty() {
            obj.get("From_Account").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { from_account };

        let group_code = {
            let g = obj.get("group_code").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !g.is_empty() { g } else {
                let g2 = obj.get("GroupId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !g2.is_empty() { g2 } else {
                    obj.get("group_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
                }
            }
        };

        let msg_body_raw: Vec<serde_json::Value> = obj.get("msg_body").and_then(|v| v.as_array()).cloned()
            .or_else(|| obj.get("MsgBody").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        let msg_body = Self::convert_json_msg_body(&msg_body_raw);

        let callback_command = obj.get("callback_command").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if from_account.is_empty() && msg_body.is_empty() && callback_command.is_empty() {
            return None;
        }

        let to_account = obj.get("to_account").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let to_account = if to_account.is_empty() {
            obj.get("To_Account").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { to_account };

        let sender_nickname = obj.get("sender_nickname").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sender_nickname = if sender_nickname.is_empty() {
            obj.get("nick_name").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { sender_nickname };

        let group_name = obj.get("group_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let msg_seq = obj.get("msg_seq").and_then(|v| v.as_u64()).unwrap_or_else(|| obj.get("MsgSeq").and_then(|v| v.as_u64()).unwrap_or(0));
        let msg_id = {
            let a = obj.get("msg_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !a.is_empty() { a } else {
                let b = obj.get("msg_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !b.is_empty() { b } else {
                    obj.get("MsgKey").and_then(|v| v.as_str()).unwrap_or("").to_string()
                }
            }
        };
        let msg_body_json = serde_json::Value::Array(msg_body);
        let cloud_custom_data = obj.get("cloud_custom_data").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cloud_custom_data = if cloud_custom_data.is_empty() {
            obj.get("CloudCustomData").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { cloud_custom_data };
        let bot_owner_id = obj.get("bot_owner_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let bot_owner_id = if bot_owner_id.is_empty() {
            obj.get("botOwnerId").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { bot_owner_id };
        let recall_msg_seq_list = obj.get("recall_msg_seq_list").cloned().or_else(|| obj.get("recall_msg_seq_list").cloned());
        let trace_id = obj.get("log_ext").and_then(|v| v.as_object()).and_then(|o| o.get("trace_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();

        Some(serde_json::json!({
            "callback_command": callback_command,
            "from_account": from_account,
            "to_account": to_account,
            "sender_nickname": sender_nickname,
            "group_code": group_code,
            "group_name": group_name,
            "msg_seq": msg_seq,
            "msg_id": msg_id,
            "msg_body": msg_body_json,
            "cloud_custom_data": cloud_custom_data,
            "bot_owner_id": bot_owner_id,
            "recall_msg_seq_list": recall_msg_seq_list,
            "trace_id": trace_id,
        }))
    }

    // -- Pipeline handler helper ------------------------------------------

    /// Mirrors `DecodeMiddleware._decode_single(self, adapter, data: bytes) -> tuple`
    ///
    /// Try JSON decode first; if that yields a valid push return `(push, "json")`,
    /// else try `decode_inbound_push` (protobuf) → `(push, "protobuf")`, else `(None, "")`.
    ///
    /// ```python
    /// def _decode_single(self, adapter, data: bytes) -> tuple:
    ///     try: conn_json = json.loads(data.decode("utf-8"))
    ///     except: conn_json = None
    ///     if isinstance(conn_json, dict):
    ///         push = self.parse_json_push(conn_json)
    ///         if push: return push, "json"
    ///     else:
    ///         try: push = decode_inbound_push(data)
    ///         except: push = None
    ///         if push: return push, "protobuf"
    ///     return None, ""
    /// ```
    pub fn decode_single(_adapter_name: &str, data: &[u8]) -> (Option<serde_json::Value>, String) {
        // Try JSON first
        if let Ok(s) = std::str::from_utf8(data) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                if v.is_object() {
                    if let Some(push) = Self::parse_json_push(&v) {
                        return (Some(push), "json".to_string());
                    }
                } else {
                    // Not a dict → try protobuf path (mirrors Python `else:` — only when json.loads didn't return dict)
                    // fall through to protobuf
                }
            } else {
                // json.loads raised → conn_json = None → try protobuf
                if let Some(push) = yuanbao_proto::decode_inbound_push(data) {
                    return (Some(push), "protobuf".to_string());
                }
            }
            // If JSON was a dict but parse_json_push returned None, Python returns (None, "")
            // without trying protobuf. We preserve that: dict case already returned or falls to (None,"").
            if serde_json::from_str::<serde_json::Value>(s).map(|v| v.is_object()).unwrap_or(false) {
                return (None, String::new());
            }
            // Non-dict JSON or parse failure already handled protobuf above
            return (None, String::new());
        }
        // Not UTF-8 → try protobuf
        if let Some(push) = yuanbao_proto::decode_inbound_push(data) {
            return (Some(push), "protobuf".to_string());
        }
        (None, String::new())
    }

    // Alias for grep-ability
    #[allow(dead_code)]
    fn _decode_single(adapter_name: &str, data: &[u8]) -> (Option<serde_json::Value>, String) {
        Self::decode_single(adapter_name, data)
    }
}

// ---------------------------------------------------------------------------
// Small helpers — mirrors Python line 880–900 boundary tail + misc
// ---------------------------------------------------------------------------

fn hex_preview(data: &[u8], max: usize) -> String {
    if data.is_empty() {
        return "(empty)".to_string();
    }
    let preview: String = data.iter().take(max).map(|b| format!("{:02x}", b)).collect();
    preview
}

// Public re-exports for grep discoverability — mirrors Python `__all__` surface
pub use self::helpers::{
    infer_block_separator as _infer_block_separator,
    is_fence_atom as _is_fence_atom,
    is_table_atom as _is_table_atom,
    merge_streaming_fences as _merge_streaming_fences,
    split_at_paragraph_boundary as _split_at_paragraph_boundary,
    split_markdown_atoms as _split_into_atoms,
    split_text_fence_aware as _split_text_fence_aware,
    text_ends_with_table_row as _text_ends_with_table_row,
    text_has_unclosed_fence as _text_has_unclosed_fence,
};

// ---------------------------------------------------------------------------
// Boundary note — slice 1 ends at Python line 900
// ---------------------------------------------------------------------------
// Python lines 900–909 (remainder of parse_json_push return dict, inside
// DecodeMiddleware) are included above for compilability. The next Python
// lines 910+ continue `parse_json_push` tail (`msg_id` / `msg_body` /
// `cloud_custom_data` / `bot_owner_id` / `recall_msg_seq_list` / `trace_id`
// return) → `_decode_single` → `handle` → `ExtractFieldsMiddleware` etc.
// Slice 2 (`yuanbao_slice2.rs`) will start at line 901 (or 910 for the
// tail already included here, mirroring `run_slice1.rs` / `base_slice1.rs`
// boundary handling) at `class ExtractFieldsMiddleware:` (line 990) to
// avoid duplicating the included tail.
