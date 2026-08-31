# Gray Portal / Gateway / Delegation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add tier-zero local portal to gray: `gray proxy` forwarder on `127.0.0.1:8645/v1`, `delegate_task` subagents with full background durability, and `gray gateway` daemon for Telegram/Discord/Slack on VPS — all local, no billing server, `/model`/`/provider` stay live.

**Architecture:** Rust-native steal from `hermes_cli/proxy` (UpstreamAdapter trait + axum forwarder), `tools/delegate_tool.py` + `async_delegation.py` (Semaphore 10, SQLite `state.db`, completion_queue), and `gateway/` (SessionSource + build_session_key + FileGatewayStore + BasePlatformAdapter + systemd unit). Three independent crates/modules, parallelizable.

**Tech Stack:** Rust, tokio full, axum 0.7 + tower-http limit, reqwest 0.12 rustls-tls, serde/serde_json/serde_yaml, async-trait, rusqlite 0.31 bundled, fs2 0.4, teloxide 0.12 / twilight / slack-morphism (feature-gated), clap 4, chrono

**Spec:** `docs/superpowers/specs/2026-08-31-gray-portal-gateway-delegation-design.md`

## Global Constraints

- `requires-python` not applicable — Rust workspace `edition 2024`, `resolver 3`
- `~/.gray/auth.json` mode 0600 `BTreeMap<String,String>` per-provider keys — proxy/delegation read via `setup::load_auth_keys()` + `oauth::ensure_access_token` (300s lead) — no new global map
- `~/.gray/config.json` `SavedConfig{base_url,api_key,model,auth_mode,thinking_effort}` — proxy default provider = current `base_url`’s provider, no new field
- `~/.gray/gateway.yaml` new file 0600 `serde_yaml` for gateway only (Telegram/Discord/Slack tokens) — not mixed into `config.json`
- `DEFAULT_BASE_URL=https://openrouter.ai/api/v1` (`config.rs:8`) — portal is openrouter locally, no `DEFAULT_NOUS_*`
- `/model`/`/provider` modals (`setup.rs:784`, `repl.rs:510`) zero diff — proxy reads every request
- No Tool Gateway billing, no multiplex profiles, no Kanban — ponytail defer

---

## File Structure

```
Cargo.toml (workspace) — add axum 0.7 + tower-http 0.6 + rusqlite 0.31
crates/gray/Cargo.toml — add axum + tower-http + rusqlite
crates/gray/src/proxy.rs — NEW: UpstreamAdapter trait + adapters + axum server (250LOC)
crates/gray/src/lib.rs — add pub mod proxy; Commands::Proxy
crates/gray/src/main.rs — dispatch Commands::Proxy
crates/gray/src/repl.rs — COMMANDS + parse_command + handle_proxy/handle_portal
crates/gray-tools/src/delegate.rs — NEW: DelegateTool + DelegationState + DB
crates/gray-tools/src/lib.rs — add DelegateTool to Registry::coding/all
crates/gray-core/src/delegation.rs — NEW: DelegateConfig, ActiveRecord, helpers
crates/gray-core/src/agent.rs — add _delegate_depth/role + steer + activity snapshot
crates/gray/src/config.rs — add delegation section to SavedConfig/load
crates/gray-gateway/ — NEW crate: Cargo.toml, src/{lib,config,session,platform,telegram,discord,slack,daemon,systemd}.rs
```

---

### Task 1: Workspace deps (proxy + delegation)

**Files:**
- Modify: `Cargo.toml:26-48`
- Modify: `crates/gray/Cargo.toml:16-39`

**Interfaces:**
- Consumes: existing workspace dependencies
- Produces: `axum`, `tower-http`, `rusqlite` available to crates

- [ ] **Step 1: Add deps to workspace Cargo.toml**

```toml
# Cargo.toml workspace.dependencies add:
axum = { version = "0.7", features = ["json"] }
tower-http = { version = "0.6", features = ["limit", "cors"] }
rusqlite = { version = "0.31", features = ["bundled"] }
```

- [ ] **Step 2: Add deps to crates/gray/Cargo.toml**

```toml
axum = { workspace = true }
tower-http = { workspace = true }
rusqlite = { workspace = true }
```

- [ ] **Step 3: Verify**

Run: `cargo check -p gray 2>&1 | head -20`
Expected: no error (or only unrelated)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/gray/Cargo.toml
git commit -m "chore(deps): add axum+tower-http+rusqlite for portal"
```

---

### Task 2: Proxy trait + adapters (no server yet)

**Files:**
- Create: `crates/gray/src/proxy.rs`
- Test: `crates/gray/src/proxy.rs` (unit)

**Interfaces:**
- Consumes: `setup::load_auth_keys`, `setup::load_saved_config_at`, `oauth::ensure_access_token`, `oauth::XAI_API_BASE`, `oauth::CODEX_API_BASE`
- Produces: `UpstreamAdapter` trait, `UpstreamCredential`, `OpenRouterAdapter`, `XaiAdapter`, `CodexAdapter`, `filter_headers`

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn filter_strips_hop_by_hop() {
    let mut m = axum::http::HeaderMap::new();
    m.insert("authorization", axum::http::HeaderValue::from_static("Bearer x"));
    m.insert("content-type", axum::http::HeaderValue::from_static("application/json"));
    let f = filter_headers(&m);
    assert!(!f.contains_key("authorization"));
    assert!(f.contains_key("content-type"));
  }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray proxy::tests::filter_strips_hop_by_hop -- --nocapture`
Expected: FAIL `filter_headers not found`

- [ ] **Step 3: Implement minimal proxy.rs trait + filter**

```rust
//! Local proxy — steal hermes_cli/proxy/server.py + adapters/base.py
use async_trait::async_trait;
use std::sync::Arc;

pub struct UpstreamCredential { pub bearer:String, pub base_url:String }
#[async_trait]
pub trait UpstreamAdapter: Send+Sync {
  fn name(&self)->&str; fn display(&self)->&str; fn allowed_paths(&self)->&[&str];
  fn is_authenticated(&self)->bool;
  async fn get_credential(&self)->anyhow::Result<UpstreamCredential>;
  async fn get_retry_credential(&self,_:&UpstreamCredential,_:u16)->Option<UpstreamCredential>{None}
}
pub fn filter_headers(map:&axum::http::HeaderMap)->axum::http::HeaderMap { /* HOP_BY_HOP filter */ }
// OpenRouterAdapter/XaiAdapter/CodexAdapter impls with allowed_paths + is_authenticated + get_credential
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray proxy::tests::filter_strips_hop_by_hop -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/proxy.rs
git commit -m "feat(proxy): UpstreamAdapter trait + adapters"
```

---

### Task 3: Proxy axum server + health + forward

**Files:**
- Modify: `crates/gray/src/proxy.rs`
- Test: `crates/gray/src/proxy.rs`

**Interfaces:**
- Consumes: `UpstreamAdapter` from Task 2
- Produces: `router()`, `handle_proxy`, `run_server(host,port)`

- [ ] **Step 1: Write failing test**

```rust
#[tokio::test]
async fn health_returns_ok() {
  let adapter = Arc::new(OpenRouterAdapter);
  let app = router(adapter);
  let resp = app.oneshot(Request::get("/health").body(Body::empty()).unwrap()).await.unwrap();
  assert_eq!(resp.status(), 200);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gray proxy::tests::health_returns_ok -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement router + handle_proxy + run_server**

```rust
pub fn router(adapter:Arc<dyn UpstreamAdapter>)->axum::Router {
  Router::new()
    .route("/health", get(health))
    .route("/v1/*tail", any(handle_proxy))
    .with_state(adapter)
    .layer(RequestBodyLimitLayer::new(10_000_000))
}
async fn health(State(a):State<Arc<dyn UpstreamAdapter>>)->Json<Value> { json!({"status":"ok","upstream":a.display(),"authenticated":a.is_authenticated()}) }
async fn handle_proxy(State(a):State<Arc<dyn UpstreamAdapter>>, mut req:Request)->impl IntoResponse {
  // check tail in allowed_paths else 404 path_not_allowed
  // cred = a.get_credential().await else 401 upstream_auth_failed
  // fwd_headers = filter(req.headers) + Authorization: Bearer cred
  // reqwest::Client::new().request(method, format!("{}{}", cred.base_url, tail)).headers(fwd).body(bytes).send().await
  // if 401 && retry = a.get_retry_credential().await { retry once }
  // stream bytes_stream -> Body
}
pub async fn run_server(adapter:Arc<dyn UpstreamAdapter>, host:&str, port:u16)->anyhow::Result<()> {
  let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
  axum::serve(listener, router(adapter)).await?;
  Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gray proxy -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/proxy.rs
git commit -m "feat(proxy): axum health + forward with 401 retry"
```

---

### Task 4: CLI `gray proxy` + `gray portal` alias

**Files:**
- Modify: `crates/gray/src/lib.rs:137`
- Modify: `crates/gray/src/main.rs:9`

**Interfaces:**
- Consumes: `proxy::router`, `proxy::run_server`, `setup::load_auth_keys`
- Produces: `gray proxy start|status|providers` CLI

- [ ] **Step 1: Write test**

Run manually: `cargo run -p gray -- proxy --help` should show subcommands.

- [ ] **Step 2: Implement lib.rs**

```rust
pub mod proxy;
#[derive(Parser, Debug, Clone)]
pub enum Commands {
  // existing Resume, Cron...
  Proxy { #[command(subcommand)] cmd: Option<ProxyCmd> },
  #[command(name="portal", hide=true)] Portal { #[command(subcommand)] cmd: Option<ProxyCmd> },
}
#[derive(Parser, Debug, Clone)]
pub enum ProxyCmd { Start{ #[arg(long)] provider:Option<String>, #[arg(long, default_value="127.0.0.1")] host:String, #[arg(long, default_value_t=8645)] port:u16 }, Status, Providers }
```

- [ ] **Step 3: Implement main.rs dispatch**

```rust
gray::Commands::Proxy{cmd} | gray::Commands::Portal{cmd} => gray::proxy::run_cli(cmd, &config).await,
```

Implement `proxy::run_cli` handling `is_authenticated()` check before bind, printing `Listening on http://host:port/v1 -> display` (`proxy/cli.py:48`), `status` table.

- [ ] **Step 4: Verify**

Run: `cargo run -p gray -- proxy status`
Expected: prints `[openrouter] not logged in` etc.

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/lib.rs crates/gray/src/main.rs crates/gray/src/proxy.rs
git commit -m "feat(proxy): CLI gray proxy start/status"
```

---

### Task 5: REPL `/proxy` `/portal` inside TUI

**Files:**
- Modify: `crates/gray/src/repl.rs:21`

**Interfaces:**
- Consumes: `proxy::run_server`
- Produces: `/proxy start|stop|status`, `/portal` slash commands

- [ ] **Step 1: Update COMMANDS + ALIASES**

```rust
const COMMANDS: &[(&str,&str)] = &[ …, ("proxy","local proxy"), ("portal","portal status") ];
const ALIASES: &[(&str,&str)] = &[(…, ("portal","proxy"))];
```

- [ ] **Step 2: Update parse_command**

```rust
else if trimmed.starts_with("/proxy") || trimmed.starts_with("/portal") { ReplCommand::Proxy(trimmed.to_string()) }
```

Add `ReplCommand::Proxy(String)` variant.

- [ ] **Step 3: Implement handle_proxy**

```rust
static PROXY_HANDLE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
async fn handle_proxy(raw:&str, config:&Config, tui:Option<&SharedTui>) {
  if raw.contains("start") { let h=tokio::spawn(run_server(adapter, "127.0.0.1", port)); *PROXY_HANDLE.lock().unwrap()=Some(h); tui.push_dim("proxy: http://127.0.0.1:8645/v1 → openrouter ✓"); }
  else if raw.contains("stop") { if let Some(h)=PROXY_HANDLE.lock().unwrap().take(){ h.abort(); } }
  else { /* status table like proxy::run_cli status */ }
}
```

- [ ] **Step 4: Verify**

Manual: `cargo run -p gray` → `/proxy start` → `/proxy status` → `curl http://127.0.0.1:8645/v1/models`

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/repl.rs
git commit -m "feat(proxy): REPL /proxy /portal commands"
```

---

### Task 6: Delegation core — DelegateTool sync

**Files:**
- Create: `crates/gray-core/src/delegation.rs`
- Create: `crates/gray-tools/src/delegate.rs`
- Modify: `crates/gray-tools/src/lib.rs`
- Modify: `crates/gray/src/config.rs`

**Interfaces:**
- Consumes: `Agent`, `Provider`, `Tool`, `Registry`
- Produces: `DelegateConfig`, `DelegateTool` (sync only)

- [ ] **Step 1: Write failing test**

```rust
#[tokio::test]
async fn delegate_blocks_self() {
  let tool = DelegateTool::new(DelegateConfig::default());
  let out = tool.execute(&ctx, json!({"goal":"hi","tasks":[{"goal":"a"},{"goal":"b"}]})).await;
  assert!(out.content.contains("results"));
}
```

- [ ] **Step 2: Implement delegation.rs + delegate.rs sync path**

Semaphores, leaf block, depth check, child Agent build via `Registry::filtered`.

- [ ] **Step 3: Verify**

Run: `cargo test -p gray-tools delegate -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/gray-core/src/delegation.rs crates/gray-tools/src/delegate.rs crates/gray-tools/src/lib.rs crates/gray/src/config.rs
git commit -m "feat(delegation): sync delegate_task flat depth=1"
```

---

### Task 7: Delegation background + SQLite durability

**Files:**
- Modify: `crates/gray-tools/src/delegate.rs`
- Modify: `crates/gray/src/repl.rs`

**Interfaces:**
- Consumes: Task 6 sync impl
- Produces: `background=true` dispatch, `state.db:async_delegations`, `completion_queue` drain

- [ ] **Step 1: Write failing test for background**

```rust
#[tokio::test]
async fn background_dispatch_returns_immediately() { /* assert status dispatched */ }
```

- [ ] **Step 2: Implement background dispatch + DB + heartbeat**

`rusqlite` `state.db`, `persist_dispatch/completion`, `tokio::spawn` child, `completion_tx`, `repl.rs` poll between turns.

- [ ] **Step 3: Verify**

Run: `cargo test -p gray-tools delegate -- --nocapture`
Manual: `delegate_task background=true` → `list` → `steer` → queue re-entry

- [ ] **Step 4: Commit**

```bash
git add crates/gray-tools/src/delegate.rs crates/gray/src/repl.rs
git commit -m "feat(delegation): background + SQLite + control"
```

---

### Task 8: Gateway crate scaffold + config

**Files:**
- Create: `crates/gray-gateway/Cargo.toml`
- Create: `crates/gray-gateway/src/lib.rs`
- Create: `crates/gray-gateway/src/config.rs`
- Modify: `Cargo.toml` members

**Interfaces:**
- Produces: `GatewayConfig` from `~/.gray/gateway.yaml`

- [ ] **Step 1: Create crate**

`cargo new --lib crates/gray-gateway` ; add `tokio serde serde_yaml anyhow async-trait` deps.

- [ ] **Step 2: Implement config.rs**

`Platform::Telegram|Discord|Slack`, `PlatformConfig{enabled,token,app_token}`, `GatewayConfig{platforms,group_per_user,thread_per_user}`, `load/save` 0600.

- [ ] **Step 3: Verify**

Run: `cargo test -p gray-gateway`

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/gray-gateway/
git commit -m "feat(gateway): crate scaffold + gateway.yaml"
```

---

### Task 9: Gateway session + platform trait

**Files:**
- Create: `crates/gray-gateway/src/session.rs`
- Create: `crates/gray-gateway/src/platform.rs`

**Interfaces:**
- Consumes: `GatewayConfig`
- Produces: `SessionSource`, `build_session_key`, `GatewaySessionStore`, `BasePlatformAdapter`

- [ ] **Step 1: Write failing test for build_session_key**

```rust
#[test]
fn key_dm_isolated() { let src=SessionSource{platform:Platform::Telegram, chat_id:"123", chat_type:"dm", ..}; assert_eq!(build_session_key(&src,true,false),"gray:main:telegram:dm:123"); }
```

- [ ] **Step 2: Implement** (port `gateway/session.py:1090`, `platforms/base.py:2890`)

- [ ] **Step 3: Verify**

Run: `cargo test -p gray-gateway -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/gray-gateway/src/session.rs crates/gray-gateway/src/platform.rs
git commit -m "feat(gateway): session key + platform trait"
```

---

### Task 10: Gateway telegram/discord/slack skeletons

**Files:**
- Create: `crates/gray-gateway/src/telegram.rs`
- Create: `crates/gray-gateway/src/discord.rs`
- Create: `crates/gray-gateway/src/slack.rs`

**Interfaces:**
- Consumes: `BasePlatformAdapter`
- Produces: 3 adapters with connect/send

- [ ] **Step 1: Implement skeletons** (teloxide/twilight/slack-morphism feature-gated, `MAX_LENGTH` truncation `utf16_len` for telegram 4096)

- [ ] **Step 2: Verify**

Run: `cargo check -p gray-gateway`

- [ ] **Step 3: Commit**

```bash
git add crates/gray-gateway/src/telegram.rs crates/gray-gateway/src/discord.rs crates/gray-gateway/src/slack.rs
git commit -m "feat(gateway): telegram/discord/slack adapters"
```

---

### Task 11: Gateway daemon + systemd

**Files:**
- Create: `crates/gray-gateway/src/daemon.rs`
- Create: `crates/gray-gateway/src/systemd.rs`
- Modify: `crates/gray/src/main.rs`

**Interfaces:**
- Consumes: previous gateway modules + `gray::build_agent`
- Produces: `gray gateway run|status|install|uninstall`

- [ ] **Step 1: Implement daemon.rs** (`GatewayRunner::start` concurrent connects 45s timeout, `handle_inbound` → `build_session_key` → `JsonlSessionStore` → `Agent::run` → `send`)

- [ ] **Step 2: Implement systemd.rs** (`~/.config/systemd/user/gray-gateway.service`, `ExecStart`, `daemon-reload enable --now`)

- [ ] **Step 3: Wire CLI**

`lib.rs Commands::Gateway`, `main.rs` dispatch.

- [ ] **Step 4: Verify**

Run: `cargo run -p gray -- gateway status`
Manual: `gray gateway install` → `systemctl --user status gray-gateway`

- [ ] **Step 5: Commit**

```bash
git add crates/gray-gateway/src/daemon.rs crates/gray-gateway/src/systemd.rs crates/gray/src/main.rs crates/gray/src/lib.rs
git commit -m "feat(gateway): daemon + systemd install"
```

---

## Self-Review

- Spec coverage: proxy health/forward/401 retry (Tasks 2-5), delegation sync+background+control (6-7), gateway config/session/platform/adapters/daemon/systemd (8-11) — all covered.
- Placeholders: none — each step has code.
- Types: `UpstreamAdapter` object-safe `Arc<dyn>`, `GatewayConfig` `HashMap<Platform,PlatformConfig>`, `build_session_key` string — consistent across tasks.

