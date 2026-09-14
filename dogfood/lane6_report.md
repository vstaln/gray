# lane6 dig — session/resume/continue, /compact+recall, corrupt recovery, cache, gateway (FAKE tokens)

Branch `feat/batch-reunite`, analysis-only. No source writes, no commits, no `cargo test`.
Sandbox `GRAY_HOME=/tmp/lane6_grayhome_*` (never `~/.gray`). Binary `target/debug/gray` (prebuilt 2026-09-08).
tmux `lane6_gw` created + killed; no other tmux touched. No real tokens. Zero live LLM turns (no 429 possible).
Probe: `dogfood/lane6_probe.py` → `dogfood/lane6_results.json`. Second run total **9.7s**.

## Per-flow table

| flow | result | wall | live vs simulated | note |
|---|---|---|---|---|
| F1 `resume --last` empty | PASS | 0.01s | live CLI, no LLM | `Error: no saved sessions in this directory (try --all)` |
| F2 `resume bogus` | PASS | 0.01s | live CLI, no LLM | `no session matching 'bogus…': session … not found` (`resume.rs:200-212`) |
| F3 resume valid prefix announce | PASS | 0.39s | live load+announce; follow-up turn NOT run | `⬢ Resumed session lane6-… (2 messages)` (`resume.rs:245-258`); 0.4–5s = announce + REPL EOF exit |
| F4 `gray -p hi -c` no key | PASS | 0.02s | live `-c` resolution; LLM NOT executed | `no model configured yet — run /provider…` — proves `-c` resolved before provider gate (`print.rs:116-138`) |
| F5 compact marker+recall | PASS | 0.00s | **SIM** marker append + `find_last_checkpoint`/`has_content_since` replica; summarization NOT run | `last_ckpt=3 has_content_since=False fact714=True` (`gray-session/src/lib.rs:743-755`) |
| F6 corrupt header quarantine | PASS | 0.01s | live (`lib.rs:461-470,260-289` via `resume.rs:200-212`) | file → `.corrupt-1`, orig gone; CLI says `no session matching` (see H1) |
| F7 torn final line | PASS | 0.16s | live (`lib.rs:492-498`) | `Resumed session … (1 messages)` — prior entries preserved, torn tail warned+skipped |
| F8 corrupt middle line | PASS | 0.01s | live (`lib.rs:499-505`) | `corrupt entry at …:3`; file left in place (see H2) |
| F9 cache progression | PASS | 0.00s | **SIM** Usage math only | hit_rate `[0.0, 0.692, 0.889]` total_in=3650 (`event.rs:62-66`, `status.rs:34-63`) |
| F10a `gateway run` no platforms | PASS | 0.02s | live boot guard | `no gateway platforms enabled — edit $GRAY_HOME/gateway.yaml` (`daemon_boot.rs:132-134`) |
| F10b `gateway status --probe` | PASS* | 0.01s | live (`health.rs:32-50`) | `healthy: heartbeat 0s ago` rc=0 — *stale-positive, see H3 (heartbeat written by failed F10a boot)* |
| F10c `pairing list all` | PASS | 0.01s | live registry, no creds | `slack: 0 pending, 0 approved` (sandbox store) |
| F10d FAKE-token `gateway run` (tmux `lane6_gw`, `timeout 8`) | PASS | 9.03s | live boot, fake creds; connect/poll NOT reachable | `EXIT:124` (timeout), no `online` notice — hangs in retry, see H6 |
| F10e synthetic inbound pure-fns | PASS | 0.00s | **SIM** replicas | slash/key/split all true (`daemon.rs:110`, `authz.rs:76`, `session.rs:22`, `platform.rs:242`) |

14/14 PASS (F10b passes mechanically but records a false-healthy — counted as hole H3, not a code fix).

Tokens/cache, live shape: `session_usages()` rows carry `in/out/reasoning/cached/read/write`
(`scenarios/dfutil.py:85-106`); `SessionTotals::from_entries` sums only entries with `usage`
(`repl/status.rs:55-62`); footer `⬡ N tok · …` (`status.rs:67-86`). Live prefix-cache pins
`prompt_cache_key`=session id (`provider/openai.rs:34,83,1059`) + Anthropic `cache_control` breakpoints
(`openai.rs:401-427`); usage mapping inclusive/non-cached/read/write (`openai.rs:770-830`).
Nothing live exercised here — F9 synthetic only.

## Holes (file:line + severity)

- **H1 MED — strict-resolve masks corruption as NotFound.** `resume.rs:200-212`
  `resolve_session_strict` calls `resolve_prefix` → `store.list()` quarantines+skips the corrupt
  header (`gray-session/src/lib.rs:555-561`) → subsequent `load()` finds no file → `NotFound`
  (`lib.rs:443-445`). F6 evidence: `.corrupt-1` created + orig gone, yet message is
  `no session matching 'lane6bad-…': session … not found`, no `corrupt` word. Operator must
  notice the sidecar file; picker/`--last` also silently hide it.
- **H2 MED — mid-file corruption never quarantined, resume blocked until manual fix.**
  `load()` returns `Corrupt` for non-final bad lines (`lib.rs:499-505`) with no
  `quarantine_corrupt_file` call (header-only, `lib.rs:461-470`). F8 reproduces: every
  `resume` fails at `:3`, file stays. Meanwhile `list()` skips unparseable entry lines when
  deriving `first_user_text` (`lib.rs:565-575`), so the session still looks healthy in the picker.
- **H3 MED — failed boot poisons the probe healthy.** `daemon_boot.rs:111-119` marks boot +
  writes heartbeat *before* the empty-adapters bail (`:132-134`); `probe_full` falls back to
  heartbeat when no snapshot exists (`supervise/health.rs:88-91`, `gateway/status.rs:180-190`).
  F10a→F10b evidence: failing `gateway run` leaves `state/gateway.heartbeat` +
  `gateway.lifecycle.json`, next `status --probe` says `healthy: heartbeat 0s ago` rc=0.
- **H4 LOW — `-c` silently crosses directories.** `latest_session_anywhere`
  (`resume.rs:217-223`) + REPL `-c` path (`repl/mod.rs:389-430`) fall back to global latest when
  cwd has none. Running `-c` in the wrong dir continues an unrelated session with only the
  `⬢ Resumed session …` line as signal.
- **H5 LOW — `/usage` undercounts turns without usage.** `SessionTotals::from_entries`
  (`repl/status.rs:55-62`) counts only `usage.is_some()` entries; resumed legacy/failed turns
  are invisible in totals while still occupying context.
- **H6 INFO — plausible-fake tokens hang instead of fast-failing.** Shape checks pass
  (`telegram.rs:127`, `discord.rs:180`, `slack.rs:102,116`) for `123456:FAKE…`/`a`*50/`xoxb-FAKE…`,
  then boot sits in `connect_adapter_with_retry` (`daemon_boot.rs:163-173`,
  `BOOT_MAX_ATTEMPTS=3` in `daemon_supervise.rs:34`, backoff `platform.rs:119-123`) until outer
  timeout. F10d: 9s, `EXIT:124`, no terminal reason on stdout. Real misconfig vs network
  indistinguishable without log tail.
- **H7 LOW — threshold auto-compact persists via N single appends.**
  `maybe_threshold_compact` / `maybe_overflow_compact` (`repl/session.rs:517-521,551-555`)
  loop `store.append` per message (each re-reads the file, `lib.rs:371-438`). No batching;
  a torn write mid-loop risks partial duplication. Code-only; live path needs LLM so not executed.

## What was simulated vs live (no fixes)

- Live (sandbox, no LLM, no network success): F1, F2, F3 (load+announce only), F4 (resolution
  up to provider gate), F6, F7, F8, F10a, F10b, F10c, F10d (boot attempt under `timeout 8`).
- Simulated (pure-logic replicas, no creds/network): F5 (checkpoint marker math, not
  `compact_with_keep`/`run_compaction_call` in `compact/mod.rs:172-203` + `core/compact.rs:129-149`
  + `status.rs:445-510` which need `complete_prompt`), F9 (Usage/hit-rate arithmetic),
  F10e (slash/authz/key/dedup/split replicas).
- Not reachable without real keys (documented, not attempted): adapter `connect()`
  (`telegram.rs:382-393` teloxide `get_me`, `discord.rs:429-452` current_user,
  `slack.rs:315-331` auth.test + Socket Mode `app_token`), steady-state polling/shards,
  `DeliveryRouter` sends/edits/deletes, cron `home_channel` fan-out. Token *values* never
  printed (redacted); only `allowed_users` IDs and shapes inspected.
- Wall times above are sandbox-local; F3 variance (0.4–5s) is REPL EOF shutdown, F10d is the
  8s `timeout` + tmux overhead. `429=environmental`: zero live model calls, so no rate-limit
  signal in this lane.
