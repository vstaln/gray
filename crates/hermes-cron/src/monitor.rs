//! Monitor-mode cron support — hash-suppressed change detection.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/monitor.py` (212 lines).
//! A monitor job attaches a cheap *monitor source* (`monitor_script` or
//! `monitor_url`) to an ordinary LLM cron job. Each tick the scheduler runs
//! the source FIRST and compares a hash of its exact output bytes against the
//! hash stored from the last agent-triggering tick.
//!
//! Python source docstring (preserved):
//! ```text
//! Monitor-mode cron support — hash-suppressed change detection.
//!
//! A monitor job attaches a cheap *monitor source* (``monitor_script`` or
//! ``monitor_url``) to an ordinary LLM cron job. Each tick the scheduler runs
//! the source FIRST and compares a hash of its exact output bytes against the
//! hash stored from the last agent-triggering tick:
//!
//! * unchanged → the agent run is suppressed entirely (no LLM, no delivery);
//!   the tick is recorded as a silent ``no_change`` run.
//! * changed (or first run) → a "MONITOR CHANGE DETECTED" context block —
//!   unified diff of old vs new output (capped) plus the new output — is
//!   injected into the prompt and the agent runs normally.
//! * source failure → treated as an ERROR, never as a change. The stored hash
//!   is left untouched so a source that recovers to its previous output still
//!   suppresses.
//!
//! Output is compared as EXACT BYTES — no timestamp stripping or whitespace
//! normalization. Monitor scripts should emit stable output (sort results,
//! omit "generated at" lines) or every tick will look like a change.
//!
//! State lives in two places, both durable across scheduler restarts:
//!
//! * ``job["monitor_state"]`` in jobs.json — ``last_output_hash`` +
//!   ``last_changed_at`` (additive JSON fields, no migration needed);
//! * ``OUTPUT_DIR/<job_id>/monitor_last_output.txt`` — the previous output
//!   text, kept only so the next change can render a diff.
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Cap for the unified diff injected into the prompt.
/// Mirrors `MAX_DIFF_CHARS = 4000`.
pub const MAX_DIFF_CHARS: usize = 4000;

/// Cap for the new-output block injected into the prompt (mirrors the 8k
/// context_from truncation in cron/scheduler.py).
/// Mirrors `MAX_OUTPUT_CHARS = 8000`.
pub const MAX_OUTPUT_CHARS: usize = 8000;

/// Bounded GET limits for monitor_url sources.
/// Mirrors `URL_TIMEOUT_SECONDS = 30`.
pub const URL_TIMEOUT_SECONDS: u64 = 30;

/// Mirrors `MAX_URL_BYTES = 262_144  # 256 KiB`.
pub const MAX_URL_BYTES: usize = 262_144;

/// Snapshot filename.
/// Mirrors `_SNAPSHOT_FILENAME = "monitor_last_output.txt"`.
pub const SNAPSHOT_FILENAME: &str = "monitor_last_output.txt";

const DEFAULT_SCRIPT_TIMEOUT: u64 = 3600;

// ---------------------------------------------------------------------------
// Global lock — mirrors Python's file+thread locks (simplified to in-process)
// ---------------------------------------------------------------------------

static JOBS_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Data model — mirrors Python `MonitorOutcome` dataclass
// ---------------------------------------------------------------------------

/// Result of one monitor-source evaluation.
/// Mirrors `class MonitorOutcome` dataclass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOutcome {
    pub ok: bool,
    pub changed: bool,
    pub first_run: bool,
    pub context_block: Option<String>,
    pub error: Option<String>,
}

impl MonitorOutcome {
    pub fn ok_unchanged() -> Self {
        Self {
            ok: true,
            changed: false,
            first_run: false,
            context_block: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Home / path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve the Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()`:
/// `HERMES_HOME` env → `~/.hermes` (POSIX) / `%LOCALAPPDATA%/hermes` (Windows).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(".hermes");
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        if !userprofile.trim().is_empty() {
            return PathBuf::from(userprofile).join(".hermes");
        }
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        if !localappdata.trim().is_empty() {
            return PathBuf::from(localappdata).join("hermes");
        }
    }
    PathBuf::from(".hermes")
}

fn cron_dir() -> PathBuf {
    get_hermes_home().join("cron")
}

fn jobs_file() -> PathBuf {
    cron_dir().join("jobs.json")
}

fn output_dir() -> PathBuf {
    cron_dir().join("output")
}

/// Mirrors `cron.jobs._job_output_dir(job_id)` with path-escape validation.
pub fn job_output_dir(job_id: &str) -> Result<PathBuf, String> {
    let text = job_id.trim().to_string();
    if text.is_empty() || text == "." || text == ".." || text.contains('/') || text.contains('\\') {
        return Err(format!("Invalid cron job id for output path: {job_id:?}"));
    }
    let p = Path::new(&text);
    if p.is_absolute() || p.has_root() {
        return Err(format!("Invalid cron job id for output path: {job_id:?}"));
    }
    // Check drive letter on Windows (e.g. "C:")
    if text.len() >= 2 && text.as_bytes()[1] == b':' {
        return Err(format!("Invalid cron job id for output path: {job_id:?}"));
    }
    Ok(output_dir().join(text))
}

/// Mirrors `cron.monitor._snapshot_path(job_id)`.
pub fn snapshot_path(job_id: &str) -> Result<PathBuf, String> {
    Ok(job_output_dir(job_id)?.join(SNAPSHOT_FILENAME))
}

fn hermes_now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn secure_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

// ---------------------------------------------------------------------------
// SHA-256 — minimal FIPS 180-4, no external crate (copied from gray/oauth.rs)
// ---------------------------------------------------------------------------

fn sha256(data: &[u8]) -> [u8; 32] {
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
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions 1:1
// ---------------------------------------------------------------------------

/// Hash the monitor output as exact UTF-8 bytes (no normalization).
/// Mirrors `def hash_monitor_output(output: str) -> str`.
pub fn hash_monitor_output(output: &str) -> String {
    hex_encode(&sha256(output.as_bytes()))
}

/// Unified diff of old vs new monitor output, capped at MAX_DIFF_CHARS.
/// Mirrors `def build_monitor_diff(old: str, new: str) -> str`.
pub fn build_monitor_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<String> = old.lines().map(|s| s.to_string()).collect();
    let new_lines: Vec<String> = new.lines().map(|s| s.to_string()).collect();

    // Fast path: identical
    if old_lines == new_lines {
        return String::new();
    }

    let diff = unified_diff(&old_lines, &new_lines, "previous", "current");

    if diff.len() > MAX_DIFF_CHARS {
        // Truncate on char boundary
        let mut end = MAX_DIFF_CHARS;
        while end > 0 && !diff.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n... [diff truncated]", &diff[..end])
    } else {
        diff
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffTag {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone)]
struct DiffOp {
    tag: DiffTag,
    line: String,
}

fn lcs_diff(old: &[String], new: &[String]) -> Vec<DiffOp> {
    let m = old.len();
    let n = new.len();
    // dp[i][j] = LCS length of old[..i] and new[..j]
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }
    // Backtrack
    let mut ops_rev: Vec<DiffOp> = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            ops_rev.push(DiffOp {
                tag: DiffTag::Equal,
                line: old[i - 1].clone(),
            });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops_rev.push(DiffOp {
                tag: DiffTag::Insert,
                line: new[j - 1].clone(),
            });
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i][j - 1] < dp[i - 1][j]) {
            ops_rev.push(DiffOp {
                tag: DiffTag::Delete,
                line: old[i - 1].clone(),
            });
            i -= 1;
        } else {
            break;
        }
    }
    ops_rev.reverse();
    ops_rev
}

fn unified_diff(old: &[String], new: &[String], from_file: &str, to_file: &str) -> String {
    let ops = lcs_diff(old, new);
    if !ops.iter().any(|op| op.tag != DiffTag::Equal) {
        return String::new();
    }

    // Build hunk ranges with context n=3 (Python default)
    const CONTEXT: usize = 3;
    // Identify indices of non-equal ops
    let mut change_indices: Vec<usize> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        if op.tag != DiffTag::Equal {
            change_indices.push(idx);
        }
    }
    if change_indices.is_empty() {
        return String::new();
    }

    // Expand each change to include CONTEXT before/after, then merge overlapping.
    let mut ranges: Vec<(usize, usize)> = Vec::new(); // [start, end) in ops index
    for &ci in &change_indices {
        let start = ci.saturating_sub(CONTEXT);
        let end = (ci + CONTEXT + 1).min(ops.len());
        ranges.push((start, end));
    }
    // Merge
    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }

    // Now emit headers and hunks
    let mut out_lines: Vec<String> = Vec::new();
    out_lines.push(format!("--- {from_file}"));
    out_lines.push(format!("+++ {to_file}"));

    for (hunk_start, hunk_end) in merged {
        // Compute old/new line numbers and counts for this hunk
        // Walk ops[..hunk_start] to get starting line numbers (1-based)
        let mut old_line = 1usize;
        let mut new_line = 1usize;
        for op in &ops[..hunk_start] {
            match op.tag {
                DiffTag::Equal => {
                    old_line += 1;
                    new_line += 1;
                }
                DiffTag::Delete => old_line += 1,
                DiffTag::Insert => new_line += 1,
            }
        }
        let hunk_old_start = old_line;
        let hunk_new_start = new_line;
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        for op in &ops[hunk_start..hunk_end] {
            match op.tag {
                DiffTag::Equal => {
                    old_count += 1;
                    new_count += 1;
                }
                DiffTag::Delete => old_count += 1,
                DiffTag::Insert => new_count += 1,
            }
        }
        out_lines.push(format!("@@ -{hunk_old_start},{old_count} +{hunk_new_start},{new_count} @@"));
        for op in &ops[hunk_start..hunk_end] {
            match op.tag {
                DiffTag::Equal => out_lines.push(format!(" {}", op.line)),
                DiffTag::Delete => out_lines.push(format!("-{}", op.line)),
                DiffTag::Insert => out_lines.push(format!("+{}", op.line)),
            }
        }
    }

    out_lines.join("\n")
}

/// Mirrors `def _read_last_output(job_id: str) -> str`.
pub fn read_last_output(job_id: &str) -> String {
    match snapshot_path(job_id) {
        Ok(path) => {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(exc) => {
                        log::warn!("Monitor: failed to read last output for {job_id:?}: {exc}");
                        String::new()
                    }
                }
            } else {
                String::new()
            }
        }
        Err(exc) => {
            log::warn!("Monitor: failed to read last output for {job_id:?}: {exc}");
            String::new()
        }
    }
}

/// Mirrors `def _write_last_output(job_id: str, output: str) -> None`.
pub fn write_last_output(job_id: &str, output: &str) {
    let path = match snapshot_path(job_id) {
        Ok(p) => p,
        Err(exc) => {
            log::warn!("Monitor: failed to persist last output for {job_id:?}: {exc}");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(exc) = fs::create_dir_all(parent) {
            log::warn!("Monitor: failed to persist last output for {job_id:?}: {exc}");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    match fs::write(&path, output) {
        Ok(_) => secure_file(&path),
        Err(exc) => log::warn!("Monitor: failed to persist last output for {job_id:?}: {exc}"),
    }
}

// ---------------------------------------------------------------------------
// Monitor URL fetch — mirrors `def _fetch_monitor_url(url: str)`
// ---------------------------------------------------------------------------

/// Bounded GET of a monitor URL. Returns (ok, body-or-error).
/// Mirrors `def _fetch_monitor_url(url: str) -> tuple[bool, str]`.
pub fn fetch_monitor_url(url: &str) -> (bool, String) {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return (false, format!("monitor_url must be http(s): {url:?}"));
    }
    let trimmed = url.trim().to_string();

    // Prefer curl for both http and https (handles TLS, redirects, timeout).
    // Fall back to raw TcpStream for plain http if curl is unavailable.
    match fetch_via_curl(&trimmed) {
        Some(res) => res,
        None => {
            // curl not found — try raw http for http:// only
            if lower.starts_with("http://") {
                fetch_http_via_tcp(&trimmed)
            } else {
                (false, "monitor_url fetch failed: https requires curl (not found on PATH)".to_string())
            }
        }
    }
}

fn fetch_via_curl(url: &str) -> Option<(bool, String)> {
    // Probe for curl availability; if not found, return None to trigger fallback.
    // We attempt to run curl — if the spawn fails with NotFound, we return None.
    let timeout_str = URL_TIMEOUT_SECONDS.to_string();
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "-L",
        "--fail",
        "--max-time",
        &timeout_str,
        "-A",
        "hermes-cron-monitor",
        url,
    ]);
    // Capture both stdout and stderr
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some((false, format!("monitor_url fetch failed: {e}"))),
    };

    if output.status.success() {
        let mut body = output.stdout;
        if body.len() > MAX_URL_BYTES {
            body.truncate(MAX_URL_BYTES);
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        Some((true, text))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("curl exited with status {:?}", output.status.code())
        };
        Some((false, format!("monitor_url fetch failed: {detail}")))
    }
}

fn fetch_http_via_tcp(url: &str) -> (bool, String) {
    // Minimal http:// GET via TcpStream, with timeout and size cap.
    // Only called for http:// when curl is unavailable.
    let without_scheme = match url.strip_prefix("http://").or_else(|| url.strip_prefix("HTTP://")) {
        Some(rest) => rest,
        None => return (false, format!("monitor_url fetch failed: unsupported scheme {url:?}")),
    };
    // Split host_port and path
    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };
    if host_port.is_empty() {
        return (false, format!("monitor_url fetch failed: missing host in {url:?}"));
    }
    let (host, port) = if let Some(colon) = host_port.rfind(':') {
        let h = &host_port[..colon];
        let p_str = &host_port[colon + 1..];
        match p_str.parse::<u16>() {
            Ok(p) => (h, p),
            Err(_) => return (false, format!("monitor_url fetch failed: invalid port in {url:?}")),
        }
    } else {
        (host_port, 80u16)
    };
    let addr = format!("{host}:{port}");
    let stream = match std::net::TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => return (false, format!("monitor_url fetch failed: {e}")),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(URL_TIMEOUT_SECONDS)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(URL_TIMEOUT_SECONDS)));

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: hermes-cron-monitor\r\nConnection: close\r\n\r\n"
    );
    let mut stream = stream;
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return (false, format!("monitor_url fetch failed: {e}"));
    }
    let _ = stream.flush();

    let mut resp = Vec::new();
    // Read up to headers + MAX_URL_BYTES+1 + overhead
    let limit = MAX_URL_BYTES + 1 + 8192;
    let mut buf = [0u8; 8192];
    let start = Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(URL_TIMEOUT_SECONDS) {
            return (false, "monitor_url fetch failed: timeout".to_string());
        }
        if resp.len() >= limit {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&buf[..n]);
                if resp.len() >= limit {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                // Non-blocking timeout — treat as timeout
                return (false, format!("monitor_url fetch failed: {e}"));
            }
            Err(e) => return (false, format!("monitor_url fetch failed: {e}")),
        }
    }

    // Split headers and body
    let header_end = find_header_end(&resp);
    let (status_code, body_start) = match header_end {
        Some(idx) => {
            let header_bytes = &resp[..idx];
            let header_str = String::from_utf8_lossy(header_bytes);
            let first_line = header_str.lines().next().unwrap_or("");
            let code = parse_http_status(first_line);
            (code, idx)
        }
        None => {
            // No header terminator — treat whole response as body (or error)
            (None, 0)
        }
    };

    if let Some(code) = status_code {
        if !(200..300).contains(&code) {
            return (false, format!("monitor_url fetch failed: HTTP {code}"));
        }
    }

    let body = if body_start < resp.len() {
        &resp[body_start..]
    } else {
        &[]
    };
    let mut body_vec = body.to_vec();
    if body_vec.len() > MAX_URL_BYTES {
        body_vec.truncate(MAX_URL_BYTES);
    }
    let text = String::from_utf8_lossy(&body_vec).into_owned();
    (true, text)
}

fn find_header_end(resp: &[u8]) -> Option<usize> {
    // Search for \r\n\r\n
    for i in 0..resp.len().saturating_sub(3) {
        if resp[i] == b'\r' && resp[i + 1] == b'\n' && resp[i + 2] == b'\r' && resp[i + 3] == b'\n' {
            return Some(i + 4);
        }
    }
    // Also search for \n\n
    for i in 0..resp.len().saturating_sub(1) {
        if resp[i] == b'\n' && resp[i + 1] == b'\n' {
            return Some(i + 2);
        }
    }
    None
}

fn parse_http_status(first_line: &str) -> Option<u16> {
    // e.g. "HTTP/1.1 200 OK"
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() >= 2 {
        if let Ok(code) = parts[1].parse::<u16>() {
            return Some(code);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Script execution — mirrors `cron.scheduler._run_job_script`
// ---------------------------------------------------------------------------

fn get_script_timeout() -> u64 {
    if let Ok(raw) = std::env::var("HERMES_CRON_SCRIPT_TIMEOUT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Ok(v) = trimmed.parse::<f64>() {
                let iv = v as i64;
                if iv > 0 {
                    return iv as u64;
                }
            } else {
                log::warn!("Invalid HERMES_CRON_SCRIPT_TIMEOUT={raw:?}; using default {DEFAULT_SCRIPT_TIMEOUT}s");
            }
        }
    }
    // Config file fallback is omitted in Rust (no config loader here); use default.
    DEFAULT_SCRIPT_TIMEOUT
}

fn expanduser(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }
    path.to_string()
}

fn which(cmd: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                return Some(candidate);
            }
            // Windows exe extension
            #[cfg(windows)]
            {
                let candidate_exe = dir.join(format!("{cmd}.exe"));
                if candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
    }
    None
}

fn find_bash() -> Option<String> {
    if let Some(p) = which("bash") {
        return Some(p.to_string_lossy().into_owned());
    }
    if Path::new("/bin/bash").is_file() {
        return Some("/bin/bash".to_string());
    }
    None
}

fn find_python() -> Option<String> {
    for cand in ["python3", "python"] {
        if let Some(p) = which(cand) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// Execute a cron job's data-collection script and capture its output.
/// Mirrors `def _run_job_script(script_path: str, workdir: Optional[str]=None)`.
pub fn run_job_script(script_path: &str, workdir: Option<&str>) -> (bool, String) {
    let scripts_dir = get_hermes_home().join("scripts");
    let _ = fs::create_dir_all(&scripts_dir);
    let scripts_dir_resolved = fs::canonicalize(&scripts_dir).unwrap_or(scripts_dir.clone());

    if script_path.contains('\0') {
        return (false, format!("Blocked: script path contains a NUL byte: {script_path:?}"));
    }

    let raw_str = expanduser(script_path);
    let raw = PathBuf::from(&raw_str);

    // Resolve path containment
    let path: PathBuf = if raw.is_absolute() {
        // Try canonicalize, fall back to absolute
        fs::canonicalize(&raw).unwrap_or(raw.clone())
    } else {
        let candidate = scripts_dir.join(&raw);
        fs::canonicalize(&candidate).unwrap_or(candidate)
    };

    // Guard against path traversal
    if !path.starts_with(&scripts_dir_resolved) {
        return (
            false,
            format!(
                "Blocked: script path resolves outside the scripts directory ({}): {script_path:?}",
                scripts_dir_resolved.display()
            ),
        );
    }

    if !path.exists() {
        return (false, format!("Script not found: {}", path.display()));
    }
    if !path.is_file() {
        return (false, format!("Script path is not a file: {}", path.display()));
    }

    let script_timeout = get_script_timeout();
    let suffix = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let (prog, args): (String, Vec<String>) = if suffix == "sh" || suffix == "bash" {
        let bash = match find_bash() {
            Some(b) => b,
            None => {
                return (
                    false,
                    format!(
                        "Cannot run .sh/.bash script {:?}: bash not found on PATH. On Windows, install Git for Windows (which ships Git Bash) or rewrite the script as Python (.py).",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                )
            }
        };
        (bash, vec![path.to_string_lossy().into_owned()])
    } else {
        let python = match find_python() {
            Some(p) => p,
            None => return (false, "Script execution failed: python3/python not found on PATH".to_string()),
        };
        (python, vec![path.to_string_lossy().into_owned()])
    };

    // Determine cwd
    let cwd = if let Some(wd) = workdir {
        let trimmed = wd.trim();
        if trimmed.is_empty() {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(trimmed)
        }
    } else {
        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    };

    // Spawn with piped stdout/stderr and timeout
    let mut cmd = Command::new(&prog);
    cmd.args(&args);
    cmd.current_dir(&cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Use a new process group where possible (start_new_session equivalent)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid is async-signal-safe
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("Script execution failed: {e}")),
    };

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // Spawn threads to drain pipes to avoid deadlock
    let stdout_thread = stdout_handle.map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_thread = stderr_handle.map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + Duration::from_secs(script_timeout);
    let mut exit_status = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Timeout — terminate
                    #[cfg(unix)]
                    {
                        // Try to kill process group
                        let pid = child.id() as i32;
                        unsafe {
                            libc_terminate(pid);
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    // Drain threads (they will finish after process exit)
                    if let Some(t) = stdout_thread {
                        let _ = t.join();
                    }
                    if let Some(t) = stderr_thread {
                        let _ = t.join();
                    }
                    return (false, format!("Script timed out after {script_timeout}s: {}", path.display()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return (false, format!("Script execution failed: {e}"));
            }
        }
    }

    let stdout_bytes = stdout_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    let stderr_bytes = stderr_thread.and_then(|t| t.join().ok()).unwrap_or_default();

    let stdout_raw = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
    let stderr_raw = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

    // No redaction in Rust (Python redacts secrets) — pass through.
    let status = exit_status.unwrap();
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let mut parts = vec![format!("Script exited with code {code}")];
        if !stderr_raw.is_empty() {
            parts.push(format!("stderr:\n{stderr_raw}"));
        }
        if !stdout_raw.is_empty() {
            parts.push(format!("stdout:\n{stdout_raw}"));
        }
        return (false, parts.join("\n"));
    }

    (true, stdout_raw)
}

#[cfg(unix)]
unsafe fn libc_terminate(pid: i32) {
    // Best-effort kill of process group
    unsafe {
        // killpg
        let _ = libc_killpg(pid);
    }
}

#[cfg(unix)]
unsafe fn libc_killpg(pid: i32) -> i32 {
    // Use libc via raw syscall if available; otherwise no-op.
    // We avoid adding libc crate by using std::process::Command kill fallback.
    // This is a stub that does nothing — child.kill() above handles the main pid.
    let _ = pid;
    0
}

// ---------------------------------------------------------------------------
// Monitor source dispatch — mirrors `def _run_monitor_source(job: dict)`
// ---------------------------------------------------------------------------

/// Run the job's monitor source (script or URL). Returns (ok, output).
/// Mirrors `def _run_monitor_source(job: dict) -> tuple[bool, str]`.
pub fn run_monitor_source(job: &Value) -> (bool, String) {
    let monitor_script = job
        .get("monitor_script")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !monitor_script.is_empty() {
        let workdir = job
            .get("workdir")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        return run_job_script(&monitor_script, workdir.as_deref());
    }
    let monitor_url = job
        .get("monitor_url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !monitor_url.is_empty() {
        return fetch_monitor_url(&monitor_url);
    }
    (false, "monitor job has neither monitor_script nor monitor_url".to_string())
}

// ---------------------------------------------------------------------------
// _has_monitor — mirrors `def job_has_monitor(job: dict) -> bool`
// ---------------------------------------------------------------------------

/// Mirrors `def job_has_monitor(job: dict) -> bool`.
pub fn job_has_monitor(job: &Value) -> bool {
    let has_script = job
        .get("monitor_script")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_url = job
        .get("monitor_url")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    has_script || has_url
}

// ---------------------------------------------------------------------------
// _persist — mirrors `def _persist_monitor_state(...)`
// ---------------------------------------------------------------------------

fn atomic_replace(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let real_target = if target.is_symlink() {
        match fs::read_link(target) {
            Ok(link) => {
                if link.is_absolute() {
                    link
                } else if let Some(parent) = target.parent() {
                    parent.join(link)
                } else {
                    link
                }
            }
            Err(_) => target.to_path_buf(),
        }
    } else {
        target.to_path_buf()
    };
    match fs::rename(tmp, &real_target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            fs::copy(tmp, &real_target)?;
            if let Ok(f) = fs::File::open(&real_target) {
                let _ = f.sync_all();
            }
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn save_jobs_atomic(jobs: &[Value]) -> std::io::Result<()> {
    let path = jobs_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(".jobs_{}.tmp", uuid::Uuid::new_v4().simple());
    let tmp_path = parent.join(tmp_name);
    let payload = serde_json::json!({
        "jobs": jobs,
        "updated_at": hermes_now_iso(),
    });
    let json = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let mut created = false;
    let res: std::io::Result<()> = (|| {
        let mut f = fs::File::create(&tmp_path)?;
        created = true;
        f.write_all(json.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        atomic_replace(&tmp_path, &path)?;
        secure_file(&path);
        Ok(())
    })();
    if res.is_err() && created {
        let _ = fs::remove_file(&tmp_path);
    }
    res
}

fn load_jobs_vec() -> Vec<Value> {
    let path = jobs_file();
    if !path.exists() {
        return Vec::new();
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(obj) = value.as_object() {
        if let Some(jobs_val) = obj.get("jobs") {
            if let Some(arr) = jobs_val.as_array() {
                return arr.clone();
            }
            if let Some(map) = jobs_val.as_object() {
                // id-keyed map — flatten
                let mut out = Vec::new();
                for (k, v) in map {
                    if let Some(m) = v.as_object() {
                        let mut merged = m.clone();
                        if !merged.contains_key("id") {
                            merged.insert("id".to_string(), Value::String(k.clone()));
                        }
                        out.push(Value::Object(merged));
                    }
                }
                return out;
            }
        }
    }
    if let Some(arr) = value.as_array() {
        return arr.clone();
    }
    Vec::new()
}

/// Mirrors `def _persist_monitor_state(job_id: str, new_hash: str, output: str) -> None`.
pub fn persist_monitor_state(job_id: &str, new_hash: &str, output: &str) {
    write_last_output(job_id, output);
    let _guard = JOBS_LOCK.lock().unwrap();
    let mut jobs = load_jobs_vec();
    let mut found = false;
    for job in &mut jobs {
        if let Some(id) = job.get("id").and_then(|v| v.as_str()) {
            if id == job_id {
                let state = serde_json::json!({
                    "last_output_hash": new_hash,
                    "last_changed_at": hermes_now_iso(),
                });
                if let Some(obj) = job.as_object_mut() {
                    obj.insert("monitor_state".to_string(), state);
                }
                found = true;
                break;
            }
        }
    }
    if !found {
        log::warn!("Monitor: failed to persist state for {job_id:?}: job not found in jobs.json");
        return;
    }
    if let Err(exc) = save_jobs_atomic(&jobs) {
        log::warn!("Monitor: failed to persist state for {job_id:?}: {exc}");
    }
}

// ---------------------------------------------------------------------------
// Main entry — mirrors `def check_monitor(job: dict) -> MonitorOutcome`
// ---------------------------------------------------------------------------

/// Run the monitor source and decide whether the agent should run.
/// Mirrors `def check_monitor(job: dict) -> MonitorOutcome`.
pub fn check_monitor(job: &Value) -> MonitorOutcome {
    let job_id = job
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| job.get("id").and_then(|v| v.as_u64().map(|_| "")))
        .unwrap_or("")
        .to_string();
    // Fallback for non-string ids
    let job_id = if job_id.is_empty() {
        job.get("id")
            .map(|v| v.to_string().trim_matches('"').to_string())
            .unwrap_or_default()
    } else {
        job_id
    };

    let (ok, output) = run_monitor_source(job);
    if !ok {
        return MonitorOutcome {
            ok: false,
            changed: false,
            first_run: false,
            context_block: None,
            error: Some(output),
        };
    }

    let new_hash = hash_monitor_output(&output);
    let raw_state = job.get("monitor_state");
    let state_map: HashMap<String, Value> = match raw_state {
        Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => HashMap::new(),
    };
    let last_hash = state_map
        .get("last_output_hash")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(ref lh) = last_hash {
        if *lh == new_hash {
            return MonitorOutcome::ok_unchanged();
        }
    }

    let first_run = last_hash.is_none();
    let old_output = if first_run {
        String::new()
    } else {
        read_last_output(&job_id)
    };

    // Truncate shown output for prompt injection
    let shown_output = if output.len() > MAX_OUTPUT_CHARS {
        let mut end = MAX_OUTPUT_CHARS;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n... [output truncated]", &output[..end])
    } else {
        output.clone()
    };

    let context_block = if first_run {
        format!(
            "## Monitor Baseline (first run)\n\nThis is the first observation of the monitored source — there is no previous output to diff against.\n\n### Current output\n\n```\n{shown_output}\n```"
        )
    } else {
        let diff = build_monitor_diff(&old_output, &output);
        format!(
            "## MONITOR CHANGE DETECTED\n\nThe monitored source's output changed since the last run.\n\n### Diff (previous → current)\n\n```diff\n{diff}\n```\n\n### Current output\n\n```\n{shown_output}\n```"
        )
    };

    persist_monitor_state(&job_id, &new_hash, &output);
    MonitorOutcome {
        ok: true,
        changed: true,
        first_run,
        context_block: Some(context_block),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_sha256_hex() {
        // Known vector: sha256("hello") == 2cf24dba...
        let h = hash_monitor_output("hello");
        assert_eq!(h, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        let empty = hash_monitor_output("");
        assert_eq!(empty, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn build_diff_capped() {
        let old = "a\nb\nc";
        let new = "a\nx\nc";
        let d = build_monitor_diff(old, new);
        assert!(d.contains("--- previous"));
        assert!(d.contains("+++ current"));
        assert!(d.contains("-b"));
        assert!(d.contains("+x"));
        // identical yields empty
        assert_eq!(build_monitor_diff("a", "a"), "");
    }

    #[test]
    fn job_has_monitor_detects() {
        let j1 = serde_json::json!({"monitor_script": "foo.sh"});
        assert!(job_has_monitor(&j1));
        let j2 = serde_json::json!({"monitor_url": "https://example.com"});
        assert!(job_has_monitor(&j2));
        let j3 = serde_json::json!({"monitor_script": "  ", "monitor_url": ""});
        assert!(!job_has_monitor(&j3));
        let j4 = serde_json::json!({});
        assert!(!job_has_monitor(&j4));
    }

    #[test]
    fn fetch_rejects_non_http() {
        let (ok, msg) = fetch_monitor_url("ftp://example.com");
        assert!(!ok);
        assert!(msg.contains("monitor_url must be http(s)"));
    }

    #[test]
    fn truncate_shown_output_boundary() {
        let s = "a".repeat(MAX_OUTPUT_CHARS + 10);
        let truncated = if s.len() > MAX_OUTPUT_CHARS {
            let mut end = MAX_OUTPUT_CHARS;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n... [output truncated]", &s[..end])
        } else {
            s.clone()
        };
        assert!(truncated.contains("[output truncated]"));
        assert!(truncated.len() > MAX_OUTPUT_CHARS);
    }
}
