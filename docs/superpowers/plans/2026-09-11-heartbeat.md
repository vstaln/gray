# Heartbeat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give gray a 24/7 heartbeat — a standing goal the agent works on a schedule, reaching the user on chat only when there is something to say — as a gated `gray-heartbeat` crate that never touches `gray-core`.

**Architecture:** Reuse `gray_cron` for scheduling and the gateway's existing cron ticker + `[SILENT]`-aware delivery. Heartbeat owns a goal file (`$GRAY_HOME/heartbeat/goal.md`), a config file (`$GRAY_HOME/heartbeat.json`), and one cron job named `heartbeat`. A standalone `gray-heartbeat` binary manages it; the same binary runs as a sidecar plugin exposing a `heartbeat` tool.

**Tech Stack:** Rust 2024, `anyhow`, `clap`, `serde`/`serde_json`, `chrono`, `gray-cron`; stdio (no tokio).

**Spec:** `docs/superpowers/specs/2026-09-11-heartbeat-design.md`

## Global Constraints

- The crate is named `gray-heartbeat`; binary is `gray-heartbeat`.
- It MUST NOT be added to `default-members`, and MUST NOT depend on `gray`, `gray-core`, `gray-plugin`, or `gray-gateway` (only `gray-cron` + leaf deps) — no cycles, core untouched.
- Config file is JSON at `$GRAY_HOME/heartbeat.json`; goal at `$GRAY_HOME/heartbeat/goal.md`; cron job name is exactly `heartbeat`.
- Home resolution: read `GRAY_HOME`, else `$HOME/.gray`; do NOT depend on `gray-gateway::config::gray_home_dir`.
- Quiet convention is `[SILENT]` (the gateway already suppresses it) — do not invent a new token.
- Use workspace deps (`dep.workspace = true`); `version/edition/license.workspace = true`.
- Every task ends with `cargo test -p gray-heartbeat` green and a commit.

---

### Task 1: Crate skeleton, config, goal file

**Files:**
- Create: `crates/gray-heartbeat/Cargo.toml`
- Create: `crates/gray-heartbeat/src/lib.rs`
- Create: `crates/gray-heartbeat/src/config.rs`
- Create: `crates/gray-heartbeat/src/goal.rs`
- Modify: `Cargo.toml` (root: add `members` entry + `[workspace.dependencies]` entry)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn gray_home() -> anyhow::Result<PathBuf>`
  - `pub struct HeartbeatConfig { pub enabled: bool, pub schedule: String, pub deliver: String }` (Default: enabled false, schedule `"every 30m"`, deliver `"local"`)
  - `pub fn config_path() -> anyhow::Result<PathBuf>`; `pub fn load_config() -> anyhow::Result<HeartbeatConfig>`; `pub fn save_config(&HeartbeatConfig) -> anyhow::Result<()>`
  - `pub fn goal_path() -> anyhow::Result<PathBuf>`; `pub fn read_goal() -> anyhow::Result<String>`; `pub fn write_goal(&str) -> anyhow::Result<()>`

- [ ] **Step 1: Root manifest wiring**

In root `Cargo.toml`: add `"crates/gray-heartbeat",` to `members` (NOT `default-members`), and under `[workspace.dependencies]` add:
```toml
gray-heartbeat = { path = "crates/gray-heartbeat" }
```

- [ ] **Step 2: Crate manifest**

Create `crates/gray-heartbeat/Cargo.toml`:
```toml
[package]
name = "gray-heartbeat"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "gray's 24/7 heartbeat — a standing goal run on a schedule"

[dependencies]
anyhow.workspace = true
chrono.workspace = true
clap.workspace = true
gray-cron.workspace = true
log.workspace = true
serde.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Write the failing tests**

Create `crates/gray-heartbeat/src/goal.rs` tests and `config.rs` tests. In `config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        assert_eq!(load_config().unwrap().schedule, "every 30m");
        let cfg = HeartbeatConfig { enabled: true, schedule: "every 1h".into(), deliver: "telegram:1".into() };
        save_config(&cfg).unwrap();
        let got = load_config().unwrap();
        assert!(got.enabled);
        assert_eq!(got.schedule, "every 1h");
        assert_eq!(got.deliver, "telegram:1");
    }
}
```
In `goal.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_round_trips_and_is_empty_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        assert_eq!(read_goal().unwrap(), "");
        write_goal("Ship the thing.\n").unwrap();
        assert_eq!(read_goal().unwrap(), "Ship the thing.\n");
    }
}
```
Note: these tests mutate a process-global env var; put both modules' tests behind a shared `static ENV_LOCK: std::sync::Mutex<()>` (define in `lib.rs` as `pub(crate)`) and lock it in each test to avoid cross-test races.

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p gray-heartbeat`
Expected: FAIL (crate/functions don't exist).

- [ ] **Step 5: Implement**

`lib.rs`:
```rust
//! gray-heartbeat: a standing goal run on a schedule by the gateway cron ticker.
pub mod config;
pub mod goal;

pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use std::path::PathBuf;

/// `$GRAY_HOME`, else `$HOME/.gray`.
pub fn gray_home() -> anyhow::Result<PathBuf> {
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return Ok(PathBuf::from(h));
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("cannot resolve HOME"))?;
    Ok(PathBuf::from(home).join(".gray"))
}
```
`config.rs`: serde struct as in Interfaces, `config_path()` = `gray_home()?.join("heartbeat.json")`, `load_config()` returns Default when the file is absent, `save_config` writes pretty JSON (create parent dirs).
`goal.rs`: `goal_path()` = `gray_home()?.join("heartbeat").join("goal.md")`, `read_goal()` returns `""` when absent, `write_goal` creates parent dirs and writes.

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p gray-heartbeat`
Expected: PASS (3 tests).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/gray-heartbeat
git commit -m "feat(heartbeat): crate skeleton, config and goal file"
```

---

### Task 2: Heartbeat prompt + cron job sync

**Files:**
- Create: `crates/gray-heartbeat/src/job.rs`
- Modify: `crates/gray-heartbeat/src/lib.rs` (add `pub mod job;`)

**Interfaces:**
- Consumes: `config::{HeartbeatConfig, load_config}`, `goal::read_goal`, `gray_cron`.
- Produces:
  - `pub const JOB_NAME: &str = "heartbeat";`
  - `pub fn render_prompt(goal: &str) -> String`
  - `pub fn cron_dir() -> anyhow::Result<PathBuf>` (`gray_home()?.join("cron")`)
  - `pub fn sync_job(cfg: &HeartbeatConfig, goal: &str) -> anyhow::Result<String>` — removes any existing job named `heartbeat`, and if `cfg.enabled`, adds one with `schedule=cfg.schedule`, `deliver=Deliver::Target(cfg.deliver)`, `prompt=render_prompt(goal)`; returns the job id or `""` when disabled.
  - `pub enum JobStatus { Disabled, Missing, Live { next_run_at: Option<i64> } }`
  - `pub fn job_status(cfg: &HeartbeatConfig) -> anyhow::Result<JobStatus>`

- [ ] **Step 1: Write the failing tests** (`job.rs`)
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HeartbeatConfig;
    use crate::ENV_LOCK;

    #[test]
    fn render_prompt_embeds_goal_and_silence_token() {
        let p = render_prompt("Ship it.");
        assert!(p.contains("Ship it."), "{p}");
        assert!(p.contains("[SILENT]"), "{p}");
    }

    #[test]
    fn sync_adds_updates_and_removes_job() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let mut cfg = HeartbeatConfig { enabled: true, schedule: "every 30m".into(), deliver: "local".into() };
        let id = sync_job(&cfg, "goal one").unwrap();
        assert!(!id.is_empty());
        let store = gray_cron::CronStore::open(cron_dir().unwrap()).unwrap();
        assert_eq!(store.get(JOB_NAME).unwrap().unwrap().prompt.contains("goal one"), true);

        // Re-sync after a goal edit replaces the job (no duplicates).
        sync_job(&cfg, "goal two").unwrap();
        let jobs = store.list().unwrap();
        assert_eq!(jobs.iter().filter(|j| j.name == JOB_NAME).count(), 1);
        assert!(jobs[0].prompt.contains("goal two"));

        cfg.enabled = false;
        assert_eq!(sync_job(&cfg, "goal two").unwrap(), "");
        assert!(store.get(JOB_NAME).unwrap().is_none());
    }

    #[test]
    fn status_reports_disabled_missing_and_live() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let cfg = HeartbeatConfig::default();
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Disabled));
        let cfg = HeartbeatConfig { enabled: true, schedule: "every 30m".into(), deliver: "local".into() };
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Missing));
        sync_job(&cfg, "g").unwrap();
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Live { .. }));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p gray-heartbeat job`
Expected: FAIL.

- [ ] **Step 3: Implement `job.rs`**

```rust
use crate::config::HeartbeatConfig;
use anyhow::Context;

pub const JOB_NAME: &str = "heartbeat";

pub fn cron_dir() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::gray_home()?.join("cron"))
}

pub fn render_prompt(goal: &str) -> String {
    format!(
        "Heartbeat wake-up. Your standing goal:\n\n{goal}\n\n\
You woke on your own; no user message triggered this. Take the next useful step \
toward the goal using your tools, then reply for the user.\n\
If there is nothing to do, nothing changed, or nothing worth reporting, reply \
with exactly [SILENT] and nothing else."
    )
}

pub fn sync_job(cfg: &HeartbeatConfig, goal: &str) -> anyhow::Result<String> {
    let store = gray_cron::CronStore::open(cron_dir()?).context("open cron store")?;
    let _ = store.remove(JOB_NAME);
    if !cfg.enabled {
        return Ok(String::new());
    }
    let id = store.add_full(
        JOB_NAME,
        &cfg.schedule,
        &render_prompt(goal),
        gray_cron::Deliver::Target(cfg.deliver.clone()),
        None,
        None,
    )?;
    Ok(id)
}

pub enum JobStatus {
    Disabled,
    Missing,
    Live { next_run_at: Option<i64> },
}

pub fn job_status(cfg: &HeartbeatConfig) -> anyhow::Result<JobStatus> {
    if !cfg.enabled {
        return Ok(JobStatus::Disabled);
    }
    let store = gray_cron::CronStore::open(cron_dir()?)?;
    match store.get(JOB_NAME)? {
        Some(j) => Ok(JobStatus::Live { next_run_at: j.next_run_at }),
        None => Ok(JobStatus::Missing),
    }
}
```
If `add_full`'s `Deliver`/`Origin`/`workdir` argument types differ, match the real signature (research: `add_full(name, schedule, prompt, deliver, origin: Option<Origin>, workdir: Option<PathBuf>)`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p gray-heartbeat`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gray-heartbeat
git commit -m "feat(heartbeat): prompt template and cron job sync"
```

---

### Task 3: `gray-heartbeat` CLI binary

**Files:**
- Create: `crates/gray-heartbeat/src/main.rs`
- Modify: `crates/gray-heartbeat/Cargo.toml` (add `[[bin]]`)

**Interfaces:**
- Consumes: `config::*`, `goal::*`, `job::*`.
- Produces: binary `gray-heartbeat` with subcommands:
  - `on [--every <sched>] [--deliver <target>]`
  - `off`
  - `status`
  - `goal` (print) / `goal set <text…>`
  - `sync` (re-render the job from the current goal/config)

- [ ] **Step 1: Manifest**

Add to `crates/gray-heartbeat/Cargo.toml`:
```toml
[[bin]]
name = "gray-heartbeat"
path = "src/main.rs"
```

- [ ] **Step 2: Write a failing test**

CLI behavior is thin over the lib; test the one non-trivial helper. Add to `job.rs` (or a `cli.rs` module) `pub fn enable(cfg: &mut HeartbeatConfig, schedule: Option<String>, deliver: Option<String>) -> anyhow::Result<String>` that applies overrides, sets `enabled = true`, saves config, and syncs the job. Test:
```rust
#[test]
fn enable_applies_overrides_and_persists() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
    let mut cfg = HeartbeatConfig::default();
    enable(&mut cfg, Some("every 1h".into()), Some("telegram:9".into())).unwrap();
    let saved = crate::config::load_config().unwrap();
    assert!(saved.enabled);
    assert_eq!(saved.schedule, "every 1h");
    assert_eq!(saved.deliver, "telegram:9");
}
```
(Export `pub use job::enable;` from `lib.rs`.)

- [ ] **Step 3: Run to verify failure** — `cargo test -p gray-heartbeat enable` → FAIL.

- [ ] **Step 4: Implement `enable` + `main.rs`**

```rust
pub fn enable(cfg: &mut HeartbeatConfig, schedule: Option<String>, deliver: Option<String>) -> anyhow::Result<String> {
    if let Some(s) = schedule { cfg.schedule = s; }
    if let Some(d) = deliver { cfg.deliver = d; }
    cfg.enabled = true;
    crate::config::save_config(cfg)?;
    sync_job(cfg, &crate::goal::read_goal()?)
}
```
`main.rs` uses `clap` derive to parse the subcommands and prints human-readable status; `off` sets `enabled=false`, saves, `sync_job`. `sync` calls `sync_job(&load_config()?, &read_goal()?)`.

- [ ] **Step 5: Run to verify pass + smoke** 
Run: `cargo test -p gray-heartbeat && cargo run -p gray-heartbeat -- status`
Expected: PASS; `status` prints `disabled`.

- [ ] **Step 6: Commit**

```bash
git add crates/gray-heartbeat
git commit -m "feat(heartbeat): gray-heartbeat CLI (on/off/status/goal/sync)"
```

---

### Task 4: Sidecar plugin mode (`--plugin`)

**Files:**
- Create: `crates/gray-heartbeat/src/plugin.rs`
- Modify: `crates/gray-heartbeat/src/lib.rs` (`pub mod plugin;`), `src/main.rs` (handle `--plugin` before subcommands)

**Interfaces:**
- Consumes: `config`, `goal`, `job`.
- Produces: `pub fn serve_plugin() -> anyhow::Result<()>` — reads newline-delimited JSON on stdin, writes replies on stdout, exposing one tool `heartbeat` with `action` ∈ `{status, goal_get, goal_set, on, off, sync}` (+ optional `text`, `schedule`, `deliver`), and answers `plugin/manifest`. Exits 0 on `plugin/shutdown`.

Protocol (from `docs/protocol-v1.md` + `plugins/echo/echo.sh`):
- Request: `{"id":<u64>,"method":"<m>","params":{...}}`; reply `{"id":<same>,"result":{...}}`.
- `plugin/manifest` result: `{name, version, protocol:"1.1", tools:[{name, description, parameters}], commands:[], hooks:[]}`.
- `tool/call` params: `{name, args, session}`; reply `{content, is_error}`.
- Notification `{"method":"plugin/shutdown"}` (no id) → exit 0.

- [ ] **Step 1: Write the failing test** (`plugin.rs`)
Drive the loop over an in-memory pair by extracting a pure `pub fn handle_line(line: &str) -> Option<String>`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn manifest_advertises_the_heartbeat_tool() {
        let out = handle_line(r#"{"id":1,"method":"plugin/manifest"}"#).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["tools"][0]["name"], "heartbeat");
    }

    #[test]
    fn tool_call_status_and_goal_set() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let out = handle_line(r#"{"id":2,"method":"tool/call","params":{"name":"heartbeat","args":{"action":"goal_set","text":"do x"}}}"#).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["is_error"], false);
        assert_eq!(crate::goal::read_goal().unwrap(), "do x");
    }

    #[test]
    fn shutdown_is_a_notification_with_no_reply() {
        assert!(handle_line(r#"{"method":"plugin/shutdown"}"#).is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p gray-heartbeat plugin` → FAIL.

- [ ] **Step 3: Implement `handle_line` + `serve_plugin`**

`handle_line` parses JSON, matches `method`, returns `None` for `plugin/shutdown` (and for a notification with no id), else `Some(json_string)`. `tool/call` maps `action` to config/goal/job calls and returns `{content, is_error:false}` (or `is_error:true` with the message on error). `serve_plugin` loops `stdin.lock().lines()`, writes each `Some` reply + `'\n'`, flushes, and returns on `None`/EOF.

- [ ] **Step 4: Run to verify pass + real smoke**

Run:
```bash
cargo test -p gray-heartbeat
printf '%s\n' '{"id":1,"method":"plugin/manifest"}' | cargo run -q -p gray-heartbeat -- --plugin
```
Expected: PASS; smoke prints a manifest JSON line naming the `heartbeat` tool.

- [ ] **Step 5: Commit**

```bash
git add crates/gray-heartbeat
git commit -m "feat(heartbeat): sidecar plugin mode (heartbeat tool)"
```

---

### Task 5: Docs

**Files:**
- Modify: `README.md` (a short "Heartbeat" subsection under Gateway)
- Create: `docs/heartbeat.md`

- [ ] **Step 1: Write `docs/heartbeat.md`** covering: what it is, `gray-heartbeat on --every 30m --deliver telegram:123`, goal file, `[SILENT]` quieting, that the gateway daemon must be running, and how to register it as a plugin:
```yaml
plugins:
  - tools-minimal
  - sidecar: ~/.gray/plugins/heartbeat
```
- [ ] **Step 2: Add a 3-line README pointer** under the Gateway section linking `docs/heartbeat.md`.
- [ ] **Step 3: Verify links/format** — `cargo fmt --all --check`; read both docs.
- [ ] **Step 4: Commit**
```bash
git add README.md docs/heartbeat.md
git commit -m "docs(heartbeat): usage, goal file, plugin registration"
```

---

## Self-Review

- Spec coverage: architecture → Task 2 (cron job + ticker reuse); state files → Task 1; CLI → Task 3; plugin configurability → Task 4; quiet-by-default → Task 2 prompt + existing `is_silent`; packaging (gated crate) → Task 1. Docs → Task 5. No gaps.
- Placeholders: none — each step names concrete files, signatures, and code.
- Type consistency: `HeartbeatConfig`, `sync_job`, `render_prompt`, `JOB_NAME`, `job_status`/`JobStatus`, `enable`, `handle_line` are used consistently across tasks.
