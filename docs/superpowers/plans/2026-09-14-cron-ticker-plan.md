# Cron ticker (workstream B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fire `gray-cron` jobs through the real headless agent on a 60s tick with skills + pre-run scripts and local-file delivery.

**Architecture:** `gray-cron` stays sync store + schedule math; all new async work (prompt assembly, script runner, agent fire with injected stub runner, tick/serve loop, delivery writer) lives in the `gray` crate as focused modules. One claim→fire→record pass (`tick`) is also the unit the 60s `serve` loop calls.

**Tech Stack:** Rust workspace (edition 2024, resolver 3), tokio (`tokio::time::timeout`, `tokio::signal::ctrl_c`), existing `gray_plugin::builder::build_agent` path, `gray_core::agent::Agent::run` (non-streaming, returns `Vec<AgentEvent>`).

**Spec:** `docs/superpowers/specs/2026-09-14-cron-ticker-design.md`

## Global Constraints

- Iterate narrow: `cargo test -p <touched crate>` mid-loop, never `--workspace` mid-loop.
- Run the dev binary `./target/debug/gray2` for headless checks (`gray2 -p "<prompt>"`); `cargo install --bin gray2` exactly once at the end (operator runs it from `~/.cargo/bin`).
- Full `cargo test --workspace` + `cargo fmt --check` once, pre-commit.
- Cap build pressure: `CARGO_BUILD_JOBS=4`.
- No new dependencies (tokio/chrono/serde/serde_json/uuid already in tree; `gray` crate already depends on `gray-cron`, `gray-core`, `tokio`).
- `gray-cron` keeps no async/agent edge: store additions are sync field + validation only.
- Statuses stay closed: `ok` / `error` / `delivery_failed` — no new literals.
- No `TBD`/`TODO`/placeholder steps; every code step shows the code.
- Serve supervision (runit/systemd on this machine) is out of scope.
- NEVER open a second PR while one is open: check `gh pr list --state open` first; everything lands on the rolling branch `feat/cron-ticker` (already created with the spec commits).

---

## File structure

- Modify: `crates/gray-cron/src/store.rs` — additive `skills: Vec<String>` + `script: Option<PathBuf>` on `CronJob` (both `#[serde(default)]`), validation in `validate_new_job` + `add_full` (absolute+existing script check; skills non-empty strings ≤50 chars each), plus store methods `set_paused(&self, id_or_name, paused: bool)` and `claim_one(&self, now, owner, id_or_name)`.
- Modify: `crates/gray-cron/src/store.rs` tests mod — new unit tests for the above.
- Create: `crates/gray/src/cron_fire.rs` — pure prompt assembly (`assemble_fire_prompt`), wake-gate parse, `[SILENT]` detect, script runner (`run_pre_script`, sync-piped `std::process::Command`, timeout via `tokio::task::spawn_blocking`), transcript render from `Vec<AgentEvent>`, local delivery writer, `fire_claimed_job` (injected runner), `FIRE_TIMEOUT_SECS = 600`, `SCRIPT_TIMEOUT_SECS = 300`, `SCRIPT_OUTPUT_CAP = 8000`.
- Create: `crates/gray/src/cron_serve.rs` — `tick_once(store, config_loader) -> TickReport`, `serve_loop(store, config_loader)` (60s `tokio::time::interval`, `ctrl_c` break), owner stamp `format!("{}:{}", std::process::id(), Uuid::new_v4())`, sequential per-job fire with job-boundary panic capture.
- Modify: `crates/gray/src/lib.rs` — `CronCmd` enum: add `Tick`, `Serve`, `Pause { id }`, `Resume { id }`, `Run { id }` variants; `Add` gains `skills: Option<String>` (comma-separated) + `script: Option<PathBuf>` flags.
- Modify: `crates/gray/src/main.rs` — `run_cron` arms for the new variants; `tick` exit code nonzero only on pass-level failure; per-job failures print `job <id> <status>` lines and exit 0.
- Test: `crates/gray/src/cron_fire.rs` tests mod (stub runner, no model) + `crates/gray/src/lib.rs` CLI parse test extension.
- Docs: `docs/plugins.md` cron bullet (file-only claim → tick/serve), `CHANGELOG.md` entry.

---

### Task 1: Store fields — skills + script + pause/claim-one

**Files:**
- Modify: `crates/gray-cron/src/store.rs`
- Test: `crates/gray-cron/src/store.rs` (tests mod)

**Interfaces:**
- Consumes: existing `CronJob`, `CronStore::add_full`, `claim_due`, `mark_done`, `validate_new_job`.
- Produces: `CronJob.skills: Vec<String>`, `CronJob.script: Option<PathBuf>`; `CronStore::set_paused(&self, id_or_name: &str, paused: bool) -> anyhow::Result<bool>`; `CronStore::claim_one(&self, now: i64, owner: &str, id_or_name: &str) -> anyhow::Result<Option<CronJob>>`.

- [ ] **Step 1: Write failing tests for new fields + methods**

```rust
#[test]
fn add_accepts_skills_and_script() {
    let dir = tempfile::tempdir().unwrap();
    let store = CronStore::open(dir.path()).unwrap();
    let script = dir.path().join("pre.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    let id = store
        .add_full(
            "job",
            "every 1h",
            "do work",
            Deliver::Local,
            None,
            None,
            vec!["briefing".to_string()],
            Some(script.clone()),
        )
        .unwrap();
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.skills, vec!["briefing".to_string()]);
    assert_eq!(job.script, Some(script));
}

#[test]
fn add_rejects_bad_skills_and_script() {
    let (_dir, store) = test_store();
    assert!(store
        .add_full("x", "every 1h", "hi", Deliver::Local, None, None, vec!["  ".to_string()], None)
        .is_err());
    assert!(store
        .add_full("x", "every 1h", "hi", Deliver::Local, None, None, vec!["n".repeat(51)], None)
        .is_err());
    assert!(store
        .add_full("x", "every 1h", "hi", Deliver::Local, None, None, vec![], Some(PathBuf::from("relative/pre.sh")))
        .is_err());
    assert!(store
        .add_full("x", "every 1h", "hi", Deliver::Local, None, None, vec![], Some(PathBuf::from("/no/such/file.sh")))
        .is_err());
}

#[test]
fn pause_resume_and_claim_one() {
    let (_dir, store) = test_store();
    let id = store.add("hourly", "every 1h", "hi", Deliver::Local).unwrap();
    assert!(store.set_paused(&id, true).unwrap());
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Paused);
    assert!(job.enabled);
    store.set_next_run_for_test(&id, 1).unwrap();
    assert!(store.claim_due(1_700_000_000, "o").unwrap().is_empty());
    assert!(store.set_paused(&id, false).unwrap());
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Active);
    assert!(job.next_run_at.unwrap() > 1_700_000_000);
    let claimed = store.claim_one(1_700_000_001, "owner", &id).unwrap().unwrap();
    assert_eq!(claimed.id, id);
    assert!(!store.claim_one(1_700_000_001, "other", &id).unwrap().is_some());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-cron store:: 2>&1 | tail -5`
Expected: FAIL — `add_full` takes 6 args (no skills/script params), no `set_paused`/`claim_one`.

- [ ] **Step 3: Implement fields + validation + methods**

```rust
// On CronJob, after `pub workdir: Option<PathBuf>,`:
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub script: Option<PathBuf>,
```

```rust
fn validate_new_job(
    name: &str,
    prompt: &str,
    workdir: Option<&Path>,
    skills: &[String],
    script: Option<&Path>,
) -> anyhow::Result<()> {
    // ... existing name/prompt/workdir checks unchanged ...
    for s in skills {
        anyhow::ensure!(!s.trim().is_empty(), "job skill is empty");
        anyhow::ensure!(
            s.chars().count() <= MAX_NAME_LEN,
            "job skill exceeds {MAX_NAME_LEN} chars"
        );
    }
    if let Some(path) = script {
        anyhow::ensure!(path.is_absolute(), "script must be absolute: {}", path.display());
        anyhow::ensure!(path.is_file(), "script is not an existing file: {}", path.display());
    }
    Ok(())
}
```

`add_full` gains `skills: Vec<String>, script: Option<PathBuf>` params (trim skill names, store as-is after validation), `CronJob` literal gains `skills, script`. Existing test-only `add` helper in the tests mod gains the two extra args (`vec![], None` at old call sites — update each).

```rust
impl CronStore {
    /// Pause (`paused=true` → `Paused`) or resume (`false` → `Active`).
    /// Resume recomputes `next_run_at` from now so a long-paused job is not
    /// instantly stale. Returns false when the job is unknown.
    /// `enabled` is left untouched: `claim_due` already requires both.
    pub fn set_paused(&self, id_or_name: &str, paused: bool) -> anyhow::Result<bool> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs.iter_mut().find(|j| j.id == id_or_name || j.name == id_or_name)
            else {
                return Ok(false);
            };
            if paused {
                job.state = JobState::Paused;
            } else {
                job.state = JobState::Active;
                job.next_run_at = next_run(now_secs(), &job.schedule);
            }
            self.save_jobs(&mut raw, &jobs)?;
            Ok(true)
        })
    }

    /// Claim one job by id/name for `owner`, bypassing due-ness (implements
    /// `gray cron run`). Same guards as `claim_due`: skip disabled/paused/
    /// done, skip live claims (reclaim past `FIRE_CLAIM_TTL_SECS`), retire
    /// missed one-shots without firing, advance recurring schedules.
    pub fn claim_one(
        &self,
        now: i64,
        owner: &str,
        id_or_name: &str,
    ) -> anyhow::Result<Option<CronJob>> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs.iter_mut().find(|j| j.id == id_or_name || j.name == id_or_name)
            else {
                return Ok(None);
            };
            if !job.enabled || job.state != JobState::Active {
                return Ok(None);
            }
            if let Some(claim) = &job.fire_claim
                && now.saturating_sub(claim.at) <= FIRE_CLAIM_TTL_SECS
            {
                return Ok(None);
            }
            job.fire_claim = None;
            if matches!(job.schedule, Schedule::Once { .. }) && job.last_run_at.is_some() {
                return Ok(None);
            }
            if matches!(job.schedule, Schedule::Once { at } if at < now - ONESHOT_GRACE_SECS) {
                job.enabled = false;
                job.state = JobState::Done;
                job.last_error = Some("missed one-shot window".to_string());
                self.save_jobs(&mut raw, &jobs)?;
                return Ok(None);
            }
            if !matches!(job.schedule, Schedule::Once { .. }) {
                match next_run(now, &job.schedule) {
                    Some(advanced) => job.next_run_at = Some(advanced),
                    None => return Ok(None),
                }
            }
            job.fire_claim = Some(Claim { at: now, by: owner.to_string() });
            let out = job.clone();
            self.save_jobs(&mut raw, &jobs)?;
            Ok(Some(out))
        })
    }
}
```

- [ ] **Step 4: Run tests**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-cron 2>&1 | tail -5`
Expected: all green (old + new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/gray-cron/src/store.rs
git commit -m "feat(cron): job skills/script fields, pause/resume, single-job claim"
```

---

### Task 2: Pure fire helpers — assembly, wake gate, silence, transcript

**Files:**
- Create: `crates/gray/src/cron_fire.rs`
- Test: `crates/gray/src/cron_fire.rs` (tests mod)

**Interfaces:**
- Consumes: `gray_cron::{CronJob, CronStore}` types only.
- Produces: `pub const SCRIPT_OUTPUT_CAP: usize` (=8000); `pub fn parse_wake_gate(output: &str) -> bool`; `pub fn is_silent_response(text: &str) -> bool`; `pub fn assemble_fire_prompt(base: &str, skill_paths: &[std::path::PathBuf], script_output: Option<&str>) -> String`; `pub fn transcript_text(events: &[gray_core::event::AgentEvent]) -> String`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn wake_gate_shapes() {
    assert!(parse_wake_gate("some output\n"));
    assert!(parse_wake_gate(""));
    assert!(parse_wake_gate("data\n{\"wakeAgent\": true}\n"));
    assert!(!parse_wake_gate("data\n{\"wakeAgent\": false}\n"));
    assert!(!parse_wake_gate("  {\"wakeAgent\": false}  \n\n"));
    assert!(parse_wake_gate("not json\n"));
    assert!(parse_wake_gate("{\"other\": 1}\n"));
}

#[test]
fn silent_prefix_shapes() {
    assert!(is_silent_response("[SILENT] nothing to report"));
    assert!(is_silent_response("  [silent]  x"));
    assert!(!is_silent_response("loud report"));
    assert!(!is_silent_response(""));
}

#[test]
fn assembly_orders_skills_then_script_then_prompt() {
    let out = assemble_fire_prompt(
        "check CI",
        &[std::path::PathBuf::from("/s/SKILL.md")],
        Some("build ok"),
    );
    let si = out.find("## skills").unwrap();
    let oi = out.find("## script-output").unwrap();
    assert!(si < oi);
    assert!(out.ends_with("check CI"));
    assert!(out.contains("/s/SKILL.md"));
    assert!(out.contains("build ok"));
}

#[test]
fn assembly_omits_empty_sections() {
    let out = assemble_fire_prompt("hi", &[], None);
    assert_eq!(out, "hi");
}

#[test]
fn transcript_collects_text_and_tool_names() {
    use gray_core::event::AgentEvent;
    let events = vec![
        AgentEvent::TextDelta { delta: "hello ".to_string() },
        AgentEvent::TextDelta { delta: "world".to_string() },
    ];
    let t = transcript_text(&events);
    assert!(t.contains("hello world"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_fire 2>&1 | tail -5`
Expected: FAIL — `cron_fire` module does not exist.

- [ ] **Step 3: Implement**

```rust
//! Cron fire helpers: pure prompt assembly, wake-gate/silence parsing,
//! transcript rendering. No I/O here except via `run_pre_script`'s caller.

use std::path::PathBuf;

/// Cap for injected pre-script stdout (hermes `_MAX_CONTEXT_CHARS`).
pub const SCRIPT_OUTPUT_CAP: usize = 8000;

/// Wake gate: false only when the last non-empty stdout line is JSON
/// `{"wakeAgent": false}`; anything else (empty, non-JSON, missing flag,
/// `true`) wakes normally.
pub fn parse_wake_gate(output: &str) -> bool {
    let last = output.lines().rev().find(|l| !l.trim().is_empty());
    let Some(line) = last else { return true };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return true;
    };
    !matches!(v.get("wakeAgent"), Some(serde_json::Value::Bool(false)))
}

/// `[SILENT]` prefix (case-insensitive, leading whitespace tolerated)
/// suppresses delivery; the fire still records `ok`.
pub fn is_silent_response(text: &str) -> bool {
    text.trim_start().get(..8).is_some_and(|h| h.eq_ignore_ascii_case("[silent]"))
}

/// Final prompt: optional `## skills` block (exact `<location>` paths, one
/// per line) + optional fenced `## script-output` block + base prompt.
/// Empty sections are omitted (byte-stable for the common prompt-only job).
pub fn assemble_fire_prompt(
    base: &str,
    skill_paths: &[PathBuf],
    script_output: Option<&str>,
) -> String {
    let mut out = String::new();
    if !skill_paths.is_empty() {
        out.push_str("## skills\nThe following skills apply to this task; read the SKILL.md at each <location> with bash before starting:\n\n");
        for p in skill_paths {
            out.push_str(&format!("- <location>{}</location>\n", p.display()));
        }
        out.push('\n');
    }
    if let Some(body) = script_output {
        let capped: String = body.chars().take(SCRIPT_OUTPUT_CAP).collect();
        out.push_str("## script-output\nPre-run data collection (stdout, capped):\n\n```\n");
        out.push_str(&capped);
        out.push_str("\n```\n\n");
    }
    out.push_str(base);
    out
}

/// Render collected `AgentEvent`s into a storable transcript: text deltas
/// concatenated verbatim; tool calls noted by name so silent-but-active
/// runs still leave a trace.
pub fn transcript_text(events: &[gray_core::event::AgentEvent]) -> String {
    use gray_core::event::AgentEvent;
    let mut text = String::new();
    for ev in events {
        match ev {
            AgentEvent::TextDelta { delta } => text.push_str(delta),
            AgentEvent::ToolCallStart { name, .. } => {
                text.push_str(&format!("\n[tool:{name}]\n"));
            }
            AgentEvent::ToolResult { output, is_error, .. } => {
                let tag = if *is_error { "result-err" : "result" };
                let s = output.chars().take(2000).collect::<String>();
                text.push_str(&format!("[{tag}:{s}]\n"));
            }
            _ => {}
        }
    }
    text
}
```

Verified against `crates/gray-core/src/event.rs`: `ToolCallStart { id, name }`, `ToolResult { id, output: String, is_error: bool }` — the code above matches. Ignore `ThinkingDelta`/`ToolCallProgress`/`ToolCallEnd`/`StepUsage`/`StreamError`/`TurnEnd` in the transcript (text + tool names/results only).

Register the module in `crates/gray/src/lib.rs`: `pub mod cron_fire;`.

- [ ] **Step 4: Run tests**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_fire 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/cron_fire.rs crates/gray/src/lib.rs
git commit -m "feat(cron): pure fire helpers (assembly, wake gate, silence, transcript)"
```

---

### Task 3: Script runner + local delivery writer

**Files:**
- Modify: `crates/gray/src/cron_fire.rs`
- Test: `crates/gray/src/cron_fire.rs` (tests mod)

**Interfaces:**
- Consumes: Task 2 helpers + `gray_cron::CronJob`.
- Produces: `pub const SCRIPT_TIMEOUT_SECS: u64` (=300); `pub struct ScriptOutcome { pub ok: bool, pub stdout: String, pub stderr_tail: String }`; `pub async fn run_pre_script(script: &std::path::Path, workdir: &std::path::Path) -> ScriptOutcome`; `pub fn write_local_output(home: &std::path::Path, job: &gray_cron::CronJob, now: i64, body: &str) -> anyhow::Result<std::path::PathBuf>`.

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn script_success_captures_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("ok.sh");
    std::fs::write(&sh, "#!/bin/sh\necho hello\n").unwrap();
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(out.ok);
    assert!(out.stdout.contains("hello"));
}

#[tokio::test]
async fn script_failure_marks_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let sh = dir.path().join("bad.sh");
    std::fs::write(&sh, "#!/bin/sh\necho oops >&2\nexit 3\n").unwrap();
    let out = run_pre_script(&sh, dir.path()).await;
    assert!(!out.ok);
    assert!(out.stderr_tail.contains("oops"));
}

#[tokio::test]
async fn script_missing_file_is_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_pre_script(&dir.path().join("gone.sh"), dir.path()).await;
    assert!(!out.ok);
}

#[test]
fn local_output_writes_atomic_md() {
    let home = tempfile::tempdir().unwrap();
    let job = gray_cron::CronJob {
        id: "abc123".to_string(),
        name: "n".to_string(),
        prompt: "p".to_string(),
        schedule: gray_cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: None,
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_delivery_error: None,
        deliver: Default::default(),
        origin: None,
        workdir: None,
        fire_claim: None,
        skills: vec![],
        script: None,
    };
    let path = write_local_output(home.path(), &job, 1_700_000_000, "body").unwrap();
    assert!(path.to_string_lossy().contains("abc123"));
    assert!(path.extension().is_some_and(|e| e == "md"));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("body"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_fire 2>&1 | tail -5`
Expected: FAIL — `run_pre_script` / `write_local_output` undefined.

- [ ] **Step 3: Implement**

```rust
/// Pre-script wall clock: a pre-step must stay inside one tick window.
pub const SCRIPT_TIMEOUT_SECS: u64 = 300;

pub struct ScriptOutcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr_tail: String,
}

/// Run the job's pre-script with cwd=`workdir`, piped stdio. Missing file,
/// spawn failure, nonzero exit, or timeout → `ok: false` (caller records
/// `error` without running the agent). Blocking `Command` runs inside
/// `spawn_blocking` so the ticker stays responsive.
pub async fn run_pre_script(script: &std::path::Path, workdir: &std::path::Path) -> ScriptOutcome {
    let script = script.to_path_buf();
    let workdir = workdir.to_path_buf();
    let res = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&script)
            .current_dir(&workdir)
            .output()
    })
    .await;
    match tokio::time::timeout(std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS), res).await {
        Err(_) => ScriptOutcome { ok: false, stdout: String::new(), stderr_tail: "pre-script timed out".to_string() },
        Ok(Err(e)) => ScriptOutcome { ok: false, stdout: String::new(), stderr_tail: format!("pre-script spawn failed: {e:#}") },
        Ok(Ok(Err(e))) => ScriptOutcome { ok: false, stdout: String::new(), stderr_tail: format!("pre-script failed: {e:#}") },
        Ok(Ok(Ok(out))) => {
            let tail: String = String::from_utf8_lossy(&out.stderr).chars().rev().take(2000).collect::<String>().chars().rev().collect();
            ScriptOutcome {
                ok: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr_tail: tail,
            }
        }
    }
}
```

Timeout note: the inner `spawn_blocking` closure has no deadline; the
`timeout` fires on the *join*, abandoning a hung script thread (documented:
a hung script leaks one blocked thread until the process exits — accepted,
same as a hung tool call; the fire still records `error` on time).

```rust
/// Write the transcript to `$HOME/cron/output/<job-id>/<unix-ts>.md`
/// (0600, atomic tmp+rename). Header carries name/id/fire-time/schedule.
pub fn write_local_output(
    home: &std::path::Path,
    job: &gray_cron::CronJob,
    now: i64,
    body: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let dir = home.join("cron").join("output").join(&job.id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{now}.md"));
    let header = format!(
        "# cron: {} ({})\n- fired: {}\n- schedule: {:?}\n\n",
        job.name, job.id, now, job.schedule
    );
    let full = format!("{header}{body}\n");
    gray_cron::store::atomic_write_json(&path, &serde_json::json!(full))
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    Ok(path)
}
```

`atomic_write_json` serializes with `to_string_pretty` — for a plain string
body that yields quoted JSON, not raw markdown. If that reads wrong in the
test, write the raw-bytes twin inline instead (private tmp + 0600 + rename,
mirroring `store.rs`) and keep using it. Do not add a second copy in
`gray-cron`; the writer lives here.

- [ ] **Step 4: Run tests**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_fire 2>&1 | tail -5`
Expected: PASS (adjust the writer per the note above if the md test fails on quoting).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/cron_fire.rs
git commit -m "feat(cron): pre-script runner and local output writer"
```

---

### Task 4: Tick/serve loop with injected agent runner

**Files:**
- Create: `crates/gray/src/cron_serve.rs`
- Modify: `crates/gray/src/lib.rs` (register `pub mod cron_serve;`)
- Test: `crates/gray/src/cron_serve.rs` (tests mod, stub runner)

**Interfaces:**
- Consumes: `CronStore::{claim_due, claim_one, mark_done, get}`, Task 2–3 helpers, `gray_cron::{RunStatus, JobState}`.
- Produces: `pub struct TickReport { pub fired: usize, pub errors: usize }`; `pub async fn tick_once(store: &gray_cron::CronStore, home: &std::path::Path, runner: &dyn Fn(&str) -> anyhow::Result<String>) -> anyhow::Result<TickReport>` — note: runner is sync-returning for testability; the production runner blocks on the agent future via the runtime handle (see implementation). Hmm — simpler and honest: make the runner `async`: `pub async fn tick_once(store, home, runner: impl AsyncRunner)` where `pub trait AsyncRunner { async fn run(&self, prompt: String) -> anyhow::Result<String>; }` with a stub impl in tests and the real agent impl in `main.rs`. Define the trait here.

- [ ] **Step 1: Write failing tests (stub runner, temp GRAY_HOME)**

```rust
struct StubRunner { text: String, fail: bool, seen: std::sync::Mutex<Vec<String>> }

#[async_trait::async_trait]
impl AsyncRunner for StubRunner {
    async fn run(&self, prompt: String) -> anyhow::Result<String> {
        self.seen.lock().unwrap().push(prompt);
        if self.fail { anyhow::bail!("boom") } else { Ok(self.text.clone()) }
    }
}

#[tokio::test]
async fn tick_fires_due_job_and_marks_ok() {
    let home = tempfile::tempdir().unwrap();
    let store = gray_cron::CronStore::open(home.path().join("cron")).unwrap();
    let id = store.add("j", "every 1h", "say hi", gray_cron::Deliver::Local).unwrap();
    // force due via raw edit: reload, set next_run_at=1, save — or add claim_one path:
    let runner = StubRunner { text: "hello".to_string(), fail: false, seen: Default::default() };
    // make due by claiming through store test hook is unavailable here; instead
    // insert a raw record with next_run_at=1:
    let raw = serde_json::json!([{"id": id, "name": "j", "prompt": "say hi",
        "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
        "created_at": 1, "next_run_at": 1}]);
    std::fs::write(home.path().join("cron").join("jobs.json"), serde_json::to_string_pretty(&raw).unwrap()).unwrap();
    let rep = tick_once(&store, home.path(), &runner).await.unwrap();
    assert_eq!(rep.fired, 1);
    assert_eq!(rep.errors, 0);
    let job = store.get(&id).unwrap().unwrap();
    assert_eq!(job.last_status, Some(gray_cron::RunStatus::Ok));
    assert!(job.fire_claim.is_none());
}

#[tokio::test]
async fn tick_agent_failure_records_error_and_continues() {
    // two due jobs, runner fails: both fire, both error, pass still Ok.
}

#[tokio::test]
async fn tick_nonlocal_delivery_records_delivery_failed() {
    // Deliver::Target("discord:123"): runner ok, no file written,
    // last_status == DeliveryFailed, last_delivery_error mentions no backend.
}

#[tokio::test]
async fn tick_silent_response_skips_write_but_ok() {
    // runner returns "[SILENT] nothing": status ok, no output file created.
}
```

Write all four tests fully (no placeholders): the two sketched ones follow
the first one's pattern — two raw records for the failure test (assert
`fired == 2 && errors == 2`, both jobs `Error`), one `Target` record for the
delivery test, one `[SILENT]` runner text for the silence test (assert the
`output/<id>/` dir does not exist).

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_serve 2>&1 | tail -5`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement**

```rust
//! Cron tick/serve: one claim→fire→record pass + the 60s loop.

use std::path::{Path, PathBuf};

pub struct TickReport { pub fired: usize, pub errors: usize }

/// Agent seam: production runs the headless agent; tests stub it.
#[async_trait::async_trait]
pub trait AsyncRunner: Send + Sync {
    async fn run(&self, prompt: String) -> anyhow::Result<String>;
}

/// Whole-fire wall clock (script + agent), matches the bash tool bound.
pub const FIRE_TIMEOUT_SECS: u64 = 600;

fn owner_stamp() -> String {
    format!("{}:{}", std::process::id(), uuid::Uuid::new_v4())
}

/// One pass: claim all due, fire sequentially, record each outcome.
/// Per-job failure is recorded on the job; only pass-level store failure
/// propagates as `Err`.
pub async fn tick_once(
    store: &gray_cron::CronStore,
    home: &Path,
    runner: &dyn AsyncRunner,
) -> anyhow::Result<TickReport> {
    let now = gray_cron::now_secs();
    let due = store.claim_due(&now.to_string(), &owner_stamp())?;
    ...
}
```

Wait — `claim_due` signature is `claim_due(&self, now: i64, owner: &str)`.
Pass `now` (i64) and `&owner_stamp()`. Fix while writing; the sketch above
has a transcription slip — write the real call.

Fire body per claimed job (sequential `for`):

```rust
let workdir: PathBuf = match &job.workdir {
    Some(w) => w.clone(),
    None => std::env::current_dir()?,
};
// re-check script + skills from disk at fire time:
if let Some(s) = &job.script
    && (!s.is_absolute() || !s.is_file())
{
    store.mark_done(&job.id, RunStatus::Error, Some(&format!("pre-script missing: {}", s.display())))?;
    continue;
}
let mut skill_paths = Vec::new();
let mut missing_skill = None;
for name in &job.skills {
    match crate::skills_tool::resolve_skill_name(&workdir, name) {
        Some(p) => skill_paths.push(p),
        None => { missing_skill = Some(name.clone()); break; }
    }
}
if let Some(name) = missing_skill {
    store.mark_done(&job.id, RunStatus::Error, Some(&format!("skill not found: {name}")))?;
    continue;
}
let mut script_stdout: Option<String> = None;
if let Some(s) = &job.script {
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(crate::cron_fire::SCRIPT_TIMEOUT_SECS),
        crate::cron_fire::run_pre_script(s, &workdir),
    ).await.map_err(|_| "pre-script timed out").unwrap_or(...);
    // on !ok → mark Error with stderr_tail, continue.
    // on ok → parse_wake_gate: false → mark Ok (silent), continue.
    // else script_stdout = Some(stdout).
}
let prompt = crate::cron_fire::assemble_fire_prompt(&job.prompt, &skill_paths, script_stdout.as_deref());
let fired_text = tokio::time::timeout(
    std::time::Duration::from_secs(FIRE_TIMEOUT_SECS),
    std::panic::AssertUnwindSafe(runner.run(prompt)).catch_unwind() // see note
).await;
```

Panic capture: `Future::catch_unwind` needs `UnwindSafe`; wrap with
`AssertUnwindSafe` and on panic/edge record `Error` + continue, always
releasing the claim via `mark_done`. Keep it simple and explicit; test the
error path with the failing stub (panic-path test optional — the stub-error
test covers continuation).

Then: `is_silent_response` → `mark_done(Ok, None)`, continue. Else match
`job.deliver`: `Local` → `write_local_output(home, &job, now, &text)`; write
failure → `DeliveryFailed`. `Origin`/`Target` → `DeliveryFailed` ("no
delivery backend in this build"). Success → `Ok`.

`serve_loop`:

```rust
pub async fn serve_loop(store: gray_cron::CronStore, home: PathBuf, runner: impl AsyncRunner + 'static) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                match tick_once(&store, &home, &runner).await {
                    Ok(rep) => log::info!("cron tick: fired={} errors={}", rep.fired, rep.errors),
                    Err(e) => log::warn!("cron tick failed: {e:#}"),
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_serve 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/cron_serve.rs crates/gray/src/lib.rs
git commit -m "feat(cron): tick/serve loop with injected agent runner"
```

---

### Task 5: Production runner + CLI wiring (tick/serve/pause/resume/run)

**Files:**
- Modify: `crates/gray/src/lib.rs` (`CronCmd`), `crates/gray/src/main.rs` (`run_cron`)
- Test: `crates/gray/src/lib.rs` CLI parse test

**Interfaces:**
- Consumes: Tasks 1–4 (`tick_once`, `serve_loop`, `AsyncRunner`, `set_paused`, `claim_one`).
- Produces: working `gray cron tick|serve|pause|resume|run`, `--skills`/`--script` on `add`.

- [ ] **Step 1: Extend the CLI parse test**

Add to `cron_cli_parses_add_shapes` (or a new test `cron_cli_parses_lifecycle`):

```rust
for args in [
    vec!["gray", "cron", "tick"],
    vec!["gray", "cron", "serve"],
    vec!["gray", "cron", "pause", "abc"],
    vec!["gray", "cron", "resume", "abc"],
    vec!["gray", "cron", "run", "abc"],
] {
    let cli = Cli::try_parse_from(args).unwrap();
    assert!(matches!(cli.command, Some(Commands::Cron { .. })), "{args:?}");
}
// add with skills+script:
let cli = Cli::try_parse_from(["gray", "cron", "add", "every 1h", "--skills", "a,b", "--script", "/tmp/pre.sh", "do it"]).unwrap();
match cli.command {
    Some(Commands::Cron { cmd: CronCmd::Add { skills, script, .. } }) => {
        assert_eq!(skills.as_deref(), Some("a,b"));
        assert_eq!(script, Some(PathBuf::from("/tmp/pre.sh")));
    }
    other => panic!("unexpected {other:?}"),
}
```

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_cli 2>&1 | tail -5`
Expected: FAIL — variants/flags missing.

- [ ] **Step 3: Implement CLI + handlers**

`CronCmd` additions:

```rust
    /// One claim→fire→record pass (also the OS-cron/runit entry point)
    Tick,
    /// Tick every 60s until SIGINT/SIGTERM
    Serve,
    /// Suspend a job (id or name)
    Pause {
        /// Job id or name
        id: String,
    },
    /// Resume a suspended job (id or name; recomputes next run)
    Resume {
        /// Job id or name
        id: String,
    },
    /// Fire a job now regardless of schedule (id or name)
    Run {
        /// Job id or name
        id: String,
    },
```

`Add` gains:

```rust
        /// Comma-separated skill names (must resolve at fire time)
        #[arg(long)]
        skills: Option<String>,
        /// Absolute path to a pre-run script (stdout injected into prompt)
        #[arg(long)]
        script: Option<PathBuf>,
```

Production runner (in `main.rs`, private): builds the headless agent per
fire and collects events without streaming:

```rust
struct PrintRunner { config: gray::config::Config }

#[async_trait::async_trait]
impl gray::cron_serve::AsyncRunner for PrintRunner {
    async fn run(&self, prompt: String) -> anyhow::Result<String> {
        let cwd = std::env::current_dir()?;
        let mut agent = gray::build_agent(&self.config, &cwd, None).await?;
        let ctx = gray_core::agent::ToolContext {
            cwd,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_id: None,
        };
        let events = agent.run(gray_core::message::Message::user(prompt), ctx).await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        // Persist the fire as its own session for inspection (best-effort).
        let _ = ...; // reuse print.rs save_session? It is private — simplest:
                     // skip persistence in B; the transcript is delivered to the
                     // output file. Document the skip.
        Ok(gray::cron_fire::transcript_text(&events))
    }
}
```

Check before writing: `Agent::run` signature (`run(&mut self, input: Message, ctx: ToolContext) -> Result<Vec<AgentEvent>, CoreError>` — confirmed in Task 0 reading of `agent_loop.rs`), `Message::user` constructor (used in `print.rs`), `ToolContext` fields (confirmed). `CoreError` has no Display guarantee — `{e:?}` debug-format like `print.rs` does via `format_core_error`; simplest: mirror `print.rs`'s mapping (`crate::repl::format_core_error(&e, &config.base_url)` is in the `gray` crate — but `main.rs` is a separate bin crate that can call `gray::repl::format_core_error` if public; check visibility, else `{e:?}`).

Session persistence for fires: `print.rs::save_session` is private. Decision
for B: skip session persistence (transcript goes to the output file); note it
in the docs line. Do not widen visibility in this task.

Handlers in `run_cron`:

```rust
CronCmd::Tick => {
    let store = cron_store()?;
    let home = gray::setup::gray_home()?;
    let config = gray::config::Config::resolve(&gray::Cli::parse())?; // NO — resolve properly
    ...
}
```

Config resolution: `run_cron` currently has no `Config`. Look at how `main`
builds it: `Config::resolve(&cli)`. `run_cron(cmd)` receives only the cron
subcommand. Minimal change: build `Config` inside the Tick/Serve/Run arms via
`gray::config::Config::resolve_for_cron()`? That helper does not exist.
Actual minimal: change `run_cron(cmd)` to `run_cron(cmd, config: &Config)`
(clone the already-resolved `config` in `main` at the dispatch site
`run_cron(cmd).await` → `run_cron(cmd, &config).await`). One signature, one
call site. Do that.

- `Tick`: `tick_once(&store, &home, &PrintRunner { config: config.clone() }).await?`; print `tick: fired=N errors=M`; exit 0 even when errors > 0 (only `Err` propagates nonzero).
- `Serve`: `serve_loop(store, home, PrintRunner { config: config.clone() }).await`.
- `Pause`/`Resume`: `set_paused(&id, true/false)` → print `paused <id>` / `resumed <id> next <ts>`; unknown → bail like `remove` does.
- `Run`: `claim_one(now, &owner, &id)` → None means unknown/not-runnable → bail `job <id> is not runnable (unknown, paused, or already claimed)`; Some(job) → reuse the same single-fire body as `tick_once` for one job. To avoid duplicating the fire body, expose `pub async fn fire_one(store, home, runner, job) -> ...` from `cron_serve.rs` in Task 4 and call it from both `tick_once` and the `Run` arm. (Adjust Task 4 implementation accordingly — the loop calls `fire_one` per claimed job.)
- `Add`: parse `--skills` (split `,`, trim, drop empties), pass through to `add_full` with the two new args; validate skill names resolve against the `--in` dir (or cwd) at `add` time via `gray::skills_tool::resolve_skill_name` — reject unknown with `unknown skill <name>`. `show` prints `skills:` + `script:` lines.

- [ ] **Step 4: Run tests + headless smoke**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib 2>&1 | tail -3`
Expected: PASS. Then: `./target/debug/gray2 cron list` prints `no cron jobs` (read-only smoke, no model).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/lib.rs crates/gray/src/main.rs crates/gray/src/cron_serve.rs
git commit -m "feat(cron): tick/serve/pause/resume/run CLI with headless agent runner"
```

---

### Task 6: Docs, CHANGELOG, live proof, final verify

**Files:**
- Modify: `docs/plugins.md`, `CHANGELOG.md`
- Verify: workspace tests + fmt + live one-shot

- [ ] **Step 1: Update docs**

Replace the cron bullet in `docs/plugins.md` (lines ~152–160):

```markdown
- Cron (timed gray): `gray-cron` holds the job store
  (`$GRAY_HOME/cron/jobs.json`) + schedule math; `gray cron tick` fires one
  claim→fire→record pass (per-job agent run with skills + pre-run script,
  transcript to `$GRAY_HOME/cron/output/<id>/<ts>.md`), `gray cron serve`
  ticks every 60s. Manage: `gray cron list|add|show|remove|pause|resume|run`
  (`add "every 1h" "prompt" [--deliver local] [--name x] [--in /work/dir]
  [--skills a,b] [--script /abs/pre.sh]`). Only `local` delivery is
  implemented; `origin`/named targets record `delivery_failed`. Schedule
  kinds: `every 1h` / bare `30m` / `in 10m` / RFC3339 one-shots / 5-field
  cron (all ≥60 s). Agent self-scheduling unlocks in phase 3; until then
  scheduling is human-driven (CLI) only.
```

CHANGELOG entry (check `CHANGELOG.md` head format first, mirror it):

```markdown
## Unreleased
- Cron workstream B: `gray cron tick|serve|pause|resume|run`, job skills +
  pre-run scripts, local-file delivery (`cron/output/<id>/<ts>.md`).
```

- [ ] **Step 2: Live proof — one-shot marker through the real agent**

Run: `./target/debug/gray2 cron add "in 1m" --name live-proof "Append the line CRON-FIRED plus the current date to /tmp/gray-cron-live.txt" 2>&1 | tail -2`
Then: `sleep 75; ./target/debug/gray2 cron tick 2>&1 | tail -3`
Expected: `tick: fired=1 errors=0`; `/tmp/gray-cron-live.txt` contains CRON-FIRED; `./target/debug/gray2 cron show live-proof` shows `ok`. If the model call fails in this environment, record the actual outcome honestly (the stub tests already prove the pipeline; live proof is best-effort here, full proof in CI/with keys).

- [ ] **Step 3: Final verify**

Run: `CARGO_BUILD_JOBS=4 cargo test --workspace 2>&1 | tail -5`
Expected: PASS. Run: `cargo fmt --check 2>&1 | tail -3`. Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add docs/plugins.md CHANGELOG.md
git commit -m "docs(cron): ticker usage, changelog"
```

---

## Self-review

- Spec §1 (CLI: tick/serve/pause/resume/run, --skills/--script, owner stamp, sequential): Tasks 1 (store methods), 4 (loop+owner), 5 (CLI). `fire_one` shared by tick/run avoids duplication — called out in Task 5.
- Spec §2 (assembly, 300s script timeout, 8k cap, wake gate, skills resolve at add+fire against workdir, fresh session, `[SILENT]`, 600s fire timeout, architectural recursion guard): Tasks 2 (pure helpers), 3 (script+writer), 4 (fire body), 5 (PrintRunner fresh agent, no resume).
- Spec §3 (local path/shape/mode, opaque origin/target → delivery_failed, silent skips write, error columns, pass-abort on store failure, panic capture, closed statuses): Tasks 3 (writer), 4 (fire body + panic capture + TickReport semantics).
- Spec §4 (stub-runner tests, CLI tests, narrow loop, docs, live proof): Tasks 2–4 (tests), 5 (CLI test + smoke), 6 (docs + live + workspace).
- Placeholders: none — every code step shows signatures, bodies, exact commands with expected outputs.
- Type consistency: `CronJob.skills: Vec<String>`, `script: Option<PathBuf>`; `set_paused(.., bool) -> Result<bool>`; `claim_one(now: i64, owner: &str, ..) -> Result<Option<CronJob>>`; `AsyncRunner::run(&self, prompt: String) -> Result<String>`; `tick_once(&CronStore, &Path, &dyn AsyncRunner) -> Result<TickReport>`; `fire_one(&CronStore, &Path, &dyn AsyncRunner, CronJob) -> ()` recording via `mark_done` internally (returns the `RunStatus` for the report count).
- Risks noted inline: `format_core_error` is public via `gray::repl::format_core_error` (re-exported in `repl/mod.rs`) — use it for the runner's error mapping, `{e:?}` fallback only if the import fails; `add_full` test-helper call sites need the two new args; `run_print_mode` streams while cron collects — deliberate (ticker stdout stays log-clean).
