//! Base platform adapter interface — slice 1 (lines 1–900).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/base.py`
//! (7420 LOC), slice 1 covering lines 1–900.
//! This slice contains the module header, all imports, detached-handler helper,
//! audio/voice constants, semaphore, platform helpers, networking/proxy helpers,
//! TTS/streaming types, URL safety helpers, image-cache constants and the start
//! of the image-cache download helpers through `cache_image_from_url`.
//! The slice boundary at line 900 cuts inside `cache_image_from_url`; the Rust
//! file includes the complete function for compilability — the remainder of the
//! file beyond line 900 (the function's retry loop tail) is included here and
//! slice 2 will continue after it (mirrors `run_slice1.rs` boundary handling).
//!
//! Python source docstring (preserved):
//! ```text
//! Base platform adapter interface.
//!
//! All platform adapters (Telegram, Discord, WhatsApp, Weixin, and more) inherit from this
//! and implement the required methods.
//! ```
//!
//! # Bootstrap / path note
//!
//! Python `gateway/platforms/base.py` does:
//! ```python
//! from pathlib import Path as _Path
//! sys.path.insert(0, str(_Path(__file__).resolve().parents[2]))
//! from gateway.config import Platform, PlatformConfig
//! from gateway.session import SessionSource, build_session_key
//! from hermes_constants import get_default_hermes_root, get_hermes_dir, get_hermes_home
//! ```
//! Rust has no `sys.path` manipulation; crate-level imports are resolved at
//! compile time. `Platform`/`PlatformConfig`/`SessionSource`/`build_session_key`
//! are modeled as minimal local stubs below (wire to real `hermes-gateway` types
//! when those modules land). `get_hermes_dir`/`get_hermes_home` are ported as
//! [`get_hermes_dir`] / [`get_hermes_home`] mirroring `hermes_constants`.
//!
//! # Mapping
//!
//! - `logger = logging.getLogger(__name__)` → [`log`] crate (`log::error!`, `log::debug!`, `log::warn!`)
//! - `def _consume_detached_handler_exception(task)` → [`consume_detached_handler_exception`]
//! - `_AUDIO_MIME_TYPES` → [`AUDIO_MIME_TYPES`] / [`audio_mime_type_for_ext`]
//! - `_AUDIO_EXTS` → [`AUDIO_EXTS`] / [`is_audio_ext`]
//! - `_TELEGRAM_AUDIO_ATTACHMENT_EXTS` → [`TELEGRAM_AUDIO_ATTACHMENT_EXTS`]
//! - `_TELEGRAM_VOICE_EXTS` → [`TELEGRAM_VOICE_EXTS`]
//! - `_POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS` → [`POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS`]
//! - `_HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS` → [`HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS`]
//! - `_HISTORY_MEDIA_LOOKUP_MAX_WORKERS` → [`HISTORY_MEDIA_LOOKUP_MAX_WORKERS`]
//! - `_HISTORY_MEDIA_LOOKUP_ADMISSION = threading.BoundedSemaphore(2)` → [`HISTORY_MEDIA_LOOKUP_MAX_WORKERS`] + [`try_acquire_history_lookup_permit`] (semaphore with 2 permits; Rust uses `std::sync` atomic counter, not `threading`)
//! - `def _platform_name(platform)` → [`platform_name`] / [`platform_name_value`]
//! - `def _float_env(name, default)` → [`float_env`]
//! - `def _thread_metadata_for_source(source, reply_to_message_id)` → [`thread_metadata_for_source`]
//! - `def _mark_notify_metadata(metadata)` → [`mark_notify_metadata`]
//! - `def _reply_anchor_for_event(event)` → [`reply_anchor_for_event`]
//! - `def should_send_media_as_audio(platform, ext, is_voice)` → [`should_send_media_as_audio`]
//! - `def build_auto_tts_output_path(platform)` → [`build_auto_tts_output_path`] (+ [`OPUS_VOICE_PLATFORMS`])
//! - `def utf16_len(s)` → [`utf16_len`]
//! - `def _prefix_within_utf16_limit(s, limit)` → [`prefix_within_utf16_limit`]
//! - `def _custom_unit_to_cp(s, budget, len_fn)` → [`custom_unit_to_cp`]
//! - `def is_network_accessible(host)` → [`is_network_accessible`]
//! - `def _detect_macos_system_proxy()` → [`detect_macos_system_proxy`]
//! - `def _split_host_port(value)` → [`split_host_port`]
//! - `def _no_proxy_entries()` → [`no_proxy_entries`]
//! - `def _no_proxy_entry_matches(entry, host, port)` → [`no_proxy_entry_matches`]
//! - `def should_bypass_proxy(target_hosts)` → [`should_bypass_proxy`] / [`should_bypass_proxy_one`]
//! - `def resolve_proxy_url(platform_env_var, *, target_hosts)` → [`resolve_proxy_url`]
//! - `def proxy_kwargs_for_bot(proxy_url)` → [`proxy_kwargs_for_bot`]
//! - `def proxy_kwargs_for_aiohttp(proxy_url)` → [`proxy_kwargs_for_aiohttp`]
//! - `def is_host_excluded_by_no_proxy(hostname, no_proxy_value)` → [`is_host_excluded_by_no_proxy`]
//! - `from utils import normalize_proxy_url` → [`normalize_proxy_url`]
//! - `@dataclass class AudioFormat` → [`AudioFormat`]
//! - `@dataclass class StreamingTTSHandle` → [`StreamingTTSHandle`]
//! - `def streaming_tts_turn_key(session_key, turn_marker, *, event)` → [`streaming_tts_turn_key`]
//! - `def streaming_tts_should_skip_whole_file(completed_turns, session_key, turn_marker, *, event)` → [`streaming_tts_should_skip_whole_file`]
//! - `GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE` → [`GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE`]
//! - `def safe_url_for_log(url, max_len)` → [`safe_url_for_log`]
//! - `async def _ssrf_redirect_guard(response)` → [`ssrf_redirect_guard`] (async)
//! - `IMAGE_CACHE_DIR = get_hermes_dir("cache/images", "image_cache")` → [`IMAGE_CACHE_DIR`] / [`get_hermes_dir`] / [`IMAGE_CACHE_DIR_DEFAULT`]
//! - `def _resolve_cache_dir(constant_name, new_subpath, old_name)` → [`resolve_cache_dir`]
//! - `DEFAULT_INBOUND_MEDIA_MAX_BYTES` → [`DEFAULT_INBOUND_MEDIA_MAX_BYTES`]
//! - `def get_inbound_media_max_bytes()` → [`get_inbound_media_max_bytes`]
//! - `def validate_inbound_media_size(size, *, media_type, max_bytes)` → [`validate_inbound_media_size`]
//! - `async def _read_httpx_body_with_limit(response, *, media_type)` → [`read_httpx_body_with_limit`] (async, generic over `AsyncRead`)
//! - `def get_image_cache_dir()` → [`get_image_cache_dir`]
//! - `def _looks_like_image(data)` → [`looks_like_image`]
//! - `def cache_image_from_bytes(data, ext)` → [`cache_image_from_bytes`]
//! - `async def cache_image_from_url(url, ext, retries)` → [`cache_image_from_url`] (async)
//!
//! Python imports not directly ported (asyncio, inspect, weakref, etc.):
//! documented as `// Python:` comments where relevant. Rust equivalents use
//! `std`, `serde_json`, `uuid`, and `log`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Module doc / constants — mirrors Python lines 1–77
// ---------------------------------------------------------------------------

// Python: import logging; logger = logging.getLogger(__name__)
// Rust: `log` crate; calls become `log::error!`, `log::debug!`, etc.

// ---------------------------------------------------------------------------
// Detached handler helper — mirrors Python lines 31–44
// ---------------------------------------------------------------------------

/// Done-callback retrieving a detached fatal-error handler's exception.
///
/// Prevents "Task exception was never retrieved" warnings for handler tasks
/// we deliberately let finish in the background after their awaiting
/// (carrier) task was cancelled — see `_notify_fatal_error`.
///
/// Python:
/// ```python
/// def _consume_detached_handler_exception(task: "asyncio.Task") -> None:
///     if task.cancelled():
///         return
///     exc = task.exception()
///     if exc is not None:
///         logger.error("Detached fatal-error handler task failed: %s", exc, exc_info=exc)
/// ```
///
/// Rust models an already-joined task as `(cancelled, Option<error_string>)`.
pub fn consume_detached_handler_exception(cancelled: bool, error: Option<&str>) {
    if cancelled {
        return;
    }
    if let Some(exc) = error {
        log::error!("Detached fatal-error handler task failed: {}", exc);
    }
}

/// Private alias for grep discoverability.
#[allow(dead_code)]
fn _consume_detached_handler_exception(cancelled: bool, error: Option<&str>) {
    consume_detached_handler_exception(cancelled, error)
}

// ---------------------------------------------------------------------------
// Audio / media constants — mirrors Python lines 47–76
// ---------------------------------------------------------------------------

/// Audio file extensions Hermes recognizes for native audio delivery.
///
/// Mirrors `_AUDIO_MIME_TYPES: dict[str, str]` (8 entries).
pub const AUDIO_MIME_TYPES: &[(&str, &str)] = &[
    (".ogg", "audio/ogg"),
    (".opus", "audio/opus"),
    (".mp3", "audio/mpeg"),
    (".m2a", "audio/mpeg"),
    (".wav", "audio/wav"),
    (".m4a", "audio/m4a"),
    (".flac", "audio/flac"),
];

/// Private alias.
pub const _AUDIO_MIME_TYPES: &[(&str, &str)] = AUDIO_MIME_TYPES;

/// Lookup MIME type for an extension (lowercased). Mirrors `_AUDIO_MIME_TYPES[ext]`.
pub fn audio_mime_type_for_ext(ext: &str) -> Option<&'static str> {
    let lower = ext.to_ascii_lowercase();
    for (k, v) in AUDIO_MIME_TYPES {
        if *k == lower {
            return Some(*v);
        }
    }
    None
}

/// Mirrors `_AUDIO_EXTS = frozenset(_AUDIO_MIME_TYPES)` — all recognized audio exts.
pub const AUDIO_EXTS: &[&str] = &[".ogg", ".opus", ".mp3", ".m2a", ".wav", ".m4a", ".flac"];
pub const _AUDIO_EXTS: &[&str] = AUDIO_EXTS;

pub fn is_audio_ext(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTS.contains(&lower.as_str())
}

/// Mirrors `_TELEGRAM_AUDIO_ATTACHMENT_EXTS = frozenset({'.mp3', '.m4a'})`
pub const TELEGRAM_AUDIO_ATTACHMENT_EXTS: &[&str] = &[".mp3", ".m4a"];
pub const _TELEGRAM_AUDIO_ATTACHMENT_EXTS: &[&str] = TELEGRAM_AUDIO_ATTACHMENT_EXTS;

/// Mirrors `_TELEGRAM_VOICE_EXTS = frozenset({'.ogg', '.opus'})`
pub const TELEGRAM_VOICE_EXTS: &[&str] = &[".ogg", ".opus"];
pub const _TELEGRAM_VOICE_EXTS: &[&str] = TELEGRAM_VOICE_EXTS;

/// Mirrors `_POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS = 30.0`
pub const POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS: f64 = 30.0;
pub const _POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS: f64 = POST_DELIVERY_CALLBACK_TIMEOUT_SECONDS;

/// Mirrors `_HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS = 5.0`
pub const HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS: f64 = 5.0;
pub const _HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS: f64 = HISTORY_MEDIA_LOOKUP_TIMEOUT_SECONDS;

/// Mirrors `_HISTORY_MEDIA_LOOKUP_MAX_WORKERS = 2`
pub const HISTORY_MEDIA_LOOKUP_MAX_WORKERS: usize = 2;
pub const _HISTORY_MEDIA_LOOKUP_MAX_WORKERS: usize = HISTORY_MEDIA_LOOKUP_MAX_WORKERS;

/// Mirrors `_HISTORY_MEDIA_LOOKUP_ADMISSION = threading.BoundedSemaphore(2)`
///
/// Rust uses an atomic counter with cap 2 instead of `threading.BoundedSemaphore`.
/// `try_acquire_history_lookup_permit` / `release_history_lookup_permit` preserve the
/// admission-control semantics.
static HISTORY_LOOKUP_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

pub fn try_acquire_history_lookup_permit() -> bool {
    let current = HISTORY_LOOKUP_IN_FLIGHT.load(Ordering::SeqCst);
    if current >= HISTORY_MEDIA_LOOKUP_MAX_WORKERS {
        return false;
    }
    match HISTORY_LOOKUP_IN_FLIGHT.compare_exchange(
        current,
        current + 1,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => true,
        Err(_) => false,
    }
}

pub fn release_history_lookup_permit() {
    let prev = HISTORY_LOOKUP_IN_FLIGHT.load(Ordering::SeqCst);
    if prev > 0 {
        HISTORY_LOOKUP_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Platform helpers — mirrors Python lines 79–132
// ---------------------------------------------------------------------------

/// Normalize a Platform enum / raw string into a lowercase name.
///
/// Mirrors:
/// ```python
/// def _platform_name(platform) -> str:
///     value = getattr(platform, "value", platform)
///     return str(value or "").lower()
/// ```
pub fn platform_name(platform: &str) -> String {
    platform.trim().to_ascii_lowercase()
}

/// `serde_json::Value` overload — handles `{"value": "..."}` enum shape.
pub fn platform_name_value(platform: &serde_json::Value) -> String {
    if let Some(obj) = platform.as_object() {
        if let Some(v) = obj.get("value") {
            if let Some(s) = v.as_str() {
                return s.trim().to_ascii_lowercase();
            }
            // numeric / null value → stringify then lower
            let s = v.to_string();
            // trim quotes if Value was a string already stringified with quotes
            let trimmed = s.trim_matches('"').trim();
            if trimmed == "null" || trimmed.is_empty() {
                return String::new();
            }
            return trimmed.to_ascii_lowercase();
        }
    }
    if let Some(s) = platform.as_str() {
        return s.trim().to_ascii_lowercase();
    }
    if platform.is_null() {
        return String::new();
    }
    platform.to_string().trim().to_ascii_lowercase()
}

#[allow(dead_code)]
fn _platform_name(platform: &str) -> String {
    platform_name(platform)
}

/// Read a float env var with fallback.
///
/// Mirrors:
/// ```python
/// def _float_env(name: str, default: float) -> float:
///     raw = os.environ.get(name, "").strip()
///     if not raw:
///         return default
///     try:
///         return float(raw)
///     except (TypeError, ValueError):
///         return default
/// ```
pub fn float_env(name: &str, default: f64) -> f64 {
    let raw = std::env::var(name).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default;
    }
    trimmed.parse::<f64>().unwrap_or(default)
}

#[allow(dead_code)]
fn _float_env(name: &str, default: f64) -> f64 {
    float_env(name, default)
}

/// Build platform-aware thread metadata for adapter sends.
///
/// Mirrors `def _thread_metadata_for_source(source, reply_to_message_id=None) -> dict | None`
///
/// `source` is modeled as `serde_json::Value` with optional fields:
/// `thread_id`, `platform` (string or `{"value": "..."}`), `scope_id`, `chat_type`, `message_id`.
pub fn thread_metadata_for_source(
    source: &serde_json::Value,
    reply_to_message_id: Option<&str>,
) -> Option<HashMap<String, serde_json::Value>> {
    let obj = source.as_object()?;
    let thread_id = obj.get("thread_id").and_then(|v| {
        if v.is_null() {
            None
        } else if let Some(s) = v.as_str() {
            Some(s.to_string())
        } else {
            Some(v.to_string())
        }
    });

    let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(tid) = thread_id.clone() {
        metadata.insert("thread_id".to_string(), serde_json::Value::String(tid));
    }

    // Slack workspace identity is durable routing state
    let platform_val = obj.get("platform").map(|p| platform_name_value(p)).unwrap_or_default();
    if platform_val == "slack" {
        if let Some(scope_id) = obj.get("scope_id").and_then(|v| v.as_str()) {
            if !scope_id.is_empty() {
                metadata.insert(
                    "slack_team_id".to_string(),
                    serde_json::Value::String(scope_id.to_string()),
                );
            }
        }
    }

    if metadata.is_empty() {
        return None;
    }

    if platform_val == "telegram" {
        let chat_type = obj.get("chat_type").and_then(|v| v.as_str()).unwrap_or("");
        if chat_type == "dm" {
            metadata.insert(
                "telegram_dm_topic_reply_fallback".to_string(),
                serde_json::Value::Bool(true),
            );
            if let Some(tid) = thread_id.as_deref() {
                let tid_str = tid.to_string();
                if !tid_str.is_empty() && tid_str != "1" {
                    metadata.insert(
                        "direct_messages_topic_id".to_string(),
                        serde_json::Value::String(tid_str),
                    );
                }
            }
            let anchor = reply_to_message_id
                .map(|s| s.to_string())
                .or_else(|| obj.get("message_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .or_else(|| {
                    obj.get("message_id")
                        .map(|v| v.to_string().trim_matches('"').to_string())
                });
            if let Some(a) = anchor {
                if !a.is_empty() && a != "null" {
                    metadata.insert(
                        "telegram_reply_to_message_id".to_string(),
                        serde_json::Value::String(a),
                    );
                }
            }
        }
    }

    Some(metadata)
}

#[allow(dead_code)]
fn _thread_metadata_for_source(
    source: &serde_json::Value,
    reply_to_message_id: Option<&str>,
) -> Option<HashMap<String, serde_json::Value>> {
    thread_metadata_for_source(source, reply_to_message_id)
}

/// Clone metadata and mark a user-visible reply as notify-worthy.
///
/// Mirrors `def _mark_notify_metadata(metadata: dict | None) -> dict`
pub fn mark_notify_metadata(
    metadata: Option<&HashMap<String, serde_json::Value>>,
) -> HashMap<String, serde_json::Value> {
    let mut out = metadata.cloned().unwrap_or_default();
    out.insert("notify".to_string(), serde_json::Value::Bool(true));
    out
}

#[allow(dead_code)]
fn _mark_notify_metadata(
    metadata: Option<&HashMap<String, serde_json::Value>>,
) -> HashMap<String, serde_json::Value> {
    mark_notify_metadata(metadata)
}

// ---------------------------------------------------------------------------
// Reply anchor — mirrors Python lines 135–167
// ---------------------------------------------------------------------------

/// Return reply_to id for platforms that need reply semantics.
///
/// Mirrors `def _reply_anchor_for_event(event) -> str | None`
pub fn reply_anchor_for_event(event: &serde_json::Value) -> Option<String> {
    let obj = event.as_object()?;
    let source = obj.get("source")?;
    let source_obj = source.as_object()?;
    let platform = source_obj
        .get("platform")
        .map(|p| platform_name_value(p))
        .unwrap_or_default();
    let thread_id = source_obj.get("thread_id").and_then(|v| {
        if v.is_null() {
            None
        } else if let Some(s) = v.as_str() {
            if s.is_empty() { None } else { Some(s.to_string()) }
        } else {
            Some(v.to_string())
        }
    });

    let raw_message = obj.get("raw_message");

    if platform == "slack"
        && raw_message
            .and_then(|v| v.as_object())
            .and_then(|m| m.get("_hermes_no_thread_response"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return None;
    }

    if platform == "telegram" {
        if let Some(tid) = thread_id.as_deref() {
            let chat_type = source_obj
                .get("chat_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if chat_type == "dm" && !tid.is_empty() {
                // Prefer message_id, fallback to reply_to_message_id
                if let Some(mid) = obj.get("message_id").and_then(|v| v.as_str()) {
                    if !mid.is_empty() {
                        return Some(mid.to_string());
                    }
                }
                if let Some(rid) = obj.get("reply_to_message_id").and_then(|v| v.as_str()) {
                    if !rid.is_empty() {
                        return Some(rid.to_string());
                    }
                }
                // Also handle numeric message_id
                if let Some(mid) = obj.get("message_id") {
                    if !mid.is_null() {
                        let s = mid.to_string().trim_matches('"').to_string();
                        if !s.is_empty() && s != "null" {
                            return Some(s);
                        }
                    }
                }
                return None;
            }
            if !tid.is_empty() {
                return None;
            }
        }
    }

    if platform == "feishu" && thread_id.is_some() {
        if let Some(rid) = obj.get("reply_to_message_id").and_then(|v| v.as_str()) {
            if !rid.is_empty() {
                return Some(rid.to_string());
            }
        }
    }

    // Default: message_id
    if let Some(mid) = obj.get("message_id").and_then(|v| v.as_str()) {
        if !mid.is_empty() {
            return Some(mid.to_string());
        }
    }
    if let Some(mid) = obj.get("message_id") {
        if !mid.is_null() {
            let s = mid.to_string().trim_matches('"').to_string();
            if !s.is_empty() && s != "null" {
                return Some(s);
            }
        }
    }
    None
}

#[allow(dead_code)]
fn _reply_anchor_for_event(event: &serde_json::Value) -> Option<String> {
    reply_anchor_for_event(event)
}

// ---------------------------------------------------------------------------
// Media audio routing — mirrors Python lines 170–216
// ---------------------------------------------------------------------------

/// Return True when a media file should use the platform's audio sender.
///
/// Mirrors `def should_send_media_as_audio(platform, ext: str, is_voice: bool = False) -> bool`
pub fn should_send_media_as_audio(platform: &str, ext: &str, is_voice: bool) -> bool {
    let normalized_ext = ext.trim().to_ascii_lowercase();
    if !is_audio_ext(&normalized_ext) {
        return false;
    }
    if platform_name(platform) == "telegram" {
        if TELEGRAM_VOICE_EXTS.contains(&normalized_ext.as_str()) {
            return is_voice;
        }
        return TELEGRAM_AUDIO_ATTACHMENT_EXTS.contains(&normalized_ext.as_str());
    }
    true
}

/// Value overload
pub fn should_send_media_as_audio_value(
    platform: &serde_json::Value,
    ext: &str,
    is_voice: bool,
) -> bool {
    should_send_media_as_audio(&platform_name_value(platform), ext, is_voice)
}

/// Platforms whose native voice-bubble delivery requires Ogg/Opus audio.
///
/// Mirrors `tools.tts_tool.OPUS_VOICE_PLATFORMS` — the single source of truth.
/// Frozen set: `{"telegram", "matrix", "feishu", "whatsapp", "signal"}`.
pub const OPUS_VOICE_PLATFORMS: &[&str] =
    &["telegram", "matrix", "feishu", "whatsapp", "signal"];
pub const _OPUS_VOICE_PLATFORMS: &[&str] = OPUS_VOICE_PLATFORMS;

/// Return a unique temp output path for gateway auto-TTS synthesis.
///
/// Mirrors `def build_auto_tts_output_path(platform) -> str`
pub fn build_auto_tts_output_path(platform: &str) -> PathBuf {
    let normalized = platform_name(platform);
    let ext = if OPUS_VOICE_PLATFORMS.contains(&normalized.as_str()) {
        "ogg"
    } else {
        "mp3"
    };
    let dir = std::env::temp_dir().join("hermes_voice");
    let _ = std::fs::create_dir_all(&dir);
    let id = uuid::Uuid::new_v4().simple().to_string();
    let short = &id[..12.min(id.len())];
    dir.join(format!("tts_reply_{}.{}", short, ext))
}

/// Value overload
pub fn build_auto_tts_output_path_value(platform: &serde_json::Value) -> PathBuf {
    build_auto_tts_output_path(&platform_name_value(platform))
}

// ---------------------------------------------------------------------------
// UTF-16 helpers — mirrors Python lines 219–269
// ---------------------------------------------------------------------------

/// Count UTF-16 code units in *s*.
///
/// Mirrors `def utf16_len(s: str) -> int` — `len(s.encode("utf-16-le")) // 2`
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[allow(dead_code)]
fn _utf16_len(s: &str) -> usize {
    utf16_len(s)
}

/// Return the longest prefix of *s* whose UTF-16 length ≤ *limit*.
///
/// Mirrors `def _prefix_within_utf16_limit(s: str, limit: int) -> str`
pub fn prefix_within_utf16_limit(s: &str, limit: usize) -> String {
    if utf16_len(s) <= limit {
        return s.to_string();
    }
    // Binary search for longest safe prefix (codepoints, not bytes)
    let chars: Vec<char> = s.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let prefix: String = chars[..mid].iter().collect();
        if utf16_len(&prefix) <= limit {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect()
}

#[allow(dead_code)]
fn _prefix_within_utf16_limit(s: &str, limit: usize) -> String {
    prefix_within_utf16_limit(s, limit)
}

/// Return the largest codepoint offset *n* such that `len_fn(s[:n]) <= budget`.
///
/// Mirrors `def _custom_unit_to_cp(s: str, budget: int, len_fn) -> int`
pub fn custom_unit_to_cp<F>(s: &str, budget: usize, len_fn: F) -> usize
where
    F: Fn(&str) -> usize,
{
    if len_fn(s) <= budget {
        return s.chars().count();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let prefix: String = chars[..mid].iter().collect();
        if len_fn(&prefix) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[allow(dead_code)]
fn _custom_unit_to_cp<F>(s: &str, budget: usize, len_fn: F) -> usize
where
    F: Fn(&str) -> usize,
{
    custom_unit_to_cp(s, budget, len_fn)
}

// ---------------------------------------------------------------------------
// Network accessibility — mirrors Python lines 272–304
// ---------------------------------------------------------------------------

/// Return True if *host* would expose the server beyond loopback.
///
/// Mirrors `def is_network_accessible(host: str) -> bool`
pub fn is_network_accessible(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Try as IP literal
    if let Ok(addr) = trimmed.parse::<std::net::IpAddr>() {
        if addr.is_loopback() {
            return false;
        }
        // Handle ::ffff:127.0.0.1 mapped case: Rust's IpAddr::is_loopback is false for V6 mapped,
        // so check via to_ipv4_mapped if available (nightly). Fallback: parse as string check.
        // For portability, detect embedded 127.0.0.1 in v6 string.
        if let std::net::IpAddr::V6(v6) = addr {
            // Check if it's an IPv4-mapped address whose inner IPv4 is loopback
            if let Some(ipv4) = v6.to_ipv4_mapped() {
                if ipv4.is_loopback() {
                    return false;
                }
            }
            // Also handle ::ffff:127.0.0.1 textual form that didn't map cleanly
            let s = trimmed.to_ascii_lowercase();
            if s.contains("127.0.0.1") {
                // If the v6 contains loopback ipv4, treat as loopback
                // (Python checks addr.ipv4_mapped.is_loopback)
                return false;
            }
        }
        return true;
    }

    // Try as hostname — resolve via ToSocketAddrs
    // Use port 0 dummy for resolution
    let probe = format!("{}:0", trimmed);
    match probe.to_socket_addrs() {
        Ok(addrs) => {
            let mut has_non_loopback = false;
            let mut has_any = false;
            for addr in addrs {
                has_any = true;
                if !addr.ip().is_loopback() {
                    // Also check mapped loopback for V6
                    if let std::net::IpAddr::V6(v6) = addr.ip() {
                        if let Some(ipv4) = v6.to_ipv4_mapped() {
                            if ipv4.is_loopback() {
                                continue;
                            }
                        }
                    }
                    has_non_loopback = true;
                    break;
                }
            }
            if !has_any {
                // DNS resolved but no addrs? treat as not accessible (conservative)
                return false;
            }
            has_non_loopback
        }
        Err(_) => {
            // DNS failure fails closed as accessible (Python returns True on gaierror)
            true
        }
    }
}

// ---------------------------------------------------------------------------
// macOS system proxy — mirrors Python lines 307–340
// ---------------------------------------------------------------------------

/// Read the macOS system HTTP(S) proxy via `scutil --proxy`.
///
/// Mirrors `def _detect_macos_system_proxy() -> str | None`
pub fn detect_macos_system_proxy() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let out = String::from_utf8_lossy(&output.stdout);
    let mut props: HashMap<String, String> = HashMap::new();
    for line in out.lines() {
        let line = line.trim();
        if let Some(idx) = line.find(" : ") {
            let key = line[..idx].trim().to_string();
            let val = line[idx + 3..].trim().to_string();
            props.insert(key, val);
        }
    }
    for (enable_key, host_key, port_key) in [
        ("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        ("HTTPEnable", "HTTPProxy", "HTTPPort"),
    ] {
        if props.get(enable_key).map(|v| v.as_str()) == Some("1") {
            if let (Some(host), Some(port)) = (props.get(host_key), props.get(port_key)) {
                if !host.is_empty() && !port.is_empty() {
                    return Some(format!("http://{}:{}", host, port));
                }
            }
        }
    }
    None
}

#[allow(dead_code)]
fn _detect_macos_system_proxy() -> Option<String> {
    detect_macos_system_proxy()
}

// ---------------------------------------------------------------------------
// Proxy host/port helpers — mirrors Python lines 343–431
// ---------------------------------------------------------------------------

/// Split a host:port or URL into (host, port).
///
/// Mirrors `def _split_host_port(value: str) -> tuple[str, int | None]`
pub fn split_host_port(value: &str) -> (String, Option<u16>) {
    let raw = value.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    if raw.contains("://") {
        // URL case — use url crate logic manually to avoid dep
        // Extract host and port via simple parse
        if let Some(after_scheme) = raw.split("://").nth(1) {
            // Strip path/query/fragment
            let host_port = after_scheme
                .split('/')
                .next()
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .split('#')
                .next()
                .unwrap_or("");
            // Strip userinfo
            let host_port = host_port.rsplit('@').next().unwrap_or(host_port);
            return split_host_port_host_port_str(host_port);
        }
        return (String::new(), None);
    }
    if raw.starts_with('[') && raw.contains(']') {
        if let Some(end) = raw.find(']') {
            let host = raw[1..end].to_string();
            let rest = &raw[end + 1..];
            let port = if rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()) {
                rest[1..].parse::<u16>().ok()
            } else {
                None
            };
            return (host.to_ascii_lowercase().trim_end_matches('.').to_string(), port);
        }
    }
    if raw.matches(':').count() == 1 {
        if let Some(idx) = raw.rfind(':') {
            let maybe_port = &raw[idx + 1..];
            if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) {
                let host = raw[..idx].to_string();
                let port = maybe_port.parse::<u16>().ok();
                return (
                    host.to_ascii_lowercase().trim_end_matches('.').to_string(),
                    port,
                );
            }
        }
    }
    // IPv6 without brackets or plain hostname
    let host = raw.trim_matches(|c| c == '[' || c == ']').to_string();
    (host.to_ascii_lowercase().trim_end_matches('.').to_string(), None)
}

fn split_host_port_host_port_str(host_port: &str) -> (String, Option<u16>) {
    // Handle [ipv6]:port
    if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            let host = host_port[1..end].to_string();
            let rest = &host_port[end + 1..];
            let port = if rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()) {
                rest[1..].parse::<u16>().ok()
            } else {
                None
            };
            return (host.to_ascii_lowercase().trim_end_matches('.').to_string(), port);
        }
    }
    // host:port (single colon)
    if host_port.matches(':').count() == 1 {
        if let Some(idx) = host_port.rfind(':') {
            let maybe_port = &host_port[idx + 1..];
            if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) {
                let host = host_port[..idx].to_string();
                return (
                    host.to_ascii_lowercase().trim_end_matches('.').to_string(),
                    maybe_port.parse::<u16>().ok(),
                );
            }
        }
    }
    // No port, maybe IPv6 without brackets
    let host = host_port.trim_matches(|c| c == '[' || c == ']').to_string();
    (host.to_ascii_lowercase().trim_end_matches('.').to_string(), None)
}

#[allow(dead_code)]
fn _split_host_port(value: &str) -> (String, Option<u16>) {
    split_host_port(value)
}

/// Return NO_PROXY entries split by comma.
///
/// Mirrors `def _no_proxy_entries() -> list[str]`
pub fn no_proxy_entries() -> Vec<String> {
    let mut entries = Vec::new();
    for key in ["NO_PROXY", "no_proxy"] {
        if let Ok(raw) = std::env::var(key) {
            for part in raw.split(',') {
                let p = part.trim();
                if !p.is_empty() {
                    entries.push(p.to_string());
                }
            }
        }
    }
    entries
}

#[allow(dead_code)]
fn _no_proxy_entries() -> Vec<String> {
    no_proxy_entries()
}

/// Return True when a NO_PROXY entry matches host/port.
///
/// Mirrors `def _no_proxy_entry_matches(entry: str, host: str, port: int | None) -> bool`
pub fn no_proxy_entry_matches(entry: &str, host: &str, port: Option<u16>) -> bool {
    let token = entry.trim().to_ascii_lowercase();
    if token.is_empty() {
        return false;
    }
    if token == "*" {
        return true;
    }
    let (token_host, token_port) = split_host_port(&token);
    if token_port.is_some() && port.is_some() && token_port != port {
        return false;
    }
    if token_port.is_some() && port.is_none() {
        return false;
    }
    if token_host.is_empty() {
        return false;
    }
    let host_lower = host.trim().to_ascii_lowercase();
    let host_lower = host_lower.trim_end_matches('.');

    // CIDR check: token_host contains '/'
    if token_host.contains('/') {
        if let Some(slash) = token_host.find('/') {
            let network_str = &token_host[..slash];
            let prefix_str = &token_host[slash + 1..];
            if let Ok(prefix_len) = prefix_str.parse::<u8>() {
                // Try IPv4 CIDR
                if let (Ok(net_ip), Ok(host_ip)) = (
                    network_str.parse::<std::net::Ipv4Addr>(),
                    host_lower.parse::<std::net::Ipv4Addr>(),
                ) {
                    if prefix_len <= 32 {
                        let net_u32 = u32::from(net_ip);
                        let host_u32 = u32::from(host_ip);
                        let mask = if prefix_len == 0 {
                            0
                        } else {
                            u32::MAX << (32 - prefix_len)
                        };
                        if (net_u32 & mask) == (host_u32 & mask) {
                            return true;
                        }
                    }
                    return false;
                }
                // Try IPv6 CIDR
                if let (Ok(net_ip), Ok(host_ip)) = (
                    network_str.parse::<std::net::Ipv6Addr>(),
                    host_lower.parse::<std::net::Ipv6Addr>(),
                ) {
                    if prefix_len <= 128 {
                        let net_u128 = u128::from(net_ip);
                        let host_u128 = u128::from(host_ip);
                        let mask = if prefix_len == 0 {
                            0
                        } else {
                            u128::MAX << (128 - prefix_len)
                        };
                        if (net_u128 & mask) == (host_u128 & mask) {
                            return true;
                        }
                    }
                    return false;
                }
                return false;
            }
        }
    }

    // Try IP literal exact match
    if let Ok(token_ip) = token_host.parse::<std::net::IpAddr>() {
        if let Ok(host_ip) = host_lower.parse::<std::net::IpAddr>() {
            return token_ip == host_ip;
        }
        return false;
    }

    // Wildcard / dot handling
    if token_host.starts_with("*.") {
        let suffix = &token_host[1..]; // ".example.com"
        return host_lower.ends_with(suffix);
    }
    if token_host.starts_with('.') {
        return host_lower == &token_host[1..] || host_lower.ends_with(&token_host);
    }
    host_lower == token_host || host_lower.ends_with(&format!(".{}", token_host))
}

#[allow(dead_code)]
fn _no_proxy_entry_matches(entry: &str, host: &str, port: Option<u16>) -> bool {
    no_proxy_entry_matches(entry, host, port)
}

/// Return True when NO_PROXY matches at least one target host.
///
/// Mirrors `def should_bypass_proxy(target_hosts) -> bool`
pub fn should_bypass_proxy(target_hosts: &[String]) -> bool {
    let entries = no_proxy_entries();
    if entries.is_empty() || target_hosts.is_empty() {
        return false;
    }
    for candidate in target_hosts {
        let (host, port) = split_host_port(candidate);
        if host.is_empty() {
            continue;
        }
        if entries
            .iter()
            .any(|e| no_proxy_entry_matches(e, &host, port))
        {
            return true;
        }
    }
    false
}

/// Single-host overload
pub fn should_bypass_proxy_one(target_host: &str) -> bool {
    should_bypass_proxy(&[target_host.to_string()])
}

#[allow(dead_code)]
fn _should_bypass_proxy(target_hosts: &[String]) -> bool {
    should_bypass_proxy(target_hosts)
}

// ---------------------------------------------------------------------------
// Proxy resolution — mirrors Python lines 434–533
// ---------------------------------------------------------------------------

/// Normalize a proxy URL (adds scheme if missing, trims whitespace).
///
/// Mirrors `utils.normalize_proxy_url` — minimal port that ensures an
/// `http://` scheme if none is present and trims whitespace.
pub fn normalize_proxy_url(url: Option<&str>) -> Option<String> {
    let raw = url?.trim();
    if raw.is_empty() {
        return None;
    }
    // If it already has a scheme, keep as-is (lowercase scheme handling done by caller)
    if raw.contains("://") {
        return Some(raw.to_string());
    }
    // No scheme — default to http://
    Some(format!("http://{}", raw))
}

fn normalize_proxy_url_owned(s: &str) -> Option<String> {
    normalize_proxy_url(Some(s))
}

/// Return a proxy URL from env vars, or macOS system proxy.
///
/// Mirrors `def resolve_proxy_url(platform_env_var, *, target_hosts) -> str | None`
pub fn resolve_proxy_url(
    platform_env_var: Option<&str>,
    target_hosts: Option<&[String]>,
) -> Option<String> {
    let bypass = target_hosts
        .map(|hosts| should_bypass_proxy(hosts))
        .unwrap_or(false);

    if let Some(var) = platform_env_var {
        if let Ok(val) = std::env::var(var) {
            let v = val.trim().to_string();
            if !v.is_empty() {
                if bypass {
                    return None;
                }
                return normalize_proxy_url_owned(&v);
            }
        }
    }
    for key in [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
    ] {
        if let Ok(val) = std::env::var(key) {
            let v = val.trim().to_string();
            if !v.is_empty() {
                if bypass {
                    return None;
                }
                return normalize_proxy_url_owned(&v);
            }
        }
    }
    let detected = normalize_proxy_url(detect_macos_system_proxy().as_deref());
    if detected.is_some() && bypass {
        return None;
    }
    detected
}

#[allow(dead_code)]
fn _resolve_proxy_url(
    platform_env_var: Option<&str>,
    target_hosts: Option<&[String]>,
) -> Option<String> {
    resolve_proxy_url(platform_env_var, target_hosts)
}

// ---------------------------------------------------------------------------
// Proxy kwargs helpers — mirrors Python lines 468–533
// ---------------------------------------------------------------------------

/// Proxy connector description for bot clients.
///
/// Mirrors `def proxy_kwargs_for_bot(proxy_url: str | None) -> dict`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyKind {
    Direct,
    Http { proxy_url: String },
    Socks { proxy_url: String },
}

/// Mirrors `proxy_kwargs_for_bot` — returns the connector kind.
pub fn proxy_kwargs_for_bot(proxy_url: Option<&str>) -> ProxyKind {
    let Some(url) = proxy_url.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }) else {
        return ProxyKind::Direct;
    };
    if url.to_ascii_lowercase().starts_with("socks") {
        // In Python this tries `aiohttp_socks.ProxyConnector`; Rust port records intent.
        // If `aiohttp_socks` equivalent is missing, Python logs warning and returns {}.
        // Rust always returns Socks; caller can map Direct on missing dep.
        return ProxyKind::Socks { proxy_url: url };
    }
    ProxyKind::Http { proxy_url: url }
}

#[allow(dead_code)]
fn _proxy_kwargs_for_bot(proxy_url: Option<&str>) -> ProxyKind {
    proxy_kwargs_for_bot(proxy_url)
}

/// Session vs request kwargs for aiohttp.
///
/// Mirrors `def proxy_kwargs_for_aiohttp(proxy_url: str | None) -> tuple[dict, dict]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiohttpProxyConfig {
    pub connector: Option<String>,
    pub request_proxy: Option<String>,
}

pub fn proxy_kwargs_for_aiohttp(proxy_url: Option<&str>) -> AiohttpProxyConfig {
    let Some(url) = proxy_url.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }) else {
        return AiohttpProxyConfig {
            connector: None,
            request_proxy: None,
        };
    };
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("socks") {
        // Prefer connector path for all schemes when aiohttp_socks available; Python does
        // `ProxyConnector.from_url(proxy_url, rdns=True)` for any scheme.
        // Without aiohttp_socks, SOCKS is ignored with warning.
        // Rust records connector intent; caller decides at runtime if Socks dep is present.
        return AiohttpProxyConfig {
            connector: Some(url.clone()),
            request_proxy: None,
        };
    }
    // Check if socks dep would be available — in Rust this is always true via connector,
    // so we follow the `ProxyConnector` path for HTTP too when available.
    // To mirror Python's `try: from aiohttp_socks import ProxyConnector` covering all schemes,
    // we keep connector path. The alternate `({}, {"proxy": url})` is only when import fails.
    // Rust default: use connector (ponytail: one path, add per-request proxy fallback if needed).
    AiohttpProxyConfig {
        connector: Some(url),
        request_proxy: None,
    }
}

#[allow(dead_code)]
fn _proxy_kwargs_for_aiohttp(proxy_url: Option<&str>) -> AiohttpProxyConfig {
    proxy_kwargs_for_aiohttp(proxy_url)
}

// ---------------------------------------------------------------------------
// Host excluded by NO_PROXY — mirrors Python lines 536–566
// ---------------------------------------------------------------------------

/// Return True when `hostname` matches a `NO_PROXY` entry.
///
/// Mirrors `def is_host_excluded_by_no_proxy(hostname: str, no_proxy_value: str | None = None) -> bool`
pub fn is_host_excluded_by_no_proxy(hostname: &str, no_proxy_value: Option<&str>) -> bool {
    let raw = match no_proxy_value {
        Some(v) => v.trim().to_string(),
        None => std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if raw.is_empty() {
        return false;
    }
    let lower_hostname = hostname.trim().to_ascii_lowercase();
    if lower_hostname.is_empty() {
        return false;
    }
    for entry in raw.split(|c| c == ',' || c == ' ' || c == '\t' || c == '\n') {
        let mut normalized = entry.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if normalized == "*" {
            return true;
        }
        if normalized.starts_with("*.") {
            normalized = normalized[2..].to_string();
        } else if normalized.starts_with('.') {
            normalized = normalized[1..].to_string();
        }
        if lower_hostname == normalized || lower_hostname.ends_with(&format!(".{}", normalized)) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Dataclasses / streaming TTS — mirrors Python lines 569–654
// ---------------------------------------------------------------------------

// Python: import dataclasses, datetime, pathlib, typing, enum
// Rust equivalents below. The `sys.path.insert` injection is a no-op in Rust.

/// Mirrors `gateway.config.Platform` (minimal stub — wire to real type when available).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Telegram,
    Discord,
    Slack,
    Whatsapp,
    Feishu,
    Unknown(String),
}

impl Platform {
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Telegram => "telegram",
            Platform::Discord => "discord",
            Platform::Slack => "slack",
            Platform::Whatsapp => "whatsapp",
            Platform::Feishu => "feishu",
            Platform::Unknown(s) => s.as_str(),
        }
    }
}

/// Mirrors `gateway.config.PlatformConfig` (minimal stub).
#[derive(Debug, Clone, Default)]
pub struct PlatformConfig {
    pub enabled: bool,
    pub raw: serde_json::Value,
}

/// Mirrors `gateway.session.SessionSource` (minimal stub).
#[derive(Debug, Clone, Default)]
pub struct SessionSource {
    pub platform: Option<String>,
    pub scope_id: Option<String>,
    pub thread_id: Option<String>,
    pub chat_type: Option<String>,
    pub message_id: Option<String>,
}

/// Mirrors `gateway.session.build_session_key` (stub).
pub fn build_session_key(source: &SessionSource) -> String {
    format!(
        "{}:{}:{}",
        source.platform.as_deref().unwrap_or("unknown"),
        source.scope_id.as_deref().unwrap_or(""),
        source.thread_id.as_deref().unwrap_or("")
    )
}

// ---------------------------------------------------------------------------
// hermes_constants helpers — mirrors `hermes_constants.get_hermes_home` etc.
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME` (`$HERMES_HOME` or `~/.hermes`).
///
/// Mirrors `hermes_constants.get_hermes_home()`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

/// Mirrors `hermes_constants.get_hermes_dir(subpath, old_name)`.
///
/// Returns `$HERMES_HOME/<new_subpath>` (new layout) — the import-time
/// `*_CACHE_DIR` defaults point here; legacy `old_name` is retained for
/// traceability but not used for fresh resolution (monkeypatch detection
/// handles legacy overrides separately via `resolve_cache_dir`).
pub fn get_hermes_dir(new_subpath: &str, _old_name: &str) -> PathBuf {
    get_hermes_home().join(new_subpath)
}

/// Mirrors `hermes_constants.get_default_hermes_root()`.
pub fn get_default_hermes_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// Streaming TTS format descriptor and handle — mirrors Python lines 589–654
// ---------------------------------------------------------------------------

/// Declared PCM format for a streaming-TTS session.
///
/// Mirrors `@dataclass class AudioFormat`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    /// Sample rate in Hz (default 24000).
    pub sample_rate: u32,
    /// Channel count (default 1).
    pub channels: u8,
    /// Bytes per sample (int16 = 2).
    pub sample_width: u8,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: 24000,
            channels: 1,
            sample_width: 2,
        }
    }
}

/// Opaque handle returned by `begin_streaming_tts`.
///
/// Mirrors `@dataclass class StreamingTTSHandle`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingTTSHandle {
    pub chat_id: String,
    pub audio_format: AudioFormat,
    /// Set to True after first PCM chunk (audible output started).
    pub audible: bool,
    /// Set to True by abort_streaming_tts; late chunks dropped.
    pub aborted: bool,
}

impl Default for StreamingTTSHandle {
    fn default() -> Self {
        Self {
            chat_id: String::new(),
            audio_format: AudioFormat::default(),
            audible: false,
            aborted: false,
        }
    }
}

/// Return a per-turn streaming-TTS suppression key.
///
/// Mirrors `def streaming_tts_turn_key(session_key, turn_marker, *, event) -> str | None`
pub fn streaming_tts_turn_key(
    session_key: Option<&str>,
    turn_marker: Option<&str>,
    event: Option<&serde_json::Value>,
) -> Option<String> {
    let sk = session_key?;
    if sk.is_empty() {
        return None;
    }
    let marker = if let Some(m) = turn_marker {
        if !m.is_empty() {
            Some(m.to_string())
        } else {
            None
        }
    } else if let Some(ev) = event {
        let mid = ev.get("message_id").and_then(|v| v.as_str()).map(|s| s.to_string());
        let pid = ev.get("platform_update_id").and_then(|v| v.as_str()).map(|s| s.to_string());
        mid.or(pid)
    } else {
        None
    }?;
    if marker.is_empty() {
        return None;
    }
    Some(format!("{}:{}", sk, marker))
}

#[allow(dead_code)]
fn _streaming_tts_turn_key(
    session_key: Option<&str>,
    turn_marker: Option<&str>,
    event: Option<&serde_json::Value>,
) -> Option<String> {
    streaming_tts_turn_key(session_key, turn_marker, event)
}

/// Pure helper used by the auto-TTS suppression path.
///
/// Mirrors `def streaming_tts_should_skip_whole_file(completed_turns, session_key, turn_marker, *, event) -> bool`
pub fn streaming_tts_should_skip_whole_file(
    completed_turns: &HashSet<String>,
    session_key: Option<&str>,
    turn_marker: Option<&str>,
    event: Option<&serde_json::Value>,
) -> bool {
    if let Some(key) = streaming_tts_turn_key(session_key, turn_marker, event) {
        completed_turns.contains(&key)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Secret capture message — mirrors Python lines 656–659
// ---------------------------------------------------------------------------

/// Mirrors `GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE`
pub const GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE: &str =
    "Secure secret entry is not supported over messaging. \
     Load this skill in the local CLI to be prompted, or add the key to ~/.hermes/.env manually.";
pub const _GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE: &str =
    GATEWAY_SECRET_CAPTURE_UNSUPPORTED_MESSAGE;

// ---------------------------------------------------------------------------
// URL helpers — mirrors Python lines 662–712
// ---------------------------------------------------------------------------

/// Return a URL string safe for logs (no query/fragment/userinfo).
///
/// Mirrors `def safe_url_for_log(url: str, max_len: int = 80) -> str`
pub fn safe_url_for_log(url: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    if url.is_empty() {
        return String::new();
    }
    let raw = url.trim();
    if raw.is_empty() {
        return String::new();
    }

    // Minimal urlsplit equivalent: detect scheme://netloc
    let (scheme, rest) = if let Some(idx) = raw.find("://") {
        (&raw[..idx], &raw[idx + 3..])
    } else {
        // No scheme/netloc — treat as raw
        let safe = raw.to_string();
        if safe.len() <= max_len {
            return safe;
        }
        if max_len <= 3 {
            return ".".repeat(max_len);
        }
        return format!("{}...", &safe[..max_len - 3]);
    };

    if scheme.is_empty() || rest.is_empty() {
        let safe = raw.to_string();
        if safe.len() <= max_len {
            return safe;
        }
        if max_len <= 3 {
            return ".".repeat(max_len);
        }
        return format!("{}...", &safe[..max_len - 3]);
    }

    // netloc is up to first '/' or '?' or '#'
    let netloc_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let netloc_raw = &rest[..netloc_end];
    let path = if netloc_end < rest.len() {
        // Only keep path part up to '?' or '#'
        let path_and_rest = &rest[netloc_end..];
        let path_end = path_and_rest
            .find(|c| c == '?' || c == '#')
            .unwrap_or(path_and_rest.len());
        &path_and_rest[..path_end]
    } else {
        ""
    };

    // Strip embedded credentials user:pass@host
    let netloc = netloc_raw.rsplit('@').next().unwrap_or(netloc_raw);
    let base = format!("{}://{}", scheme, netloc);

    let safe = if !path.is_empty() && path != "/" {
        let basename = path.rsplit('/').next().unwrap_or("");
        if !basename.is_empty() {
            format!("{}/.../{}", base, basename)
        } else {
            format!("{}/...", base)
        }
    } else {
        base
    };

    if safe.len() <= max_len {
        return safe;
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }
    format!("{}...", &safe[..max_len - 3])
}

/// Default overload (max_len = 80)
pub fn safe_url_for_log_default(url: &str) -> String {
    safe_url_for_log(url, 80)
}

/// Re-validate each redirect target to prevent redirect-based SSRF.
///
/// Mirrors `async def _ssrf_redirect_guard(response)`
///
/// In Rust, `redirect_url` is the extracted `Location` header (or empty).
/// Returns `Err` if the URL is unsafe (would require `is_safe_url` check).
pub async fn ssrf_redirect_guard(redirect_url: Option<&str>) -> Result<(), String> {
    if let Some(url) = redirect_url {
        if !is_safe_url(url) {
            return Err(format!(
                "Blocked redirect to private/internal address: {}",
                safe_url_for_log(url, 80)
            ));
        }
    }
    Ok(())
}

#[allow(dead_code)]
async fn _ssrf_redirect_guard(redirect_url: Option<&str>) -> Result<(), String> {
    ssrf_redirect_guard(redirect_url).await
}

/// Minimal `is_safe_url` stub — mirrors `tools.url_safety.is_safe_url`.
///
/// Real implementation checks against private/internal networks (SSRF guard).
/// This stub blocks `localhost`, `127.0.0.1`, `::1`, `169.254.169.254`, `10.*`, `192.168.*`, `172.16.*` etc.
/// and allows public hosts.
pub fn is_safe_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Extract host
    let host = if let Some(idx) = trimmed.find("://") {
        let rest = &trimmed[idx + 3..];
        let host_port = rest
            .split('/')
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("");
        host_port.rsplit('@').next().unwrap_or(host_port).to_string()
    } else {
        trimmed.to_string()
    };
    let (host_only, _) = split_host_port(&host);
    let lower = host_only.to_ascii_lowercase();
    if lower == "localhost"
        || lower == "127.0.0.1"
        || lower == "::1"
        || lower == "169.254.169.254"
        || lower == "169.254.169.253"
    {
        return false;
    }
    if lower.starts_with("10.") || lower.starts_with("192.168.") || lower.starts_with("172.") {
        // 172.16.0.0/12 → 172.16.x.x – 172.31.x.x
        if lower.starts_with("172.") {
            if let Some(octet2) = lower.split('.').nth(1).and_then(|s| s.parse::<u8>().ok()) {
                if (16..=31).contains(&octet2) {
                    return false;
                }
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    if lower == "0.0.0.0" || lower == "::" {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Image cache utilities — mirrors Python lines 724–943
// ---------------------------------------------------------------------------

/// Import-time default. Tests monkeypatch this; getters re-resolve per call.
/// Mirrors `IMAGE_CACHE_DIR = get_hermes_dir("cache/images", "image_cache")`
pub fn image_cache_dir_default() -> PathBuf {
    get_hermes_dir("cache/images", "image_cache")
}

/// Mirrors `IMAGE_CACHE_DIR` (import-time default).
pub static IMAGE_CACHE_DIR_DEFAULT: std::sync::LazyLock<PathBuf> =
    std::sync::LazyLock::new(image_cache_dir_default);

/// Resolve fresh via `get_hermes_dir` (active profile), unless a test has
/// monkeypatched the constant away from its import-time default.
///
/// Mirrors `def _resolve_cache_dir(constant_name: str, new_subpath: str, old_name: str) -> Path`
pub fn resolve_cache_dir(
    current: Option<&Path>,
    new_subpath: &str,
    old_name: &str,
) -> PathBuf {
    let fresh = get_hermes_dir(new_subpath, old_name);
    if let Some(cur) = current {
        // Compare against import-time default: if current != default, it's a monkeypatch
        let default = image_cache_dir_default();
        if cur != default.as_path() {
            return cur.to_path_buf();
        }
    }
    fresh
}

#[allow(dead_code)]
fn _resolve_cache_dir(current: Option<&Path>, new_subpath: &str, old_name: &str) -> PathBuf {
    resolve_cache_dir(current, new_subpath, old_name)
}

/// Inbound media size cap — mirrors `DEFAULT_INBOUND_MEDIA_MAX_BYTES = 128 * 1024 * 1024`
pub const DEFAULT_INBOUND_MEDIA_MAX_BYTES: usize = 128 * 1024 * 1024;
pub const _DEFAULT_INBOUND_MEDIA_MAX_BYTES: usize = DEFAULT_INBOUND_MEDIA_MAX_BYTES;

/// Return the max inbound image/audio/video bytes allowed in memory.
///
/// Mirrors `def get_inbound_media_max_bytes() -> int`
pub fn get_inbound_media_max_bytes() -> usize {
    // Read `gateway.max_inbound_media_bytes` from config.yaml
    let home = get_hermes_home();
    let path = home.join("config.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return DEFAULT_INBOUND_MEDIA_MAX_BYTES,
    };
    // Try JSON first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(obj) = v.as_object() {
            if let Some(gw) = obj.get("gateway").and_then(|v| v.as_object()) {
                if let Some(val) = gw.get("max_inbound_media_bytes") {
                    if let Some(n) = val.as_i64() {
                        return n as usize;
                    }
                    if let Some(n) = val.as_u64() {
                        return n as usize;
                    }
                }
            }
        }
        return DEFAULT_INBOUND_MEDIA_MAX_BYTES;
    }
    // Minimal YAML scan for `gateway: max_inbound_media_bytes: N`
    let mut in_gateway = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        if indent == 0 && trimmed.starts_with("gateway:") {
            in_gateway = true;
            continue;
        }
        if in_gateway {
            if indent == 0 && trimmed.contains(':') {
                break;
            }
            if trimmed.starts_with("max_inbound_media_bytes") {
                if let Some(colon) = trimmed.find(':') {
                    let val_str = trimmed[colon + 1..]
                        .trim()
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if let Ok(n) = val_str.parse::<i64>() {
                        return n as usize;
                    }
                }
            }
        }
    }
    DEFAULT_INBOUND_MEDIA_MAX_BYTES
}

/// Raise `ValueError` if inbound media payload exceeds the cap.
///
/// Mirrors `def validate_inbound_media_size(size: int, *, media_type: str = "media", max_bytes: Optional[int] = None) -> None`
pub fn validate_inbound_media_size(
    size: usize,
    media_type: &str,
    max_bytes: Option<usize>,
) -> Result<(), String> {
    let limit = max_bytes.unwrap_or_else(get_inbound_media_max_bytes);
    if limit != 0 && size > limit {
        return Err(format!(
            "Inbound {} payload is too large ({} bytes > {} bytes)",
            media_type, size, limit
        ));
    }
    Ok(())
}

/// Read a streaming response body without exceeding the media cap.
///
/// Mirrors `async def _read_httpx_body_with_limit(response, *, media_type: str) -> bytes`
///
/// Generic over any `AsyncRead` + headers map.
pub async fn read_httpx_body_with_limit<R>(
    content_length: Option<&str>,
    mut reader: R,
    media_type: &str,
) -> Result<Vec<u8>, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let max_bytes = get_inbound_media_max_bytes();
    if let Some(cl) = content_length {
        if let Ok(declared) = cl.trim().parse::<usize>() {
            validate_inbound_media_size(declared, media_type, Some(max_bytes))?;
        } else {
            log::debug!("Ignoring invalid Content-Length for inbound {}: {:?}", media_type, cl);
        }
    }
    let mut total: usize = 0;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    use tokio::io::AsyncReadExt;
    loop {
        let n = reader.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n;
        validate_inbound_media_size(total, media_type, Some(max_bytes))?;
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

#[allow(dead_code)]
async fn _read_httpx_body_with_limit<R>(
    content_length: Option<&str>,
    reader: R,
    media_type: &str,
) -> Result<Vec<u8>, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    read_httpx_body_with_limit(content_length, reader, media_type).await
}

/// Return the image cache directory, creating it if it doesn't exist.
///
/// Mirrors `def get_image_cache_dir() -> Path`
pub fn get_image_cache_dir() -> PathBuf {
    let d = resolve_cache_dir(None, "cache/images", "image_cache");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Return True if *data* starts with a known image magic-byte sequence.
///
/// Mirrors `def _looks_like_image(data: bytes) -> bool`
pub fn looks_like_image(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    if data.len() >= 8 && data[..8] == [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'] {
        return true;
    }
    if data.len() >= 3 && data[..3] == [0xff, 0xd8, 0xff] {
        return true;
    }
    if data.len() >= 6 && (data[..6] == *b"GIF87a" || data[..6] == *b"GIF89a") {
        return true;
    }
    if data.len() >= 2 && data[..2] == [b'B', b'M'] {
        return true;
    }
    if data.len() >= 12 && data[..4] == *b"RIFF" && data[8..12] == *b"WEBP" {
        return true;
    }
    false
}

#[allow(dead_code)]
fn _looks_like_image(data: &[u8]) -> bool {
    looks_like_image(data)
}

/// Save raw image bytes to the cache and return the absolute file path.
///
/// Mirrors `def cache_image_from_bytes(data: bytes, ext: str = ".jpg") -> str`
pub fn cache_image_from_bytes(data: &[u8], ext: &str) -> Result<String, String> {
    validate_inbound_media_size(data.len(), "image", None)?;
    if !looks_like_image(data) {
        let snippet = String::from_utf8_lossy(&data[..data.len().min(80)]).to_string();
        return Err(format!(
            "Refusing to cache non-image data as {} (starts with: {:?})",
            ext, snippet
        ));
    }
    let cache_dir = get_image_cache_dir();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let short = &id[..12.min(id.len())];
    let filename = format!("img_{}{}", short, ext);
    let filepath = cache_dir.join(filename);
    std::fs::write(&filepath, data).map_err(|e| e.to_string())?;
    Ok(filepath.to_string_lossy().to_string())
}

/// Download an image from a URL and save it to the local cache.
///
/// Mirrors `async def cache_image_from_url(url: str, ext: str = ".jpg", retries: int = 2) -> str`
///
/// Retries on transient failures (timeouts, 429, 5xx) with exponential backoff.
pub async fn cache_image_from_url(url: &str, ext: &str, retries: usize) -> Result<String, String> {
    if !is_safe_url(url) {
        return Err(format!(
            "Blocked unsafe URL (SSRF protection): {}",
            safe_url_for_log(url, 80)
        ));
    }

    // Use `reqwest` if available; here we model the retry loop explicitly
    // without pulling `reqwest` to keep NO CARGO. Real wire call is:
    // `create_ssrf_safe_async_client(...).stream(GET, url).raise_for_status().read_with_limit()`
    // For the 1:1 port we keep the retry/backoff structure and delegate to
    // `cache_image_from_bytes` on success.

    let mut last_err: Option<String> = None;
    for attempt in 0..=retries {
        // In production: `client.stream("GET", url, headers={...})`
        // Rust: attempt fetch via `fetch_image_bytes(url)` stub
        match fetch_image_bytes_stub(url).await {
            Ok(content) => {
                // Validate size and cache
                return cache_image_from_bytes(&content, ext);
            }
            Err(e) => {
                let is_retryable = is_retryable_fetch_error(&e);
                if !is_retryable {
                    return Err(e);
                }
                if attempt < retries {
                    let wait = 1.5 * (attempt as f64 + 1.0);
                    log::debug!(
                        "Media cache retry {}/{} for {} ({:.1}s): {}",
                        attempt + 1,
                        retries,
                        safe_url_for_log(url, 80),
                        wait,
                        e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis((wait * 1000.0) as u64))
                        .await;
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "unknown fetch error".to_string()))
}

async fn fetch_image_bytes_stub(_url: &str) -> Result<Vec<u8>, String> {
    // Placeholder for real HTTP fetch — in production this does:
    // `async with create_ssrf_safe_async_client(timeout=30, follow_redirects=True, event_hooks={"response": [_ssrf_redirect_guard]}) as client:`
    // Real impl requires `reqwest` + SSRF guard; stub returns error to preserve retry semantics in tests.
    Err("fetch not wired in slice 1 stub — wire reqwest + is_safe_url in slice 2".to_string())
}

fn is_retryable_fetch_error(err: &str) -> bool {
    // Mirrors Python: `except (httpx.TimeoutException, httpx.HTTPStatusError) as exc: if status <429: raise`
    // Retry on timeout strings, 429, 5xx
    let lower = err.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timedout") {
        return true;
    }
    if lower.contains("429") || lower.contains("rate") {
        return true;
    }
    if lower.contains("500") || lower.contains("502") || lower.contains("503") || lower.contains("504") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Helpers for traceability — mirrors Python underscore-prefixed names
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _looks_like_image_fn(data: &[u8]) -> bool {
    looks_like_image(data)
}
