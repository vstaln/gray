# Supervision core (24/7) — design

Status: approved 2026-09-06 (minimal scope; §1–§3 signed off in chat).
Scope lock: Hermes parity was considered and rejected for this cut;
this spec is the minimal supervision core only. SQLite recovery,
cron-in-gateway, Docker/s6, Windows service, and scale-to-zero are
explicit non-goals with follow-up notes at the bottom.

## 1. Goal

Give gray a VPS-grade always-on story without touching platform code:
`gray gateway install` produces a hardened auto-restart service,
the daemon reports liveness through a heartbeat file, stalls and
unclean exits are detectable, logs rotate, and `gray gateway status`
can probe health with one command. Discord/Telegram/Slack adapters
stay behind existing features (`telegram`, `discord`, `slack`,
`all-platforms`) and are not modified except to call into the new core.

## 2. Architecture

New crate `crates/gray-supervise` (pure std + tokio + serde_json +
anyhow; no teloxide/twilight/slack-morphism). It owns:

- exit-code contract (`0` clean, `75` restart, `78` fatal)
- heartbeat writer + startup/shutdown watchdogs
- lifecycle ledger (`state/gateway.lifecycle.json`)
- health/readiness probe (file-based, no TCP port)
- systemd + launchd unit generators
- size-capped log-rotation helper

`gray-gateway` depends on `gray-supervise` for boot, shutdown, restart,
and `gateway status --probe`. Platform surface is unchanged:
`platform.rs` trait, `telegram.rs` / `discord.rs` / `slack.rs`
adapters, `daemon.rs` inbound pipeline, `delivery.rs` ledger,
`pairing.rs`, `session.rs` mapping all stay where they are.

Current anchors (read 2026-09-06): unit generation in
`crates/gray-gateway/src/systemd.rs:6-15`, boot in
`crates/gray-gateway/src/daemon_boot.rs:68-80`, singleton flock in
`crates/gray-gateway/src/lock.rs:1-33`, reconnect ladder in
`crates/gray-gateway/src/daemon_supervise.rs:29-36,97-169`,
append-only logging in `crates/gray/src/logging.rs:123-141`,
restart marker in `crates/gray-gateway/src/daemon.rs:324-335`.

## 3. Components

- **Exit codes.** `0` = clean stop (no respawn needed),
  `75` = supervisor must restart (drain/reload, `/restart`),
  `78` = fatal config (do not respawn). `/restart` exits 75
  (today: `exit(0)`); config-load failure exits 78. Units map with
  `RestartForceExitStatus=75` / `RestartPreventExitStatus=78`.
- **Heartbeat.** `~/.gray/state/gateway.heartbeat` rewritten every 15s
  on a thread off the hot loop (`GRAY_HEARTBEAT_SECS` override).
  Startup watchdog 120s: if boot has not marked ready by then, log one
  line and exit 75. Shutdown drain budget 30s on SIGTERM, then exit.
- **Lifecycle.** `state/gateway.lifecycle.json` with `boot_id`,
  `started_at`, `clean_shutdown`. Boot writes `clean_shutdown:false`;
  clean shutdown flips to `true`. Next boot seeing `false` logs
  "previous exit unclean" (no guessing, no PID-reuse heuristics).
- **Health.** No TCP port (keeps zero-deps story). Freshness rule:
  heartbeat mtime < 60s + lock held + `gateway.yaml` parses =
  healthy. `gray gateway status --probe` implements it, exit 0/1 with
  one line; used by humans and by `ExecCondition`-style checks.
- **Units.** systemd user unit hardened: `Restart=always`,
  `RestartSec=5`, `StartLimitIntervalSec=0`,
  `TimeoutStopSec=90` (drain 30s + headroom),
  `After=network.target`, `WantedBy=default.target`, plus the two
  exit-status lines. Install prints `loginctl enable-linger` hint when
  linger is off. Launchd plist generator beside it (macOS
  `KeepAlive`, `RunAtLoad`, `ThrottleInterval`, log paths).
  Linux + macOS only in this cut.
- **Logs.** `logging.rs` gains rotation: 10MB × 3 (`gray.log`,
  `gray.log.1`, `gray.log.2`), same redaction rules, no new deps.

## 4. Data flow

Boot: acquire flock → read lifecycle (log unclean if set) → write
lifecycle (`clean_shutdown:false`, new `boot_id`) → start heartbeat
thread → connect adapters via existing ladder → on ready, kick
watchdog → serve. SIGTERM: stop accepting, drain 30s, delete nothing
(delivery ledger still sweeps on next boot as today), flip lifecycle
to `clean_shutdown:true`, exit 0. `/restart`: write existing restart
marker, exit 75; systemd revives; boot pings requester + `● Gray
gateway online.` as today.

## 5. Error handling

Probe never throws: missing/stale heartbeat, missing lock, or bad
config each yield `unhealthy: <reason>` + non-zero exit, one line.
Watchdog failure path logs a single line and exits 75 (no panic
payload). Unit install failures keep today's behavior (write file,
best-effort `daemon-reload` / `enable --now`, print path). Delete and
delivery paths stay best-effort; supervision never fails a turn.

## 6. Testing

- Unit: exit-code map, heartbeat freshness boundary (59s vs 61s),
  rotation cap (4th write drops oldest), systemd/launchd golden text
  (contains the Restart/Timeout/KeepAlive lines, no secrets).
- Integration (tokio): stall boot past watchdog → exit 75;
  clean SIGTERM → lifecycle flips to `true`; `status --probe` against
  a fresh vs stale heartbeat dir.
- Gates: `cargo test -p gray-supervise`, `cargo test -p gray-gateway`
  (no features), `cargo check -p gray-gateway --features
  all-platforms`. No network.

## 7. Rollout

`gray gateway install` writes the hardened unit (existing
`gray-gateway.service` is copied to `.bak` first, only if no `.bak`
exists yet); `status --probe`
documented in README Gateway section; no new config keys except
`GRAY_HEARTBEAT_SECS`. Existing `gateway run` foreground behavior
unchanged apart from heartbeat + exit codes.

## 8. Deliberate non-goals (later)

SQLite WAL state + auto-resume, cron ticker in core, Docker/s6 image,
Windows Scheduled Task, `Type=notify`/`WatchdogSec` + `sd_notify`,
`systemd --system` unit, metrics endpoint, scale-to-zero. Each gets
its own spec if wanted; nothing here closes those doors.
