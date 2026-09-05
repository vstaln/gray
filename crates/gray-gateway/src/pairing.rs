//! DM pairing for unknown senders.
//!
//! Unknown DM senders receive a one-time code; the operator approves it from
//! the CLI (`gray gateway pairing approve <platform> <code>`). Until then the
//! sender's messages are *not* processed.
//!
//! Security properties (follows OWASP / NIST 800-63-4):
//! - 8-char codes from a 32-char unambiguous alphabet (no 0/O/1/I);
//! - codes are stored only as salted SHA-256 hashes;
//! - codes expire after 1 hour; max 3 pending codes per platform;
//! - one code request per user per 10 minutes;
//! - 5 failed approvals lock the platform for 1 hour;
//! - all files are written 0600 under `~/.gray/pairing/`;
//! - codes are never logged.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Platform;

pub const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LENGTH: usize = 8;
pub const CODE_TTL_SECS: i64 = 3600;
pub const RATE_LIMIT_SECS: i64 = 600;
pub const LOCKOUT_SECS: i64 = 3600;
pub const MAX_PENDING_PER_PLATFORM: usize = 3;
pub const MAX_FAILED_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEntry {
    pub hash: String,
    pub salt: String,
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedEntry {
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    pub approved_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RateState {
    /// `platform:user_id` -> last request unix ts
    #[serde(default)]
    requests: HashMap<String, i64>,
    /// platform -> (failed attempts, first failure ts)
    #[serde(default)]
    failures: HashMap<String, (u32, i64)>,
}

/// Outcome of a code request for an unknown sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingOffer {
    /// Send this code to the user (only time it exists in plaintext).
    Code(String),
    /// Too soon since this user's last request — stay silent (anti-spam).
    RateLimited,
    /// Queue full / platform locked out — tell the user to retry later.
    Unavailable,
}

pub struct PairingStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn secure_write(path: &Path, data: &str) -> anyhow::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 128 random bits from the OS CSPRNG (uuid v4 is backed by `getrandom`).
fn random_bytes() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}

/// Generate a code from the unambiguous alphabet. Each byte is reduced
/// modulo 32 — the alphabet length divides 256, so there is no modulo bias.
pub fn generate_code() -> String {
    let bytes = random_bytes();
    bytes
        .iter()
        .take(CODE_LENGTH)
        .map(|b| ALPHABET[(*b % 32) as usize] as char)
        .collect()
}

pub fn hash_code(code: &str, salt_hex: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt_hex.as_bytes());
    h.update(b":");
    h.update(code.trim().to_ascii_uppercase().as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Normalize user ids so allowlists and approvals compare equal
/// (Slack ids are case-insensitive in practice; Telegram/Discord are numeric).
pub fn normalize_user_id(platform: Platform, user_id: &str) -> String {
    let t = user_id.trim();
    match platform {
        Platform::Slack => t.to_ascii_uppercase(),
        Platform::Telegram | Platform::Discord => t.trim_start_matches('@').to_string(),
    }
}

impl PairingStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lock: Mutex::new(()),
        }
    }

    pub fn default_dir() -> anyhow::Result<PathBuf> {
        crate::config::gray_home_dir().map(|h| h.join("pairing"))
    }

    pub fn open_default() -> Self {
        let dir = Self::default_dir().unwrap_or_else(|_| PathBuf::from("/tmp/gray-pairing"));
        Self::new(dir)
    }

    fn pending_path(&self, platform: Platform) -> PathBuf {
        self.dir.join(format!("{platform}-pending.json"))
    }
    fn approved_path(&self, platform: Platform) -> PathBuf {
        self.dir.join(format!("{platform}-approved.json"))
    }
    fn rate_path(&self) -> PathBuf {
        self.dir.join("rate_limits.json")
    }

    fn load_pending(&self, platform: Platform) -> HashMap<String, PendingEntry> {
        load_json(&self.pending_path(platform))
    }
    fn save_pending(&self, platform: Platform, m: &HashMap<String, PendingEntry>) {
        if let Ok(s) = serde_json::to_string_pretty(m)
            && let Err(e) = secure_write(&self.pending_path(platform), &s)
        {
            log::warn!("pairing: write pending failed: {e}");
        }
    }
    fn load_approved(&self, platform: Platform) -> HashMap<String, ApprovedEntry> {
        load_json(&self.approved_path(platform))
    }
    fn save_approved(&self, platform: Platform, m: &HashMap<String, ApprovedEntry>) {
        if let Ok(s) = serde_json::to_string_pretty(m)
            && let Err(e) = secure_write(&self.approved_path(platform), &s)
        {
            log::warn!("pairing: write approved failed: {e}");
        }
    }
    fn load_rate(&self) -> RateState {
        load_json(&self.rate_path())
    }
    fn save_rate(&self, r: &RateState) {
        if let Ok(s) = serde_json::to_string_pretty(r) {
            let _ = secure_write(&self.rate_path(), &s);
        }
    }

    fn cleanup_expired(&self, platform: Platform) -> HashMap<String, PendingEntry> {
        let mut pending = self.load_pending(platform);
        let cutoff = now() - CODE_TTL_SECS;
        let before = pending.len();
        pending.retain(|_, e| e.created_at > cutoff);
        if pending.len() != before {
            self.save_pending(platform, &pending);
        }
        pending
    }

    fn is_locked_out(&self, rate: &RateState, platform: Platform) -> bool {
        match rate.failures.get(&platform.to_string()) {
            Some((n, first)) => *n >= MAX_FAILED_ATTEMPTS && now() - *first < LOCKOUT_SECS,
            None => false,
        }
    }

    pub fn is_approved(&self, platform: Platform, user_id: &str) -> bool {
        let uid = normalize_user_id(platform, user_id);
        self.load_approved(platform).contains_key(&uid)
    }

    /// Whether this user already has a live pending code (so we don't re-prompt).
    pub fn has_pending(&self, platform: Platform, user_id: &str) -> bool {
        let uid = normalize_user_id(platform, user_id);
        let cutoff = now() - CODE_TTL_SECS;
        self.load_pending(platform)
            .values()
            .any(|e| e.user_id == uid && e.created_at > cutoff)
    }

    /// Request a pairing code for an unknown DM sender.
    pub fn request_code(&self, platform: Platform, user_id: &str, user_name: &str) -> PairingOffer {
        let _g = self.lock.lock().unwrap();
        let uid = normalize_user_id(platform, user_id);
        let mut pending = self.cleanup_expired(platform);
        let mut rate = self.load_rate();
        if self.is_locked_out(&rate, platform) {
            return PairingOffer::Unavailable;
        }
        let rate_key = format!("{platform}:{uid}");
        if let Some(last) = rate.requests.get(&rate_key)
            && now() - *last < RATE_LIMIT_SECS
        {
            return PairingOffer::RateLimited;
        }
        // Re-request from the same user replaces their old code instead of eating a slot.
        pending.retain(|_, e| e.user_id != uid);
        if pending.len() >= MAX_PENDING_PER_PLATFORM {
            return PairingOffer::Unavailable;
        }
        let code = generate_code();
        let salt = hex(&random_bytes());
        let entry_id = hex(&random_bytes()[..8]);
        pending.insert(
            entry_id,
            PendingEntry {
                hash: hash_code(&code, &salt),
                salt,
                user_id: uid,
                user_name: user_name.to_string(),
                created_at: now(),
            },
        );
        self.save_pending(platform, &pending);
        rate.requests.insert(rate_key, now());
        self.save_rate(&rate);
        PairingOffer::Code(code)
    }

    /// Operator approval by code. Returns the approved user on success.
    pub fn approve_code(&self, platform: Platform, code: &str) -> Option<ApprovedEntry> {
        let _g = self.lock.lock().unwrap();
        let mut rate = self.load_rate();
        if self.is_locked_out(&rate, platform) {
            return None;
        }
        let mut pending = self.cleanup_expired(platform);
        let found = pending
            .iter()
            .find(|(_, e)| hash_code(code, &e.salt) == e.hash)
            .map(|(k, _)| k.clone());
        let Some(id) = found else {
            let f = rate
                .failures
                .entry(platform.to_string())
                .or_insert((0, now()));
            if now() - f.1 >= LOCKOUT_SECS {
                *f = (0, now());
            }
            f.0 += 1;
            self.save_rate(&rate);
            return None;
        };
        let entry = pending.remove(&id)?;
        self.save_pending(platform, &pending);
        rate.failures.remove(&platform.to_string());
        self.save_rate(&rate);
        let mut approved = self.load_approved(platform);
        let a = ApprovedEntry {
            user_id: entry.user_id.clone(),
            user_name: entry.user_name,
            approved_at: now(),
        };
        approved.insert(entry.user_id, a.clone());
        self.save_approved(platform, &approved);
        Some(a)
    }

    /// Operator approval by user id, bypassing the code (e.g. from a status screen).
    pub fn approve_user(&self, platform: Platform, user_id: &str, user_name: &str) {
        let _g = self.lock.lock().unwrap();
        let uid = normalize_user_id(platform, user_id);
        let mut pending = self.cleanup_expired(platform);
        pending.retain(|_, e| e.user_id != uid);
        self.save_pending(platform, &pending);
        let mut approved = self.load_approved(platform);
        approved.insert(
            uid.clone(),
            ApprovedEntry {
                user_id: uid,
                user_name: user_name.to_string(),
                approved_at: now(),
            },
        );
        self.save_approved(platform, &approved);
    }

    pub fn revoke(&self, platform: Platform, user_id: &str) -> bool {
        let _g = self.lock.lock().unwrap();
        let uid = normalize_user_id(platform, user_id);
        let mut approved = self.load_approved(platform);
        let removed = approved.remove(&uid).is_some();
        if removed {
            self.save_approved(platform, &approved);
        }
        removed
    }

    pub fn list_approved(&self, platform: Platform) -> Vec<ApprovedEntry> {
        let mut v: Vec<_> = self.load_approved(platform).into_values().collect();
        v.sort_by_key(|e| e.approved_at);
        v
    }

    /// Pending requests (user ids + ages) — codes are never recoverable from here.
    pub fn list_pending(&self, platform: Platform) -> Vec<PendingEntry> {
        let _g = self.lock.lock().unwrap();
        let mut v: Vec<_> = self.cleanup_expired(platform).into_values().collect();
        v.sort_by_key(|e| e.created_at);
        v
    }
}

// ---------------------------------------------------------------------------
// Operator actions shared by the `gray gateway pairing` CLI and the
// `/gateway pairing` REPL command.
// ---------------------------------------------------------------------------

fn parse_platform(raw: &str) -> anyhow::Result<Platform> {
    raw.parse::<Platform>().map_err(|e| anyhow::anyhow!("{e}"))
}

/// Approve a pending code, returning a human line. Approving the first-ever
/// user on a platform with an empty allowlist also writes them into
/// `allowed_users` so the owner
/// bind happens with no file editing. `cfg` is mutated; the caller saves it.
pub fn pairing_approve_with(
    store: &PairingStore,
    cfg: &mut crate::config::GatewayConfig,
    platform: Platform,
    code: &str,
) -> anyhow::Result<String> {
    let Some(entry) = store.approve_code(platform, code.trim()) else {
        anyhow::bail!(
            "no pending {platform} code matches — expired or mistyped? (`pairing list {platform}`)"
        );
    };
    let who = if entry.user_name.is_empty() {
        entry.user_id.clone()
    } else {
        format!("{} ({})", entry.user_name, entry.user_id)
    };
    let mut msg = format!("approved {platform} user {who}");
    if let Some(pc) = cfg.platforms.get_mut(&platform)
        && pc.allowed_users.is_empty()
    {
        pc.allowed_users.push(entry.user_id.clone());
        msg += " — first user: added to allowed_users (owner bound)";
    }
    Ok(msg)
}

/// Approve against the default store + live gateway.yaml (CLI/REPL entry point).
pub fn pairing_approve(platform_raw: &str, code: &str) -> anyhow::Result<String> {
    let platform = parse_platform(platform_raw)?;
    let store = PairingStore::open_default();
    let mut cfg = crate::config::load_gateway_config();
    let msg = pairing_approve_with(&store, &mut cfg, platform, code)?;
    crate::config::save_gateway_config(&cfg)?;
    Ok(msg)
}

/// One human block: pending + approved users, one platform or all.
pub fn pairing_list(platform_raw: Option<&str>) -> anyhow::Result<String> {
    use Platform::{Discord, Slack, Telegram};
    let plats = match platform_raw {
        Some(p) if !p.eq_ignore_ascii_case("all") => vec![parse_platform(p)?],
        _ => vec![Telegram, Discord, Slack],
    };
    let store = PairingStore::open_default();
    let mut out = String::new();
    for p in plats {
        let pending = store.list_pending(p);
        let approved = store.list_approved(p);
        out += &format!(
            "{p}: {} pending, {} approved\n",
            pending.len(),
            approved.len()
        );
        for e in &pending {
            out += &format!("  pending {} {}\n", e.user_id, e.user_name);
        }
        for a in &approved {
            out += &format!("  approved {} {}\n", a.user_id, a.user_name);
        }
    }
    Ok(out.trim_end().to_string())
}

/// Drop a user's approval (pairing store only; config allowlists are untouched).
pub fn pairing_revoke(platform_raw: &str, user_raw: &str) -> anyhow::Result<String> {
    let platform = parse_platform(platform_raw)?;
    let store = PairingStore::open_default();
    if store.revoke(platform, user_raw) {
        Ok(format!("revoked {platform} user {}", user_raw.trim()))
    } else {
        anyhow::bail!("no {platform} approval for {}", user_raw.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PairingStore) {
        let d = tempfile::tempdir().unwrap();
        let s = PairingStore::new(d.path().join("pairing"));
        (d, s)
    }

    #[test]
    fn approve_bootstraps_owner_when_allowlist_empty() {
        use crate::config::{GatewayConfig, PlatformConfig};
        let (_d, s) = store();
        let PairingOffer::Code(code) = s.request_code(Platform::Discord, "4242", "v") else {
            panic!("expected a code");
        };
        let mut cfg = GatewayConfig::default();
        cfg.platforms
            .insert(Platform::Discord, PlatformConfig::with_token("t"));
        let msg = pairing_approve_with(&s, &mut cfg, Platform::Discord, &code).unwrap();
        assert!(msg.contains("approved"), "got: {msg}");
        assert_eq!(
            cfg.platforms[&Platform::Discord].allowed_users,
            vec!["4242"]
        );
        // Second approval must not touch the list.
        let PairingOffer::Code(code2) = s.request_code(Platform::Discord, "9999", "") else {
            panic!("expected a code");
        };
        pairing_approve_with(&s, &mut cfg, Platform::Discord, &code2).unwrap();
        assert_eq!(
            cfg.platforms[&Platform::Discord].allowed_users,
            vec!["4242"]
        );
    }

    #[test]
    fn approve_bad_code_fails() {
        use crate::config::GatewayConfig;
        let (_d, s) = store();
        let mut cfg = GatewayConfig::default();
        assert!(pairing_approve_with(&s, &mut cfg, Platform::Discord, "nope").is_err());
    }

    #[test]
    fn code_shape() {
        for _ in 0..50 {
            let c = generate_code();
            assert_eq!(c.len(), CODE_LENGTH);
            assert!(c.bytes().all(|b| ALPHABET.contains(&b)), "{c}");
            assert!(!c.contains(['0', 'O', '1', 'I']));
        }
    }

    #[test]
    fn request_then_approve_roundtrip() {
        let (_d, s) = store();
        assert!(!s.is_approved(Platform::Telegram, "42"));
        let PairingOffer::Code(code) = s.request_code(Platform::Telegram, "42", "alice") else {
            panic!("expected code")
        };
        assert!(s.has_pending(Platform::Telegram, "42"));
        // Plaintext code never touches disk.
        let raw = std::fs::read_to_string(s.pending_path(Platform::Telegram)).unwrap();
        assert!(!raw.contains(&code));
        // Wrong code fails and counts as a failure.
        assert!(s.approve_code(Platform::Telegram, "ZZZZZZZZ").is_none());
        // Case-insensitive approval.
        let a = s
            .approve_code(Platform::Telegram, &code.to_lowercase())
            .unwrap();
        assert_eq!(a.user_id, "42");
        assert_eq!(a.user_name, "alice");
        assert!(s.is_approved(Platform::Telegram, "42"));
        assert!(!s.has_pending(Platform::Telegram, "42"));
        // Second approve of the same code fails (consumed).
        assert!(s.approve_code(Platform::Telegram, &code).is_none());
        assert!(s.revoke(Platform::Telegram, "42"));
        assert!(!s.is_approved(Platform::Telegram, "42"));
    }

    #[test]
    fn rate_limit_and_pending_cap() {
        let (_d, s) = store();
        assert!(matches!(
            s.request_code(Platform::Discord, "1", ""),
            PairingOffer::Code(_)
        ));
        assert_eq!(
            s.request_code(Platform::Discord, "1", ""),
            PairingOffer::RateLimited
        );
        assert!(matches!(
            s.request_code(Platform::Discord, "2", ""),
            PairingOffer::Code(_)
        ));
        assert!(matches!(
            s.request_code(Platform::Discord, "3", ""),
            PairingOffer::Code(_)
        ));
        assert_eq!(
            s.request_code(Platform::Discord, "4", ""),
            PairingOffer::Unavailable
        );
        assert_eq!(s.list_pending(Platform::Discord).len(), 3);
        // Platforms are isolated.
        assert!(matches!(
            s.request_code(Platform::Slack, "U1", ""),
            PairingOffer::Code(_)
        ));
    }

    #[test]
    fn lockout_after_failures() {
        let (_d, s) = store();
        let PairingOffer::Code(code) = s.request_code(Platform::Slack, "U9", "") else {
            panic!()
        };
        for _ in 0..MAX_FAILED_ATTEMPTS {
            assert!(s.approve_code(Platform::Slack, "BADCODE1").is_none());
        }
        // Locked: even the right code is refused now.
        assert!(s.approve_code(Platform::Slack, &code).is_none());
        assert_eq!(
            s.request_code(Platform::Slack, "U10", ""),
            PairingOffer::Unavailable
        );
    }

    #[test]
    fn slack_ids_normalized() {
        let (_d, s) = store();
        s.approve_user(Platform::Slack, "u123abc", "bob");
        assert!(s.is_approved(Platform::Slack, "U123ABC"));
        assert_eq!(normalize_user_id(Platform::Telegram, "@42"), "42");
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store();
        let _ = s.request_code(Platform::Telegram, "7", "");
        let mode = std::fs::metadata(s.pending_path(Platform::Telegram))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
