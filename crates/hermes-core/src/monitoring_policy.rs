//! Install identity for gateway monitoring.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/monitoring/policy.py` (57 lines).
//!
//! The install id is a stable, resettable pseudonymous identifier attached to
//! exported health signals so an operator can tell instances apart in their
//! collector. It carries no account identity and can be rotated by clearing
//! `monitoring.install_id` in config.
//!
//! Python source docstring (preserved):
//! ```text
//! Install identity for gateway monitoring.
//!
//! The install id is a stable, resettable pseudonymous identifier attached to
//! exported health signals so an operator can tell instances apart in their
//! collector. It carries no account identity and can be rotated by clearing
//! ``monitoring.install_id`` in config.
//! ```

use serde_json::Value;

// ---------------------------------------------------------------------------
// helpers — mirrors Python line-numbered blocks
// ---------------------------------------------------------------------------

/// Mirrors the `isinstance(existing, str) and existing.strip()` check (line 32).
#[inline]
pub fn is_valid_install_id(s: &str) -> bool {
    !s.trim().is_empty()
}

#[allow(dead_code)]
fn _is_valid_install_id(s: &str) -> bool {
    is_valid_install_id(s)
}

/// Extract `monitoring.install_id` as a trimmed non-empty String if present.
///
/// Mirrors lines 30-33:
/// ```python
/// mon = config.get("monitoring") if isinstance(config, dict) else None
/// existing = (mon or {}).get("install_id") if isinstance(mon, dict) else None
/// if isinstance(existing, str) and existing.strip():
///     return existing
/// ```
pub fn get_existing_install_id(config: &Value) -> Option<String> {
    let mon = match config {
        Value::Object(map) => map.get("monitoring"),
        _ => None,
    }?;
    let mon_obj = match mon {
        Value::Object(m) => m,
        _ => return None,
    };
    let existing = mon_obj.get("install_id")?;
    match existing {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

#[allow(dead_code)]
fn _get_existing_install_id(config: &Value) -> Option<String> {
    get_existing_install_id(config)
}

/// Mirrors `str(slot.get("install_id") or "").strip()` emptiness check (line 42).
///
/// Python: `not str(slot.get("install_id") or "").strip()` — falsy values
/// (None, "", 0, False, empty containers) become `""` via `or ""`, then
/// `str(...).strip() == ""` is considered empty and will be overwritten.
/// Rust approximation: only `String` non-empty trimmed counts as present;
/// missing / Null / empty String → empty (should mint), everything else
/// stringified via `to_string` trimmed check preserves the spirit for
/// non-string stored ids.
fn slot_install_id_is_empty(slot: &Value) -> bool {
    match slot {
        Value::Object(map) => match map.get("install_id") {
            None => true,
            Some(Value::String(s)) => s.trim().is_empty(),
            Some(Value::Null) => true,
            Some(v) => {
                // Mirror Python `str(v or "")` truthiness: falsy Rust values
                // (Null already handled, Bool(false), Number 0) → empty.
                match v {
                    Value::Bool(false) => true,
                    Value::Number(n) => {
                        // 0 / 0.0 are falsy in Python (`0 or ""` → "")
                        if let Some(i) = n.as_i64() {
                            if i == 0 {
                                return true;
                            }
                        }
                        if let Some(f) = n.as_f64() {
                            if f == 0.0 {
                                return true;
                            }
                        }
                        // non-zero numbers stringify to non-empty → not empty
                        let s = v.to_string();
                        s.trim().is_empty()
                    }
                    Value::Array(arr) if arr.is_empty() => true,
                    Value::Object(obj) if obj.is_empty() => true,
                    _ => {
                        let s = match v {
                            Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        };
                        s.trim().is_empty()
                    }
                }
            }
        },
        _ => true,
    }
}

#[allow(dead_code)]
fn _slot_install_id_is_empty(slot: &Value) -> bool {
    slot_install_id_is_empty(slot)
}

/// Mirrors `minted = str(uuid.uuid4())` (line 35).
#[inline]
pub fn mint_install_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[allow(dead_code)]
fn _mint_install_id() -> String {
    mint_install_id()
}

/// Ensure `config` is an Object and `config["monitoring"]` is an Object,
/// then set `install_id = minted`. Mirrors lines 48-51:
/// ```python
/// if isinstance(config, dict):
///     config.setdefault("monitoring", {})
///     if isinstance(config["monitoring"], dict):
///         config["monitoring"]["install_id"] = minted
/// ```
pub fn set_install_id(config: &mut Value, minted: String) {
    let map = match config {
        Value::Object(m) => m,
        _ => return,
    };
    // setdefault("monitoring", {})
    let entry = map
        .entry("monitoring".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(slot) = entry {
        slot.insert("install_id".to_string(), Value::String(minted));
    }
}

#[allow(dead_code)]
fn _set_install_id(config: &mut Value, minted: String) {
    set_install_id(config, minted)
}

// ---------------------------------------------------------------------------
// core — mirrors `ensure_install_id(config: Dict[str, Any]) -> str` (lines 18-52)
// ---------------------------------------------------------------------------

/// Return a stable install id, minting and persisting one when empty.
///
/// Mirrors `ensure_install_id` (lines 18-52). The id must survive gateway
/// restarts (it becomes `service.instance.id` on exported signals), so a
/// freshly minted UUID is written back to `config.yaml` immediately. The
/// write is fail-open: if persisting fails (read-only home, managed scope),
/// the ephemeral id is still returned and a new one is minted next start.
///
/// Clearing `monitoring.install_id` (e.g. `hermes config set
/// monitoring.install_id ""`) rotates the id on the next gateway start.
///
/// `load` / `save` are injected to keep this crate dependency-free from
/// `hermes_cli.config` YAML I/O and to allow hermetic tests. `load` mirrors
/// `load_config()` (line 39) and `save` mirrors `save_config(fresh)` (line 44).
/// Both are fail-open: any error is logged at debug and ignored, matching
/// `except Exception: logger.debug(..., exc_info=True)` (lines 45-46).
pub fn ensure_install_id_with<F, G, E>(config: &mut Value, load: F, save: G) -> String
where
    F: Fn() -> Option<Value>,
    G: Fn(&Value) -> Result<(), E>,
    E: std::fmt::Debug,
{
    // Mirrors lines 30-33: early return if existing valid id
    if let Some(existing) = get_existing_install_id(config) {
        return existing;
    }

    // Mirrors line 35: mint
    let minted = mint_install_id();

    // Mirrors lines 36-46: try to persist to fresh load
    // ```python
    // try:
    //     from hermes_cli.config import load_config, save_config
    //     fresh = load_config()
    //     if isinstance(fresh, dict):
    //         slot = fresh.setdefault("monitoring", {})
    //         if isinstance(slot, dict) and not str(slot.get("install_id") or "").strip():
    //             slot["install_id"] = minted
    //             save_config(fresh)
    // except Exception:
    //     logger.debug("install_id persist failed; using ephemeral id", exc_info=True)
    // ```
    // Fail-open: any panic/error in load/save is caught and debug-logged.
    let persist_result: Result<(), ()> = (|| {
        let mut fresh = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&load)) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(()),
            Err(_) => {
                log::debug!("install_id persist failed; using ephemeral id");
                return Ok(());
            }
        };
        // if isinstance(fresh, dict)
        let is_object = matches!(fresh, Value::Object(_));
        if !is_object {
            return Ok(());
        }
        // fresh.setdefault("monitoring", {})
        let slot_empty = {
            let map = match &mut fresh {
                Value::Object(m) => m,
                _ => return Ok(()),
            };
            let slot = map
                .entry("monitoring".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            // if isinstance(slot, dict) and not str(slot.get("install_id") or "").strip()
            if !matches!(slot, Value::Object(_)) {
                return Ok(());
            }
            slot_install_id_is_empty(slot)
        };
        if !slot_empty {
            return Ok(());
        }
        // slot["install_id"] = minted
        if let Value::Object(map) = &mut fresh {
            if let Some(Value::Object(slot)) = map.get_mut("monitoring") {
                slot.insert("install_id".to_string(), Value::String(minted.clone()));
            }
        }
        // save_config(fresh) — fail-open
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| save(&fresh))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                log::debug!("install_id persist failed; using ephemeral id: {:?}", e);
                Ok(())
            }
            Err(_) => {
                log::debug!("install_id persist failed; using ephemeral id");
                Ok(())
            }
        }
    })();
    let _ = persist_result;

    // Mirrors lines 48-51: keep in-memory config consistent
    set_install_id(config, minted.clone());
    minted
}

/// Deterministic variant for tests — inject the minted id directly.
///
/// Mirrors same logic as [`ensure_install_id_with`] but uses the supplied
/// `minted` string instead of `uuid4()`, allowing hermetic assertions
/// without mocking `uuid`.
pub fn ensure_install_id_with_minted<F, G, E>(
    config: &mut Value,
    minted: String,
    load: F,
    save: G,
) -> String
where
    F: Fn() -> Option<Value>,
    G: Fn(&Value) -> Result<(), E>,
    E: std::fmt::Debug,
{
    if let Some(existing) = get_existing_install_id(config) {
        return existing;
    }
    let persist_result: Result<(), ()> = (|| {
        let mut fresh = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&load)) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(()),
            Err(_) => {
                log::debug!("install_id persist failed; using ephemeral id");
                return Ok(());
            }
        };
        if !matches!(fresh, Value::Object(_)) {
            return Ok(());
        }
        let slot_empty = {
            let map = match &mut fresh {
                Value::Object(m) => m,
                _ => return Ok(()),
            };
            let slot = map
                .entry("monitoring".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if !matches!(slot, Value::Object(_)) {
                return Ok(());
            }
            slot_install_id_is_empty(slot)
        };
        if !slot_empty {
            return Ok(());
        }
        if let Value::Object(map) = &mut fresh {
            if let Some(Value::Object(slot)) = map.get_mut("monitoring") {
                slot.insert("install_id".to_string(), Value::String(minted.clone()));
            }
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| save(&fresh))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                log::debug!("install_id persist failed; using ephemeral id: {:?}", e);
                Ok(())
            }
            Err(_) => {
                log::debug!("install_id persist failed; using ephemeral id");
                Ok(())
            }
        }
    })();
    let _ = persist_result;
    set_install_id(config, minted.clone());
    minted
}

/// Convenience in-memory-only variant (no disk persistence attempt).
///
/// Mirrors the in-memory tail of `ensure_install_id` (lines 48-52) plus the
/// early-return (lines 30-33) and mint (line 35). Persistence is omitted;
/// use [`ensure_install_id_with`] when you have `load_config`/`save_config`
/// closures. This still satisfies the fail-open contract: if persistence
/// cannot run, the ephemeral id is returned and `config` is mutated.
pub fn ensure_install_id(config: &mut Value) -> String {
    if let Some(existing) = get_existing_install_id(config) {
        return existing;
    }
    let minted = mint_install_id();
    set_install_id(config, minted.clone());
    minted
}

#[allow(dead_code)]
fn _ensure_install_id(config: &mut Value) -> String {
    ensure_install_id(config)
}

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names
#[allow(dead_code)]
const _MODULE_DOC: &str = "Install identity for gateway monitoring.";
