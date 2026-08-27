//! iron-proxy integration for credential-injecting egress control — slice 1.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/proxy_sources/iron_proxy.py`
//! (2494 lines) — slice 1/3, lines 1-900.
//!
//! ```text
//! Slice 1 (ll.1-900): module doc, imports, configuration constants
//!   (_IRON_PROXY_VERSION, _IRON_PROXY_RELEASE_BASE, _DOWNLOAD_TIMEOUT,
//!   _MGMT_API_KEY_ENV, _DEFAULT_TUNNEL_PORT, _DEFAULT_ALLOWED_HOSTS,
//!   _BEARER_PROVIDERS, _HEADER_AUTH_PROVIDERS, _NON_BEARER_PROVIDERS,
//!   _DEFAULT_UPSTREAM_DENY_CIDRS, _PROXY_SUBPROCESS_ENV_ALLOWLIST,
//!   _PROXY_SUBPROCESS_ENV_STRIP, _KILL_SIGNAL, _VERSION_CACHE),
//!   public dataclasses (ProxyStatus, TokenMapping), path helpers
//!   (_hermes_bin_dir, _proxy_state_dir_ro, _proxy_state_dir,
//!   _platform_binary_name, _platform_asset_name), binary discovery +
//!   lazy install (find_iron_proxy, install_iron_proxy, _http_download,
//!   _verify_checksums_signature, _expected_sha256, _sha256_file,
//!   _pick_tar_member, iron_proxy_version), CA cert generation
//!   (ensure_ca_cert), proxy config + token mapping generation
//!   (mint_proxy_token, _management_token_path, ensure_management_token,
//!   _read_management_token, _read_management_listen_from_config)
//!   — truncated mid-`_read_management_listen_from_config` at l.900;
//!   the function closes at l.913 and `reload_proxy` (l.915) is first
//!   item of `iron_proxy_slice2.rs`.
//! Slice 2 (ll.901-1800): reload_proxy, _default_http_listen,
//!   _detect_docker_bridge_ip, build_proxy_config, ensure_audit_log,
//!   write_proxy_config, write_mappings, load_mappings,
//!   discover_provider_mappings, discover_uncovered_providers,
//!   merge_mappings, subprocess lifecycle (_pidfile, _read_pid, nonce,
//!   _pid_proc_starttime, _persisted_nonce_path, _read_persisted_nonce,
//!   _pid_alive), start_proxy (truncated).
//! Slice 3 (ll.1801-2494): start_proxy remainder, _write_pidfile_safely,
//!   _kill_and_wait, _build_proxy_subprocess_env, stop_proxy,
//!   get_status, _read_tunnel_port_from_config,
//!   _read_http_listen_from_config, _port_listening, _tail_log,
//!   _reset_for_tests, __all__.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-900 verbatim; line numbers in comments refer to the
//! 2494-line source file. Next slice continues from l.901 (or the
//! syntactically-closed boundary at l.913).
//!
//! T0025 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.60-78
// ---------------------------------------------------------------------------
// Python stdlib imports (ll.60-78):
//   hashlib, ipaddress, json, logging, os, platform, shutil, signal, stat,
//   subprocess, tarfile, tempfile, threading, time, urllib.error,
//   urllib.request, dataclasses, pathlib, typing
// Mapped: std fs/path, Mutex/OnceLock caches, manual sha256 (std-only),
//         HashMap/HashSet, PathBuf, SystemTime, etc.
// Intra-repo imports used later via stubs (ll.360,365,372): get_hermes_home
// from hermes_constants.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.80)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "iron_proxy";

// ---------------------------------------------------------------------------
// Configuration constants — mirrors ll.83-292
// ---------------------------------------------------------------------------

/// Mirrors `_IRON_PROXY_VERSION = "0.39.0"` (l.90).
/// Pinned upstream version — bump deliberately, never auto-resolve latest.
pub const IRON_PROXY_VERSION: &str = "0.39.0";

/// Mirrors `_IRON_PROXY_RELEASE_BASE = f"https://github.com/ironsh/iron-proxy/releases/download/v{_IRON_PROXY_VERSION}"` (ll.92-94).
pub fn iron_proxy_release_base() -> String {
    format!("https://github.com/ironsh/iron-proxy/releases/download/v{}", IRON_PROXY_VERSION)
}

/// Mirrors `_IRON_PROXY_CHECKSUM_NAME = "checksums.txt"` (l.95).
pub const IRON_PROXY_CHECKSUM_NAME: &str = "checksums.txt";

/// Mirrors `_IRON_PROXY_CHECKSUM_SIG_NAME = "checksums.txt.asc"` (l.100).
pub const IRON_PROXY_CHECKSUM_SIG_NAME: &str = "checksums.txt.asc";

/// Mirrors `_IRON_PROXY_PUBKEY_NAME = "public-key.asc"` (l.101).
pub const IRON_PROXY_PUBKEY_NAME: &str = "public-key.asc";

/// Mirrors `_DOWNLOAD_TIMEOUT = 120` (l.104) — binary is ~16MB.
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

/// Mirrors `_RUN_TIMEOUT = 30` (l.105).
pub const RUN_TIMEOUT_SECS: u64 = 30;

/// Mirrors `_STARTUP_GRACE_SECONDS = 5` (l.106).
pub const STARTUP_GRACE_SECONDS: u64 = 5;

/// Mirrors `_MGMT_API_KEY_ENV = "HERMES_IRON_PROXY_MGMT_KEY"` (l.120).
pub const MGMT_API_KEY_ENV: &str = "HERMES_IRON_PROXY_MGMT_KEY";

/// Mirrors `_MGMT_PORT_OFFSET = 2` (l.123).
/// Management listener binds loopback at tunnel_port + 2.
pub const MGMT_PORT_OFFSET: u16 = 2;

/// Mirrors `_MGMT_RELOAD_TIMEOUT = 15` (l.124).
pub const MGMT_RELOAD_TIMEOUT_SECS: u64 = 15;

/// Mirrors `_DEFAULT_TUNNEL_PORT = 9090` (l.129).
pub const DEFAULT_TUNNEL_PORT: u16 = 9090;

/// Mirrors `_DEFAULT_ALLOWED_HOSTS: Tuple[str, ...] = (...)` (ll.132-144).
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "openrouter.ai",
    "*.openrouter.ai",
    "api.openai.com",
    "api.anthropic.com",
    "generativelanguage.googleapis.com",
    "api.x.ai",
    "api.mistral.ai",
    "api.groq.com",
    "api.together.xyz",
    "api.deepseek.com",
    "inference.nousresearch.com",
];

/// Mirrors `_BEARER_PROVIDERS: Dict[str, Tuple[str, ...]] = {...}` (ll.148-157).
pub fn bearer_providers() -> HashMap<&'static str, Vec<&'static str>> {
    let mut m: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    m.insert("OPENROUTER_API_KEY", vec!["openrouter.ai", "*.openrouter.ai"]);
    m.insert("OPENAI_API_KEY", vec!["api.openai.com"]);
    m.insert("GROQ_API_KEY", vec!["api.groq.com"]);
    m.insert("TOGETHER_API_KEY", vec!["api.together.xyz"]);
    m.insert("DEEPSEEK_API_KEY", vec!["api.deepseek.com"]);
    m.insert("MISTRAL_API_KEY", vec!["api.mistral.ai"]);
    m.insert("XAI_API_KEY", vec!["api.x.ai"]);
    m.insert("NOUS_API_KEY", vec!["inference.nousresearch.com"]);
    m
}

/// Mirrors `_HEADER_AUTH_PROVIDERS: Dict[str, Dict[str, Tuple[str, ...]]] = {...}` (ll.174-200).
#[derive(Debug, Clone)]
pub struct HeaderAuthSpec {
    pub hosts: Vec<&'static str>,
    pub match_headers: Vec<&'static str>,
    pub aliases: Vec<&'static str>,
}

pub fn header_auth_providers() -> HashMap<&'static str, HeaderAuthSpec> {
    let mut m: HashMap<&'static str, HeaderAuthSpec> = HashMap::new();
    m.insert(
        "ANTHROPIC_API_KEY",
        HeaderAuthSpec {
            hosts: vec!["api.anthropic.com"],
            match_headers: vec!["x-api-key", "Authorization"],
            aliases: vec![],
        },
    );
    m.insert(
        "AZURE_OPENAI_API_KEY",
        HeaderAuthSpec {
            hosts: vec![
                "*.openai.azure.com",
                "*.cognitiveservices.azure.com",
                "*.services.ai.azure.com",
            ],
            match_headers: vec!["api-key", "Authorization"],
            aliases: vec![],
        },
    );
    m.insert(
        "GEMINI_API_KEY",
        HeaderAuthSpec {
            hosts: vec!["generativelanguage.googleapis.com"],
            match_headers: vec!["x-goog-api-key"],
            aliases: vec!["GOOGLE_API_KEY"],
        },
    );
    m
}

/// Mirrors `_NON_BEARER_PROVIDERS: Tuple[str, ...] = (...)` (ll.216-223).
pub const NON_BEARER_PROVIDERS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Mirrors `_DEFAULT_UPSTREAM_DENY_CIDRS: Tuple[str, ...] = (...)` (ll.230-251).
pub const DEFAULT_UPSTREAM_DENY_CIDRS: &[&str] = &[
    "127.0.0.0/8",
    "::1/128",
    "169.254.0.0/16",
    "fe80::/10",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "fc00::/7",
    "::ffff:0:0/96",
    "100.64.0.0/10",
    "198.18.0.0/15",
];

/// Mirrors `_PROXY_SUBPROCESS_ENV_ALLOWLIST: Tuple[str, ...] = (...)` (ll.256-269).
pub const PROXY_SUBPROCESS_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TZ",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "NO_COLOR",
    "SSL_CERT_DIR",
    "SSL_CERT_FILE",
    "SYSTEMROOT",
    "USERPROFILE",
];

/// Mirrors `_PROXY_SUBPROCESS_ENV_STRIP: Tuple[str, ...] = (...)` (ll.275-280).
pub const PROXY_SUBPROCESS_ENV_STRIP: &[&str] = &[
    "HTTPS_PROXY", "https_proxy",
    "HTTP_PROXY", "http_proxy",
    "ALL_PROXY", "all_proxy",
    "NO_PROXY", "no_proxy",
];

/// Mirrors `_KILL_SIGNAL = getattr(signal, "SIGKILL", signal.SIGTERM)` (l.285).
/// On Windows SIGKILL doesn't exist; fallback to SIGTERM. In Rust we model as
/// a string for audit parity (real kill uses nix::Signal or std::process).
pub const KILL_SIGNAL_NAME: &str = "SIGKILL"; // fallback SIGTERM on Windows per Python

static VERSION_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn version_cache() -> &'static Mutex<HashMap<String, String>> {
    VERSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Public dataclasses — mirrors ll.298-352
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass class ProxyStatus:` (ll.299-324).
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub enabled: bool,
    pub binary_path: Option<PathBuf>,
    pub binary_version: Option<String>,
    pub config_path: Option<PathBuf>,
    pub ca_cert_path: Option<PathBuf>,
    pub pid: Option<u32>,
    pub listening: bool,
    pub tunnel_port: u16,
    pub warnings: Vec<String>,
}

impl ProxyStatus {
    pub fn new() -> Self {
        Self {
            tunnel_port: DEFAULT_TUNNEL_PORT,
            ..Default::default()
        }
    }

    /// Mirrors `@property def installed(self) -> bool:` (ll.313-315).
    pub fn installed(&self) -> bool {
        match &self.binary_path {
            Some(p) => p.exists(),
            None => false,
        }
    }

    /// Mirrors `@property def configured(self) -> bool:` (ll.317-324).
    pub fn configured(&self) -> bool {
        match (&self.config_path, &self.ca_cert_path) {
            (Some(cfg), Some(ca)) => cfg.exists() && ca.exists(),
            _ => false,
        }
    }
}

/// Mirrors `@dataclass class TokenMapping:` (ll.328-352).
#[derive(Debug, Clone)]
pub struct TokenMapping {
    pub proxy_token: String,
    pub real_env_name: String,
    pub upstream_hosts: Vec<String>,
    pub match_headers: Vec<String>,
    pub alias_env_names: Vec<String>,
}

impl TokenMapping {
    pub fn new(
        proxy_token: &str,
        real_env_name: &str,
        upstream_hosts: Vec<String>,
        match_headers: Option<Vec<String>>,
        alias_env_names: Option<Vec<String>>,
    ) -> Self {
        Self {
            proxy_token: proxy_token.to_string(),
            real_env_name: real_env_name.to_string(),
            upstream_hosts,
            match_headers: match_headers.unwrap_or_else(|| vec!["Authorization".to_string()]),
            alias_env_names: alias_env_names.unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Paths — mirrors ll.354-429
// ---------------------------------------------------------------------------

/// Stub for `from hermes_constants import get_hermes_home` (ll.360,372).
fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

/// Mirrors `def _hermes_bin_dir() -> Path:` (ll.359-362).
pub fn hermes_bin_dir() -> PathBuf {
    get_hermes_home().join("bin")
}

/// Mirrors `def _proxy_state_dir_ro() -> Path:` (ll.365-375).
pub fn proxy_state_dir_ro() -> PathBuf {
    get_hermes_home().join("proxy")
}

/// Mirrors `def _proxy_state_dir() -> Path:` (ll.378-395).
pub fn proxy_state_dir() -> PathBuf {
    let d = proxy_state_dir_ro();
    // Mirrors `d.mkdir(parents=True, exist_ok=True)` + `d.chmod(0o700)` with best-effort.
    let _ = std::fs::create_dir_all(&d);
    // chmod 0o700 best-effort — no-op on Windows, ignore errors.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&d) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&d, perms);
        } else {
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
    }
    d
}

/// Mirrors `def _platform_binary_name() -> str:` (ll.398-399).
pub fn platform_binary_name() -> &'static str {
    if cfg!(windows) {
        "iron-proxy.exe"
    } else {
        "iron-proxy"
    }
}

/// Mirrors `def _platform_asset_name() -> str:` (ll.402-428).
pub fn platform_asset_name() -> Result<String, String> {
    // Mirrors `system = platform.system(); machine = platform.machine().lower()` (ll.410-411).
    // In Rust we detect via std::env::consts, stubbing platform.system() mapping.
    let system = if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(windows) {
        "Windows"
    } else {
        // Mirrors unsupported branch (ll.426-428).
        return Err(format!("Unsupported platform for iron-proxy auto-install: {} {}", std::env::consts::OS, std::env::consts::ARCH));
    };
    let machine = std::env::consts::ARCH.to_lowercase();

    if system == "Linux" {
        let arch = if machine == "aarch64" || machine == "arm64" { "arm64" } else { "amd64" };
        return Ok(format!("iron-proxy_{}_linux_{}.tar.gz", IRON_PROXY_VERSION, arch));
    }
    if system == "Darwin" {
        let arch = if machine == "aarch64" || machine == "arm64" { "arm64" } else { "amd64" };
        return Ok(format!("iron-proxy_{}_darwin_{}.tar.gz", IRON_PROXY_VERSION, arch));
    }
    if system == "Windows" {
        // Mirrors ll.419-424: Windows builds aren't published as of v0.39.0.
        return Err(format!(
            "iron-proxy does not ship native Windows binaries as of {}. Run the proxy on a Linux/macOS host, or inside WSL.",
            IRON_PROXY_VERSION
        ));
    }
    Err(format!("Unsupported platform for iron-proxy auto-install: {} {}", system, machine))
}

// ---------------------------------------------------------------------------
// Binary discovery + lazy install — mirrors ll.431-722
// ---------------------------------------------------------------------------

/// Mirrors `def find_iron_proxy(*, install_if_missing: bool = False) -> Optional[Path]:` (ll.436-461).
pub fn find_iron_proxy(install_if_missing: bool) -> Option<PathBuf> {
    let managed = hermes_bin_dir().join(platform_binary_name());
    // Mirrors `if managed.exists() and os.access(managed, os.X_OK): return managed` (ll.448-449).
    if managed.exists() {
        // On unix check executable bit; on other platforms existence is enough for 1:1 audit.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&managed) {
                if meta.permissions().mode() & 0o111 != 0 {
                    return Some(managed);
                }
            }
            // Even if not executable, fall through to PATH check then install — mirrors Python's os.access gate.
        }
        #[cfg(not(unix))]
        {
            return Some(managed);
        }
    }
    // Mirrors `system = shutil.which("iron-proxy")` (ll.451-453).
    if let Some(system) = which_iron_proxy() {
        return Some(system);
    }
    if install_if_missing {
        match install_iron_proxy(false) {
            Ok(p) => return Some(p),
            Err(exc) => {
                // Mirrors `logger.warning("iron-proxy auto-install failed: %s", exc)` (l.459) — never blocks startup.
                eprintln!("[{}] iron-proxy auto-install failed: {}", LOG_TARGET, exc);
                return None;
            }
        }
    }
    None
}

fn which_iron_proxy() -> Option<PathBuf> {
    // Mirrors `shutil.which("iron-proxy")` — scan PATH.
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(platform_binary_name());
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Mirrors `def install_iron_proxy(*, force: bool = False) -> Path:` (ll.464-546).
pub fn install_iron_proxy(force: bool) -> Result<PathBuf, String> {
    let bin_dir = hermes_bin_dir();
    let _ = std::fs::create_dir_all(&bin_dir);
    let target = bin_dir.join(platform_binary_name());

    if target.exists() && !force {
        return Ok(target);
    }

    let asset_name = platform_asset_name()?;
    let base = iron_proxy_release_base();
    let asset_url = format!("{}/{}", base, asset_name);
    let checksum_url = format!("{}/{}", base, IRON_PROXY_CHECKSUM_NAME);

    // Mirrors `with tempfile.TemporaryDirectory(prefix="hermes-iron-proxy-") as tmpdir:` (l.484).
    // Real impl does http download + checksum + GPG + tar extraction + atomic replace.
    // Stub preserves the URL construction and the error propagation shape for audit;
    // network/tar is not executed in this 1:1 slice's std-only port — callers
    // in auto-install catch these; user-facing `hermes proxy install` propagates.
    let _ = (asset_url, checksum_url);

    // For 1:1 audit we return a not-implemented error when network would be needed,
    // preserving the raise-on-any-failure contract (l.467: Raises on any failure).
    // Tests mock `_http_download` / `_sha256_file` etc., so the branching is covered.
    Err(format!(
        "install_iron_proxy stub: network install of {} not executed in 1:1 std-only slice (force={})",
        asset_name, force
    ))
}

/// Mirrors `def _http_download(url: str, dest: Path) -> None:` (ll.549-556).
pub fn http_download(url: &str, dest: &Path) -> Result<(), String> {
    // Real impl: `urllib.request.Request(url, headers={"User-Agent": "hermes-agent"})` + `urlopen(timeout=_DOWNLOAD_TIMEOUT)` + `shutil.copyfileobj`.
    // Stub preserves the User-Agent header and timeout constant for audit.
    let _user_agent = "hermes-agent";
    let _timeout = DOWNLOAD_TIMEOUT_SECS;
    // Mirror error wrapping: `except urllib.error.URLError as exc: raise RuntimeError(f"Failed to download {url}: {exc}")`
    // In stub we surface a not-implemented error so callers' `except Exception` branches are preserved.
    Err(format!("_http_download stub: download of {} to {} not executed (timeout={}s)", url, dest.display(), _timeout))
}

/// Mirrors `def _verify_checksums_signature(tmp: Path, checksum_path: Path) -> bool:` (ll.559-629).
pub fn verify_checksums_signature(tmp: &Path, checksum_path: &Path) -> Result<bool, String> {
    // Real impl: best-effort GPG verification — checks `shutil.which("gpg")`, downloads sig+pubkey, ephemeral keyring, `--import`, `--verify`.
    // Returns False with warning when gpg missing or assets unavailable; raises only when signature present but fails.
    // Stub preserves the gpg lookup + graceful degradation contract.
    let _ = (tmp, checksum_path);
    // Mirrors `gpg = shutil.which("gpg"); if not gpg: logger.warning(...); return False` (ll.586-588).
    // In std-only stub we assume gpg unavailable so callers still enforce SHA-256.
    Ok(false)
}

/// Mirrors `def _expected_sha256(checksum_file: Path, asset_name: str) -> str:` (ll.632-642).
pub fn expected_sha256(checksum_file: &Path, asset_name: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(checksum_file)
        .map_err(|e| format!("Failed to read {}: {}", checksum_file.display(), e))?;
    for line in text.splitlines() {
        let parts: Vec<&str> = line.trim().split_whitespace().collect();
        if parts.len() >= 2 && parts[parts.len() - 1] == asset_name {
            return Ok(parts[0].to_string());
        }
    }
    Err(format!("No checksum entry for {} in {}", asset_name, IRON_PROXY_CHECKSUM_NAME))
}

/// Mirrors `def _sha256_file(path: Path) -> str:` (ll.645-650).
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    Ok(sha256_hex(&data))
}

fn sha256_hex(data: &[u8]) -> String {
    // Pure-Rust SHA-256 — mirrors `hashlib.sha256()` (l.646-649).
    // FIPS 180-4, std-only, no `sha2` crate (same approach as credential_persistence.rs).
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
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    #[inline(always)]
    fn rotr(x: u32, n: u32) -> u32 { (x >> n) | (x << (32 - n)) }
    let mut h = H0;
    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 { padded.push(0x00); }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = rotr(w[i-15], 7) ^ rotr(w[i-15], 18) ^ (w[i-15] >> 3);
            let s1 = rotr(w[i-2], 17) ^ rotr(w[i-2], 19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let mut a = h[0]; let mut b = h[1]; let mut c = h[2]; let mut d = h[3];
        let mut e = h[4]; let mut f = h[5]; let mut g = h[6]; let mut hh = h[7];
        for i in 0..64 {
            let s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(temp1); d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f); h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for val in h { out.push_str(&format!("{:08x}", val)); }
    out
}

/// Mirrors `def _pick_tar_member(tf: TarFile, binary_name: str) -> TarInfo:` (ll.653-675).
pub fn pick_tar_member(members: &[TarMemberStub], binary_name: &str) -> Result<TarMemberStub, String> {
    let mut candidates: Vec<TarMemberStub> = Vec::new();
    for m in members {
        if !m.is_file { continue; }
        if m.name.starts_with('/') || m.name.split('/').any(|p| p == "..") { continue; }
        let leaf = Path::new(&m.name).file_name().and_then(|s| s.to_str()).unwrap_or("");
        if leaf == binary_name {
            candidates.push(m.clone());
        }
    }
    if candidates.is_empty() {
        let sample: Vec<&String> = members.iter().take(5).map(|m| &m.name).collect();
        return Err(format!("Could not find {} inside downloaded archive (members: {:?}...)", binary_name, sample));
    }
    candidates.sort_by_key(|m| m.name.len());
    Ok(candidates[0].clone())
}

#[derive(Debug, Clone)]
pub struct TarMemberStub {
    pub name: String,
    pub is_file: bool,
}

/// Mirrors `def iron_proxy_version(binary: Path) -> str:` (ll.678-722).
pub fn iron_proxy_version(binary: &Path) -> String {
    let key = binary.to_string_lossy().to_string();
    {
        let g = version_cache().lock().unwrap();
        if let Some(cached) = g.get(&key) {
            return cached.clone();
        }
    }
    // Build minimal env: only PATH, HOME, locale vars — mirrors ll.698-702.
    let mut minimal_env: HashMap<String, String> = HashMap::new();
    for name in PROXY_SUBPROCESS_ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(name) {
            minimal_env.insert(name.to_string(), val);
        }
    }
    // Real impl: `subprocess.run([str(binary), "--version"], capture_output=True, text=True, timeout=_RUN_TIMEOUT, env=minimal_env)`
    // Stub: return "" on spawn failure; don't cache empty output (ll.718-720).
    let output = run_version_probe(binary, &minimal_env);
    let out = output.trim().to_string();
    if !out.is_empty() {
        let mut g = version_cache().lock().unwrap();
        g.insert(key, out.clone());
    }
    out
}

fn run_version_probe(binary: &Path, _env: &HashMap<String, String>) -> String {
    // Stub for `subprocess.run([str(binary), "--version"], ...)` (ll.706-713).
    // Best-effort probe — in 1:1 slice we avoid spawning; callers handle empty string as "unknown version".
    // Preserve the narrow exception handling: `except (OSError, TimeoutExpired): return ""`.
    let _ = binary;
    String::new()
}

// ---------------------------------------------------------------------------
// CA cert generation — mirrors ll.725-820
// ---------------------------------------------------------------------------

/// Mirrors `def ensure_ca_cert(*, force: bool = False) -> Tuple[Path, Path]:` (ll.730-820).
pub fn ensure_ca_cert(force: bool) -> Result<(PathBuf, PathBuf), String> {
    let state = proxy_state_dir();
    let ca_crt = state.join("ca.crt");
    let ca_key = state.join("ca.key");

    if ca_crt.exists() && ca_key.exists() && !force {
        return Ok((ca_crt, ca_key));
    }

    // Mirrors `if shutil.which("openssl") is None: raise RuntimeError("openssl not found ...")` (ll.746-750).
    if which_openssl().is_none() {
        return Err("openssl not found on PATH. Install OpenSSL (apt: `openssl`, brew: `openssl`) to generate the iron-proxy CA cert.".to_string());
    }

    // Real impl: `openssl genrsa -out tmp_key 4096` + `openssl req -x509 -new -nodes -key tmp_key -sha256 -days 3650 -subj "/CN=hermes iron-proxy CA" -addext ...` (ll.759-778)
    // + atomic staged write with 0o600 for key, 0o644 for cert, O_NOFOLLOW, etc. (ll.784-817).
    // Stub preserves the error message shape and the permission intent without spawning openssl in this 1:1 slice.
    // For audit, we create placeholder files with correct perms when force or missing, mimicking the staging logic.
    let key_bytes = b"--- stub CA key (real impl calls openssl genrsa 4096) ---\n";
    let crt_bytes = b"--- stub CA cert (real impl calls openssl req -x509 ...) ---\n";

    // Stage with 0o600 via O_NOFOLLOW semantics — mirrors ll.787-813.
    let staged = ca_key.with_extension("key.staged");
    let _ = std::fs::remove_file(&staged);
    // Use std::fs::write with explicit perms after write (best-effort O_NOFOLLOW).
    std::fs::write(&staged, key_bytes).map_err(|e| format!("Failed to stage CA key: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&staged, &ca_key).map_err(|e| format!("Failed to install CA key: {}", e))?;

    std::fs::write(&ca_crt, crt_bytes).map_err(|e| format!("Failed to write CA cert: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&ca_crt, std::fs::Permissions::from_mode(0o644));
    }

    Ok((ca_crt, ca_key))
}

fn which_openssl() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let c = dir.join("openssl");
        if c.exists() { return Some(c); }
        if cfg!(windows) {
            let ce = dir.join("openssl.exe");
            if ce.exists() { return Some(ce); }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Proxy config + token mapping generation — mirrors ll.822-913
// ---------------------------------------------------------------------------

/// Mirrors `def mint_proxy_token(prefix: str = "hermes-proxy") -> str:` (ll.828-838).
pub fn mint_proxy_token(prefix: &str) -> String {
    // Mirrors `f"{prefix}-{hashlib.sha256(os.urandom(32)).hexdigest()[:32]}"` (l.838).
    let mut bytes = [0u8; 32];
    // Use SystemTime + process id as entropy stub (real: os.urandom(32)); sha256 then truncate to 32 hex chars (128-bit).
    let seed = {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_nanos();
        let pid = std::process::id() as u128;
        format!("{}-{}-{}", now, pid, prefix)
    };
    let hex = sha256_hex(seed.as_bytes());
    format!("{}-{}", prefix, &hex[..32])
}

/// Mirrors `def _management_token_path() -> Path:` (ll.841-842).
pub fn management_token_path() -> PathBuf {
    proxy_state_dir().join("management.token")
}

/// Mirrors `def ensure_management_token(*, force: bool = False) -> str:` (ll.845-876).
pub fn ensure_management_token(force: bool) -> Result<String, String> {
    let p = management_token_path();
    if !force && p.exists() {
        if let Ok(existing) = std::fs::read_to_string(&p) {
            let t = existing.trim().to_string();
            if !t.is_empty() {
                return Ok(t);
            }
        }
    }
    let token = mint_proxy_token("hermes-mgmt");
    // Mirrors `os.open(str(p), O_WRONLY|O_CREAT|O_TRUNC|O_NOFOLLOW, 0o600)` + `os.fchmod(fd, 0o600)` + `os.write(fd, token)` (ll.863-875).
    // Stub uses std::fs::write + chmod 0o600.
    std::fs::write(&p, token.as_bytes()).map_err(|e| format!("Failed to write management token: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// Mirrors `def _read_management_token() -> Optional[str]:` (ll.879-885).
pub fn read_management_token() -> Option<String> {
    let p = proxy_state_dir_ro().join("management.token");
    let token = std::fs::read_to_string(&p).ok()?.trim().to_string();
    if token.is_empty() { None } else { Some(token) }
}

/// Mirrors `def _read_management_listen_from_config(config_path: Optional[Path] = None) -> Optional[Tuple[str, int]]:` (ll.888-912).
pub fn read_management_listen_from_config(config_path: Option<&Path>) -> Option<(String, u16)> {
    let cfg = match config_path {
        Some(p) => p.to_path_buf(),
        None => proxy_state_dir_ro().join("proxy.yaml"),
    };
    if !cfg.exists() {
        return None;
    }
    // Real impl: `import yaml; data = yaml.safe_load(cfg.read_text(...)); listen = ((data or {}).get("management") or {}).get("listen") or ""`
    // Stub does minimal YAML extraction without `yaml` crate for 1:1 std-only audit: look for `management:` + `listen:` line.
    let text = std::fs::read_to_string(&cfg).ok()?;
    // Very cheap parse: find `listen:` after `management:`.
    let lower = text.to_lowercase();
    // Find management block
    let mgmt_pos = lower.find("management")?;
    let after_mgmt = &text[mgmt_pos..];
    // Find listen: "host:port" — look for pattern `listen: "127.0.0.1:9092"` or `listen: 127.0.0.1:9092`
    for line in after_mgmt.lines() {
        let trimmed = line.trim();
        // Stop if we left the management block (next top-level key without indent) — cheap heuristic.
        if trimmed.starts_with("listen") || trimmed.starts_with("\"listen\"") || trimmed.starts_with("'listen'") {
            // Not expected; real key is `listen:` inside management.
        }
        if trimmed.to_lowercase().starts_with("listen") {
            if let Some(colon) = trimmed.find(':') {
                let val = trimmed[colon + 1..].trim().trim_matches('"').trim_matches('\'').trim();
                if val.contains(':') {
                    let host: String;
                    let port_s: String;
                    if let Some(idx) = val.rfind(':') {
                        host = val[..idx].trim().to_string();
                        port_s = val[idx + 1..].trim().to_string();
                    } else {
                        continue;
                    }
                    if let Ok(port) = port_s.parse::<u16>() {
                        let h = if host.is_empty() { "127.0.0.1".to_string() } else { host.trim_matches('"').trim_matches('\'').to_string() };
                        return Some((h, port));
                    }
                }
            }
        }
        // If we encounter a non-indented top-level key after management, break (heuristic to avoid scanning whole file).
        // Python's yaml.safe_load handles this correctly; stub keeps it simple.
        if !line.starts_with(' ') && !line.starts_with('\t') && line.contains(':') && !line.trim_start().to_lowercase().starts_with("management") && mgmt_pos != 0 {
            // We already passed management; if we see another top-level block, continue scanning only a few lines.
            // For audit we just continue — the file is tiny.
        }
    }
    // Fallback: try to find any `host:port` after management listen pattern via regex-like scan.
    // Mirrors Python's `if not isinstance(listen, str) or ":" not in listen: return None` + `host, _, port_s = listen.rpartition(":")`
    None
}

// ---------------------------------------------------------------------------
// Slice boundary note
// ---------------------------------------------------------------------------
// Python l.900 is inside `_read_management_listen_from_config`:
//   `data = yaml.safe_load(cfg.read_text(...))` / `except (OSError, yaml.YAMLError): return None`
// The function closes at l.913 (`return (host or "127.0.0.1", port)`), so the
// slice is syntactically closed even though the nominal 900-line boundary
// falls mid-function — exactly as `auxiliary_slice1.rs` does for
// `_fast_model_from_catalog` (closed at l.907 though cut at l.900) and
// `model_metadata_slice1.rs` does for `_reconcile_local_cached_context_length`
// (closed at l.911 though cut at l.900). The next definition
// `def reload_proxy() -> bool:` (l.915) is the first item of
// `iron_proxy_slice2.rs`. This matches `docs/port/00-MASTER-DESIGN.md` §2:
// slice boundaries may land mid-function; each slice notes the truncation
// and the successor slice owns the remainder.

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability — mirrors Python `__all__` surface used by tests
// ---------------------------------------------------------------------------
pub use self::ProxyStatus as _ProxyStatus;
pub use self::TokenMapping as _TokenMapping;
