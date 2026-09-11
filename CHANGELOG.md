# Changelog

## [Unreleased]

### Added
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

### Changed
- Removed the native messaging gateway: deleted `crates/gray-gateway` (adapters, daemon, pairing, delivery, systemd), the `plugins/gateway` sidecar, `gray gateway ...`/`gray send`, and the `telegram`/`discord`/`slack`/`all-platforms` features. Chat returns as a plugin; `gray cron --deliver` targets are stored opaquely until a delivery backend exists. Dropped the `--all-features` CI checks.

### Fixed
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

## [0.1.0] - 2026-09-07

### Added
- Verified installs: SHA256SUMS published per release, checked by install.sh (S1)
- `GRAY_NO_UPDATE_CHECK=1` and 24h update-check cache (L4)
- Gateway autostart defaults off; corrupt gateway.yaml warns instead of silently resetting (S2, S3)
- Safety / Subcommands / Platform / gateway docs in README (D2, S4)

### Fixed
- Stable update channel: `latest-stable.txt` now published; beta builds embed the beta channel (D1)
- Single-writer publish job: all four platform tarballs land atomically (D4, R2)
- Installer defaults to `~/.local/bin` (`--system` / `GRAY_INSTALL_DIR` for system-wide) (L3)
- Swapped unmaintained `serde_yaml` for `serde_yaml_ng`; `cargo audit` in CI (C2)
- Gateway `--help` names all three platforms (Telegram/Discord/Slack)
- TUI: bottom status line spans the full width; diff overlay extends fully to the right edge

### Known issues
- macOS binaries are not notarized (curl-install unaffected) (D3)
- Destructive-command guard is best-effort, not a sandbox — see README Safety (S4)
