# Questions + permissions as standalone Rust sidecar plugins (approved design)

Status: approved 2026-09-18 (user: "looks right", all six sections).
Branch: `feat/plugin-ask` (host work committed as `74b95f6`).
Spec commit: this file only; implementation follows after spec review.

## Background

`ccbd014` (`feat(minimal): default to one bash tool; remove guard, approvals,
questions`, parent `d81c1a7`) deleted both features from gray core:

- Questions: `crates/gray-core/src/questions.rs` +
  `crates/gray-tools/src/request_user_input.rs` + TUI overlay in
  `crates/gray/src/composer/question/{mod,panel,session}.rs`, plus
  `ToolContext.questions` wiring, stdin asker, transcript Q&A replay.
- Permissions: `crates/gray-core/src/approvals.rs` (ApprovalGate, modes,
  cache, verdicts) + `crates/gray/src/setup/permissions_modal.rs` +
  `/permissions` `/perms` `/access` commands and Shift+Tab/badge wiring.

The guard (`crates/gray-tools/src/shell/guard.rs`) stays deleted: out of scope.

## Decision (locked)

Two separate Rust repos outside `/home/vstaln/gray`, under `~/grayplugins/`:

- `~/grayplugins/gray-questions` → `vstaln/gray-questions` (public),
  commit `e865878`.
- `~/grayplugins/gray-permissions` → `vstaln/gray-permissions` (public),
  commit `ab8cfeb`.

Both pushed with `origin` set. Design A: thin Rust sidecars + generic
`host/ask`. The plugin owns schema/policy; the host owns user I/O.

## 1. Repos and releases

Each repo: `Cargo.toml` (binary + lib, `serde`/`serde_json` only,
`tempfile` dev-only), `src/lib.rs`, `src/main.rs`, `tests/wire.rs`,
`README.md`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, test-only CI (`test.yml`:
`cargo fmt --check` + `cargo test --locked`).

- `gray-questions`: `request_user_input` tool (1–3 questions, codex schema
  verbatim, non-empty options, max 3, `is_other` forced); `tool/call` sends
  `host/ask` and waits up to 300s; `prompt/context` injects the 2-line usage
  guidelines. Manifest: `{name: "questions", tools: [request_user_input],
  hooks: ["prompt/context"]}`, protocol `1.1`.
- `gray-permissions`: no tools; claims `tool/before` +
  `/permissions|/perms|/access`. Static verdicts (Allow reads + workspace
  writes, Deny read-only-mode mutations, Ask everything else — unknown tools
  fail closed). Ask goes through `host/ask` (Yes / Yes-for-session / No,
  exact-label match only; empty/timeout denies). Session memory in-process
  (raw command bytes / resolved paths). Mode persisted to
  `$GRAY_HOME/permissions-mode.json`, seeded from `GRAY_PERMISSION`.

Blocking std I/O with a reader thread (same shape as
`crates/gray-plugin/testdata/echo_plugin.rs`); no async runtime, no gray
dependencies — each binary is self-contained.

Still to do: add `release.yml` to both repos copied from the
`gray-background` template (4-target matrix, tag-must-equal-version check,
`shasum -a 256`, `gh release create`; assets `questions-<target>` /
`permissions-<target>` + `.sha256`), then tag `v0.1.0` on both.

## 2. Gray host accommodation (minimal, on `feat/plugin-ask`)

Keep only what asking needs; nothing else:

- `crates/gray-plugin/src/sidecar.rs`: `HOST_ASK` (`host/ask`), `HOST_TTL`
  (30s default), `ASK_TTL` (330s outer), `ASK_HANDLER_TTL` (300s handler).
  Per-sidecar `asks` flag, protocol-gated (manifest `protocol == "1.1"`,
  not hooks-gated): asking sidecars get the extended TTL on
  `tool/call`/`tool/before` and the handler task; pre-v1 sidecars keep the
  30s fail-fast. The plugin enforces the shorter inner TTL (300s) so it
  reports its own timeout instead of surfacing the host's generic one.
- `crates/gray/src/ask.rs` (new): process `AskService` — `install`,
  `shutdown`, `handle_ask` (300s race, cancel token so a replaced agent
  can't strand a sidecar past 330s). Surfaces: interactive TTY inline TUI
  modal (digits pick, Tab notes, Enter submit, Esc resolves empty);
  piped stdin number-or-free-text per question (deleted `StdinQuestionAsker`
  semantics); headless resolves empty immediately.
- `crates/gray/src/host.rs`: route `HOST_ASK` to `ask::handle_ask`.
- `crates/gray/src/composer/mod.rs` + `draw/mod.rs`: `AskModal` inline slot
  above the input box (measured rows, no overlay restore, no alternate
  screen); transcript `?`/`→` replay on resolve; pill and footer gauge share
  `live_context_total` so both tick per chunk.
- `crates/gray/src/repl/key_watcher.rs`: turn key-watcher yields to a live
  modal (only resize + Ctrl-C pass through; Ctrl-C resolves empty).
- `crates/gray/src/repl/mod.rs` + `print.rs`: `ask::install` at boot
  (TUI handle once the composer exists; `None` for piped/headless),
  `ask::shutdown` on exit paths.
- `crates/gray/src/lib.rs`: `pub mod ask`; `visible_alias = "plugins"` so
  `gray plugins install questions` works alongside `gray plugin install`.
- `crates/gray/src/plugin_check.rs`: conformance covers `tool/before`
  (allow/deny-without-host both pass; hang fails) and `command/run`.

Explicitly out: guard restore, Shift+Tab cycling, permission badge,
`blocking:false` follow-up injection (v1 resolves empty).

## 3. Install and index

- Day one (works today): `gray plugin install <release-tarball-url>` —
  `install_url` accepts any `https` tarball via `replace_archive`
  (unverified, warned). `git:` installs do NOT work for sidecars (that arm
  only extracts pi skills).
- After publish: `gray plugins install questions` and
  `gray plugins install permissions` — index names `questions` +
  `permissions`, hash-verified `gray-native`/`tarball` entries
  (`{ecosystem, version, source{type,url}, hash: "sha256:<hex>", scope}`).
- The index server (`https://gray.alignment.id/plugins/index.json`,
  override `GRAY_PLUGIN_INDEX`) is not owned from this repo: publishing the
  two entries is an explicit ops step, not an implementation step here.
- Local verification uses `GRAY_PLUGIN_INDEX` pointed at a loopback index
  plus a local-tarball install round-trip.

## 4. Docs

- New `docs/plugins.md` in gray (resolves the existing reference at
  `crates/gray/src/ask.rs:145`): wire shapes, `host/ask` TTLs,
  `blocking:false` → empty in v1, both install spellings, cron/gateway
  headless behavior (empty answers → permissions fail closed, questions
  report no user reachable).
- Both plugin READMEs already document the tarball-then-index install path;
  no changes needed there.

## 5. Error handling

- No host handler → loud `{"error": …}`, never a hang.
- Bad tool args rejected without asking.
- Empty/timeout answers deny (permissions fail closed).
- Unknown `tool/before` tools fail closed to Ask.
- Graceful shutdown via `plugin/shutdown`; in-flight asks resolve empty.
- Corrupt mode file falls back to env seed, then `auto`.

## 6. Verification

- Both repos: `cargo test --locked` + `cargo fmt --check` (already green:
  questions 6+3, permissions 9+4).
- Loopback tarball install round-trip for both + `gray plugin check` PASS.
- `gray plugins --help` shows the alias.
- Gray pre-commit: full `cargo test --workspace` + `cargo fmt --check`.
- PTY modal smoke noted as manual.
- Cron/gateway ask coverage: headless empty-resolution only.
- After green: ask about a PR (never push to `main` unasked; no second PR
  while one is open per AGENTS.md).

## Follow-ups (explicitly out of scope)

Publishing the index entries (needs index-server access), `blocking:false`
follow-up injection, permission badge / Shift+Tab, guard restore, release
automation beyond the background-template workflow.
