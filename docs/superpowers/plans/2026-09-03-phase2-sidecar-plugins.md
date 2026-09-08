# Phase 2: Sidecar Plugins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Out-of-process `gray-plugin-*` binaries speaking JSON-RPC over stdio, behind the same `Plugin` trait from Phase 1.

**Architecture:** `SidecarPlugin` struct in `gray-plugin/src/sidecar.rs` spawns the child, sends newline-delimited JSON-RPC (`plugin/manifest`, `tool/list`, `tool/call`, `event/notify`), enforces a 5s hook timeout. Tool calls map to `ToolOutput`; timeout/crash degrades to skip + warning, never a dead CLI.

**Tech Stack:** Rust, tokio process + timeout, serde_json. Test stub: a POSIX shell script.

**Spec:** `docs/superpowers/specs/2026-09-03-plugin-system-design.md`

## Global Constraints

- Same `Plugin` trait, no trait or loop changes.
- Hook timeout default 5s, skip + `log::warn!` on timeout/crash.
- `cargo check` clean after every task.
- Requires Phase 1 plan complete (trait, manifest, profile loader exist).

---

### Task 1: `SidecarPlugin` — manifest + tool call

**Files:**
- Create: `crates/gray-plugin/src/sidecar.rs`
- Modify: `crates/gray-plugin/src/lib.rs` (add `pub mod sidecar;`)
- Create: `crates/gray-plugin/testdata/echo_plugin.sh` (stub answering manifest/list/call)

**Interfaces:**
- Consumes: `gray_plugin::{Plugin, Manifest, CoreEvent}` (Phase 1)
- Produces: `gray_plugin::sidecar::SidecarPlugin::spawn(argv: Vec<String>) -> anyhow::Result<Self>`

Protocol (newline JSON, `id` echoed):
- `{"id":1,"method":"plugin/manifest"}` → `{"id":1,"result":{"name":"echo","version":"0.1.0","tools":["echo"]}}`
- `{"id":2,"method":"tool/call","params":{"name":"echo","args":{}}}` → `{"id":2,"result":{"content":"hi","is_error":false}}`
- `event/notify` is a notification (no `id`, no reply expected).

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn sidecar_manifest_and_tool_call() {
    let p = SidecarPlugin::spawn(vec!["testdata/echo_plugin.sh".into()]).await.unwrap();
    assert_eq!(p.manifest().name, "echo");
    let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
    assert_eq!(out.content, "hi");
    assert!(!out.is_error);
}
```

Stub script (testdata/echo_plugin.sh): read lines on stdin, case on method, print canned JSON replies.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray-plugin sidecar 2>&1 | tail -5`
Expected: FAIL, module not found

- [ ] **Step 3: Write minimal implementation**

```rust
pub struct SidecarPlugin { name: String, tools: Vec<Arc<dyn Tool>>, child: tokio::process::Child, /* + bufio handles behind Mutex */ }
// spawn: start child with piped stdio, call plugin/manifest, build SidecarTool handles
// SidecarTool::execute: send tool/call with 30s timeout; on timeout/kill return ToolOutput::error("plugin timeout: echo")
```

Tool-call timeout 30s; `on_event` RPC timeout 5s returning `None` (skip) on expiry.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray-plugin 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-plugin
git commit -m "feat(plugin): SidecarPlugin with manifest and tool/call over stdio"
```

### Task 2: `event/notify` + crash/timeout degradation

**Files:**
- Modify: `crates/gray-plugin/src/sidecar.rs`
- Create: `crates/gray-plugin/testdata/hang_plugin.sh` (never replies), `crates/gray-plugin/testdata/crash_plugin.sh` (exits 1 on second call)

**Interfaces:**
- Consumes: `SidecarPlugin` from Task 1
- Produces: `Plugin::on_event` for sidecars (5s timeout → `None` + warn; dead child → `None` + warn + lazy respawn on next call)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn hanging_hook_times_out_and_skips() {
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()]).await.unwrap();
    let t = std::time::Instant::now();
    assert!(p.on_event(CoreEvent::TurnEnd { usage: Usage::default() }).await.is_none());
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn crashed_plugin_returns_error_not_panic() {
    let p = SidecarPlugin::spawn(vec!["testdata/crash_plugin.sh".into()]).await.unwrap();
    let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
    assert!(out.is_error);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gray-plugin sidecar 2>&1 | tail -8`
Expected: FAIL (hang test exceeds timeout or panics)

- [ ] **Step 3: Write minimal implementation**

Wrap the stdio write+read in `tokio::time::timeout(Duration::from_secs(5), ...)`. On elapsed: `log::warn!(target: "gray_plugin", "sidecar {name} hook timeout, skipping")`, return `None`. On child exit detected (`child.try_wait()` → `Some`): warn once, return error `ToolOutput`/skip; respawn lazily on next `execute`/`on_event` via stored argv.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gray-plugin 2>&1 | tail -5`
Expected: PASS, both new tests green

- [ ] **Step 5: Commit**

```bash
git add crates/gray-plugin
git commit -m "feat(plugin): sidecar hook timeout and crash degradation"
```

### Task 3: `sidecar:` entries in `gray.yml`

**Files:**
- Modify: `crates/gray-plugin/src/profile.rs` (accept `sidecar:` entries: string path or argv list)
- Modify: `crates/gray/src/lib.rs` (instantiate `SidecarPlugin::spawn` per entry, push to plugin list)

**Interfaces:**
- Consumes: `SidecarPlugin::spawn`, `load_profile` (Phase 1 Task 3)
- Produces: profile entries `sidecar: ~/.gray/plugins/my-tools` and `sidecar: [npx, -y, my-tools]`; boot failure names the offending entry

```yaml
plugins:
  - builtin: tools-basic
  - sidecar: ~/.gray/plugins/my-tools
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn load_profile_with_sidecar() {
    let entries = gray_plugin::profile::load_entries("testdata/gray_sidecar.yml");
    assert!(matches!(entries[1], PluginEntry::Sidecar(_)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray-plugin profile 2>&1 | tail -5`
Expected: FAIL, `load_entries`/`Sidecar` variant missing

- [ ] **Step 3: Write minimal implementation**

Change `ProfileEntry` to `enum { Builtin { builtin: String }, Sidecar(SidecarSpec) }` with `#[serde(untagged)]`-friendly shape: `{builtin: "..."}` vs `{sidecar: "..." | [...]}`. Keep `load_profile` returning names for builtins; add `load_entries` returning the enum. Boot: `anyhow::Context` with entry index on spawn failure (`sidecar[1] (~/.gray/plugins/x) failed to spawn: ...`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray-plugin 2>&1 | tail -3 && cargo check 2>&1 | tail -2`
Expected: PASS, check clean

- [ ] **Step 5: Commit**

```bash
git add crates/gray-plugin crates/gray/src/lib.rs gray.yml
git commit -m "feat(plugin): sidecar entries in gray.yml profile"
```
