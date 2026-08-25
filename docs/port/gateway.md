# Gateway subsystem recon (port-reconnaissance)

Source: `reference/NousResearch/hermes-agent/gateway/` — 95 `.py` files, ~114k LOC.
Labels: PROVEN = read directly; CONJECTURED = inferred from names/counts only.

## 1. Module inventory by responsibility

### Core orchestration
- **`run.py`** (PROVEN: 31,358 lines) — the monster. Structure:
  - Lines 1–4293: ~120 module-level helper functions (status messages, hygiene/compaction cooldowns, media placeholders, resume/replay notes, adapter disposal, reconnect backoff, config loading, model-context resolution, turn-process reaping, wedged-turn stack dumps).
  - L4294 `class TurnRunner` — runs a single agent turn (~2,400 lines to next class).
  - L6729 `class GatewayRunner(GatewayAuthorizationMixin, GatewayKanbanWatchersMixin, GatewaySlashCommandsMixin)` — the god object: **371 methods** across ~23k lines (L6729–30112). Owns platform adapters, session DB, delivery ledger, stream dispatch, shutdown.
  - L30112+ `_run_planned_stop_watcher` and tail functions.
  - Notable mid-file imports (L1765, L2073–2315) → circular-import workarounds; Rust port must break these into real modules.
- Mixins (split OUT of run.py conceptually): `authz_mixin.py`, `kanban_watchers.py`, `slash_commands.py` (+`slash_access.py`).

### Platforms (`platforms/`)
- `base.py` (adapter ABC), `helpers.py`, `_http_client_limits.py`
- Per-platform: `signal.py` + `signal_format.py` + `signal_rate_limit.py`; `whatsapp_common.py` / `whatsapp_cloud.py`; `qqbot/` package (adapter, crypto, chunked_upload, keyboards, onboard); `webhook.py` + `webhook_filters.py`; `msgraph_webhook.py`; `yuanbao.py` + `yuanbao_proto.py` + `yuanbao_media.py` + `yuanbao_sticker.py`; `weixin.py`; `bluebubbles.py`; `api_server.py`; `media_cache.py`.
- Registration (PROVEN): `platform_registry.py` is a true runtime self-registration system — adapters call `platform_registry.register(PlatformEntry(...))` at import time; plugins register via `PluginContext.register_platform()`. Supports **deferred loaders** (`register_deferred` — a cheap placeholder replaced by the real adapter on first use, direct registration wins), per-plugin "scope keys", and `snapshot_registration`/`restore_registration` for hot reload. Built-in adapters found: `APIServerAdapter`, `BasePlatformAdapter` (ABC in base.py:2890), `BlueBubblesAdapter`, `MSGraphWebhookAdapter`, `SignalAdapter`, `WebhookAdapter`, `WeixinAdapter`, `WhatsAppCloudAdapter` (+`WhatsAppBehaviorMixin`), `YuanbaoAdapter`, `QQAdapter`.
- **Plugin platforms live OUTSIDE gateway/** (PROVEN): `plugins/platforms/{discord,dingtalk,feishu,mattermost,simplex,homeassistant,raft,photon,buzz}/adapter.py` each subclass `BasePlatformAdapter` and self-register via `hermes_cli/plugins.py`. A 1:1 Rust port of gateway/ alone will not cover telegram/discord/slack/etc. unless the plugin registry is part of the contract.

### Relay (`relay/`)
- `ws_transport.py`, `transport.py`, `adapter.py` (ws bridge for remote clients), `auth.py`, `media.py`, `descriptor.py`, `command_manifest.py`.

### Session & state
- `session.py`, `session_db_recovery.py`, `session_context.py`, `session_state.py`, `session_stall.py`, `turn_context.py`, `turn_lease.py`, `message_timestamps.py`, `profile_routing.py`.

### Delivery & streaming
- `delivery.py`, `delivery_ledger.py`, `stream_dispatch.py`, `stream_consumer.py`, `stream_events.py`, `streaming_tts_consumer.py`, `dead_targets.py`, `rich_sent_store.py`, `mirror.py`, `response_filters.py`.

### Lifecycle / watchdogs
- `shutdown_watchdog.py`, `shutdown_flush.py`, `shutdown_forensics.py`, `restart.py`, `restart_loop_guard.py`, `drain_control.py`, `readiness.py`, `scale_to_zero.py`, `cgroup_cleanup.py`, `memory_monitor.py`, `code_skew.py`, `systemd_notify.py`, `wake.py`, `lifecycle_ledger.py`.

### Control & access
- `control_socket.py`, `pairing.py`, `slash_access.py`, `channel_directory.py`, `display_config.py`, `config.py`.

### Misc/peripheral
- `hooks.py` + `builtin_hooks/`, `browser_control_broker.py`, `browser_control_artifacts.py`, `sticker_cache.py`, `whatsapp_identity.py`, `memory_status.py`, `disk_status.py`, `status.py`, `status_phrases.py`, `runtime_footer.py`, `agent_cache_pressure.py`, `cwd_placeholder.py`, `utils.py`-style helpers.

## 2. Deps reaching outside gateway/ (PROVEN from import grep)
- `hermes_constants` (root module), `utils` (root: atomic_json_write etc.)
- `hermes_cli.*`: `config`, `fallback_config`, `env_loader`, `config_defaults`, `plugins` (plugin platform registration)
- `agent.*`: `secret_scope`, `turn_context`, `i18n`, `interrupt_compat`, `async_utils`, `conversation_compression`, `conversation_loop`, `compaction_display`, `replay_cleanup`
- `plugins/platforms/*/adapter.py`: 9+ adapters subclass gateway's `BasePlatformAdapter` — the base-class trait is a cross-package contract.

## 3. PyPI deps → Rust crates
| Python | Rust |
|---|---|
| asyncio, concurrent.futures | tokio (+ `tokio::task::JoinSet`) |
| websockets/ws relay transport | tokio-tungstenite (axum if HTTP-serving platforms merged here) |
| httpx | reqwest |
| sqlite3 | rusqlite |
| yaml | serde_yaml |
| dotenv | dotenvy |
| signal/systemd notify (systemd_notify.py, sd_notify via socket) | zbus not needed for sd_notify — raw datagram or `sd-notify` crate; dbus only if bluebubbles/weixin need it |
| struct/binascii/crypto in qqbot/crypto.py, hmac/secrets | ring or RustCrypto crates (hmac, sha2, aes-gcm as needed) |
| mimetypes, unicodedata | mime_guess, unicode-normalization |

## 4. Port order within subsystem
1. `config.py` + hermes_cli.config surface (everything depends on it)
2. `session.py` / session-state modules + `delivery_ledger.py` (sqlite schemas first)
3. `platforms/base.py` trait + `platform_registry.py`
4. One simple platform end-to-end (webhook.py is likely simplest) to prove the adapter loop
5. run.py decomposition: extract TurnRunner first (self-contained per-turn logic), then GatewayRunner slice-by-slice following its mixin seams (slash_commands, kanban_watchers, authz)
6. relay/ (independent ws server), then remaining platforms, then watchdogs/lifecycle last

## 5. Rust risks
- **Dynamic platform registration** (PROVEN): import-time self-registration + deferred loaders + snapshot/restore for plugin reload. Rust has no import side effects — needs an explicit registry (inventory crate, or a `Platform` enum + factory match) and a decision on whether out-of-tree plugin adapters are in scope. The scope-key/deferred semantics must be preserved or adapter resolution diverges.
- **Async task supervision**: run.py spawns many fire-and-forget tasks (`consume_detached_task_result`, `safe_schedule_threadsafe`, watchdogs reaping wedged turns). Rust needs JoinSet/cancellation-token discipline or these become leaks; the "wedged turn" reaper logic maps to tokio::time::timeout + abort.
- **SQLite WAL concurrency** (PROVEN): session.py guards against "WAL split-brain" across multiplexed profile homes (secondary profiles must not hold WAL sidecars of the canonical DB); kanban_watchers notes concurrent manual WAL checkpoints can corrupt index pages; sqlite calls are pushed to threads (`asyncio.to_thread`) so the WAL lock never blocks the loop; delivery_ledger ties liveness to owner pid + process-start-time. Rust: rusqlite behind one writer task or a pool, `busy_timeout`, and replicate the pid-liveness ledger logic; multi-profile homes mean per-profile DB handles, not a global one.
- Mid-file imports in run.py signal hidden module coupling; the 371-method GatewayRunner will resist clean trait splits — decompose along data ownership, not method count.

## 6. Load-bearing vs peripheral
- **Load-bearing**: run.py (TurnRunner + GatewayRunner), config.py, session*, delivery*, stream_dispatch, platforms/base + registry, control_socket, pairing, shutdown_watchdog.
- **Peripheral**: sticker_cache, browser_control_*, yuanbao_* (single vendor), memory_status/disk_status/status_phrases/runtime_footer, cwd_placeholder, hooks/builtin_hooks (unless hook API is contractual).
