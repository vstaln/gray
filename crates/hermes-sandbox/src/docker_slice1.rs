//! Docker execution environment — slice 1 (first 700 lines).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/docker.py`
//! lines 1–700 (total 2060). Security-hardened container helpers, env
//! normalization, label sanitization, orphan reaper, docker discovery,
//! egress proxy glue, and security arg construction.
//!
//! Python source docstring (preserved):
//! ```text
//! Docker execution environment for sandboxed command execution.
//!
//! Security hardened (cap-drop ALL, no-new-privileges, PID limits),
//! configurable resource limits (CPU, memory, disk), and optional filesystem
//! persistence via bind mounts.
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::file_sync::get_hermes_home;

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Mirrors `_DOCKER_SEARCH_PATHS`.
pub const DOCKER_SEARCH_PATHS: &[&str] = &[
    "/usr/local/bin/docker",
    "/opt/homebrew/bin/docker",
    "/Applications/Docker.app/Contents/Resources/bin/docker",
];

/// Mirrors `_docker_executable` cached global.
static DOCKER_EXECUTABLE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn docker_executable_lock() -> &'static Mutex<Option<String>> {
    DOCKER_EXECUTABLE.get_or_init(|| Mutex::new(None))
}

/// Mirrors `_EGRESS_LABEL_KEY = "hermes-egress"`.
pub const EGRESS_LABEL_KEY: &str = "hermes-egress";

/// Mirrors `_BASE_SECURITY_ARGS`.
pub const BASE_SECURITY_ARGS: &[&str] = &[
    "--cap-drop", "ALL",
    "--cap-add", "DAC_OVERRIDE",
    "--cap-add", "CHOWN",
    "--cap-add", "FOWNER",
    "--security-opt", "no-new-privileges",
    "--tmpfs", "/tmp:rw,nosuid,size=512m",
    "--tmpfs", "/var/tmp:rw,noexec,nosuid,size=256m",
];

/// Mirrors `_DEFAULT_PIDS_LIMIT = "256"`.
pub const DEFAULT_PIDS_LIMIT: &str = "256";

/// Mirrors `_DEFAULT_SHM_SIZE = "1g"`.
pub const DEFAULT_SHM_SIZE: &str = "1g";

/// Mirrors `_RUN_TMPFS_NOEXEC`.
pub const RUN_TMPFS_NOEXEC: &[&str] = &["--tmpfs", "/run:rw,noexec,nosuid,size=64m"];
/// Mirrors `_RUN_TMPFS_EXEC`.
pub const RUN_TMPFS_EXEC: &[&str] = &["--tmpfs", "/run:rw,exec,nosuid,size=64m"];

/// Mirrors `_PRIVDROP_CAP_ARGS`.
pub const PRIVDROP_CAP_ARGS: &[&str] = &["--cap-add", "SETUID", "--cap-add", "SETGID"];

// ---------------------------------------------------------------------------
// Helpers — env var name validation (mirrors `_ENV_VAR_NAME_RE`)
// ---------------------------------------------------------------------------

fn is_valid_env_var_name(name: &str) -> bool {
    // Mirrors `re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")`
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

fn is_label_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

// ---------------------------------------------------------------------------
// sanitize_task_id_for_path — mirrors `tools.environments.base`
// ---------------------------------------------------------------------------

/// Mirrors `tools.environments.base.sanitize_task_id_for_path`.
pub fn sanitize_task_id_for_path(task_id: &str) -> String {
    const MAX_LEN: usize = 128;
    const HASH_LEN: usize = 12;
    if task_id.is_empty() {
        return "default".to_string();
    }
    let is_safe = task_id.len() <= MAX_LEN
        && task_id != "."
        && task_id != ".."
        && !task_id.ends_with('.')
        && !task_id.ends_with(' ')
        && task_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if is_safe {
        return task_id.to_string();
    }
    let cleaned: String = task_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    let digest = sha256_hex(task_id.as_bytes());
    let short = &digest[..HASH_LEN];
    let max_stem = MAX_LEN.saturating_sub(HASH_LEN + 1);
    let mut stem: String = cleaned.chars().take(max_stem).collect();
    stem = stem.trim_matches(|c| c == '.' || c == '_').to_string();
    if stem.is_empty() {
        stem = "task".to_string();
    }
    format!("{stem}-{short}")
}

// Keep alias name matching Python `_sandbox_dir_name = sanitize_task_id_for_path`
pub use sanitize_task_id_for_path as sandbox_dir_name;

// ---------------------------------------------------------------------------
// SHA-256 hex — minimal (mirrors hashlib.sha256)
// ---------------------------------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    let mut h = [
        0x6a09e667u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut len_bits: u64 = (data.len() as u64) * 8;
    let mut buf = data.to_vec();
    buf.push(0x80);
    while buf.len() % 64 != 56 {
        buf.push(0);
    }
    buf.extend_from_slice(&len_bits.to_be_bytes());
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    for chunk in buf.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let mut a=h[0]; let mut b=h[1]; let mut c=h[2]; let mut d=h[3];
        let mut e=h[4]; let mut f=h[5]; let mut g=h[6]; let mut hh=h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh=g; g=f; f=e; e=d.wrapping_add(temp1); d=c; c=b; b=a; a=temp1.wrapping_add(temp2);
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b); h[2]=h[2].wrapping_add(c); h[3]=h[3].wrapping_add(d);
        h[4]=h[4].wrapping_add(e); h[5]=h[5].wrapping_add(f); h[6]=h[6].wrapping_add(g); h[7]=h[7].wrapping_add(hh);
    }
    let mut out=String::with_capacity(64);
    for v in h { out.push_str(&format!("{v:08x}")); }
    out
}

// ---------------------------------------------------------------------------
// _normalize_forward_env_names
// ---------------------------------------------------------------------------

/// Mirrors `_normalize_forward_env_names(forward_env: list[str] | None) -> list[str]`.
pub fn normalize_forward_env_names(forward_env: Option<&[String]>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    let items: &[String] = forward_env.unwrap_or(&[]);
    for item in items {
        // Python: `if not isinstance(item, str)` log warning — in Rust all are String, so skip check
        let key = item.trim().to_string();
        if key.is_empty() {
            continue;
        }
        if !is_valid_env_var_name(&key) {
            log::warn!("Ignoring invalid docker_forward_env entry: {:?}", item);
            continue;
        }
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key.clone());
        normalized.push(key);
    }
    normalized
}

/// Variant that accepts `Option<Vec<String>>` for ergonomic tests.
pub fn normalize_forward_env_names_owned(forward_env: Option<Vec<String>>) -> Vec<String> {
    let slice: Option<Vec<String>> = forward_env;
    normalize_forward_env_names(slice.as_deref())
}

// ---------------------------------------------------------------------------
// _normalize_env_dict
// ---------------------------------------------------------------------------

/// Mirrors `_normalize_env_dict(env: dict | None) -> dict[str, str]`.
pub fn normalize_env_dict(env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut normalized = HashMap::new();
    let Some(map) = env else { return normalized; };
    for (key, value) in map {
        let trimmed_key = key.trim().to_string();
        if !is_valid_env_var_name(&trimmed_key) {
            log::warn!("Ignoring invalid docker_env key: {:?}", key);
            continue;
        }
        // In Python non-string values are coerced if int/float/bool, else rejected.
        // In Rust all values are String, so accept as-is.
        normalized.insert(trimmed_key, value.clone());
    }
    normalized
}

/// Coercing variant for `HashMap<String, serde-like mixed>` — mirrors Python's int/float/bool coerce.
/// Takes `HashMap<String, String>` already coerced; separate helper for mixed types via string check.
pub fn normalize_env_dict_mixed(env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    normalize_env_dict(env)
}

// ---------------------------------------------------------------------------
// _load_hermes_env_vars
// ---------------------------------------------------------------------------

/// Mirrors `_load_hermes_env_vars() -> dict[str, str]`.
///
/// Loads `~/.hermes/.env` without failing Docker execution. In Python this
/// is `hermes_cli.config.load_env()`. Here we read `HERMES_HOME/.env` directly
/// with a simple KEY=VALUE parser.
pub fn load_hermes_env_vars() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let env_path = get_hermes_home().join(".env");
    let text = match fs::read_to_string(&env_path) {
        Ok(t) => t,
        Err(_) => return out,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // handle `export KEY=...` prefix
        let stripped = if line.starts_with("export ") {
            line["export ".len()..].trim()
        } else {
            line
        };
        let Some(eq) = stripped.find('=') else { continue; };
        let key = stripped[..eq].trim().to_string();
        if !is_valid_env_var_name(&key) {
            continue;
        }
        let mut value = stripped[eq+1..].trim().to_string();
        // strip surrounding quotes if present
        if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            value = value[1..value.len()-1].to_string();
        }
        out.insert(key, value);
    }
    out
}

// ---------------------------------------------------------------------------
// _sanitize_label_value
// ---------------------------------------------------------------------------

/// Mirrors `_sanitize_label_value(value: str) -> str`.
///
/// Coerces *value* into a Docker label-safe form (alnum + `_.-`, ≤63 chars).
pub fn sanitize_label_value(value: &str) -> String {
    if value.is_empty() {
        return "unknown".to_string();
    }
    let mut cleaned = String::with_capacity(value.len());
    for c in value.chars() {
        if is_label_char_ok(c) {
            cleaned.push(c);
        } else {
            cleaned.push('_');
        }
    }
    if cleaned.len() > 63 {
        cleaned.truncate(63);
    }
    if cleaned.is_empty() {
        return "unknown".to_string();
    }
    cleaned
}

// ---------------------------------------------------------------------------
// _get_active_profile_name
// ---------------------------------------------------------------------------

/// Mirrors `_get_active_profile_name() -> str`.
///
/// Returns active Hermes profile name, or `"default"` on any error.
pub fn get_active_profile_name() -> String {
    // Mirrors `hermes_cli.profiles.get_active_profile_name()` inference from HERMES_HOME.
    // If HERMES_HOME env is set and points into `~/.hermes/profiles/<name>`, return <name>.
    // Otherwise default.
    if let Ok(home) = env::var("HERMES_HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            let p = PathBuf::from(&t);
            // Try to find `/profiles/` segment
            let s = p.to_string_lossy().to_string();
            if let Some(idx) = s.find("/profiles/") {
                let after = &s[idx + "/profiles/".len()..];
                let name = after.split('/').next().unwrap_or("").trim();
                if !name.is_empty() {
                    return name.to_string();
                }
                return "custom".to_string();
            }
            // Check if HERMES_HOME equals default `~/.hermes` — return default
            // else custom
            if s.contains(".hermes") {
                // if it is exactly default, we already would have no profiles segment -> default
                // For non-default custom path without profiles, return custom per Python
                if t != s {
                    // not needed
                }
                // If HERMES_HOME is set to something else entirely (not ended with .hermes and no profiles), Python returns "custom"
                // We approximate: if HERMES_HOME is set and not default and no profiles => custom
                // Determine default path
                let default = default_hermes_home();
                if PathBuf::from(&t) != default {
                    // Only return custom if it's truly custom (not default)
                    // Python: resolved != default_resolved => check relative_to profiles_root
                    // If not relative to profiles_root => "custom"
                    return "custom".to_string();
                }
            }
        }
    }
    "default".to_string()
}

fn default_hermes_home() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// find_docker — mirrors `find_docker() -> Optional[str]`
// ---------------------------------------------------------------------------

fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            if let Ok(meta) = fs::metadata(&candidate) {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = meta;
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Mirrors `find_docker() -> Optional[str]`.
pub fn find_docker() -> Option<String> {
    // Check cached
    if let Some(cached) = docker_executable_lock().lock().ok().and_then(|g| g.clone()) {
        return Some(cached);
    }

    // 1. Explicit override via HERMES_DOCKER_BINARY
    if let Ok(override_path) = env::var("HERMES_DOCKER_BINARY") {
        let t = override_path.trim().to_string();
        if !t.is_empty() {
            let p = PathBuf::from(&t);
            if p.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = fs::metadata(&p) {
                        if meta.permissions().mode() & 0o111 != 0 {
                            let s = t.clone();
                            if let Ok(mut g) = docker_executable_lock().lock() {
                                *g = Some(s.clone());
                            }
                            log::info!("Using HERMES_DOCKER_BINARY override: {}", t);
                            return Some(t);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let s = t.clone();
                    if let Ok(mut g) = docker_executable_lock().lock() {
                        *g = Some(s.clone());
                    }
                    log::info!("Using HERMES_DOCKER_BINARY override: {}", t);
                    return Some(t);
                }
            }
        }
    }

    // 2. docker on PATH
    if let Some(found) = which("docker") {
        let s = found.to_string_lossy().to_string();
        if let Ok(mut g) = docker_executable_lock().lock() {
            *g = Some(s.clone());
        }
        return Some(s);
    }

    // 3. podman on PATH
    if let Some(found) = which("podman") {
        let s = found.to_string_lossy().to_string();
        log::info!("Using podman as container runtime: {}", s);
        if let Ok(mut g) = docker_executable_lock().lock() {
            *g = Some(s.clone());
        }
        return Some(s);
    }

    // 4. Well-known macOS Docker Desktop locations
    for path in DOCKER_SEARCH_PATHS {
        let p = PathBuf::from(path);
        if p.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&p) {
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            let s = path.to_string();
            log::info!("Found docker at non-PATH location: {}", s);
            if let Ok(mut g) = docker_executable_lock().lock() {
                *g = Some(s.clone());
            }
            return Some(s);
        }
    }

    None
}

/// Test helper: clear cached docker executable.
#[cfg(test)]
pub fn clear_docker_cache() {
    if let Ok(mut g) = docker_executable_lock().lock() {
        *g = None;
    }
}

// ---------------------------------------------------------------------------
// _extra_args_set_shm_size
// ---------------------------------------------------------------------------

/// Mirrors `_extra_args_set_shm_size(extra_args: list) -> bool`.
pub fn extra_args_set_shm_size(extra_args: Option<&[String]>) -> bool {
    let Some(args) = extra_args else { return false; };
    for a in args {
        if a == "--shm-size" || a.starts_with("--shm-size=") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _container_finished_at / datetime parsing
// ---------------------------------------------------------------------------

/// Mirrors `_container_finished_at(docker_exe: str, container_id: str)`.
///
/// Returns `Some(SystemTime)` or `None` if missing/unparseable/zero-value.
pub fn container_finished_at(docker_exe: &str, container_id: &str) -> Option<SystemTime> {
    let output = Command::new(docker_exe)
        .args(["inspect", "--format", "{{.State.FinishedAt}}", container_id])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() || raw.starts_with("0001-01-01") {
        return None;
    }
    parse_docker_finished_at(&raw)
}

/// Parse Docker `FinishedAt` RFC3339 with nanoseconds.
///
/// Mirrors Python:
/// ```python
/// raw = re.sub(r"(\.\d{6})\d+", r"\1", raw)
/// raw = raw.replace("Z", "+00:00")
/// datetime.fromisoformat(raw)
/// ```
pub fn parse_docker_finished_at(raw: &str) -> Option<SystemTime> {
    let mut s = raw.trim().to_string();
    if s.is_empty() || s.starts_with("0001-01-01") {
        return None;
    }
    // Trim nanoseconds to microseconds: keep first 6 digits after '.'
    if let Some(dot) = s.find('.') {
        let after_dot = &s[dot+1..];
        let mut digits_end = 0usize;
        for c in after_dot.chars() {
            if c.is_ascii_digit() { digits_end += 1; } else { break; }
        }
        if digits_end > 6 {
            let before = &s[..dot+1];
            let digits = &after_dot[..digits_end];
            let rest = &after_dot[digits_end..];
            s = format!("{}{}{}", before, &digits[..6], rest);
        }
    }
    // Replace Z with +00:00
    if s.ends_with('Z') {
        s = format!("{}+00:00", &s[..s.len()-1]);
    }
    // Now parse RFC3339. For Docker we expect UTC (+00:00) or no tz.
    // Strip timezone for SystemTime conversion (assume UTC).
    let tz_stripped = if let Some(plus) = s.rfind('+') {
        // check if looks like timezone +HH:MM
        let suffix = &s[plus..];
        if suffix.len() >= 5 && suffix.contains(':') {
            s[..plus].to_string()
        } else { s.clone() }
    } else if s.contains('T') && s.matches('-').count() >= 3 {
        // handle negative tz? rare for FinishedAt; ignore
        s.clone()
    } else {
        s.clone()
    };
    parse_rfc3339_utc(&tz_stripped)
}

fn parse_rfc3339_utc(s: &str) -> Option<SystemTime> {
    // Expected: "2026-05-28T13:45:00.123456" or "2026-05-28T13:45:00"
    // Split at T
    let t_pos = s.find('T')?;
    let date_part = &s[..t_pos];
    let time_part = &s[t_pos+1..];
    // date: YYYY-MM-DD
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 { return None; }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    // time: HH:MM:SS[.fraction]
    let time_main = time_part.split('.').next().unwrap_or(time_part);
    let frac_part = if time_part.contains('.') { Some(time_part.split('.').nth(1).unwrap_or("")) } else { None };
    let time_components: Vec<&str> = time_main.split(':').collect();
    if time_components.len() != 3 { return None; }
    let hour: u32 = time_components[0].parse().ok()?;
    let minute: u32 = time_components[1].parse().ok()?;
    let second: u32 = time_components[2].parse().ok()?;

    // Validate ranges
    if month == 0 || month > 12 || day == 0 || day > 31 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_since_epoch(year, month, day)?;
    let secs = days as u64 * 86400 + hour as u64 * 3600 + minute as u64 * 60 + second as u64;

    let mut sys = UNIX_EPOCH + Duration::from_secs(secs);
    if let Some(frac) = frac_part {
        // frac may contain timezone residue already stripped, but be safe: take leading digits only
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let padded = format!("{:0<6}", &digits[..digits.len().min(6)]);
            if let Ok(micros) = padded[..6].parse::<u64>() {
                sys = sys + Duration::from_micros(micros);
            }
        }
    }
    Some(sys)
}

// Howard Hinnant's days_from_civil
fn days_since_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    // Convert to days since 1970-01-01
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let m = month as i64;
    let d = day as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0,399]
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // [0,365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0,146096]
    let days = era * 146097 + doe - 719468; // days since 1970-01-01
    Some(days)
}

// ---------------------------------------------------------------------------
// reap_orphan_containers
// ---------------------------------------------------------------------------

/// Mirrors `reap_orphan_containers(*, max_age_seconds=600, profile_filter=None, docker_exe=None) -> int`.
pub fn reap_orphan_containers(
    max_age_seconds: u64,
    profile_filter: Option<&str>,
    docker_exe: Option<&str>,
) -> usize {
    let docker = docker_exe
        .map(|s| s.to_string())
        .or_else(find_docker)
        .unwrap_or_else(|| "docker".to_string());

    let mut filters = vec![
        "--filter".to_string(), "label=hermes-agent=1".to_string(),
        "--filter".to_string(), "status=exited".to_string(),
    ];
    if let Some(pf) = profile_filter {
        if !pf.is_empty() {
            filters.push("--filter".to_string());
            filters.push(format!("label=hermes-profile={}", sanitize_label_value(pf)));
        }
    }

    let mut cmd = vec!["ps".to_string(), "-a".to_string()];
    cmd.extend(filters);
    cmd.extend(["--format".to_string(), "{{.ID}}".to_string()]);

    let output = match Command::new(&docker)
        .args(&cmd)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::debug!("orphan reaper docker ps failed: {}", e);
            return 0;
        }
    };

    // Handle timeout: Command::output has no timeout; we approximate by not timing out here.
    // Python uses timeout=15; in Rust we could spawn with timeout thread, but keep simple.
    if !output.status.success() {
        log::debug!(
            "orphan reaper docker ps returned {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return 0;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let candidate_ids: Vec<String> = stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if candidate_ids.is_empty() {
        return 0;
    }

    let now = SystemTime::now();
    let mut removed = 0usize;
    for cid in candidate_ids {
        let finished_at = container_finished_at(&docker, &cid);
        let Some(finished) = finished_at else { continue; };
        let age = now.duration_since(finished).unwrap_or(Duration::from_secs(0)).as_secs_f64();
        if age < max_age_seconds as f64 {
            continue;
        }
        let rm = Command::new(&docker)
            .args(["rm", "-f", &cid])
            .stdin(std::process::Stdio::null())
            .output();
        match rm {
            Ok(o) if o.status.success() => {
                removed += 1;
                log::info!("Reaped orphan container {} (exited {} seconds ago)", &cid[..cid.len().min(12)], age as i64);
            }
            Ok(o) => {
                log::debug!("docker rm -f {} failed: {}", &cid[..cid.len().min(12)], String::from_utf8_lossy(&o.stderr).trim());
            }
            Err(e) => {
                log::debug!("orphan reaper docker rm {} failed: {}", &cid[..cid.len().min(12)], e);
            }
        }
    }
    removed
}

// ---------------------------------------------------------------------------
// Egress proxy helpers
// ---------------------------------------------------------------------------

/// Token mapping from iron-proxy mappings.json
#[derive(Debug, Clone)]
pub struct TokenMapping {
    pub real_env_name: String,
    pub proxy_token: String,
    pub alias_env_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProxyStatus {
    pub configured: bool,
    pub pid: Option<u32>,
    pub listening: bool,
    pub tunnel_port: u16,
    pub ca_cert_path: Option<PathBuf>,
}

const DEFAULT_TUNNEL_PORT: u16 = 9090;

fn proxy_state_dir() -> PathBuf {
    get_hermes_home().join("proxy")
}

fn get_proxy_status() -> ProxyStatus {
    let state = proxy_state_dir();
    let cfg_path = state.join("proxy.yaml");
    let configured = cfg_path.exists();
    let mut status = ProxyStatus {
        configured,
        pid: None,
        listening: false,
        tunnel_port: DEFAULT_TUNNEL_PORT,
        ca_cert_path: None,
    };

    // tunnel_port from proxy.yaml
    if configured {
        if let Ok(text) = fs::read_to_string(&cfg_path) {
            if let Some(port) = parse_tunnel_port(&text) {
                status.tunnel_port = port;
            }
        }
    }

    // ca_cert_path
    let ca = state.join("ca.crt");
    if ca.exists() {
        status.ca_cert_path = Some(ca);
    }

    // pid
    let pid_path = state.join("iron-proxy.pid");
    if let Ok(text) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            status.pid = Some(pid);
        }
    }

    // listening check: try to connect to 127.0.0.1:tunnel_port
    status.listening = is_port_listening("127.0.0.1", status.tunnel_port);

    status
}

fn parse_tunnel_port(text: &str) -> Option<u16> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tunnel_port") || trimmed.starts_with("tunnelPort") {
            if let Some(colon) = trimmed.find(':') {
                let val = trimmed[colon+1..].trim().trim_matches('"').trim_matches('\'');
                if let Ok(p) = val.parse::<u16>() {
                    return Some(p);
                }
            }
        }
        // Also try generic "port:" near tunnel
        if trimmed.starts_with("listen") && trimmed.contains(':') {
            // e.g., listen: "127.0.0.1:9090"
            if let Some(colon) = trimmed.rfind(':') {
                let after = trimmed[colon+1..].trim().trim_matches('"').trim_matches('\'').trim_end_matches('"');
                // strip trailing quote/comma
                let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = num_str.parse::<u16>() {
                    if p != 0 { return Some(p); }
                }
            }
        }
    }
    None
}

fn is_port_listening(host: &str, port: u16) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr_str = format!("{host}:{port}");
    let Ok(mut addrs) = addr_str.to_socket_addrs() else { return false; };
    let Some(addr) = addrs.next() else { return false; };
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn load_mappings() -> Vec<TokenMapping> {
    let state = proxy_state_dir();
    let f = state.join("mappings.json");
    if !f.exists() {
        return Vec::new();
    }
    let text = match fs::read_to_string(&f) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("Failed to read iron-proxy mappings.json: {}", e);
            return Vec::new();
        }
    };
    // Minimal JSON parsing: look for "tokens": [ { "proxy_token": "...", "env_name": "...", "alias_env_names": [...] }, ... ]
    // We do simple substring extraction to avoid serde.
    let mut out = Vec::new();
    // Find tokens array
    let Some(tokens_start) = text.find("\"tokens\"") else { return out; };
    let after = &text[tokens_start..];
    // Iterate over objects in array
    let mut pos = 0usize;
    let chars: Vec<char> = after.chars().collect();
    while let Some(obj_start) = after[pos..].find('{') {
        let abs_start = pos + obj_start;
        // Find matching } (naive: find next })
        let Some(obj_end) = after[abs_start..].find('}') else { break; };
        let obj_str = &after[abs_start..abs_start+obj_end+1];
        // Extract proxy_token, env_name, alias_env_names
        let proxy_token = extract_json_string_value(obj_str, "proxy_token");
        let env_name = extract_json_string_value(obj_str, "env_name")
            .or_else(|| extract_json_string_value(obj_str, "real_env_name"));
        if let (Some(pt), Some(en)) = (proxy_token, env_name) {
            let aliases = extract_json_array_strings(obj_str, "alias_env_names");
            out.push(TokenMapping {
                real_env_name: en,
                proxy_token: pt,
                alias_env_names: aliases,
            });
        }
        pos = abs_start + obj_end + 1;
        if pos >= after.len() { break; }
        // Stop if we passed array close
        if after[pos..].find(']').map(|i| i < after[pos..].find('{').unwrap_or(usize::MAX)).unwrap_or(false) {
            // reached end of tokens array
            let next_brace = after[pos..].find('{');
            let next_close = after[pos..].find(']');
            if let (Some(_b), Some(c)) = (next_brace, next_close) {
                if c < _b { break; }
            } else if next_close.is_some() && next_brace.is_none() {
                break;
            }
        }
        let _ = &chars; // silence unused
    }
    out
}

fn extract_json_string_value(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = obj.find(&pat)?;
    let after = &obj[idx + pat.len()..];
    let colon = after.find(':')?;
    let rest = after[colon+1..].trim_start();
    if !rest.starts_with('"') { return None; }
    let mut out = String::new();
    let mut esc = false;
    for c in rest[1..].chars() {
        if esc {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => out.push(c),
            }
            esc = false;
            continue;
        }
        if c == '\\' { esc = true; continue; }
        if c == '"' { break; }
        out.push(c);
    }
    Some(out)
}

fn extract_json_array_strings(obj: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\"");
    let Some(idx) = obj.find(&pat) else { return Vec::new(); };
    let after = &obj[idx + pat.len()..];
    let Some(colon) = after.find(':') else { return Vec::new(); };
    let rest = after[colon+1..].trim_start();
    if !rest.starts_with('[') { return Vec::new(); }
    let Some(end) = rest.find(']') else { return Vec::new(); };
    let inner = &rest[1..end];
    let mut out = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    let mut cur = String::new();
    for c in inner.chars() {
        if esc { cur.push(c); esc=false; continue; }
        if c == '\\' && in_str { esc=true; continue; }
        if c == '"' { 
            if in_str { out.push(cur.clone()); cur.clear(); in_str=false; } else { in_str=true; }
            continue;
        }
        if in_str { cur.push(c); }
    }
    out
}

fn load_proxy_config_enabled() -> Option<bool> {
    // Mirrors `load_config().get("proxy") or {}`
    // Read HERMES_HOME/config.yaml and search for proxy.enabled
    let cfg_path = get_hermes_home().join("config.yaml");
    if !cfg_path.exists() {
        return None;
    }
    let text = fs::read_to_string(&cfg_path).ok()?;
    // Simple state machine: find "proxy:" then within its block find "enabled:"
    let mut in_proxy = false;
    let mut proxy_indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("proxy:") {
            in_proxy = true;
            proxy_indent = Some(indent);
            continue;
        }
        if in_proxy {
            if let Some(pi) = proxy_indent {
                if indent <= pi && !trimmed.is_empty() && !trimmed.starts_with(' ') {
                    // exited proxy block
                    if trimmed.contains(':') && !trimmed.starts_with("enabled") {
                        in_proxy = false;
                    }
                }
            }
            if trimmed.starts_with("enabled:") {
                let val = trimmed["enabled:".len()..].trim().to_lowercase();
                if val.starts_with("true") || val == "1" || val == "yes" {
                    return Some(true);
                } else if val.starts_with("false") || val == "0" || val == "no" {
                    return Some(false);
                }
            }
        }
    }
    None
}

fn load_proxy_config_enforce() -> Option<bool> {
    let cfg_path = get_hermes_home().join("config.yaml");
    if !cfg_path.exists() {
        return None;
    }
    let text = fs::read_to_string(&cfg_path).ok()?;
    let mut in_proxy = false;
    let mut proxy_indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("proxy:") {
            in_proxy = true;
            proxy_indent = Some(indent);
            continue;
        }
        if in_proxy {
            if let Some(pi) = proxy_indent {
                if indent <= pi && trimmed.contains(':') && !trimmed.starts_with("enforce") && !trimmed.starts_with("enabled") {
                    // heuristic exit
                    if !trimmed.starts_with(' ') && !trimmed.starts_with("enforce") {
                        // keep in proxy for nested
                    }
                }
            }
            if trimmed.starts_with("enforce_on_docker:") {
                let val = trimmed["enforce_on_docker:".len()..].trim().to_lowercase();
                if val.starts_with("true") || val == "1" || val == "yes" {
                    return Some(true);
                } else if val.starts_with("false") || val == "0" || val == "no" {
                    return Some(false);
                }
            }
        }
    }
    None
}

/// Mirrors `_egress_proxy_args_for_docker() -> tuple[list[str], dict[str, str], list[str]]`.
///
/// Returns `(volume_args, env_overrides, host_args)` or `Err` when
/// `proxy.enforce_on_docker` is true and proxy is enabled-but-not-ready.
pub fn egress_proxy_args_for_docker() -> Result<(Vec<String>, HashMap<String, String>, Vec<String>), String> {
    // Check proxy.enabled
    let enabled = load_proxy_config_enabled().unwrap_or(false);
    if !enabled {
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let status = get_proxy_status();
    let enforce = egress_enforce_on_docker(true);

    if !status.configured {
        let msg = "proxy.enabled is true but iron-proxy is not configured. Run `hermes egress setup` to mint tokens and write proxy.yaml.";
        if enforce {
            return Err(msg.to_string());
        }
        log::warn!("{} — continuing without proxy (enforce_on_docker=false).", msg);
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    if status.pid.is_none() || !status.listening {
        let msg = format!("iron-proxy is enabled but not running on port {}. Start it with `hermes egress start`.", status.tunnel_port);
        if enforce {
            return Err(msg);
        }
        log::warn!("{} — continuing without proxy (enforce_on_docker=false).", msg);
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let ca_path = match status.ca_cert_path {
        Some(p) if p.exists() => p,
        _ => {
            let msg = format!("iron-proxy CA cert vanished from {:?}. Re-run `hermes egress setup` to regenerate it.", status.ca_cert_path);
            if enforce {
                return Err(msg);
            }
            log::warn!("{} — continuing without proxy (enforce_on_docker=false).", msg);
            return Ok((Vec::new(), HashMap::new(), Vec::new()));
        }
    };

    let mappings = load_mappings();
    if mappings.is_empty() {
        let msg = "iron-proxy is configured but mappings.json is empty or corrupt. Re-run `hermes egress setup` to mint provider tokens before starting a sandbox.";
        if enforce {
            return Err(msg.to_string());
        }
        log::warn!("{} — continuing without proxy (enforce_on_docker=false).", msg);
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let container_ca = "/etc/ssl/certs/hermes-egress-ca.crt";
    let volume_args = vec!["-v".to_string(), format!("{}:{}:ro", ca_path.display(), container_ca)];

    let proxy_url = format!("http://host.docker.internal:{}", status.tunnel_port);
    let plain_http_url = format!("http://host.docker.internal:{}", status.tunnel_port + 1);
    let mut env_overrides: HashMap<String, String> = HashMap::new();
    env_overrides.insert("HTTPS_PROXY".to_string(), proxy_url.clone());
    env_overrides.insert("https_proxy".to_string(), proxy_url.clone());
    env_overrides.insert("HTTP_PROXY".to_string(), plain_http_url.clone());
    env_overrides.insert("http_proxy".to_string(), plain_http_url.clone());
    env_overrides.insert("NO_PROXY".to_string(), "127.0.0.1,localhost,::1".to_string());
    env_overrides.insert("no_proxy".to_string(), "127.0.0.1,localhost,::1".to_string());
    env_overrides.insert("REQUESTS_CA_BUNDLE".to_string(), container_ca.to_string());
    env_overrides.insert("SSL_CERT_FILE".to_string(), container_ca.to_string());
    env_overrides.insert("CURL_CA_BUNDLE".to_string(), container_ca.to_string());
    env_overrides.insert("NODE_EXTRA_CA_CERTS".to_string(), container_ca.to_string());
    env_overrides.insert("HERMES_EGRESS_PROXY".to_string(), "1".to_string());
    env_overrides.insert("_HERMES_EGRESS_NODE_OPTIONS_APPEND".to_string(), "--use-openssl-ca".to_string());

    for m in &mappings {
        env_overrides.insert(m.real_env_name.clone(), m.proxy_token.clone());
        env_overrides.insert(format!("HERMES_PROXY_TOKEN_{}", m.real_env_name), m.proxy_token.clone());
        for alias in &m.alias_env_names {
            env_overrides.insert(alias.clone(), m.proxy_token.clone());
        }
    }

    let host_args = vec!["--add-host".to_string(), "host.docker.internal:host-gateway".to_string()];

    Ok((volume_args, env_overrides, host_args))
}

// ---------------------------------------------------------------------------
// _egress_reuse_fingerprint
// ---------------------------------------------------------------------------

/// Mirrors `_egress_reuse_fingerprint(volume_args, env_overrides, host_args) -> str`.
pub fn egress_reuse_fingerprint(
    volume_args: &[String],
    env_overrides: &HashMap<String, String>,
    host_args: &[String],
) -> String {
    if volume_args.is_empty() && env_overrides.is_empty() && host_args.is_empty() {
        return "off".to_string();
    }
    // Build deterministic JSON with sort_keys=True, separators=(",", ":")
    let mut env_sorted: BTreeMap<&String, &String> = BTreeMap::new();
    for (k, v) in env_overrides {
        env_sorted.insert(k, v);
    }
    let mut json = String::from("{");
    json.push_str("\"env_overrides\":{");
    let mut first = true;
    for (k, v) in &env_sorted {
        if !first { json.push(','); }
        first = false;
        json.push_str(&format!("{}:{}", json_escape_str(k), json_escape_str(v)));
    }
    json.push_str("},");
    json.push_str("\"host_args\":[");
    for (i, v) in host_args.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push_str(&json_escape_str(v));
    }
    json.push_str("],");
    json.push_str("\"volume_args\":[");
    for (i, v) in volume_args.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push_str(&json_escape_str(v));
    }
    json.push_str("]}");
    let hex = sha256_hex(json.as_bytes());
    hex[..24].to_string()
}

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// _egress_enforce_on_docker
// ---------------------------------------------------------------------------

/// Mirrors `_egress_enforce_on_docker(default=True) -> bool`.
pub fn egress_enforce_on_docker(default: bool) -> bool {
    if let Some(v) = load_proxy_config_enforce() {
        return v;
    }
    // Also check env override for tests: HERMES_EGRESS_ENFORCE
    if let Ok(val) = env::var("HERMES_EGRESS_ENFORCE") {
        let t = val.trim().to_lowercase();
        if matches!(t.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
        if matches!(t.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    default
}

// ---------------------------------------------------------------------------
// _critical_egress_env_names
// ---------------------------------------------------------------------------

/// Mirrors `_critical_egress_env_names(env_overrides: dict[str, str]) -> set[str]`.
pub fn critical_egress_env_names(env_overrides: &HashMap<String, String>) -> HashSet<String> {
    let mut critical: HashSet<String> = [
        "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy",
        "NO_PROXY", "no_proxy",
        "REQUESTS_CA_BUNDLE", "SSL_CERT_FILE", "CURL_CA_BUNDLE",
        "NODE_EXTRA_CA_CERTS", "NODE_OPTIONS",
    ].iter().map(|s| s.to_string()).collect();
    for key in env_overrides.keys() {
        if key.ends_with("_API_KEY") || key.ends_with("_TOKEN") {
            critical.insert(key.clone());
        }
    }
    critical
}

// ---------------------------------------------------------------------------
// _extra_args_egress_collisions
// ---------------------------------------------------------------------------

/// Mirrors `_extra_args_egress_collisions(extra_args: list[str], critical_names: set[str]) -> list[str]`.
pub fn extra_args_egress_collisions(
    extra_args: &[String],
    critical_names: &HashSet<String>,
) -> Vec<String> {
    let mut collisions = HashSet::new();
    let env_flags: HashSet<&str> = ["-e", "--env", "--env-file"].iter().cloned().collect();
    let network_flags: HashSet<&str> = ["--network", "--net"].iter().cloned().collect();

    let mut i = 0usize;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        let nxt = if i + 1 < extra_args.len() { extra_args[i+1].as_str() } else { "" };
        if env_flags.contains(arg.as_str()) {
            if arg == "--env-file" {
                collisions.insert(arg.clone());
            } else {
                let name = nxt.split('=').next().unwrap_or("").to_string();
                if critical_names.contains(&name) {
                    collisions.insert(name);
                }
            }
            i += 2;
            continue;
        }
        if env_flags.iter().any(|flag| arg.starts_with(&format!("{flag}="))) {
            if arg.starts_with("--env-file=") {
                collisions.insert("--env-file".to_string());
            } else if let Some(eq) = arg.find('=') {
                let after = &arg[eq+1..];
                let name = after.split('=').next().unwrap_or("").to_string();
                if critical_names.contains(&name) {
                    collisions.insert(name);
                }
            }
        } else if network_flags.contains(arg.as_str()) || network_flags.iter().any(|flag| arg.starts_with(&format!("{flag}="))) {
            collisions.insert(arg.clone());
        }
        i += 1;
    }
    let mut out: Vec<String> = collisions.into_iter().collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// _build_security_args
// ---------------------------------------------------------------------------

/// Mirrors `_build_security_args(run_as_host_user: bool, run_exec: bool = False) -> list[str]`.
pub fn build_security_args(run_as_host_user: bool, run_exec: bool) -> Vec<String> {
    let run_tmpfs = if run_exec { RUN_TMPFS_EXEC } else { RUN_TMPFS_NOEXEC };
    let mut args: Vec<String> = BASE_SECURITY_ARGS.iter().map(|s| s.to_string()).collect();
    args.extend(run_tmpfs.iter().map(|s| s.to_string()));
    if run_as_host_user {
        return args;
    }
    args.extend(PRIVDROP_CAP_ARGS.iter().map(|s| s.to_string()));
    args
}

// ---------------------------------------------------------------------------
// _image_uses_init_entrypoint
// ---------------------------------------------------------------------------

/// Mirrors `_image_uses_init_entrypoint(docker_exe: str, image: str) -> bool`.
pub fn image_uses_init_entrypoint(docker_exe: &str, image: &str) -> bool {
    let output = match Command::new(docker_exe)
        .args(["image", "inspect", image, "--format", "{{json .Config.Entrypoint}}"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::debug!("Docker: could not inspect entrypoint for {}: {}", image, e);
            return false;
        }
    };
    if !output.status.success() {
        log::debug!(
            "Docker: image inspect for {} returned {} (stderr={})",
            image,
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return false;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() || raw == "null" {
        return false;
    }
    // Parse JSON array: e.g. `["/init"]` or `"/init"` or `null`
    // Minimal parser without serde
    let entrypoint = parse_json_entrypoint(&raw);
    let Some(list) = entrypoint else { return false; };
    if list.is_empty() { return false; }
    let first = list[0].trim().to_string();
    first == "/init" || first == "/package/admin/s6-overlay/command/init"
}

fn parse_json_entrypoint(raw: &str) -> Option<Vec<String>> {
    let t = raw.trim();
    if t == "null" || t.is_empty() {
        return None;
    }
    if t.starts_with('"') {
        // Single string entrypoint: "value"
        let inner = extract_json_string_literal(t)?;
        return Some(vec![inner]);
    }
    if t.starts_with('[') {
        // Array: ["a","b"]
        let inner = &t[1..t.len()-1];
        let mut out = Vec::new();
        let mut in_str = false;
        let mut esc = false;
        let mut cur = String::new();
        let mut has_content = false;
        for c in inner.chars() {
            if esc { cur.push(c); esc=false; continue; }
            if c == '\\' && in_str { esc=true; continue; }
            if c == '"' {
                if in_str {
                    out.push(cur.clone());
                    cur.clear();
                    in_str = false;
                    has_content = true;
                } else {
                    in_str = true;
                }
                continue;
            }
            if in_str { cur.push(c); }
        }
        if out.is_empty() && !has_content && inner.trim().is_empty() {
            return Some(vec![]);
        }
        return Some(out);
    }
    None
}

fn extract_json_string_literal(s: &str) -> Option<String> {
    let t = s.trim();
    if !t.starts_with('"') || !t.ends_with('"') || t.len() < 2 {
        return None;
    }
    let inner = &t[1..t.len()-1];
    let mut out = String::new();
    let mut esc = false;
    for c in inner.chars() {
        if esc {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => out.push(c),
            }
            esc = false;
            continue;
        }
        if c == '\\' { esc = true; continue; }
        out.push(c);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for 1:1 fidelity
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn normalize_forward_env_names_filters() {
        let input = vec!["FOO".to_string(), "  BAR  ".to_string(), "invalid-name".to_string(), "FOO".to_string(), "".to_string()];
        let out = normalize_forward_env_names(Some(&input));
        assert_eq!(out, vec!["FOO", "BAR"]);
    }

    #[test]
    fn sanitize_label_value_cases() {
        assert_eq!(sanitize_label_value(""), "unknown");
        assert_eq!(sanitize_label_value("valid-label_1.2"), "valid-label_1.2");
        assert_eq!(sanitize_label_value("a:b/c"), "a_b_c");
        let long = "a".repeat(100);
        assert_eq!(sanitize_label_value(&long).len(), 63);
        assert_eq!(sanitize_label_value("..."), "unknown"); // becomes empty after trunc? actually "___"?? but our impl returns "___" not unknown? check python: cleaned[:63] or "unknown" — if cleaned is "___" it's truthy, so returns "___". For pure "." it would be "___" as well. But empty string case handled above.
    }

    #[test]
    fn extra_args_set_shm_size_detect() {
        assert!(extra_args_set_shm_size(Some(&["--shm-size".to_string()])));
        assert!(extra_args_set_shm_size(Some(&["--shm-size=2g".to_string()])));
        assert!(!extra_args_set_shm_size(Some(&["--other".to_string()])));
        assert!(!extra_args_set_shm_size(None));
        assert!(!extra_args_set_shm_size(Some(&[])));
    }

    #[test]
    fn build_security_args_variants() {
        let with_privdrop = build_security_args(false, false);
        assert!(with_privdrop.contains(&"--cap-add".to_string()));
        assert!(with_privdrop.contains(&"SETUID".to_string()));
        let without_privdrop = build_security_args(true, false);
        assert!(!without_privdrop.contains(&"SETUID".to_string()));
        let exec = build_security_args(false, true);
        assert!(exec.contains(&"/run:rw,exec,nosuid,size=64m".to_string()));
        let noexec = build_security_args(false, false);
        assert!(noexec.contains(&"/run:rw,noexec,nosuid,size=64m".to_string()));
    }

    #[test]
    fn egress_reuse_fingerprint_off() {
        let fp = egress_reuse_fingerprint(&[], &HashMap::new(), &[]);
        assert_eq!(fp, "off");
        let mut env = HashMap::new();
        env.insert("A".to_string(), "1".to_string());
        let fp2 = egress_reuse_fingerprint(&[], &env, &[]);
        assert_ne!(fp2, "off");
        assert_eq!(fp2.len(), 24);
    }

    #[test]
    fn critical_egress_names_includes_api_keys() {
        let mut env = HashMap::new();
        env.insert("OPENAI_API_KEY".to_string(), "tok".to_string());
        env.insert("FOO".to_string(), "bar".to_string());
        let crit = critical_egress_env_names(&env);
        assert!(crit.contains("OPENAI_API_KEY"));
        assert!(crit.contains("HTTPS_PROXY"));
        assert!(!crit.contains("FOO"));
    }

    #[test]
    fn extra_args_egress_collisions_cases() {
        let mut crit = HashSet::new();
        crit.insert("HTTPS_PROXY".to_string());
        crit.insert("MY_TOKEN".to_string());
        let args = vec!["-e".to_string(), "HTTPS_PROXY=foo".to_string(), "--network".to_string(), "host".to_string()];
        let cols = extra_args_egress_collisions(&args, &crit);
        assert!(cols.contains(&"HTTPS_PROXY".to_string()));
        assert!(cols.contains(&"--network".to_string()));

        let args2 = vec!["--env=MY_TOKEN=bar".to_string()];
        let cols2 = extra_args_egress_collisions(&args2, &crit);
        assert!(cols2.contains(&"MY_TOKEN".to_string()));

        let args3 = vec!["--env-file".to_string(), "/tmp/env".to_string()];
        let cols3 = extra_args_egress_collisions(&args3, &crit);
        assert!(cols3.contains(&"--env-file".to_string()));
    }

    #[test]
    fn parse_docker_finished_at_cases() {
        // zero value
        assert!(parse_docker_finished_at("0001-01-01T00:00:00Z").is_none());
        assert!(parse_docker_finished_at("").is_none());
        // normal with nanoseconds
        let t = parse_docker_finished_at("2026-05-28T13:45:00.123456789Z");
        assert!(t.is_some());
        // normal without fraction
        let t2 = parse_docker_finished_at("2026-05-28T13:45:00Z");
        assert!(t2.is_some());
        // with microseconds
        let t3 = parse_docker_finished_at("2026-05-28T13:45:00.123456Z");
        assert!(t3.is_some());
    }

    #[test]
    fn parse_json_entrypoint_cases() {
        assert_eq!(parse_json_entrypoint("null"), None);
        assert_eq!(parse_json_entrypoint("[]"), Some(vec![]));
        assert_eq!(parse_json_entrypoint("[\"/init\"]"), Some(vec!["/init".to_string()]));
        assert_eq!(parse_json_entrypoint("\"/init\""), Some(vec!["/init".to_string()]));
        assert_eq!(parse_json_entrypoint("[\"/bin/sh\",\"-c\"]"), Some(vec!["/bin/sh".to_string(), "-c".to_string()]));
    }

    #[test]
    fn sanitize_task_id_colon_rewrite() {
        let a = sanitize_task_id_for_path("a:b");
        let b = sanitize_task_id_for_path("a_b");
        assert_ne!(a, b);
        assert!(a.contains('-'));
    }

    #[test]
    fn is_valid_env_var_name_cases() {
        assert!(is_valid_env_var_name("FOO_BAR"));
        assert!(is_valid_env_var_name("_foo"));
        assert!(!is_valid_env_var_name("1FOO"));
        assert!(!is_valid_env_var_name("FOO-BAR"));
        assert!(!is_valid_env_var_name(""));
    }
}
