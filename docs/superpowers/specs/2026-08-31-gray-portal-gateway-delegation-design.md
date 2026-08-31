# Gray Portal / Gateway / Delegation — Design (Tier-Zero Slim B)

**Date:** 2026-08-31
**Status:** Approved (B slim ponytail)
**Reference steal:** `reference/NousResearch/hermes-agent/hermes_cli/proxy/`, `tools/delegate_tool.py`, `tools/async_delegation.py`, `gateway/` (run.py, session.py, platforms/base.py, hermes_cli/gateway.py), `hermes_cli/portal_cli.py`
**Goal:** Local-only "portal" — no `portal.nousresearch.com` billing server. `gray` on user's VPS exposes `http://127.0.0.1:8645/v1` (proxy), spawns subagents (`delegate_task`), and runs `gray gateway` daemon for Telegram/Discord/Slack. `/model`/`/provider` stay untouched.

## 1. Proxy — `gray proxy` local OpenAI forwarder

**Files:** `crates/gray/src/proxy.rs` (new ~250LOC), `crates/gray/Cargo.toml` + `Cargo.toml` (`axum 0.7` + `tower-http limit`), `crates/gray/src/lib.rs` (`pub mod proxy`, `Commands::Proxy`), `crates/gray/src/main.rs` dispatch, `crates/gray/src/repl.rs` (`/proxy`/`/portal` slash).

**Steal:** `proxy/server.py:35` HOP_BY_HOP filter, `65-85` header filtering, `88-243` `create_app(adapter)` forwarder, `51` `MAX_REQUEST_BYTES=10_000_000`, `115` `allowed_paths` 404, `125` per-request `get_credential()`, `200` 401 retry, `238` `/health` + `/v1/{tail:.*}`; `adapters/base.py:38` `UpstreamAdapter` + `UpstreamCredential{bearer,base_url,token_type}`; `adapters/nous_portal.py:93` lock+refresh+quarantine (simplified); `adapters/xai.py:69` pool rotation (deferred).

**Design:**
```rust
struct UpstreamCredential { bearer:String, base_url:String, token_type:&'static str="Bearer" }
#[async_trait] trait UpstreamAdapter: Send+Sync {
  fn name(&self)->&str; fn display(&self)->&str; fn allowed_paths(&self)->&[&str];
  fn is_authenticated(&self)->bool; // cheap, no I/O
  async fn get_credential(&self)->anyhow::Result<UpstreamCredential>;
  async fn get_retry_credential(&self, failed:&UpstreamCredential, status:u16)->Option<UpstreamCredential>;
}
struct OpenRouterAdapter; struct XaiAdapter; struct CodexAdapter;
```
- `OpenRouterAdapter`: `setup::load_auth_keys()["openrouter"]` or `setup::load_saved_config_at().api_key`; `base_url` from `SavedConfig.base_url` or `DEFAULT_BASE_URL=https://openrouter.ai/api/v1` (`config.rs:8`).
- `XaiAdapter`/`CodexAdapter`: `oauth::load_auth(provider)` + `oauth::ensure_access_token(provider).await` (`oauth.rs:844`, 300s lead), `XAI_API_BASE=https://api.x.ai/v1` (`oauth.rs:19`), `CODEX_API_BASE=https://api.openai.com/v1` (`oauth.rs:26`).
- `router(adapter:Arc<dyn UpstreamAdapter>)->Router`: `GET /health -> {status,upstream,authenticated}`, `ANY /v1/*tail` -> `handle_proxy`. `handle_proxy`: tail ∈ allowed_paths else 404 `{error:{type:"path_not_allowed"}}`; `cred=get_credential().await` else 401; `fwd_headers=filter(req.headers)` + `Authorization: Bearer cred`; `reqwest::Client::request(method, format!("{}/{tail}?{qs}")).headers(fwd).body(bytes).send()`. On 401, call `get_retry_credential` once (refresh) and retry. Stream `reqwest bytes_stream` -> `axum Body`. Strip response `content-encoding/content-length`.
- `run_server(adapter, host=127.0.0.1, port=8645)`: `tokio::net::TcpListener` + `axum::serve`, `tokio::signal` SIGINT/SIGTERM, `tower_http::limit::RequestBodyLimitLayer`.
- CLI: `gray proxy start [--provider openrouter|xai|codex] [--host 127.0.0.1] [--port 8645]` (default provider = current `config.base_url`'s provider for `/model` live), `gray proxy status` (list adapters ready), `gray proxy providers`; alias `gray portal` -> `portal info` read-only table (`portal_cli.py:34` style). REPL: `/proxy start [port]`, `/proxy stop`, `/proxy status`, `/portal` spawns `JoinHandle` in `static PROXY_HANDLE: Mutex<Option<JoinHandle>>`, `tui.push_dim` status, no TUI block. Reuse `gray_home()`/auth paths, `fs2` lock for `auth.json` R-M-W (ponytail: 10LOC).

**Skip:** CredentialPool/429 rotation, shared_nous_state, billing, Tool Gateway.

## 2. Delegation — `delegate_task` (full B, flat depth=1)

**Files:** `crates/gray-tools/src/delegate.rs` (new), `crates/gray-core/src/delegation.rs` (new) or `agent.rs` add, `crates/gray-tools/src/lib.rs` add to `Registry`, `crates/gray/src/config.rs` add `delegation:{max_concurrent_children=10,max_spawn_depth=1,orchestrator_enabled=true,child_timeout_secs}`, `crates/gray/src/repl.rs` drain `completion_queue`, `Cargo.toml` `rusqlite 0.31 bundled`, `crates/gray-session` reuse `default_root()`.

**Steal:** `tools/delegate_tool.py:122` `DELEGATE_BLOCKED`, `829` `normalize_role`, `973` `max_spawn_depth`, `846` `max_concurrent_children`, `931` child_timeout, `1606` `build_child_agent`, `2454` `run_single_child` heartbeat 30s stale 450s idle / 1200s in-tool, `3625` `delegate_task` entry sync vs `background`, `tools/async_delegation.py:146` SQLite `async_delegations` + `completion_queue` + `restore_undelivered` + `delivery_claim`.

**Design:**
- `DelegateConfig{max_concurrent_children:10,max_spawn_depth:1,orchestrator_enabled:true,child_timeout:Option<Duration>}`.
- `DelegateRole::Leaf|Orchestrator`, `DELEGATE_BLOCKED=&["delegate_task"]`.
- `DelegationState{sem:Arc<Semaphore(10)>, active:RwLock<HashMap<String,ActiveRecord>>, recent:Mutex<Lru<200>>, paused:AtomicBool, completion_tx:UnboundedSender<CompletionEvent>, db:Arc<Mutex<Connection>>>}`.
- `DelegateTool` impl `Tool` def `delegate_task{goal,context,tasks?,role?,background?,action?,subagent_id?,message?}`. `execute` branches `action∈{list,steer,stop}` sync (own-tree via `Weak` + `owner_session_id`), `background=false` -> `sem.acquire_many(N)` + `FuturesUnordered` children `Agent::new(child_provider, filtered Registry)` flat depth (role downgrade if `depth>=max_spawn_depth` or `!orchestrator_enabled`), `background=true` -> `sem.try_acquire(1)` else `rejected` error, spawn `tokio::spawn`, `persist_dispatch` SQLite, immediate `{"status":"dispatched","delegation_id":...}`.
- Child: isolated `Agent` clone provider `Box<dyn Provider>` (Arc inner), fresh `Registry` filtered, `ToolContext{cwd,cancel:CancellationToken}`, `IterationBudget`, `tokio::time::timeout(child_timeout)` + heartbeat `tokio::select!`.
- `completion_queue` drained in `repl.rs` between turns, forges `Message::user` from event, never mid-turn.

**Skip:** worktree isolation, MCP inherit, composite expansion, cost rollup (ponytail comments).

## 3. Gateway daemon — `gray gateway` VPS

**Files:** new crate `crates/gray-gateway` 6 files: `config.rs`, `session.rs`, `platform.rs`, `telegram.rs`, `discord.rs`, `slack.rs`, `daemon.rs`, `systemd.rs`, `lib.rs`; `crates/gray/Cargo.toml` optional dep, `crates/gray/src/main.rs` `Commands::Gateway{run,status,install,uninstall}`.

**Steal:** `gateway/run.py:6729` `GatewayRunner` lifecycle, `gateway/session.py:148` `SessionSource`, `1090` `build_session_key`, `1221` `AsyncSessionStore`, `gateway/platforms/base.py:2890` `BasePlatformAdapter` + `2299` `MessageEvent`/`2465` `SendResult`, `plugins/platforms/telegram/adapter.py:666` `MAX 4096 utf16_len`, `slack/adapter.py:894` Socket Mode 39000, `gateway/config.py:926` `GatewayConfig`, `hermes_cli/gateway.py:2973` `ensure_gateway_service` systemd.

**Design (per B — new `~/.gray/gateway.yaml`):**
- `GatewayConfig{platforms:HashMap<Platform,PlatformConfig>, group_per_user:bool=true, thread_per_user:bool=false}` where `Platform::Telegram|Discord|Slack`, `PlatformConfig{enabled:bool,token:Option<String>,app_token:Option<String>}`. `gray_gateway_path()=gray_home().join("gateway.yaml")` `serde_yaml` 0600.
- `SessionSource{platform,chat_id,chat_type:dm|group|channel|thread,user_id,thread_id,scope_id,message_id}`, `build_session_key(src,group_per_user,thread_per_user)->String` `"gray:main:<platform>:<chat_type>[:<scope>]:<chat_id>[:<thread>][:<user>]"` (`session.py:1090`).
- `GatewaySessionStore` trait `get_or_create(key,src)->SessionId` impl `FileGatewayStore` `~/.gray/gateway_sessions.json` `RwLock<HashMap>` atomic replace fsync (ponytail: JSON not SQLite).
- `BasePlatformAdapter: async connect/disconnect/send, set_handler(Fn(MessageEvent)->Option<String>)` + `MessageEvent{text,message_id,source,media_urls}` + `SendResult{success,message_id,retryable}` + `utf16_len` for Telegram 4096 split.
- `TelegramAdapter` (`teloxide` polling), `DiscordAdapter` (`twilight` 2000), `SlackAdapter` (`slack-morphism` Socket Mode 39000) skeletons — each `MAX_LENGTH` + `splits_long_messages=true`.
- `GatewayRunner{config,adapters:HashMap,store:Arc<dyn GatewaySessionStore>}` `start()` concurrent `connect` with 45s timeout, `SIGTERM` drain, `handle_inbound(ev)`: `build_session_key->store.get_or_create->JsonlSessionStore.load/create->build_agent(config,cwd)->Agent::run(Message::user(ev.text)).await->adapter.send`.
- `systemd.rs`: `systemd_unit_path()=~/.config/systemd/user/gray-gateway.service`, `generate_unit(gray_bin)` `[Service] ExecStart=<bin> gateway run, Restart=always, Environment=GRAY_HOME`, `install/uninstall` `daemon-reload enable --now`, `status` via `systemctl is-active` + `~/.gray/gateway.pid`.

**Reuse:** `gray_home()`, `JsonlSessionStore`, `build_agent` (`lib.rs:87`), `Config`, `tokio`/`async-trait`/`serde`. **Skip:** multiplex profiles, Kanban, `SessionResetPolicy`, media cache, FTS5, plugin discovery, WatchdogSec tuning, launchd.

## 4. Integration & Parallelism

- All three read `auth.json`/`config.json` / `gateway.yaml` written by `/provider`/`/connect` — `/model` live via `fetch_live_provider_models` stays. Proxy default provider = current `SavedConfig.base_url` provider.
- Parallel subagents: A proxy (no deps), B delegation (depends on `gray-core` `Agent`), C gateway (depends on `gray-session` + `gray` build_agent) — no shared files except `Cargo.toml` workspace deps; dispatch together.

## 5. Testing

- Proxy: `cargo test proxy::filter_headers`, `curl http://127.0.0.1:8645/v1/models -H "Authorization: Bearer fake"` -> forwarded with real bearer.
- Delegation: `delegate_task` sync `tasks=[{goal}]` concurrency 2, `background=true` -> `completion_queue` drain, `list/steer/stop` control.
- Gateway: `gray gateway install` + `systemctl --user status`, Telegram ` /health` -> `SendResult` truncation 4096.

