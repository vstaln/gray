# Supervision core (24/7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a platform-free supervision core (`gray-supervise`) so `gray gateway` runs always-on with restart contract, heartbeat, lifecycle, probe, hardened units, and rotating logs.

**Architecture:** New pure crate `crates/gray-supervise` (std + tokio + serde_json + anyhow + chrono + uuid only — no teloxide/twilight/slack-morphism). `gray-gateway` keeps all platform code behind existing features and calls into it for boot/shutdown/units/probe. `gray` (CLI) gains `gateway status --probe` and uses rotation on boot.

**Tech Stack:** Rust (edition 2024), tokio full, serde/serde_json, anyhow, chrono, uuid v4, fs2 (already in tree for flock; supervise itself uses std fs only).

**Spec:** `docs/superpowers/specs/2026-09-06-supervision-core-design.md` — the plan argues from the spec; executors read both.

## Global Constraints

- Offline builds stay green: `cargo test -p gray-gateway` (no features) must pass; real platform deps stay behind `telegram`, `discord`, `slack`, `all-platforms`.
- `gray-supervise` MUST NOT depend on teloxide / twilight-* / slack-morphism.
- No TCP port, no new background services; health is file-based only.
- Linux + macOS only in this cut; no Windows service, no Docker, no SQLite, no cron move.
- Only new env key: `GRAY_HEARTBEAT_SECS` (default 15). No new `gateway.yaml` keys.
- Secrets never logged; rotation keeps `logging.rs` redaction behavior.
- Repo flow: work on current rolling branch, never push to `main` directly; `main` needs PR + `test` check.

---

## File map

- Create: `crates/gray-supervise/Cargo.toml`
- Create: `crates/gray-supervise/src/lib.rs` (re-exports + `paths_for_home` + `heartbeat_interval_secs`)
- Create: `crates/gray-supervise/src/exit.rs` (`EXIT_CLEAN/RESTART/FATAL`)
- Create: `crates/gray-supervise/src/heartbeat.rs` (`write_heartbeat`, `heartbeat_age_secs`)
- Create: `crates/gray-supervise/src/lifecycle.rs` (`Lifecycle::{mark_boot,mark_clean,read}`)
- Create: `crates/gray-supervise/src/health.rs` (`probe` → `Health{healthy,reason}`)
- Create: `crates/gray-supervise/src/watchdog.rs` (`startup_timeout_secs`, `SHUTDOWN_DRAIN_SECS`)
- Create: `crates/gray-supervise/src/rotation.rs` (`LOG_MAX_BYTES`, `LOG_KEEP`, `rotate_if_needed`)
- Create: `crates/gray-supervise/src/units.rs` (`generate_systemd_unit`, `generate_launchd_plist`, `linger_hint`)
- Modify: `Cargo.toml` (workspace members + `gray-supervise` dep entry; NOT added to `default-members`, same as `gray-gateway`)
- Modify: `crates/gray-gateway/Cargo.toml` (add `gray-supervise = { workspace = true }`)
- Modify: `crates/gray-gateway/src/systemd.rs` (delegate generation to supervise, keep install/uninstall/status I/O, add `.bak` guard)
- Modify: `crates/gray-gateway/src/daemon_boot.rs` (lifecycle + heartbeat + startup watchdog + drain)
- Modify: `crates/gray-gateway/src/daemon.rs` (`/restart` exits 75)
- Modify: `crates/gray/src/lib.rs` (`GatewayCmd::Status { #[arg(long)] probe: bool }`)
- Modify: `crates/gray/src/main.rs` (`run_gateway` Status arm runs probe; fatal-config maps to exit 78)
- Modify: `crates/gray/src/logging.rs` (call rotation before open)
- Modify: `README.md` (Gateway section: `--probe`, heartbeat, units)

---

### Task 1: `gray-supervise` skeleton + exit codes + workspace wiring

**Files:**
- Create: `crates/gray-supervise/Cargo.toml`
- Create: `crates/gray-supervise/src/lib.rs`
- Create: `crates/gray-supervise/src/exit.rs`
- Modify: `Cargo.toml:3-15,34-46` (members + deps)
- Modify: `crates/gray-gateway/Cargo.toml:8-27` (add dep)

**Interfaces:**
- Consumes: nothing (new crate).
- Produces (used by Tasks 4–5): `gray_supervise::exit::{EXIT_CLEAN, EXIT_RESTART, EXIT_FATAL}` (`i32` consts `0/75/78`).

- [ ] **Step 1: Write the failing test** — create `crates/gray-supervise/src/exit.rs` containing ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn restart_and_fatal_codes_match_hermes_contract() {
        assert_eq!(super::EXIT_CLEAN, 0);
        assert_eq!(super::EXIT_RESTART, 75);
        assert_eq!(super::EXIT_FATAL, 78);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gray-supervise exit::` (from `/home/vstaln/gray`)
Expected: FAIL with "no external crate `gray-supervise`" / "package not found" (crate does not exist yet).

- [ ] **Step 3: Create the crate + wiring** — write these three files verbatim:

`crates/gray-supervise/Cargo.toml`:
```toml
[package]
name = "gray-supervise"
version.workspace = true
edition.workspace = true
description = "platform-free supervision core: restart contract, heartbeat, lifecycle, probe, units, rotation"
license.workspace = true

[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/gray-supervise/src/exit.rs`:
```rust
//! Restart contract (Hermes-compatible): 75 = restart me, 78 = fatal config.
pub const EXIT_CLEAN: i32 = 0;
pub const EXIT_RESTART: i32 = 75;
pub const EXIT_FATAL: i32 = 78;

#[cfg(test)]
mod tests {
    #[test]
    fn restart_and_fatal_codes_match_hermes_contract() {
        assert_eq!(super::EXIT_CLEAN, 0);
        assert_eq!(super::EXIT_RESTART, 75);
        assert_eq!(super::EXIT_FATAL, 78);
    }
}
```

`crates/gray-supervise/src/lib.rs`:
```rust
//! Platform-free supervision core (no telegram/discord/slack deps).
pub mod exit;
```

- [ ] **Step 4: Wire the workspace** — in `Cargo.toml`, add `"crates/gray-supervise"` to `members` (after `"crates/gray-session",`) and add `gray-supervise = { path = "crates/gray-supervise" }` to `[workspace.dependencies]` (after `gray-session = ...`). Do NOT touch `default-members`. In `crates/gray-gateway/Cargo.toml`, add `gray-supervise = { workspace = true }` after the `gray-plugin = { workspace = true }` line.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p gray-supervise` then `cargo check -p gray-gateway`
Expected: `1 passed`, then check green (dep unused so far — warning-free).

- [ ] **Step 6: Commit**

```bash
git add crates/gray-supervise Cargo.toml crates/gray-gateway/Cargo.toml
git commit -m "feat(supervise): new gray-supervise crate with exit-code contract"
```

---

### Task 2: Heartbeat + lifecycle + file probe

**Files:**
- Create: `crates/gray-supervise/src/heartbeat.rs`
- Create: `crates/gray-supervise/src/lifecycle.rs`
- Create: `crates/gray-supervise/src/health.rs`
- Modify: `crates/gray-supervise/src/lib.rs` (add the three modules + `paths_for_home` + `heartbeat_interval_secs`)

**Interfaces:**
- Consumes: `std::path::{Path, PathBuf}` only.
- Produces (used by Task 5): `gray_supervise::{paths_for_home, heartbeat_interval_secs} + heartbeat::{write_heartbeat, heartbeat_age_secs} + lifecycle::Lifecycle + health::{Health, probe}` with these exact signatures:
```rust
pub fn paths_for_home(home: &Path) -> (PathBuf /*state_dir*/, PathBuf /*heartbeat*/, PathBuf /*lifecycle*/);
pub fn heartbeat_interval_secs() -> u64; // GRAY_HEARTBEAT_SECS, default 15, min 5
pub fn write_heartbeat(home: &Path) -> anyhow::Result<()>;
pub fn heartbeat_age_secs(home: &Path) -> Option<u64>; // None when missing/unparseable
pub struct Lifecycle { pub boot_id: String, pub started_at: String, pub clean_shutdown: bool }
impl Lifecycle { pub fn mark_boot(home: &Path) -> anyhow::Result<Self>; pub fn mark_clean(home: &Path) -> anyhow::Result<()>; pub fn read(home: &Path) -> Option<Self>; }
pub struct Health { pub healthy: bool, pub reason: String }
pub fn probe(home: &Path) -> Health; // healthy iff heartbeat mtime < 60s; reason is one line, no secrets
```

- [ ] **Step 1: Write the failing tests** — append to each new file ONLY its test module (files do not exist yet, so create each file with just the test module plus a stub `use` that will fail):

`crates/gray-supervise/src/heartbeat.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heartbeat_roundtrip_age_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        write_heartbeat(dir.path()).unwrap();
        let age = heartbeat_age_secs(dir.path()).unwrap();
        assert!(age < 60, "fresh heartbeat must be <60s, got {age}");
    }
    #[test]
    fn missing_heartbeat_has_no_age() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(heartbeat_age_secs(dir.path()), None);
    }
}
```

`crates/gray-supervise/src/lifecycle.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_then_clean_flips_flag() {
        let dir = tempfile::tempdir().unwrap();
        let b = Lifecycle::mark_boot(dir.path()).unwrap();
        assert!(!b.clean_shutdown);
        assert_eq!(Lifecycle::read(dir.path()).unwrap().clean_shutdown, false);
        Lifecycle::mark_clean(dir.path()).unwrap();
        assert_eq!(Lifecycle::read(dir.path()).unwrap().clean_shutdown, true);
    }
}
```

`crates/gray-supervise/src/health.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_heartbeat_is_unhealthy_with_reason() {
        let dir = tempfile::tempdir().unwrap();
        let h = probe(dir.path());
        assert!(!h.healthy);
        assert!(h.reason.contains("heartbeat"), "reason must name heartbeat, got: {}", h.reason);
    }
    #[test]
    fn fresh_heartbeat_is_healthy() {
        let dir = tempfile::tempdir().unwrap();
        crate::heartbeat::write_heartbeat(dir.path()).unwrap();
        assert!(probe(dir.path()).healthy);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p gray-supervise heartbeat:: lifecycle:: health::`
Expected: FAIL with "cannot find function `write_heartbeat`" (and siblings).

- [ ] **Step 3: Write minimal implementation** — replace each file's content with impl + tests (tests stay as written above, placed after the impl):

`heartbeat.rs` full content above the test module:
```rust
//! Heartbeat file: `state/gateway.heartbeat` holds RFC3339 UTC now.
use std::path::Path;

pub fn write_heartbeat(home: &Path) -> anyhow::Result<()> {
    let (state, beat, _) = crate::paths_for_home(home);
    std::fs::create_dir_all(&state)?;
    std::fs::write(&beat, chrono::Utc::now().to_rfc3339())?;
    Ok(())
}

/// Seconds since heartbeat mtime. `None` when missing or on any IO error.
pub fn heartbeat_age_secs(home: &Path) -> Option<u64> {
    let (_, beat, _) = crate::paths_for_home(home);
    let meta = std::fs::metadata(&beat).ok()?;
    let mtime = meta.modified().ok()?;
    std::time::SystemTime::now()
        .duration_since(mtime)
        .map(|d| d.as_secs())
        .ok()
}
```

`lifecycle.rs` full content above the test module:
```rust
//! Lifecycle ledger: `state/gateway.lifecycle.json`.
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lifecycle {
    pub boot_id: String,
    pub started_at: String,
    pub clean_shutdown: bool,
}

impl Lifecycle {
    pub fn mark_boot(home: &Path) -> anyhow::Result<Self> {
        let (state, _, path) = crate::paths_for_home(home);
        std::fs::create_dir_all(&state)?;
        let lc = Lifecycle {
            boot_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            clean_shutdown: false,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&lc)?)?;
        Ok(lc)
    }

    pub fn mark_clean(home: &Path) -> anyhow::Result<()> {
        let (state, _, path) = crate::paths_for_home(home);
        std::fs::create_dir_all(&state)?;
        let mut lc = Self::read(home).unwrap_or(Lifecycle {
            boot_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            clean_shutdown: false,
        });
        lc.clean_shutdown = true;
        std::fs::write(&path, serde_json::to_string_pretty(&lc)?)?;
        Ok(())
    }

    pub fn read(home: &Path) -> Option<Self> {
        let (_, _, path) = crate::paths_for_home(home);
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}
```

`health.rs` full content above the test module:
```rust
//! File-based readiness: heartbeat mtime < 60s. No ports, no secrets in reason.
use std::path::Path;

pub struct Health {
    pub healthy: bool,
    pub reason: String,
}

pub fn probe(home: &Path) -> Health {
    match crate::heartbeat::heartbeat_age_secs(home) {
        None => Health { healthy: false, reason: "unhealthy: no heartbeat yet (gateway not running?)".into() },
        Some(age) if age < 60 => Health { healthy: true, reason: format!("healthy: heartbeat {age}s ago") },
        Some(age) => Health { healthy: false, reason: format!("unhealthy: heartbeat stale ({age}s ago)") },
    }
}
```

`lib.rs` becomes:
```rust
//! Platform-free supervision core (no telegram/discord/slack deps).
pub mod exit;
pub mod health;
pub mod heartbeat;
pub mod lifecycle;

use std::path::{Path, PathBuf};

/// `(state_dir, heartbeat_file, lifecycle_file)` under `$GRAY_HOME/state/`.
pub fn paths_for_home(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let state = home.join("state");
    (state.join("gateway.heartbeat"), state.join("gateway.heartbeat"), state.join("gateway.lifecycle.json"))
}
```

Wait — that returns heartbeat twice and drops state_dir. Fix before writing (self-caught): the correct body is:

```rust
/// `(state_dir, heartbeat_file, lifecycle_file)` under `$GRAY_HOME/state/`.
pub fn paths_for_home(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let state = home.join("state");
    let beat = state.join("gateway.heartbeat");
    let lc = state.join("gateway.lifecycle.json");
    (state, beat, lc)
}

/// Heartbeat period: `GRAY_HEARTBEAT_SECS`, default 15, clamped to min 5.
pub fn heartbeat_interval_secs() -> u64 {
    std::env::var("GRAY_HEARTBEAT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(15)
        .max(5)
}
```

Write `lib.rs` with the FIXED body (three-tuple + interval fn), not the broken sketch.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p gray-supervise`
Expected: all PASS (1 exit + 2 heartbeat + 1 lifecycle + 2 health = 6).

- [ ] **Step 5: Commit**

```bash
git add crates/gray-supervise/src/
git commit -m "feat(supervise): heartbeat, lifecycle ledger, file probe"
```

---

### Task 3: Watchdog constants + log rotation helper

**Files:**
- Create: `crates/gray-supervise/src/watchdog.rs`
- Create: `crates/gray-supervise/src/rotation.rs`
- Modify: `crates/gray-supervise/src/lib.rs` (add `pub mod watchdog; pub mod rotation;`)

**Interfaces:**
- Consumes: nothing new.
- Produces (used by Tasks 5–6): `watchdog::{startup_timeout_secs, SHUTDOWN_DRAIN_SECS}` and `rotation::{LOG_MAX_BYTES, LOG_KEEP, rotate_if_needed}`:
```rust
pub fn startup_timeout_secs() -> u64; // GRAY_STARTUP_TIMEOUT_SECS, default 120, min 30
pub const SHUTDOWN_DRAIN_SECS: u64 = 30;
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const LOG_KEEP: usize = 2; // gray.log.1, gray.log.2 beside gray.log
pub fn rotate_if_needed(path: &Path); // best-effort, never panics
```

- [ ] **Step 1: Write the failing tests**

`crates/gray-supervise/src/watchdog.rs` (test only):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_spec() {
        assert_eq!(SHUTDOWN_DRAIN_SECS, 30);
        assert_eq!(startup_timeout_secs(), 120);
    }
}
```

`crates/gray-supervise/src/rotation.rs` (test only):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_log_rotates_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gray.log");
        std::fs::write(&log, vec![b'x'; (LOG_MAX_BYTES + 1) as usize]).unwrap();
        std::fs::write(dir.path().join("gray.log.1"), b"old1").unwrap();
        std::fs::write(dir.path().join("gray.log.2"), b"old2").unwrap();
        rotate_if_needed(&log);
        assert!(std::fs::metadata(&log).unwrap().len() < LOG_MAX_BYTES);
        assert!(!dir.path().join("gray.log.3").exists(), "must cap at .2");
    }
    #[test]
    fn small_log_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gray.log");
        std::fs::write(&log, b"tiny").unwrap();
        rotate_if_needed(&log);
        assert_eq!(std::fs::read(&log).unwrap(), b"tiny");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p gray-supervise watchdog:: rotation::`
Expected: FAIL with "cannot find" (modules not declared yet).

- [ ] **Step 3: Write minimal implementation**

`watchdog.rs` above tests:
```rust
//! Startup/shutdown budgets. Boot must mark ready within the timeout or exit 75.
pub const SHUTDOWN_DRAIN_SECS: u64 = 30;

/// `GRAY_STARTUP_TIMEOUT_SECS`, default 120, clamped to min 30.
pub fn startup_timeout_secs() -> u64 {
    std::env::var("GRAY_STARTUP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(120)
        .max(30)
}
```

`rotation.rs` above tests:
```rust
//! Size-capped log rotation: `gray.log` → `.1` → `.2`, best-effort, never panics.
use std::path::Path;

pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const LOG_KEEP: usize = 2;

/// If `path` exceeds `LOG_MAX_BYTES`, shift `.1`→`.2`, `path`→`.1`, truncate `path`.
/// Missing/small files are left alone. All errors swallowed (logging must not crash boot).
pub fn rotate_if_needed(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if meta.len() <= LOG_MAX_BYTES {
        return;
    }
    let _ = std::fs::remove_file(path.with_extension("log.2"));
    let _ = std::fs::rename(path.with_extension("log.1"), path.with_extension("log.2"));
    let _ = std::fs::rename(path, path.with_extension("log.1"));
}
```

Note: `Path::with_extension("log.1")` on `gray.log` yields `gray.log.1` (replaces `log` with `log.1`) — correct for our naming.

In `lib.rs`, add after `pub mod lifecycle;`:
```rust
pub mod rotation;
pub mod watchdog;
```
(keep alphabetical: exit, health, heartbeat, lifecycle, rotation, watchdog — `rotation` before `watchdog`.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p gray-supervise`
Expected: all PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/gray-supervise/src/
git commit -m "feat(supervise): watchdog budgets + size-capped log rotation"
```

---

### Task 4: Unit generators move to core; gateway delegates

**Files:**
- Create: `crates/gray-supervise/src/units.rs`
- Modify: `crates/gray-supervise/src/lib.rs` (add `pub mod units;` between lifecycle and rotation… actually alphabetical: lifecycle, rotation, units, watchdog — insert `pub mod units;` after rotation)
- Modify: `crates/gray-gateway/src/systemd.rs:6-15` (delegate + `.bak` guard + linger hint)

**Interfaces:**
- Consumes: `gray_supervise::exit::{EXIT_RESTART, EXIT_FATAL}` for unit text.
- Produces (used by gateway `install`): `units::{generate_systemd_unit(gray_bin: &Path, gray_home: &Path) -> String, generate_launchd_plist(gray_bin: &Path, gray_home: &Path) -> String, linger_hint() -> Option<String>}`.

- [ ] **Step 1: Write the failing tests** — create `crates/gray-supervise/src/units.rs` with ONLY:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn systemd_unit_has_restart_contract_and_drain() {
        let u = generate_systemd_unit(Path::new("/home/u/.local/bin/gray"), Path::new("/home/u/.gray"));
        assert!(u.contains("ExecStart=/home/u/.local/bin/gray gateway run"), "got:\n{u}");
        assert!(u.contains("Restart=always"));
        assert!(u.contains("RestartSec=5"));
        assert!(u.contains("StartLimitIntervalSec=0"));
        assert!(u.contains("TimeoutStopSec=90"));
        assert!(u.contains("RestartForceExitStatus=75"));
        assert!(u.contains("RestartPreventExitStatus=78"));
        assert!(u.contains("Environment=GRAY_HOME=/home/u/.gray"));
    }
    #[test]
    fn launchd_plist_keeps_alive_and_runs_at_load() {
        let p = generate_launchd_plist(Path::new("/opt/homebrew/bin/gray"), Path::new("/Users/u/.gray"));
        assert!(p.contains("RunAtLoad"));
        assert!(p.contains("<true/>"));
        assert!(p.contains("KeepAlive"));
        assert!(p.contains("/opt/homebrew/bin/gray"));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p gray-supervise units::`
Expected: FAIL with "cannot find function `generate_systemd_unit`".

- [ ] **Step 3: Write minimal implementation** — prepend above the test module:

```rust
//! Service units: hardened systemd user unit + macOS launchd plist. No IO here.
use std::path::Path;

/// Hardened user unit: always restart, no start-limit stall, 90s stop budget
/// (30s drain + headroom), exit-code contract, linger-friendly target.
pub fn generate_systemd_unit(gray_bin: &Path, gray_home: &Path) -> String {
    format!(
        "[Unit]\nDescription=Gray Gateway\nAfter=network.target\n\n[Service]\nExecStart={} gateway run\nRestart=always\nRestartSec=5\nStartLimitIntervalSec=0\nTimeoutStopSec=90\nRestartForceExitStatus={}\nRestartPreventExitStatus={}\nEnvironment=GRAY_HOME={}\n\n[Install]\nWantedBy=default.target\n",
        gray_bin.display(),
        crate::exit::EXIT_RESTART,
        crate::exit::EXIT_FATAL,
        gray_home.display()
    )
}

/// macOS agent plist: `~/Library/LaunchAgents/ai.gray.gateway.plist`.
pub fn generate_launchd_plist(gray_bin: &Path, gray_home: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key><string>ai.gray.gateway</string>\n\t<key>ProgramArguments</key><array><string>{}</string><string>gateway</string><string>run</string></array>\n\t<key>EnvironmentVariables</key><dict><key>GRAY_HOME</key><string>{}</string></dict>\n\t<key>RunAtLoad</key><true/>\n\t<key>KeepAlive</key><true/>\n\t<key>ThrottleInterval</key><integer>30</integer>\n\t<key>StandardOutPath</key><string>{}/logs/gateway.out.log</string>\n\t<key>StandardErrorPath</key><string>{}/logs/gateway.err.log</string>\n</dict>\n</plist>\n",
        gray_bin.display(),
        gray_home.display(),
        gray_home.display()
    )
}

/// Human hint when systemd lingering is off (checked by the caller via loginctl).
pub fn linger_hint() -> &'static str {
    "tip: run `loginctl enable-linger $USER` so the gateway survives logout"
}
```

- [ ] **Step 4: Delegate the gateway** — in `crates/gray-gateway/src/systemd.rs`, replace `generate_unit` (lines 6–15) with:

```rust
pub fn generate_unit(gray_bin: &Path) -> String {
    let gray_home = crate::config::gray_home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| format!("{}/.gray", std::env::var("HOME").unwrap_or_default()));
    gray_supervise::units::generate_systemd_unit(gray_bin, Path::new(&gray_home))
}
```

and in `install()` (after `create_dir_all`, before `write`), insert the `.bak` guard:

```rust
    if path.exists() && !path.with_extension("service.bak").exists() {
        let _ = std::fs::copy(&path, path.with_extension("service.bak"));
    }
```

(`gray-gateway.service` → `with_extension("service.bak")` yields `gray-gateway.service.bak` — correct.) After the `enable --now` block, add:

```rust
    if std::process::Command::new("loginctl")
        .args(["show-user", &std::env::var("USER").unwrap_or_default(), "-p", "Linger"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Linger=yes"))
        .unwrap_or(true)
    {
    } else {
        println!("{}", gray_supervise::units::linger_hint());
    }
```

Keep the shape minimal: if linger cannot be determined, stay silent (no false hint).

- [ ] **Step 5: Run to verify**

Run: `cargo test -p gray-supervise units::` then `cargo test -p gray-gateway systemd::` then `cargo check -p gray-gateway --features all-platforms`
Expected: all PASS / green. Existing `systemd::tests` (missing-systemctl, absent-service, active-service) still pass untouched.

- [ ] **Step 6: Commit**

```bash
git add crates/gray-supervise/src/units.rs crates/gray-supervise/src/lib.rs crates/gray-gateway/src/systemd.rs
git commit -m "feat(supervise): hardened systemd + launchd units, gateway delegates"
```

---

### Task 5: Gateway boot/shutdown/probe wiring + CLI

**Files:**
- Modify: `crates/gray-gateway/src/daemon_boot.rs:100-160,191-210` (lifecycle + heartbeat + watchdog + drain)
- Modify: `crates/gray-gateway/src/daemon.rs:324-335` (`/restart` → exit 75)
- Modify: `crates/gray/src/lib.rs:295-303` (`Status` gains `#[arg(long)] probe: bool`)
- Modify: `crates/gray/src/main.rs:164-170` (Status arm + fatal-config exit mapping)

**Interfaces:**
- Consumes: Task 2 (`paths_for_home`, `write_heartbeat`, `heartbeat_interval_secs`, `Lifecycle`, `probe`), Task 3 (`startup_timeout_secs`, `SHUTDOWN_DRAIN_SECS`), Task 1 (`EXIT_RESTART`, `EXIT_FATAL`).
- Produces: `gray gateway status` (service state as today) + `gray gateway status --probe` (file probe, exit 0/1, one line).

- [ ] **Step 1: Write the failing test** — append to `crates/gray-gateway/src/systemd.rs` test module:

```rust
    #[test]
    fn probe_reports_healthy_after_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        gray_supervise::heartbeat::write_heartbeat(dir.path()).unwrap();
        let h = gray_supervise::health::probe(dir.path());
        assert!(h.healthy, "probe must be healthy, got: {}", h.reason);
    }
```

(`tempfile` is already a `gray-gateway` dev-dependency per `crates/gray-gateway/Cargo.toml:44-45`.)

- [ ] **Step 2: Run to verify it passes already (wiring test, no new code yet)**

Run: `cargo test -p gray-gateway probe_reports_healthy`
Expected: PASS (proves the dep wiring from Task 1 works from inside gateway tests).

- [ ] **Step 3: Wire boot** — in `run_gateway_inner` (`daemon_boot.rs`), after the flock block (lines 104–110) insert:

```rust
    let home = crate::config::gray_home_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.gray"));
    if let Some(prev) = gray_supervise::lifecycle::Lifecycle::read(&home) {
        if !prev.clean_shutdown {
            log::warn!("gateway previous exit unclean (boot {})", prev.boot_id);
        }
    }
    let _ = gray_supervise::lifecycle::Lifecycle::mark_boot(&home);
    let _ = gray_supervise::heartbeat::write_heartbeat(&home);
    let beat_home = home.clone();
    let beat_every = gray_supervise::heartbeat_interval_secs();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(beat_every)).await;
            let _ = gray_supervise::heartbeat::write_heartbeat(&beat_home);
        }
    });
    let boot_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(gray_supervise::watchdog::startup_timeout_secs());
```

after `runner.send_startup_notifications().await;` (line 159) insert:

```rust
    if std::time::Instant::now() > boot_deadline {
        log::error!("gateway startup watchdog: boot exceeded budget; exiting 75");
        std::process::exit(gray_supervise::exit::EXIT_RESTART);
    }
```

- [ ] **Step 4: Wire shutdown drain + clean mark** — replace the `#[cfg(unix)]` select block (lines 191–201) with a drain that waits up to `SHUTDOWN_DRAIN_SECS` for the token, then marks clean:

```rust
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        tokio::select! {
            _ = token.cancelled() => {},
            _ = sigterm.recv() => {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(gray_supervise::watchdog::SHUTDOWN_DRAIN_SECS),
                    token.cancelled(),
                )
                .await;
            },
            _ = sigint.recv() => {},
        }
    }
```

and just before the final `Ok(())` (line 209) insert:

```rust
    if let Ok(home) = crate::config::gray_home_dir() {
        let _ = gray_supervise::lifecycle::Lifecycle::mark_clean(&home);
    }
```

- [ ] **Step 5: Wire `/restart` exit code** — in `daemon.rs` `SlashCommand::Restart` arm, change `std::process::exit(0)` to `std::process::exit(gray_supervise::exit::EXIT_RESTART)` and update the comment above it from `systemd (Restart=always) revives us` to `systemd (RestartForceExitStatus=75) revives us`.

- [ ] **Step 6: Wire CLI probe + fatal mapping** — in `crates/gray/src/lib.rs`, change the `Status` variant to:

```rust
    /// Show gateway status
    Status {
        /// File-based health probe (heartbeat freshness), exit 0/1
        #[arg(long)]
        probe: bool,
    },
```

In `crates/gray/src/main.rs`, change the `None | Some(GatewayCmd::Status)` arm: the `GatewayCmd` enum is struct-variant now, so replace

```rust
        None | Some(GatewayCmd::Status) => gray_gateway::systemd::status(),
```

with

```rust
        None | Some(GatewayCmd::Status { probe: false }) => gray_gateway::systemd::status(),
        Some(GatewayCmd::Status { probe: true }) => {
            let home = std::env::var("GRAY_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".gray")).unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.gray"))
                });
            let h = gray_supervise::health::probe(&home);
            println!("{}", h.reason);
            if !h.healthy {
                std::process::exit(1);
            }
            Ok(())
        }
```

and add `gray-supervise = { workspace = true }` to `crates/gray/Cargo.toml` `[dependencies]` (check the file first; append after the `gray-session` line). Then map fatal config: in `run_gateway`, after the `Run` arm, wrap — change

```rust
        Some(GatewayCmd::Run) => gray_gateway::daemon::run_gateway().await,
```

to

```rust
        Some(GatewayCmd::Run) => match gray_gateway::daemon::run_gateway().await {
            Err(e) if format!("{e:#}").contains("no gateway platforms enabled") => {
                eprintln!("{e:#}");
                std::process::exit(gray_supervise::exit::EXIT_FATAL);
            }
            r => r,
        },
```

- [ ] **Step 7: Run to verify**

Run: `cargo test -p gray-gateway` then `cargo check -p gray --all-targets` then `cargo check -p gray-gateway --features all-platforms`
Expected: all green. Manual: `gray gateway status --probe` prints `unhealthy: no heartbeat yet…` + exit 1 when gateway down.

- [ ] **Step 8: Commit**

```bash
git add crates/gray-gateway/src/daemon_boot.rs crates/gray-gateway/src/daemon.rs crates/gray-gateway/src/systemd.rs crates/gray/src/lib.rs crates/gray/src/main.rs crates/gray/Cargo.toml
git commit -m "feat(gateway): supervise boot/drain/probe wiring, restart exits 75"
```

---

### Task 6: Log rotation on boot + README

**Files:**
- Modify: `crates/gray/src/logging.rs:121-141` (`init`)
- Modify: `README.md:77-86,158-196` (Gateway section)

**Interfaces:**
- Consumes: Task 3 (`rotation::rotate_if_needed`). `crates/gray/Cargo.toml` already depends on workspace — add `gray-supervise` there if Task 5 did not (exactly one of the two tasks adds it; check before duplicating).

- [ ] **Step 1: Write the failing test** — append to `logging.rs` test module:

```rust
    #[test]
    fn rotation_helper_caps_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gray.log");
        std::fs::write(&log, vec![b'x'; (10 * 1024 * 1024 + 1) as usize]).unwrap();
        gray_supervise::rotation::rotate_if_needed(&log);
        assert!(std::fs::metadata(&log).unwrap().len() < 10 * 1024 * 1024);
    }
```

(`tempfile` — check `crates/gray/Cargo.toml` dev-deps first; if absent, add `tempfile = { workspace = true }` under `[dev-dependencies]`, creating the section if needed.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p gray rotation_helper_caps`
Expected: FAIL with "unresolved import / use of undeclared crate `gray_supervise`" (dep not wired yet).

- [ ] **Step 3: Write minimal implementation** — in `init()` (after `let path = home.join("logs").join("gray.log");`), insert one line:

```rust
        gray_supervise::rotation::rotate_if_needed(&path);
```

before the `if let Some(parent)` block. Add the `gray-supervise` dep to `crates/gray/Cargo.toml` if Task 5 has not already (do not add twice).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p gray logging::` then `cargo test -p gray-supervise` then `cargo test -p gray-gateway`
Expected: all PASS.

- [ ] **Step 5: Update README** — in the Gateway section (after the `gray gateway run|status|install|…` row), add one short block:

```markdown
Always-on: `gray gateway install` (systemd user service, `Restart=always`, survives reboot with linger) or `gray gateway run` under your own supervisor; `gray gateway status --probe` reports heartbeat health; heartbeats live in `~/.gray/state/gateway.heartbeat`, lifecycle in `state/gateway.lifecycle.json`; logs rotate at 10MB × 3.
```

Keep it to that paragraph — no new sections, no Docker/systemd tutorial.

- [ ] **Step 6: Commit**

```bash
git add crates/gray/src/logging.rs crates/gray/Cargo.toml README.md
git commit -m "feat: rotate gray.log at 10MB, document always-on probe"
```

---

## Self-review

- Spec coverage: exit codes (§3) → Tasks 1+4+5; heartbeat (§3) → Task 2 boot thread in 5; lifecycle (§3) → Task 2 + boot/clean marks in 5; health/probe (§3) → Tasks 2+5; systemd hardening + linger (§3) → Task 4; launchd (§3) → Task 4 generator (install wiring for macOS is a later cut — generator exists, `install()` stays systemd-only, honestly stated); logs (§3) → Tasks 3+6; drain 30s + TimeoutStopSec 90 (§3–§4) → Tasks 4+5; no-TCP/no-SQLite/no-Docker/no-Windows (§2 non-goals) → nowhere in tasks, correct.
- Placeholder scan: every step has verbatim code, exact `Run:` + `Expected:`, exact `git add` paths. No TBD/TODO. The `with_extension("log.1")` / `with_extension("service.bak")` tricks are called out with why they are correct. The Task 2 `lib.rs` broken-sketch-then-fix is intentional self-correction inside the plan so executors write the fixed body once.
- Type consistency: `paths_for_home(&Path) -> (PathBuf, PathBuf, PathBuf)` defined Task 2, used identically in heartbeat/lifecycle/health; `Lifecycle::{mark_boot, mark_clean, read}` signatures match Task 5 call sites; `probe(&Path) -> Health{healthy: bool, reason: String}` matches CLI use; `generate_systemd_unit(&Path, &Path) -> String` matches gateway delegate; `rotate_if_needed(&Path)` matches logging call; `GatewayCmd::Status { probe: bool }` matches both `main.rs` arms. `EXIT_*` are `i32` consts everywhere including `std::process::exit` (takes `i32`) — consistent.
