# /gateway REPL Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stub `/gateway` REPL command (currently aliased to the `/proxy` handler) with real wiring to the existing `gray-gateway` machinery: status, connect/disconnect platforms, in-process run/stop, systemd install/uninstall.

**Architecture:** All changes live in `crates/gray/src/repl/mod.rs` plus its test module. A new pure function `parse_gateway_args()` turns the raw command string into a `GatewayAction` enum; a new `handle_gateway()` async fn executes actions using `gray_gateway::config` (load/save `~/.gray/gateway.yaml`), `gray_gateway::systemd` (install/uninstall/status), and `gray_gateway::daemon::run_gateway` spawned as a background tokio task (mirroring the existing `PROXY_HANDLE` pattern). The REPL dispatch arm for `ReplCommand::Gateway` swaps from `handle_proxy` to `handle_gateway`.

**Tech Stack:** Rust, tokio, existing workspace crates `gray-gateway` (already a dependency of `gray`), `serde_yaml` via gray-gateway's config API.

**Spec:** No spec file — design approved in chat 2026-09-02 (bare=status, connect/disconnect, run/stop, install/uninstall; token passed as arg, never echoed).

## Global Constraints

- Follow existing handler conventions: feedback via `say(tui, msg)` (dim `╰ ` lines in TUI, stdout otherwise).
- `gateway.yaml` is 0600 and lives at `gray_gateway::config::gray_gateway_path()` — never print token values back to the user.
- No new dependencies.
- After code changes: `cargo build --release && install target/release/gray ~/.local/bin/gray`, then commit.

---

### Task 1: Parse `/gateway` arguments into a `GatewayAction`

**Files:**
- Modify: `crates/gray/src/repl/mod.rs` (insert near `handle_proxy`, ~line 794)
- Test: `crates/gray/src/repl/mod.rs` `mod tests` (~line 2608)

**Interfaces:**
- Consumes: nothing (pure function).
- Produces:
  ```rust
  enum GatewayAction {
      Status,
      Connect(gray_gateway::config::Platform, String), // platform, token
      Disconnect(gray_gateway::config::Platform),
      Run,
      Stop,
      Install,
      Uninstall,
      Help,
  }
  fn parse_gateway_args(raw: &str) -> GatewayAction;
  ```
  `raw` is the full command string, e.g. `"/gateway connect telegram 123:ABC"`.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` in `crates/gray/src/repl/mod.rs` (it already imports `super::*`; add `use super::GatewayAction;` and `use gray_gateway::config::Platform;`):

```rust
#[test]
fn gateway_args_parse_all_actions() {
    use GatewayAction as G;
    assert!(matches!(super::parse_gateway_args("/gateway"), G::Status));
    assert!(matches!(super::parse_gateway_args("/gateway status"), G::Status));
    assert!(matches!(super::parse_gateway_args("/gateway run"), G::Run));
    assert!(matches!(super::parse_gateway_args("/gateway stop"), G::Stop));
    assert!(matches!(super::parse_gateway_args("/gateway install"), G::Install));
    assert!(matches!(super::parse_gateway_args("/gateway uninstall"), G::Uninstall));
    assert!(matches!(super::parse_gateway_args("/gateway bogus"), G::Help));
    match super::parse_gateway_args("/gateway connect discord abc123") {
        G::Connect(Platform::Discord, tok) => assert_eq!(tok, "abc123"),
        other => panic!("expected connect discord, got {other:?}"),
    }
    match super::parse_gateway_args("/gateway connect TELEGRAM  123:XYZ extra") {
        G::Connect(Platform::Telegram, tok) => assert_eq!(tok, "123:XYZ"), // token = first arg after platform
        other => panic!("expected connect telegram, got {other:?}"),
    }
    assert!(matches!(
        super::parse_gateway_args("/gateway connect slack"), // no token
        G::Help
    ));
    match super::parse_gateway_args("/gateway disconnect slack") {
        G::Disconnect(Platform::Slack) => {}
        other => panic!("expected disconnect slack, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray gateway_args_parse`
Expected: FAIL — `parse_gateway_args` and `GatewayAction` not defined (compile error).

- [ ] **Step 3: Write minimal implementation**

Add above `async fn handle_proxy` in `crates/gray/src/repl/mod.rs`:

```rust
/// What `/gateway ...` should do. Parsed by [`parse_gateway_args`].
#[derive(Debug, PartialEq)]
enum GatewayAction {
    Status,
    Connect(gray_gateway::config::Platform, String),
    Disconnect(gray_gateway::config::Platform),
    Run,
    Stop,
    Install,
    Uninstall,
    Help,
}

/// Parses `/gateway [sub] [args]` — bare or unknown subcommands default to
/// Status/Help so a mistyped command never silently starts or stops anything.
fn parse_gateway_args(raw: &str) -> GatewayAction {
    let mut toks = raw.split_whitespace().skip(1); // drop "/gateway"
    match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
        None | Some("status") => GatewayAction::Status,
        Some("run") => GatewayAction::Run,
        Some("stop") => GatewayAction::Stop,
        Some("install") => GatewayAction::Install,
        Some("uninstall") => GatewayAction::Uninstall,
        Some("help") => GatewayAction::Help,
        Some("connect") => match (toks.next(), toks.next()) {
            (Some(p), Some(tok)) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Connect(plat, tok.to_string()),
                Err(_) => GatewayAction::Help,
            },
            _ => GatewayAction::Help,
        },
        Some("disconnect") => match toks.next() {
            Some(p) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Disconnect(plat),
                Err(_) => GatewayAction::Help,
            },
            None => GatewayAction::Help,
        },
        Some(_) => GatewayAction::Help,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray gateway_args_parse`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/repl/mod.rs
git commit -m "repl: parse /gateway subcommands into GatewayAction"
```

---

### Task 2: Config connect/disconnect + status rendering

**Files:**
- Modify: `crates/gray/src/repl/mod.rs` (same area as Task 1)
- Test: `crates/gray/src/repl/mod.rs` `mod tests`

**Interfaces:**
- Consumes: `GatewayAction` from Task 1; `gray_gateway::config::{GatewayConfig, Platform, PlatformConfig, load_gateway_config, save_gateway_config, gray_gateway_path}`.
- Produces:
  ```rust
  fn apply_connect(cfg: &mut GatewayConfig, plat: gray_gateway::config::Platform, token: &str);
  fn apply_disconnect(cfg: &mut GatewayConfig, plat: gray_gateway::config::Platform);
  fn gateway_status_lines(cfg: &GatewayConfig, running: bool) -> Vec<String>;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
#[test]
fn gateway_connect_disconnect_roundtrip() {
    let mut cfg = gray_gateway::config::GatewayConfig::default();
    super::apply_connect(&mut cfg, gray_gateway::config::Platform::Telegram, "tok-1");
    let pc = cfg.platforms.get(&gray_gateway::config::Platform::Telegram).unwrap();
    assert!(pc.enabled && pc.token.as_deref() == Some("tok-1"));
    super::apply_disconnect(&mut cfg, gray_gateway::config::Platform::Telegram);
    let pc = cfg.platforms.get(&gray_gateway::config::Platform::Telegram).unwrap();
    assert!(!pc.enabled && pc.token.is_none());
}

#[test]
fn gateway_status_lines_hide_tokens() {
    let mut cfg = gray_gateway::config::GatewayConfig::default();
    super::apply_connect(&mut cfg, gray_gateway::config::Platform::Discord, "secret-token");
    let lines = super::gateway_status_lines(&cfg, true);
    let joined = lines.join("\n");
    assert!(joined.contains("discord"), "should list platform: {joined}");
    assert!(joined.contains("connected"), "should show in-process running state: {joined}");
    assert!(!joined.contains("secret-token"), "token must never render: {joined}");
    let lines = super::gateway_status_lines(&cfg, false);
    assert!(lines.join("\n").contains("not running"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gray gateway_`
Expected: FAIL — `apply_connect` etc. not defined (compile error).

- [ ] **Step 3: Write minimal implementation**

Add next to `parse_gateway_args`:

```rust
/// Enables `plat` in `cfg` with `token` (mutates in place; caller saves).
fn apply_connect(cfg: &mut GatewayConfig, plat: gray_gateway::config::Platform, token: &str) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = true;
    pc.token = Some(token.to_string());
}

/// Disables `plat` and clears its token.
fn apply_disconnect(cfg: &mut GatewayConfig, plat: gray_gateway::config::Platform) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = false;
    pc.token = None;
}

/// One human line per known platform: enabled/disabled. `running` is the
/// in-process daemon state (systemd status is reported separately by callers).
fn gateway_status_lines(cfg: &GatewayConfig, running: bool) -> Vec<String> {
    use gray_gateway::config::Platform;
    let mut lines = vec![format!(
        "gateway {} — config: {}",
        if running { "running" } else { "not running" },
        gray_gateway::config::gray_gateway_path().map(|p| p.display().to_string()).unwrap_or_default(),
    )];
    for plat in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        let state = match cfg.platforms.get(&plat) {
            Some(pc) if pc.enabled => "enabled",
            _ => "disabled",
        };
        lines.push(format!("  {plat}: {state}"));
    }
    lines.push("usage: /gateway connect <telegram|discord|slack> <token> | disconnect <platform> | run | stop | install | uninstall | status".to_string());
    lines
}
```

Note: `use gray_gateway::config::GatewayConfig;` will be needed at the top of the function block region — since the file already refers to types by full path elsewhere, prefer full paths in signatures: `fn apply_connect(cfg: &mut gray_gateway::config::GatewayConfig, ...)`. Keep the test-visible names exactly as in the Interfaces block otherwise.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gray gateway_`
Expected: PASS (3 tests total across tasks 1-2).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/repl/mod.rs
git commit -m "repl: gateway config connect/disconnect + status rendering"
```

---

### Task 3: `handle_gateway` — execute actions, wire into dispatch

**Files:**
- Modify: `crates/gray/src/repl/mod.rs` — add `handle_gateway` next to `handle_proxy`; change the `ReplCommand::Gateway` arm (~line 2059)
- Test: manual (I/O-heavy: tokio spawn, systemctl); covered indirectly by Tasks 1-2 unit tests

**Interfaces:**
- Consumes: `GatewayAction`, `apply_connect`/`apply_disconnect`, `gateway_status_lines` from Tasks 1-2; `gray_gateway::daemon::run_gateway()`, `gray_gateway::systemd::{install, uninstall, status}`; existing `say()` helper and `PROXY_HANDLE`-style static.
- Produces:
  ```rust
  static GATEWAY_HANDLE: StdMutex<Option<tokio::task::JoinHandle<()>>> = StdMutex::new(None);
  async fn handle_gateway(raw: &str, tui: Option<&crate::composer::SharedTui>);
  ```

- [ ] **Step 1: Implement `handle_gateway`**

Add next to `handle_proxy`:

```rust
static GATEWAY_HANDLE: StdMutex<Option<tokio::task::JoinHandle<()>>> = StdMutex::new(None);

async fn handle_gateway(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    match parse_gateway_args(raw) {
        GatewayAction::Status => {
            let cfg = gray_gateway::config::load_gateway_config();
            let running = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
            for line in gateway_status_lines(&cfg, running) { say(tui, &line); }
            // systemd state, best-effort (mirrors gray_gateway::systemd::status)
            if let Ok(out) = std::process::Command::new("systemctl")
                .args(["--user", "is-active", "gray-gateway.service"]).output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                say(tui, &format!("systemd unit: {s}"));
            }
        }
        GatewayAction::Connect(plat, token) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_connect(&mut cfg, plat, &token);
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(tui, &format!("╰ {plat} connected — token saved to ~/.gray/gateway.yaml (start with /gateway run or /gateway install)")),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Disconnect(plat) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_disconnect(&mut cfg, plat);
            let _ = gray_gateway::config::save_gateway_config(&cfg);
            say(tui, &format!("╰ {plat} disabled"));
        }
        GatewayAction::Run => {
            let already = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
            if already { say(tui, "gateway already running"); return; }
            let h = tokio::spawn(async {
                if let Err(e) = gray_gateway::daemon::run_gateway().await {
                    log::warn!("gateway exited: {e}");
                }
            });
            *GATEWAY_HANDLE.lock().unwrap() = Some(h);
            say(tui, "gateway starting — platforms connect in background (~45s timeout each)");
        }
        GatewayAction::Stop => {
            let mut g = GATEWAY_HANDLE.lock().ok();
            if let Some(h) = g.as_mut().and_then(|g| g.take()) {
                h.abort();
                say(tui, "gateway stopped");
            } else {
                say(tui, "gateway not running in this session (if installed as a service: /gateway uninstall)");
            }
        }
        GatewayAction::Install => { let _ = with_modal_sync(tui, gray_gateway::systemd::install); say(tui, "gateway installed as systemd user service"); }
        GatewayAction::Uninstall => { let _ = with_modal_sync(tui, gray_gateway::systemd::uninstall); say(tui, "gateway systemd service removed"); }
        GatewayAction::Help => {
            for line in gateway_status_lines(&gray_gateway::config::load_gateway_config(), false) { say(tui, &line); }
        }
    }
}
```

Note: `install`/`uninstall` shell out to `systemctl` and print to stdout; wrapping in `with_modal_sync` keeps the inline viewport consistent, matching how other shelling handlers run. If `with_modal_sync`'s closure signature complains (it takes `FnOnce() -> T`), call the fns directly instead — they are sync.

- [ ] **Step 2: Rewire the dispatch arm**

In `crates/gray/src/repl/mod.rs`, replace:

```rust
ReplCommand::Gateway(raw) => {
    // reuse proxy handler for gateway status — shares same auth surface
    handle_proxy(&raw, config, tui.as_ref().map(|(s, _)| s)).await;
    continue;
}
```

with:

```rust
ReplCommand::Gateway(raw) => {
    handle_gateway(&raw, tui.as_ref().map(|(s, _)| s)).await;
    continue;
}
```

- [ ] **Step 3: Build + run full test suite**

Run: `cargo test -p gray && cargo build --release`
Expected: all tests PASS, release build finishes.

- [ ] **Step 4: Manual smoke test**

Run: `~/.local/bin/gray` then in the REPL:
1. `/gateway` → shows `gateway not running`, per-platform lines (all `disabled`), config path, usage line.
2. `/gateway connect telegram 000:test` → `telegram connected` dim line; `cat ~/.gray/gateway.yaml` shows `telegram: enabled: true, token`.
3. `/gateway disconnect telegram` → re-cat the yaml: token gone, `enabled: false`.
4. `/gateway stop` with nothing running → `gateway not running in this session`.
5. `/gateway bogus` → usage line, nothing started/stopped.

- [ ] **Step 5: Install binary and commit**

```bash
install target/release/gray ~/.local/bin/gray
git add crates/gray/src/repl/mod.rs
git commit -m "repl: real /gateway command — status, connect/disconnect, run/stop, install/uninstall"
```
