# Changelog

## [0.1.1] - 2026-09-21

### Added

- Prompt-cache warmth timer + cache-miss warning in the composer. The
  footer carries a `◷ 4m` countdown next to the cache-hit percentage —
  how long the last request's prompt cache stays warm before an idle gap
  re-bills the whole prompt (Anthropic's `cache_control` and OpenAI's
  automatic prefix caching both expire after ~5 idle minutes), fading in
  the last minute and hidden entirely when the provider never reports
  cache activity. When a request re-bills tokens the previous one should
  have served from cache — an idle gap past the TTL, a model switch, or a
  provider-side eviction — the transcript gets a warning row
  (`⚠ Cache miss after 6m idle: 100k tokens re-billed (~$0.35)`) once the
  miss crosses 20k tokens or $0.10 over a full hit, so routine breakpoint
  noise stays silent. Detection is a port of pi's
  `packages/coding-agent/src/core/cache-stats.ts` (reference checkout
  under `reference/pi-mono`) onto gray's per-round `StepUsage` reports;
  compaction and `/new` reset the baseline, since a fresh summary is new
  content rather than re-billed content
- Remove a provider from the connect modal: `shift+enter` on a highlighted
  provider (the modal footer advertises it only for a row that holds a stored
  credential) opens a confirmation naming the provider and its endpoint, and
  `enter` deletes the credential — `auth.json` entry plus the active
  provider's second copy in `config.json`, so a removal cannot resurrect on
  the next start. `/provider` reloads the agent and reports the removal
  instead of announcing a connection
- Gateway daemon (`gray gateway ...`): the always-on host that fires cron with
  no REPL open. `run` is the foreground daemon (60s cron ticker with the same
  `HeadlessRunner`/`SaveLocalDeliver` as `tick`/`serve`, plus a control socket
  at `$GRAY_HOME/gateway.sock` answering `identify`/`status` — one JSON line
  in, one out, hermes wire shape); `install` writes a user service (runit
  `run`+`log/run` on Void, systemd `--user` unit elsewhere, `--print` previews,
  `--no-start` defers), `start`/`stop`/`restart` drive it, `uninstall` removes
  it, `status` reports daemon + service + ticker health (exit 1 when down).
  Process shape follows the hermes gateway: O_EXCL pid claim with
  `/proc` start-time against PID reuse, `gateway.state.json` recording why the
  last run stopped, socket-first liveness with pid-file fallback, 0600 socket,
  supervisor detected from the real init (never inferred outside-in), and a
  bounded 65s drain of in-flight fires on SIGTERM/SIGINT. `install`/`start`
  wait up to 10s for `runsvdir` to pick up a fresh service dir (it rescans
  every ~5s) instead of racing it, and `stop` tells runit-stopped vs
  tick-draining apart (was a `⚠ still running` either way).
- Cron in-chat firing: background REPL tick, `/cron` dashboard, delivery seam.
- Cron workstream B: `gray cron tick|serve|pause|resume|run`, job skills +
  pre-run scripts, local-file delivery (`cron/output/<id>/<ts>.md`).
- Cron ticker liveness: every tick pass writes a heartbeat
  (`$GRAY_HOME/cron/.last_tick`), and `gray cron list`, `gray cron add`, and the
  `/cron` dashboard report it — a store nobody is ticking now says so instead of
  printing a `next=` that will never arrive.
- Plugin system: `gray-plugin` crate with Plugin trait, builtin tools as profile-ordered plugins, `gray.yml` profile loader + sidecar entries, sidecar hook protocol over stdio with timeout/crash degradation
- Gateway daemon: `gray-gateway` crate (Telegram/Discord/Slack), real Discord adapter with slash commands, OAuth2 invite URL, full `/gateway` REPL suite, delegation durability
- Cron jobs: schedule/store/CLI + REPL, local wall-clock daily schedules, AI self-scheduling via `schedule_task`
- Dynamic context window via models.dev + disk cache (LiteLLM table, proportional reserve/keep), `/context` visual modal with suffix completion and thousand-separator parsing
- Usage/cost tracking: session cost from LiteLLM rates, footer + `/usage`, persisted session totals
- Skills: `skill` tool loading SKILL.md bodies, `/skills` command, discovery across opencode plugins/agents/claude skills
- Anthropic prompt caching always-on with cache hit-rate display; reasoning summaries on Responses API
- `request_user_input` tool (question overlay) so the agent can ask the user mid-turn
- Codex-style session resume (`--last`/`--all`, picker, transcript replay)
- `/effort` thinking-level selector and `/thinking` toggle
- Auto-compact on threshold/overflow; exploration-stall guard
- Noir space marketing site (landing/docs/pricing, favicon/OG/robots/sitemap) deployed to gray.alignment.id
- Multi-platform releases (darwin x86_64/aarch64, linux aarch64) with release channels (`RELEASING.md`)
- Startup update check + `gray update` subcommand
- Bulk hermes→Rust 1:1 port slices across core/cli/tui/tools/plugins/gateway/provider/sandbox/state/cron/acp
- Effort picker filtering: online models.dev capability filter with offline static ARM fallback
- Wire is_error flag, DeepSeek provider path, resume-replay, retention policy, and chat-shard routing
- Windows cfg gates for platform-specific code paths
- Resume messages for restored sessions
- Shift+Enter newline handling in composer
- Footer/badge/panel TUI chrome updates
- Attach guards for file attachment paths
- Glob tool for file pattern matching
- Session-ID threading across turns
- prompt_cache_key passthrough for chat requests
- `gray sessions prune --older-than-days N` for session-store GC; `persist_redacted: true` gateway option to scrub secrets from persisted gateway transcripts
- Verified installs: SHA256SUMS published per release, checked by install.sh (S1)
- `GRAY_NO_UPDATE_CHECK=1` and 24h update-check cache (L4)
- Gateway autostart defaults off; corrupt gateway.yaml warns instead of silently resetting (S2, S3)
- Safety / Subcommands / Platform / gateway docs in README (D2, S4)

### Changed

- `gray plugin install discord` compiles the plugin from its pinned commit
  (`cargo build --release --locked`) instead of pip-installing a Python
  package, so installing a first-party plugin needs no interpreter. The
  catalog is Rust-only; user-written plugins stay language-agnostic
  (`GRAY_PLUGIN_PATH`, `plugin.sh`). A source pin must be a full commit ID,
  and the built binary has to answer `plugin/manifest` with the expected
  name before it is registered
- Clipboard/image paste is core again: `arboard` + `image` are always compiled in, no `--features clipboard` needed (kept as a no-op alias)
- Removed the native messaging gateway: deleted `crates/gray-gateway` (adapters, daemon, pairing, delivery, systemd), the `plugins/gateway` sidecar, `gray gateway ...`/`gray send`, and the `telegram`/`discord`/`slack`/`all-platforms` features. Chat returns as a plugin; `gray cron --deliver` targets are stored opaquely until a delivery backend exists. Dropped the `--all-features` CI checks.

### Fixed

- Tool headers no longer panic the REPL on multi-byte commands. The
  one-line command/arg preview byte-sliced at a fixed offset 80, so a
  command whose first line carried an emoji (or any wide char) straddling
  that offset — a pasted PR review's 🟠 merge-risk marker did exactly this,
  killing a live session mid-tool-call — crashed with `byte index 80 is
  not a char boundary`. The cap is now counted in display cells and cut on
  a char boundary (the repo's `text_width` helpers), so ASCII commands cut
  at exactly 80 as before and wide text fills the same 80 cells instead of
  a quarter of the way in. The same fixed-offset pattern was audited
  across the crates; the only other byte-slice site is ASCII-guarded
- Bash has no default timeout any more: commands run until they exit, and an
  explicit `timeout` is opt-in (clamped 1–3600 s) with the agent-level
  last-resort stop raised to 3660 s. The tool description used to promise
  120 s while the code killed at 30 s.
- A failing command piped into a pure text filter no longer reads as success.
  `sh -c` reports only the last stage's status, so `pytest -q | head -40`
  returned `exit 0`; the exit report now names the masked stage and covers
  `head`/`tail`/`cat`/`less`/`awk`/`sed`/`tr`/`cut`/`column`.
- Missing commands are explained instead of just failing: the result carries
  "`rg` is not installed here · use an equivalent you already have …".
- Truncated output names the omitted byte window and the offset of the next
  page, and pages are 16 KiB instead of 4 KiB.
- The shell inline budget is 48 KiB (was 12 KiB), the turn event cap is 500k
  (was 100k — long runs died on "turn event limit exceeded"), the loop guard
  nudges at 3 identical tool+args and only aborts at 6, and a dropped
  provider stream keeps complete tool args with a warning instead of killing
  the turn.
- Sampling is reachable: `GRAY_TEMPERATURE`/`GRAY_TOP_P` (env or saved
  config) are sent with every request and omitted when unset. Memory size
  caps are gone and `gray memory list/show/set/edit/remove/clear` exists.
- tps is now measured over streaming time only. Every rate (working pill,
  end-of-turn `Thought for … · N tps`, headless footer) divided by the
  whole-turn duration, so a turn that spent 40s in tool calls and 4s
  generating reported ~13 tps instead of ~125 — tool waits and inter-round
  gaps now never enter the denominator (`TurnStreamClock`). The `· 40s`
  duration next to it stays whole-turn on purpose
- Interrupted turns now save a complete transcript. The REPL gave a cancelled
  run 5s to clean up and then dropped it; a stall nothing can interrupt (a
  plugin `tool/before`/`pre_tool` hook, an in-flight compaction, a tool that
  ignores cancellation) skipped the loop's own cancel cleanup entirely, so the
  session stored an assistant `tool_use` with no `tool_result` — strict
  providers then 400 on resume and the session is bricked for good — and the
  partial text the user had already watched stream by was lost. The turn is now
  repaired on the way out (`Agent::repair_dropped_cancel`): unanswered calls
  get a synthetic `cancelled by user` result and the streamed text is
  salvaged, exactly once
- `plugin list` / `/plugin list` now show `install plugin` commands (e.g. discord): the list merges `commands.json` CLI entries with `lock.json` sidecars (CLI rows tagged `[command]`), and `enable`/`disable`/`remove` route to whichever registry owns the name (was `not installed`); `update <command>` warns and no-ops like other non-index sources. `gray plugins` (CLI) is pinned as the `gray plugin` alias by test
- Cancelling a turn no longer discards the in-flight tool's own report: both
  cancel paths (single dispatch, parallel join) abandoned the future on the
  same token the tool watches, so partial output and the process-group kill
  never ran and the turn answered with a bare synthetic `cancelled by user`.
  A cancelled tool now gets a bounded 3s window (inside the turn's own
  cooperative window) to report, then the turn ends with that output in
  history.
- Cron silence: `next=` rows look identical whether or not a driver (`serve`, a
  `tick` host, a REPL) is running, so jobs could sit due forever unnoticed.
  Overdue jobs are now named in `list`/`/cron`, and `add` warns at creation when
  no ticker has run inside the liveness horizon.
- Skills: folded (`description: >`) and literal (`|`) frontmatter now parse (were the bare marker) + `gray plugin install` accepts bare `https://github.com/<owner>/<repo>` URLs as git sources — `https://github.com/DietrichGebert/ponytail` installs all six skills, same as `npm:@dietrichgebert/ponytail`
- Project context: `AGENTS.md` / `CLAUDE.md` (cwd up to git root) now auto-attach as `<project_context>` hook context every turn — no more manual `cat`, and `/context` bills the exact block instead of showing `0 tokens`. `~/.gray/AGENTS.md` excluded (never double-billed); 16k chars per-file cap
- Modal backdrop: textarea copy pinned dim (box bg + text through the color map) with a universal regression test — no full-brightness composer surface may survive in any modal backdrop
- Dogfood (headless/pipe): bare `/thinking` and `/model` print status instead of a raw `No such device` error, bare `/resume` and `gray resume` (no TTY) list sessions as text, `/compact` with no model prints one line, exit-hint has no raw ANSI when piped
- Dogfood (plugin check): reference echo sidecar returns valid JSON on `{}` args — `tool/call` + concurrency checks pass
- `gray2` binary target (`cargo build` yields `gray` + `gray2`) + one regression test, zero warnings
- Dogfood (modal backdrop): input/footer text behind modals dimmed through the color map, not just SGR faint (was full-bright on terminals ignoring faint)
- Piped `/thinking` status lists the current model's filtered levels (same provider-driven filter as the modal), not the full catalog
-- `/thinking` levels: qualified models.dev entries (kilo/openrouter `meta/muse-spark-1.3-contributor` WITH `max`) no longer leak `max` into the bare contributor id via the suffix alias (gap-fill; exact provider keys stay authoritative) — the live picker showed `max` once the background models.dev fetch landed
-- Modal backdrop: transcript user cards now dim to near-black like the input box (the preserved full-gray card glowed through behind modals)
- Auto-compact UI (Codex parity): threshold/overflow/manual compaction raises a dedicated `Compacting context` status with its own clock before the summarization call; follow-up status writes can't obscure it, only the matching completion posts `Context compacted · {elapsed}`, and the input box stays mounted (viewport/transcript/textarea untouched)
- Bash-only tools with skills context: `tools-minimal` is `bash` + `shell_output` + `shell_kill` + `sleep` (no `skill` tool). The always-on context-only `skills` plugin appends the fresh `<available_skills>` list each turn and the model reads matches with bash (`cat <location>`); `/skills <name>` still pastes the skill visibly into chat before running it
- Working pill: clock anchors to the turn start (tool `Preparing tool:`/`Working` re-stamps no longer restart it at 0.0s) and the spinner carries no token estimate — exact counts stay in the footer gauge (context), the `Thought for · N tok` line (billed turn output) and `/usage` (session). Removes the chars/4 live estimator that read 2.5M on a 14s turn
- Skills: `/skills [name] [args]` (alias `/skill`) replaces the `/skills:<name>` colon form; bare `/skills` lists all discovered skills (global + project), not just `~/.gray/skills` installs, and no longer prints the text list on top of the TTY manager
- Synthesize tool outputs for orphaned function calls (unbricks sessions after mid-turn cancel)
- Classify upstream 5xx as ServerError; connection-safe errors with 10s timeout
- Detach bash tool with setsid to prevent password-prompt hangs; char-safe log preview (emoji byte-slice panic)
- SQLite session store WAL pragma; persist/restore token usage across resumes
- Legacy daily UTC crons auto-migrated to local wall-clock
- Continued TUI polish (card-box transcript/composer, alternate-screen modals, codex-style diffs, resize/reflow fixes)
- Ponytail cleanups: dropped mermaid renderer, vendored hermes port, rusqlite bundled feature, assorted dead code
- Modal dim fix for overlay backdrop rendering
- 429 backoff floor for rate-limit retries
- Billing-429 treated as terminal (no retry on insufficient credit/quota)
- 413 overflow handled as context overflow path
- Retry-After parsing for ms/date formats with cap
- REPL crash hardening against malformed input/panics
- Guard bypass batch 1+2 fixes
- Config strictness: reject unknown fields
- Popup restore on resize/refocus
- Unknown-tool fail-closed handling
- Clipboard async copy path
- History recall (Up/Down) while a turn is running, matching idle prompt behavior
- `Thought for` line and live status counter are per-turn (count from zero, final at turn end); session context stays in the footer gauge
- Log caps to bound disk/memory growth
- Transcript bound for long sessions
- Executor watchdog for hung tool runs
- Models-cache atomicity via atomic writes
- Markdown rendering fixes
- Cron: stale fire claims (>300s) from a crashed ticker are reclaimed instead of wedging the job forever
- Gateway: terminal adapter failures (revoked token, adapter not compiled) are parked until restart instead of re-entering the reconnect ladder forever
- Release: tarballs build with `--features all-platforms` (real adapters, not stubs); CI tests the all-platforms gateway code and asserts release artifact architecture
- Update: `GRAY_AUTO_UPDATE=1` auto-updates on the stable channel only; update trust model documented in SECURITY.md
- README: 0.x stability line, user-side rollback note, and raw-session persistence disclosure
- Manifests: gray-cron declared once in workspace.dependencies (GRY-003)
- README: default vs feature-gated source-build table; no blanket "ships in the binary" claim (GRY-004)
- Deps: gray-acp is an optional default-off `acp` feature (GRY-001); release builds use `--features all-platforms,acp`
- Deps: workspace Tokio declares explicit features instead of `full` (GRY-002)
- README demo GIF: dropped the 3.5s dead lead-in, stable 10fps, diff palette (2.3MB → 1.4MB, 12.8s → 9.5s)
- Stable update channel: `latest-stable.txt` now published; beta builds embed the beta channel (D1)
- Single-writer publish job: all four platform tarballs land atomically (D4, R2)
- Installer defaults to `~/.local/bin` (`--system` / `GRAY_INSTALL_DIR` for system-wide) (L3)
- Swapped unmaintained `serde_yaml` for `serde_yaml_ng`; `cargo audit` in CI (C2)
- Gateway `--help` names all three platforms (Telegram/Discord/Slack)
- TUI: bottom status line spans the full width; diff overlay extends fully to the right edge

### Known issues

- macOS binaries are not notarized (curl-install unaffected) (D3)
- Destructive-command guard is best-effort, not a sandbox — see README Safety (S4)
