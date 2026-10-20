//! Windows Terminal binds `ctrl+v`/`ctrl+shift+v`/`shift+insert` to its own
//! paste action, so image-only clipboards never reach the app at all — gray
//! sees no key, no bracketed paste, nothing. WT fragments can't touch
//! keybindings (by design), so the only way `ctrl+v` reaches gray inside WT
//! is an `unbound` entry in the user's `settings.json`.
//!
//! `ensure_ctrl_v_passthrough` runs once at startup, only when actually inside
//! Windows Terminal (`WT_SESSION`), and only rewrites settings files that
//! parse as plain JSON — files with comments are left alone. Both the modern
//! split model (`actions` + `keybindings` id refs) and the legacy unified
//! model (`keys` inline on actions) are handled. Changes are idempotent and a
//! one-time `settings.json.gray-backup` is kept next to the file.

use std::path::{Path, PathBuf};

const UNBOUND_ID: &str = "User.gray.unboundCtrlV";
const PASTE_KEY: &str = "ctrl+v";

/// Ids WT uses for the built-in clipboard paste — safe to repoint because
/// gray's own paste handler covers text and images once the key gets through.
fn is_default_paste_id(id: &serde_json::Value) -> bool {
    id.is_null() || id.as_str() == Some("Terminal.PasteFromClipboard")
}

/// `keys` may be a string or an array of strings; true when `key` is one.
fn keys_include(keys: &serde_json::Value, key: &str) -> bool {
    match keys {
        serde_json::Value::String(s) => s == key,
        serde_json::Value::Array(list) => list.iter().any(|k| k.as_str() == Some(key)),
        _ => false,
    }
}

/// Removes `key` from a `keys` value. Returns false when nothing remains —
/// the caller should then drop the whole entry.
fn strip_key(keys: &mut serde_json::Value, key: &str) -> bool {
    match keys {
        serde_json::Value::String(s) => s != key,
        serde_json::Value::Array(list) => {
            list.retain(|k| k.as_str() != Some(key));
            !list.is_empty()
        }
        _ => true,
    }
}

fn wt_settings_paths() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let base = PathBuf::from(local);
    [
        base.join(r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
        base.join(
            r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
        ),
        base.join(r"Microsoft\Windows Terminal\settings.json"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// Rewrites `obj` so `ctrl+v` reaches the app. Returns true when modified.
fn ensure_unbound(obj: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    if obj.contains_key("keybindings") {
        ensure_unbound_split(obj)
    } else {
        ensure_unbound_legacy(obj)
    }
}

/// Modern model: `actions` holds commands with `id`s, `keybindings` maps
/// ids → keys. Last entry wins for duplicate keys.
fn ensure_unbound_split(obj: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = false;

    let actions = obj
        .entry("actions")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(actions) = actions.as_array_mut() else {
        return false;
    };
    if !actions.iter().any(|a| {
        a.get("id").and_then(|v| v.as_str()) == Some(UNBOUND_ID)
            && a.get("command").and_then(|v| v.as_str()) == Some("unbound")
    }) {
        actions.push(serde_json::json!({
            "command": "unbound",
            "id": UNBOUND_ID,
            "name": "gray: send Ctrl+V to the app",
        }));
        changed = true;
    }

    let Some(serde_json::Value::Array(bindings)) = obj.get_mut("keybindings") else {
        return changed;
    };
    // `resolved` means ctrl+v ends up reaching the app: either our unbound
    // binding claims it, or nothing binds it at all.
    let mut resolved = false;
    for b in bindings.iter_mut() {
        let Some(entry) = b.as_object_mut() else {
            continue;
        };
        let id = entry.get("id");
        if id.and_then(|v| v.as_str()) == Some(UNBOUND_ID) {
            if keys_include(
                entry.get("keys").unwrap_or(&serde_json::Value::Null),
                PASTE_KEY,
            ) {
                resolved = true;
            }
            continue;
        }
        let is_paste = id.map(is_default_paste_id).unwrap_or(true);
        if !is_paste
            || !keys_include(
                entry.get("keys").unwrap_or(&serde_json::Value::Null),
                PASTE_KEY,
            )
        {
            continue;
        }
        // Repoint paste-bound ctrl+v at our unbound action; strip it from
        // multi-key arrays instead so the other keys keep working.
        if let Some(keys) = entry.get_mut("keys") {
            if keys.is_string() {
                entry.insert("id".to_string(), UNBOUND_ID.into());
            } else {
                strip_key(keys, PASTE_KEY);
            }
        }
        resolved = true;
        changed = true;
    }
    // Multi-key arrays emptied by the strip above are dead weight.
    bindings
        .retain(|b| !matches!(b.get("keys"), Some(serde_json::Value::Array(l)) if l.is_empty()));
    if !resolved {
        bindings.push(serde_json::json!({ "id": UNBOUND_ID, "keys": PASTE_KEY }));
        changed = true;
    }
    changed
}

/// Legacy model: `actions` entries carry `keys` inline.
fn ensure_unbound_legacy(obj: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = false;
    let Some(serde_json::Value::Array(actions)) = obj.get_mut("actions") else {
        return false;
    };
    for a in actions.iter_mut() {
        let Some(entry) = a.as_object_mut() else {
            continue;
        };
        let cmd_is_paste = match entry.get("command") {
            None | Some(serde_json::Value::Null) => true,
            Some(c) => {
                c.as_str() == Some("paste")
                    || c.get("action").and_then(|v| v.as_str()) == Some("paste")
            }
        };
        if cmd_is_paste
            && keys_include(
                entry.get("keys").unwrap_or(&serde_json::Value::Null),
                PASTE_KEY,
            )
        {
            if let Some(keys) = entry.get_mut("keys") {
                if keys.is_string() {
                    entry.insert(
                        "command".to_string(),
                        serde_json::Value::String("unbound".into()),
                    );
                } else {
                    strip_key(keys, PASTE_KEY);
                }
            }
            changed = true;
        }
    }
    // Entries whose multi-key arrays were stripped empty are dead weight.
    actions.retain(|a| !matches!(a.get("keys"), Some(serde_json::Value::Array(l)) if l.is_empty()));
    let already = actions.iter().any(|a| {
        a.get("command").and_then(|v| v.as_str()) == Some("unbound")
            && keys_include(a.get("keys").unwrap_or(&serde_json::Value::Null), PASTE_KEY)
    });
    if !already {
        actions.push(serde_json::json!({
            "command": "unbound",
            "keys": PASTE_KEY,
            "name": "gray: send Ctrl+V to the app",
        }));
        changed = true;
    }
    changed
}

fn update_settings_file(path: &Path) -> std::io::Result<()> {
    let body = std::fs::read_to_string(path)?;
    let mut value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        // JSONC (comments/trailing commas) — leave the file untouched.
        Err(_) => return Ok(()),
    };
    let Some(obj) = value.as_object_mut() else {
        return Ok(());
    };
    if !ensure_unbound(obj) {
        return Ok(());
    }
    let backup = path.with_extension("json.gray-backup");
    if !backup.exists() {
        let _ = std::fs::copy(path, &backup);
    }
    let pretty = serde_json::to_string_pretty(&value).unwrap_or(body);
    std::fs::write(path, format!("{pretty}\n"))
}

#[cfg(target_os = "windows")]
pub fn ensure_ctrl_v_passthrough() {
    // Only WT needs the surgery — and only WT sets WT_SESSION.
    if std::env::var_os("WT_SESSION").is_none() {
        return;
    }
    for path in wt_settings_paths() {
        if let Err(e) = update_settings_file(&path) {
            log::warn!(
                "gray: windows terminal paste setup skipped {}: {e}",
                path.display()
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_ctrl_v_passthrough() {}

#[path = "win_paste_tests.rs"]
#[cfg(test)]
mod tests;
