# Auto-Update (self + plugins) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** gray notifies about (and optionally auto-installs) self + plugin updates at REPL boot, pi-style, plus trim `/connect` aliases.

**Architecture:** Read-only `gray-pkg::ops::check_for_updates()` compares lockfile entries against index (gray-native), `git ls-remote` vs recorded source (git), registry `dist-tags.latest` vs recorded version (pi-gallery); REPL boot spawns it async after first paint and prints one dim line; `GRAY_AUTO_UPDATE=1` extends to install. Alias trim is a registry + test edit.

**Tech Stack:** Rust, tokio, reqwest (existing `fetch::client`), git CLI (`ls-remote`, no clone), existing `update.rs` lock/interval helpers.

**Spec:** Approved design in conversation 2026-09-16 (Q1a keep update engine, Q2a mirror pi): never block startup; notify-card only; skip pinned/local; silent on network failure; default off for auto-install.

## Global Constraints

- Rust edition 2024, MSRV per workspace `Cargo.toml`.
- `CARGO_BUILD_JOBS=4` for every cargo invocation mid-loop; narrow `cargo test -p <crate>` mid-loop, never `--workspace` mid-loop.
- Full `cargo test --workspace` + `cargo fmt --check` once, pre-commit.
- No network in tests except loopback (`tiny_http` stubs, `file://` git fixtures) — follow existing `ops_tests.rs` patterns under `ENV_GUARD`.
- Copy rule: dim `└ Plugin updates available — /plugin update (a, b, c)`; never auto-install unless `GRAY_AUTO_UPDATE=1`.
- ONE rolling branch; never push to `main` unasked; always offer PR, never assume.

---

## File Structure

- Modify `crates/gray-pkg/src/ops.rs` — add `PendingUpdate { name, from, to }` struct + `pub async fn check_for_updates() -> Vec<PendingUpdate>` + private per-ecosystem probes (`index_probe`, `git_probe`, `npm_probe`). Reuses `read_lock`, `index::fetch_index`/`lookup`, `npm_registry_base`/`npm_metadata_url`, `fetch::client`.
- Modify `crates/gray-pkg/src/ops_tests.rs` — failing-first tests for the checker (Tasks 1-3 consumers).
- Modify `crates/gray/src/repl/mod.rs` — boot hook in `run_repl_mode` (spawn after first paint; uses existing `say`/TUI helpers).
- Modify `crates/gray/src/update.rs` — extend `startup_check` auto path to also run plugin updates under the existing `update.lock` exclusion (reuse `acquire_update_lock_at`).
- Modify `crates/gray/src/repl/commands.rs` — trim `connect` aliases to `&["provider"]`.
- Modify `crates/gray/src/repl/commands_tests.rs` — update alias test expectations.

---

### Task 1: `PendingUpdate` + gray-native index probe

**Files:**
- Modify: `crates/gray-pkg/src/ops.rs:1-40` (struct area near `LockEntry`)
- Test: `crates/gray-pkg/src/ops_tests.rs` (append near update tests)

**Interfaces:**
- Consumes: `read_lock()` (`crates/gray-pkg/src/ops.rs:273`), `index::fetch_index` + `lookup` (`crates/gray-pkg/src/index.rs:67,101`), `INDEX_URL_ENV` (`crates/gray-pkg/src/index.rs:13`)
- Produces: `pub struct PendingUpdate { pub name: String, pub from: String, pub to: String }` and `async fn index_probe(client: &reqwest::Client, name: &str, installed: &LockEntry) -> Option<PendingUpdate>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn check_reports_gray_native_update_from_index_stub() {
    let _guard = ENV_GUARD.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    let index = serde_json::json!({"plugins": {"demo": {"ecosystem": "gray-native", "version": "1.1.0", "source": {"type": "http", "url": "https://h/demo.tar.gz"}, "hash": "sha256:abc", "scope": "user"}}});
    let _index_url = spawn_index_stub(index).await;
    let pending = check_for_updates().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "demo");
    assert_eq!(pending[0].from, "1.0.0");
    assert_eq!(pending[0].to, "1.1.0");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_gray_native_update_from_index_stub`
Expected: FAIL with "cannot find function `check_for_updates`" (also `spawn_index_stub` exists at `ops_tests.rs:1074`; reuse it verbatim — it sets `GRAY_PLUGIN_INDEX` under `ENV_GUARD`).

- [ ] **Step 3: Write minimal implementation**

```rust
/// One available plugin update: installed `from`, remote `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpdate {
    pub name: String,
    pub from: String,
    pub to: String,
}

async fn index_probe(
    client: &reqwest::Client,
    name: &str,
    installed: &LockEntry,
) -> Option<PendingUpdate> {
    let index = crate::index::fetch_index(client).await.ok()?;
    let entry = crate::index::lookup(&index, name).ok()?;
    if entry.version.is_empty() || entry.version == installed.version {
        return None;
    }
    Some(PendingUpdate {
        name: name.to_string(),
        from: installed.version.clone(),
        to: entry.version.clone(),
    })
}

/// Read-only update check over the lockfile. Never installs, never errors:
/// every per-plugin failure degrades to "no update". Network failures yield
/// an empty vec.
pub async fn check_for_updates() -> Vec<PendingUpdate> {
    let lock = match read_lock() {
        Ok(Some(l)) => l,
        _ => return Vec::new(),
    };
    if lock.plugins.is_empty() {
        return Vec::new();
    }
    let client = match crate::fetch::client() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for (name, installed) in &lock.plugins {
        if installed.ecosystem == "gray-native" {
            if let Some(p) = index_probe(&client, name, installed).await {
                out.push(p);
            }
        }
    }
    out
}
```

Place `PendingUpdate` directly after the `Report` struct; place `index_probe` + `check_for_updates` directly before `pub async fn update` at `ops.rs:1255`.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_gray_native_update_from_index_stub`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-pkg/src/ops.rs crates/gray-pkg/src/ops_tests.rs
git commit -m "feat(pkg): read-only plugin update check for index plugins"
```

### Task 2: git probe (`ls-remote` vs recorded source)

**Files:**
- Modify: `crates/gray-pkg/src/ops.rs` (extend `check_for_updates` match)
- Test: `crates/gray-pkg/src/ops_tests.rs`

**Interfaces:**
- Consumes: `PendingUpdate`, `check_for_updates` (Task 1); `init_git_fixture` + `git_fixture_key` + `use_git_env` (`ops_tests.rs:738,778`); `record_install` (`ops.rs:294`)
- Produces: `async fn git_probe(name: &str, installed: &LockEntry) -> Option<PendingUpdate>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn check_reports_git_update_when_remote_advances() {
    let _guard = ENV_GUARD.lock().unwrap();
    let (repo, _url) = init_git_fixture(&[("skills/a/SKILL.md", MULCH_SKILL)]);
    let key = git_fixture_key(&repo);
    let _home = use_git_env();
    install(
        parse_spec(&format!("git:file://{}", repo.path().display())),
        InstallOpts::default(),
    )
    .await
    .unwrap();
    // Second commit advances the remote past the recorded install.
    std::fs::write(repo.path().join("skills/a/SKILL.md"), "v2\n").unwrap();
    let st = std::process::Command::new("git")
        .args(["-C"])
        .arg(repo.path())
        .args(["commit", "-am", "v2"])
        .output()
        .unwrap();
    assert!(st.status.success());
    let pending = check_for_updates().await;
    let hit = pending.iter().find(|p| p.name == key);
    assert!(hit.is_some(), "expected git update for {key}: {pending:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_git_update_when_remote_advances`
Expected: FAIL — no update reported (`pending` empty; git arm not wired yet).

- [ ] **Step 3: Write minimal implementation**

```rust
async fn git_probe(name: &str, installed: &LockEntry) -> Option<PendingUpdate> {
    if installed.ecosystem != "git" || installed.source.trim().is_empty() {
        return None;
    }
    // Pinned refs never auto-update (pi parity: pinned = you meant it).
    if installed.source.contains('@') {
        return None;
    }
    let remote = tokio::process::Command::new("git")
        .args(["ls-remote", &installed.source, "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .ok()?;
    if !remote.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&remote.stdout);
    let head = stdout.split_whitespace().next()?;
    if head.is_empty() || head == installed.version {
        return None;
    }
    Some(PendingUpdate {
        name: name.to_string(),
        from: installed.version.clone(),
        to: head.chars().take(12).collect(),
    })
}
```

And in `check_for_updates`, after the gray-native arm, add:

```rust
        if installed.ecosystem == "git" {
            if let Some(p) = git_probe(name, installed).await {
                out.push(p);
            }
        }
```

Note: `update_inner` records git installs via `record_install` with `version` = full HEAD sha (see `install_git` at `ops.rs:951`); `source` is the bare URL without `@ref` because `parse_spec` splits the ref off (see `spec_parses_git_forms` test). A `@` in `source` therefore means a pinned install — skip. `ls-remote` performs no clone and respects `GIT_TERMINAL_PROMPT=0`; any failure returns `None`.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_git_update_when_remote_advances`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-pkg/src/ops.rs crates/gray-pkg/src/ops_tests.rs
git commit -m "feat(pkg): update check covers git remotes via ls-remote"
```

### Task 3: npm probe (registry latest vs recorded version)

**Files:**
- Modify: `crates/gray-pkg/src/ops.rs` (extend `check_for_updates` match)
- Test: `crates/gray-pkg/src/ops_tests.rs`

**Interfaces:**
- Consumes: `PendingUpdate`, `check_for_updates` (Tasks 1-2); `pi_foo_meta`, `spawn_registry`, `spawn_registry_any`, `use_npm_env`, `sha512_integrity`, `skill_tgz`, `MULCH_SKILL` (`ops_tests.rs:516,1074+`); `npm_registry_base`, `npm_metadata_url` (`ops.rs:351,359`)
- Produces: `async fn npm_probe(client: &reqwest::Client, name: &str, installed: &LockEntry) -> Option<PendingUpdate>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn check_reports_npm_update_when_registry_advances() {
    let _guard = ENV_GUARD.lock().unwrap();
    let tgz = skill_tgz(&[("skills/a/SKILL.md", MULCH_SKILL)]);
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
    let _home = use_npm_env(&base);
    install(parse_spec("npm:pi-foo"), InstallOpts::default())
        .await
        .unwrap();
    // Registry advances past the recorded install (2.0.0 > recorded latest).
    let tgz2 = skill_tgz(&[("skills/a/SKILL.md", "v2\n")]);
    let tarball2 = spawn_tarball(tgz2.clone()).await;
    let integrity2 = sha512_integrity(&tgz2);
    let meta2 = serde_json::json!({
        "dist-tags": {"latest": "2.0.0"},
        "versions": {"2.0.0": {"dist": {"tarball": tarball2, "integrity": integrity2}}},
    });
    let _base2 = spawn_registry_any(meta2).await;
    let pending = check_for_updates().await;
    let hit = pending.iter().find(|p| p.name == "pi-foo");
    assert!(hit.is_some(), "expected npm update: {pending:?}");
    assert_eq!(hit.unwrap().to, "2.0.0");
}
```

Caveat for the implementer: `use_npm_env` sets `GRAY_NPM_REGISTRY` to `base`; the second stub `spawn_registry_any` binds a *different* loopback port, so point the env at the new base before calling `check_for_updates` — reuse the exact `unsafe { std::env::set_var("GRAY_NPM_REGISTRY", ...); }` lines from `use_npm_env` (`ops_tests.rs:1041`) with the new base URL. If the helper already covers re-pointing, prefer calling it twice. The recorded install version is pi-foo's fixture latest (`0.2.0` per `pi_foo_meta`); `2.0.0` is strictly newer so string inequality is enough, no semver needed.

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_npm_update_when_registry_advances`
Expected: FAIL — no update reported (npm arm not wired yet).

- [ ] **Step 3: Write minimal implementation**

```rust
async fn npm_probe(
    client: &reqwest::Client,
    name: &str,
    installed: &LockEntry,
) -> Option<PendingUpdate> {
    if installed.ecosystem != "pi-gallery" || installed.version.trim().is_empty() {
        return None;
    }
    // Pinned `npm:foo@x.y.z` installs record the pin (see install_npm);
    // a pin means the user chose the version — never flag it.
    let url = npm_metadata_url(&npm_registry_base(), name);
    crate::fetch::check_url(&url).ok()?;
    let meta: serde_json::Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    let latest = meta
        .get("dist-tags")?
        .get("latest")?
        .as_str()?;
    if latest.is_empty() || latest == installed.version {
        return None;
    }
    Some(PendingUpdate {
        name: name.to_string(),
        from: installed.version.clone(),
        to: latest.to_string(),
    })
}
```

And in `check_for_updates`, add the third arm:

```rust
        if installed.ecosystem == "pi-gallery" {
            if let Some(p) = npm_probe(&client, name, installed).await {
                out.push(p);
            }
        }
```

Note: `install_npm` records `source` as `npm:<pkg>` and `version` as the resolved version; pinned installs record the pinned version which equals registry only when already current — but an explicit pin must still be skipped. `parse_spec("npm:foo@1.2.3")` puts the pin in the spec, not the lock; since the lock cannot distinguish pinned from floating installs, this probe flags any drift. Document that: pinned npm installs are upgraded only via explicit `/plugin update <name>`. This matches pi (pinned skipped) in spirit — gray's lock simply has no pin bit yet; do NOT add one in this task (YAGNI).

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray-pkg check_reports_npm_update_when_registry_advances`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray-pkg/src/ops.rs crates/gray-pkg/src/ops_tests.rs
git commit -m "feat(pkg): update check covers npm registry latest"
```

### Task 4: boot hook — one dim line, never blocking

**Files:**
- Modify: `crates/gray/src/repl/mod.rs` (`run_repl_mode`, after first paint — near `crate::tui::clear_screen()` at line ~368)
- Test: manual + existing suite (no new unit test — boot path is integration; covered by Task 5's suite run)

**Interfaces:**
- Consumes: `gray_pkg::ops::check_for_updates` (Tasks 1-3); `say` helper (`repl/mod.rs:127`)
- Produces: spawned background check + single dim notification line

- [ ] **Step 1: Write the failing test**

No unit test — the boot path requires a TTY + lockfile and is verified by build + suite. Instead, write the probe as a reviewable assertion: after implementing, run `./target/debug/gray plugin list` with a stale lock entry and confirm the REPL prints the line. Record the outcome in the commit message. (TDD iron law bends here because the harness is the REPL itself; Tasks 1-3 carry the logic coverage.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib repl` (baseline green before the edit; the "failure" is the absence of the feature — confirm by grepping `check_for_updates` in `crates/gray/src` returns nothing).

- [ ] **Step 3: Write minimal implementation**

```rust
    // Plugin update notice (pi parity): background, never blocking, silent
    // on failure. Fires once per interactive boot after first paint.
    if interactive {
        tokio::spawn(async move {
            let pending = gray_pkg::ops::check_for_updates().await;
            if pending.is_empty() {
                return;
            }
            let names: Vec<String> = pending.iter().take(5).map(|p| p.name.clone()).collect();
            let mut line = format!("Plugin updates available — /plugin update ({})", names.join(", "));
            if pending.len() > 5 {
                line.push_str(&format!(" +{} more", pending.len() - 5));
            }
            eprintln!("\x1b[2m└ {line}\x1b[0m");
        });
    }
```

Placement: inside `run_repl_mode` after `let interactive = ...` (line ~373) and before the context-window setup. Uses `eprintln!` with dim escape (not `say`, which needs the TUI handle not yet built at this point). No prompt, no install, no await — the spawn detaches. Skipped entirely when piped (scripts/tests unaffected).

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib repl`
Expected: PASS (no existing tests touch this path; build proves wiring).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/repl/mod.rs
git commit -m "feat(repl): background plugin-update notice at boot"
```

### Task 5: `GRAY_AUTO_UPDATE=1` extends to plugins

**Files:**
- Modify: `crates/gray/src/update.rs` (`startup_check`, inside the `auto_update_allowed()` branch)
- Test: existing `crates/gray/src/update_tests.rs` suite must stay green

**Interfaces:**
- Consumes: `gray_pkg::ops::{check_for_updates, update}` (`ops.rs:1255`); `acquire_update_lock` (`update.rs:86`); `auto_update_allowed` (`update.rs:114`)
- Produces: plugin auto-install inside the existing auto-update branch

- [ ] **Step 1: Write the failing test**

No new unit test — the auto branch shells to the installer and needs `GRAY_AUTO_UPDATE=1` + network; existing `update_tests.rs` covers the pure helpers (`update_check_due`, lock exclusion). Verify by running the suite before/after: `CARGO_BUILD_JOBS=4 cargo test -p gray update` green both times, plus `grep -n "plugin" crates/gray/src/update.rs` empty before, non-empty after.

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray update`
Expected: PASS (baseline), and `grep -n "check_for_updates" crates/gray/src/update.rs` returns nothing (feature absent).

- [ ] **Step 3: Write minimal implementation**

Inside `startup_check`, in the `auto_update_allowed()` branch after `run_installer_locked()?;`, insert:

```rust
        // Plugins ride along with self auto-update under the same lock.
        // Each failure degrades to a stderr warning; self-update already
        // succeeded above so a plugin miss never blocks boot.
        let pending = gray_pkg::ops::check_for_updates().await;
        if !pending.is_empty() {
            match gray_pkg::ops::update("all").await {
                Ok(reports) => {
                    if !reports.is_empty() {
                        let names: Vec<String> =
                            reports.iter().map(|r| r.name.clone()).collect();
                        eprintln!("  plugins updated: {}", names.join(", "));
                    }
                }
                Err(e) => eprintln!("  plugin auto-update skipped: {e:#}"),
            }
        }
```

`update("all")` already skips non-index sources with a warning and no-ops when versions match, so this is safe to call blind. The `update.lock` is already held by the enclosing branch (`run_installer_locked` holds it for the installer; the plugin update runs synchronously after, still inside `startup_check`'s critical section — do NOT acquire twice; `acquire_update_lock` uses `try_lock` semantics via `fs2::lock`, re-acquiring would deadlock. The code above runs after `run_installer_locked()?` returns (lock released) — correct ordering, no nesting.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray update`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/update.rs
git commit -m "feat(update): GRAY_AUTO_UPDATE=1 also updates plugins"
```

### Task 6: trim `/connect` aliases to `provider`

**Files:**
- Modify: `crates/gray/src/repl/commands.rs:8-16` (REGISTRY connect entry)
- Modify: `crates/gray/src/repl/commands_tests.rs` (alias expectations)

**Interfaces:**
- Consumes: nothing new. Produces: `aliases: &["provider"]` on the connect `CmdDef`.

- [ ] **Step 1: Write the failing test**

Edit `crates/gray/src/repl/commands_tests.rs` first — find the connect-alias test (currently asserts `keys/key/providers/provider/login` resolve to connect) and change it to:

```rust
#[test]
fn connect_keeps_only_provider_alias() {
    assert_eq!(parse_command("/provider"), ReplCommand::Connect);
    for dead in ["/keys", "/key", "/providers", "/login"] {
        assert!(
            matches!(parse_command(dead), ReplCommand::Unknown(_)),
            "{dead} should be unknown now"
        );
    }
}
```

(Adjust to the file's actual helper names — read `commands_tests.rs:100-230` for the real parse-fn name and assertion style before writing.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray connect_keeps_only_provider_alias`
Expected: FAIL — old aliases still resolve.

- [ ] **Step 3: Write minimal implementation**

In `crates/gray/src/repl/commands.rs`, change:

```rust
aliases: &["keys", "key", "providers", "provider", "login"],
```

to:

```rust
aliases: &["provider"],
```

Nothing else: `ReplCommand::Connect`, dispatch, `/connect` itself, and `/provider` all stay.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray connect_keeps_only_provider_alias`
Expected: PASS. Then run the full commands suite: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib repl::commands` — all green (no other test references the dead aliases; verify with `grep -rn '"/keys"\|"/login"\|"/providers"' crates/gray/src` returning nothing).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/repl/commands.rs crates/gray/src/repl/commands_tests.rs
git commit -m "refactor(repl): trim /connect aliases to /provider"
```

---

## Pre-commit gate (whole plan)

Run once after Task 6, before offering a PR:

```bash
CARGO_BUILD_JOBS=4 cargo test --workspace
cargo fmt --all -- --check
CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green. Then offer the PR (never assume, never push to `main` unasked).
