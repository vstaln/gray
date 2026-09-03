# Phase 1: In-Process Plugins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Built-in tools/providers become self-registering `Plugin` impls composed via `gray.yml` profile order.

**Architecture:** New `gray-plugin` crate owns the `Plugin` trait, `Manifest`, and profile loader. `gray-tools` and `gray-provider` each gain one `plugin()` module. `Registry::builtin()` becomes profile-ordered collection. No loop changes except event-bus hooks if trivial; otherwise events ride along as notify-only stubs.

**Tech Stack:** Rust, tokio, serde_yaml, existing `Tool`/`Provider` traits.

**Spec:** `docs/superpowers/specs/2026-09-03-plugin-system-design.md`

## Global Constraints

- `async fn on_event` on the trait from day one.
- Reuse `gray-tools::Tool` and `gray-core::Provider` as-is; no signature changes.
- Later profile entries win on tool-name conflict.
- `cargo check` clean after every task.

---

### Task 1: `gray-plugin` crate with trait + manifest

**Files:**
- Create: `crates/gray-plugin/Cargo.toml`
- Create: `crates/gray-plugin/src/lib.rs`
- Modify: `Cargo.toml` (workspace members += `crates/gray-plugin`)

**Interfaces:**
- Consumes: `gray_core::agent::{Provider, ToolContext, ToolOutput}`, `gray_tools::Tool`, `gray_core::message::ToolDef`
- Produces: `gray_plugin::{Plugin, Manifest, CoreEvent}` — `CoreEvent` is `enum { PreStep { messages: Vec<Message> }, PreTool { name: String, args: Value }, PostTool { name: String, output: ToolOutput }, TurnEnd { usage: Usage } }`, all async-compatible (`on_event(&self, e: CoreEvent) -> impl Future<Output = Option<CoreEvent>>` via `async_trait`)

- [ ] **Step 1: Write the failing test**

```rust
// crates/gray-plugin/tests/manifest.rs
#[test]
fn later_entry_wins_on_name_conflict() {
    let merged = gray_plugin::merge_manifests(vec![manifest_a(), manifest_b()]);
    assert_eq!(merged.tool_owner("read"), "plugin-b");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray-plugin 2>&1 | tail -5`
Expected: FAIL with "no such crate" / unresolved import

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/gray-plugin/src/lib.rs
use std::{collections::HashMap, sync::Arc};
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub enum CoreEvent { /* PreStep/PreTool/PostTool/TurnEnd as above */ }

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<String>,
    pub provider: Option<String>,
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn gray_tools::Tool>> { vec![] }
    fn provider(&self) -> Option<Arc<dyn gray_core::agent::Provider>> { None }
    async fn on_event(&self, _e: CoreEvent) -> Option<CoreEvent> { None }
}

/// Later manifests win on tool-name conflict. Returns owner per tool name.
pub fn merge_manifests(manifests: Vec<Manifest>) -> HashMap<String, String> {
    let mut owner = HashMap::new();
    for m in manifests {
        for t in &m.tools {
            owner.insert(t.clone(), m.name.clone());
        }
    }
    owner
}
```

Plus `Cargo.toml` with `async-trait`, `serde`, `gray-core`/`gray-tools` path deps, and workspace member registration.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray-plugin 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-plugin Cargo.toml
git commit -m "feat(plugin): gray-plugin crate with Plugin trait and manifest merge"
```

### Task 2: Built-in plugin impls + profile-ordered registry

**Files:**
- Create: `crates/gray-tools/src/plugin.rs` (`ToolsBasicPlugin`, `ToolsSearchPlugin`)
- Modify: `crates/gray-tools/src/lib.rs` (add `pub mod plugin;`, add `Registry::from_plugins`)
- Create: `gray.yml` (repo default profile)
- Modify: `crates/gray/src/lib.rs:189,215` (use profile loader instead of `Registry::builtin()`)

**Interfaces:**
- Consumes: `gray_plugin::{Plugin, Manifest}` from Task 1
- Produces: `gray_tools::plugin::{ToolsBasicPlugin, ToolsSearchPlugin}`, `Registry::from_plugins(plugins: &[Arc<dyn Plugin]]) -> Registry`

- [ ] **Step 1: Write the failing test**

```rust
// in crates/gray-tools/tests/plugin_registry.rs
#[test]
fn registry_from_plugins_collects_in_order() {
    let plugins: Vec<Arc<dyn gray_plugin::Plugin>> =
        vec![Arc::new(ToolsBasicPlugin), Arc::new(ToolsSearchPlugin)];
    let reg = Registry::from_plugins(&plugins);
    assert!(reg.get("read").is_some());
    assert!(reg.get("grep").is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray-tools --test plugin_registry 2>&1 | tail -5`
Expected: FAIL, `from_plugins` not found

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/gray-tools/src/plugin.rs
pub struct ToolsBasicPlugin;  // read/write/edit/bash/skill/request_user_input
pub struct ToolsSearchPlugin; // grep/find/ls
// each impl Plugin: manifest() names tools, tools() wraps existing structs

// in lib.rs
pub fn from_plugins(&mut self, plugins: &[Arc<dyn gray_plugin::Plugin>) {
    let owners = gray_plugin::merge_manifests(plugins.iter().map(|p| p.manifest()).collect());
    for p in plugins {
        for t in p.tools() {
            if owners.get(&t.def().name).map(|o| o == &p.manifest().name).unwrap_or(false) {
                self.tools.push(t);
            }
        }
    }
}
```

`gray.yml` default lists `tools-basic`, `tools-search`, `provider-openai`, `cron`. Keep `Registry::builtin()` as a thin wrapper over the default profile so call sites in `repl/mod.rs:586` keep working until the loader lands.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray-tools 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-tools gray.yml crates/gray/src/lib.rs
git commit -m "feat(plugin): builtin tools as plugins with profile-ordered registry"
```

### Task 3: `--dump-manifest` + profile loader

**Files:**
- Create: `crates/gray-plugin/src/profile.rs` (`load_profile(path) -> Vec<String>`)
- Modify: `crates/gray/src/main.rs` (add `--dump-manifest` flag printing merged JSON)

**Interfaces:**
- Consumes: `gray_plugin::{Manifest, merge_manifests}` from Task 1
- Produces: `gray_plugin::profile::load_profile`, CLI `--dump-manifest` output (JSON array of `{name, tools, provider}`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn load_profile_returns_ordered_names() {
    let names = gray_plugin::profile::load_profile("testdata/gray.yml");
    assert_eq!(names, vec!["tools-basic", "tools-search"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray-plugin profile 2>&1 | tail -5`
Expected: FAIL, module not found

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/gray-plugin/src/profile.rs
#[derive(serde::Deserialize)]
struct Profile { plugins: Vec<ProfileEntry> }
#[derive(serde::Deserialize)]
struct ProfileEntry { builtin: String }
pub fn load_profile(path: &str) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let p: Profile = serde_yaml::from_str(&text)?;
    Ok(p.plugins.into_iter().map(|e| e.builtin).collect())
}
```

Wire `--dump-manifest` in `main.rs`: build default plugins, print `serde_json::to_string_pretty` of manifests.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray-plugin 2>&1 | tail -3 && cargo check 2>&1 | tail -2`
Expected: PASS, check clean

- [ ] **Step 5: Commit**

```bash
git add crates/gray-plugin crates/gray/src/main.rs
git commit -m "feat(plugin): gray.yml profile loader and --dump-manifest"
```
