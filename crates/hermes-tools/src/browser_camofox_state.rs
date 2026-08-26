//! Hermes-managed Camofox state helpers.
//! Port of `tools/browser_camofox_state.py` (47 lines) — 1:1 behavior.
//!
//! Provides profile-scoped identity and state directory paths for Camofox
//! persistent browser profiles. When managed persistence is enabled, Hermes
//! sends a deterministic userId derived from the active profile so that
//! Camofox can map it to the same persistent browser profile directory
//! across restarts.

use std::path::{Path, PathBuf};

/// Mirrors `CAMOFOX_STATE_DIR_NAME` in Python.
pub const CAMOFOX_STATE_DIR_NAME: &str = "browser_auth";
/// Mirrors `CAMOFOX_STATE_SUBDIR` in Python.
pub const CAMOFOX_STATE_SUBDIR: &str = "camofox";

/// RFC 4122 `NAMESPACE_URL` = 6ba7b811-9dad-11d1-80b4-00c04fd430c8
const NAMESPACE_URL: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4,
    0x30, 0xc8,
];

/// Return the profile-scoped root directory for Camofox persistence.
///
/// Mirrors Python `get_camofox_state_dir()`:
/// `get_hermes_home() / "browser_auth" / "camofox"`.
/// In this workspace `get_hermes_home` maps to `GRAY_HOME` (compat: `HERMES_HOME`)
/// / `~/.gray`, matching `crates/gray/src/setup.rs::gray_home()`.
pub fn get_camofox_state_dir() -> PathBuf {
    get_hermes_home().join(CAMOFOX_STATE_DIR_NAME).join(CAMOFOX_STATE_SUBDIR)
}

/// Variant that resolves against an explicit home (useful for tests).
pub fn get_camofox_state_dir_for_home(home: &Path) -> PathBuf {
    home.join(CAMOFOX_STATE_DIR_NAME).join(CAMOFOX_STATE_SUBDIR)
}

/// Stable Hermes-managed Camofox identity for this profile.
///
/// * `user_id` is profile-scoped (same Hermes profile = same userId).
/// * `session_key` is scoped to the logical browser task so newly created
///   tabs within the same profile reuse the same identity contract.
///
/// Mirrors Python `get_camofox_identity(task_id: Optional[str] = None)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CamofoxIdentity {
    pub user_id: String,
    pub session_key: String,
}

/// Return the stable Hermes-managed Camofox identity for this profile.
///
/// `task_id` mirrors Python `Optional[str]` — `None` or empty string both
/// map to `"default"` (Python `task_id or "default"` is falsy for `""`).
pub fn get_camofox_identity(task_id: Option<&str>) -> CamofoxIdentity {
    let scope_root = get_camofox_state_dir().to_string_lossy().to_string();
    get_camofox_identity_for_scope(&scope_root, task_id)
}

/// Testable core: identity for an explicit `scope_root` string.
pub fn get_camofox_identity_for_scope(
    scope_root: &str,
    task_id: Option<&str>,
) -> CamofoxIdentity {
    let logical_scope = match task_id {
        Some(s) if !s.is_empty() => s,
        _ => "default",
    };
    let user_hex = uuid5_hex(&NAMESPACE_URL, &format!("camofox-user:{scope_root}"));
    let session_hex = uuid5_hex(
        &NAMESPACE_URL,
        &format!("camofox-session:{scope_root}:{logical_scope}"),
    );
    CamofoxIdentity {
        user_id: format!("hermes_{}", &user_hex[..10]),
        session_key: format!("task_{}", &session_hex[..16]),
    }
}

// ---------------------------------------------------------------------------
// Home resolution (mirrors hermes_constants.get_hermes_home / gray/src/setup)
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
    for key in ["GRAY_HOME", "HERMES_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".gray");
        }
    }
    // Fallback matching hermes_constants platform default on POSIX.
    // Python falls back to `Path.home() / ".hermes"`; gray uses `~/.gray`.
    // Use /tmp for environments without HOME.
    PathBuf::from("/tmp/.gray")
}

// ---------------------------------------------------------------------------
// UUID v5 (RFC 4122) — SHA-1 based, no external crate (1:1 with Python)
// ---------------------------------------------------------------------------

fn uuid5_hex(namespace: &[u8; 16], name: &str) -> String {
    let mut data = Vec::with_capacity(16 + name.len());
    data.extend_from_slice(namespace);
    data.extend_from_slice(name.as_bytes());
    let hash = sha1(&data);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // version 5
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    // RFC 4122 variant
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Minimal SHA-1 (FIPS 180-1), returns 20-byte digest.
fn sha1(message: &[u8]) -> [u8; 20] {
    let ml = (message.len() as u64) * 8;
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&ml.to_be_bytes());

    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!(CAMOFOX_STATE_DIR_NAME, "browser_auth");
        assert_eq!(CAMOFOX_STATE_SUBDIR, "camofox");
    }

    #[test]
    fn state_dir_joins_home() {
        let home = Path::new("/tmp/test-home");
        assert_eq!(
            get_camofox_state_dir_for_home(home),
            Path::new("/tmp/test-home/browser_auth/camofox")
        );
    }

    #[test]
    fn sha1_known_vectors() {
        fn hex(b: &[u8]) -> String {
            b.iter().map(|x| format!("{x:02x}")).collect()
        }
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"hello")), "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn uuid5_matches_python_for_known_scope() {
        // Python reference (see task verification):
        // scope_root = "/tmp/.gray/browser_auth/camofox"
        // user_hex = 9ed544681ac45d08b9cffff6905148e5 -> hermes_9ed544681a
        // session default hex = 1bfb19e61d035e5fbb32eca8fab3f643 -> task_1bfb19e61d035e5f
        let scope = "/tmp/.gray/browser_auth/camofox";
        let id = get_camofox_identity_for_scope(scope, None);
        assert_eq!(id.user_id, "hermes_9ed544681a");
        assert_eq!(id.session_key, "task_1bfb19e61d035e5f");

        let id2 = get_camofox_identity_for_scope(scope, Some("my-task"));
        assert_eq!(id2.user_id, "hermes_9ed544681a"); // profile-scoped, stable
        assert_eq!(id2.session_key, "task_7c59a1bc57be55df");

        // empty string falsy -> default
        let id3 = get_camofox_identity_for_scope(scope, Some(""));
        assert_eq!(id3.session_key, "task_1bfb19e61d035e5f");
    }

    #[test]
    fn uuid5_other_scope_matches_python() {
        let scope = "/home/vstaln/.hermes/browser_auth/camofox";
        let id = get_camofox_identity_for_scope(scope, None);
        assert_eq!(id.user_id, "hermes_1c7c013dfd");
        assert_eq!(id.session_key, "task_02b37b53c2e85472");
        let id2 = get_camofox_identity_for_scope(scope, Some("my-task"));
        assert_eq!(id2.session_key, "task_baff619b91e25b10");
    }

    #[test]
    fn identity_is_deterministic() {
        let scope = "/tmp/a/b/c";
        let a = get_camofox_identity_for_scope(scope, Some("t1"));
        let b = get_camofox_identity_for_scope(scope, Some("t1"));
        assert_eq!(a, b);
    }
}
