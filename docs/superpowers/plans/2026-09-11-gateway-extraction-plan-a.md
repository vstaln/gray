# Plan A (rev 2): Delete the Native Gateway

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `gray` ships zero native gateway. Delete `crates/gray-gateway` outright (adapters, daemon, pairing, delivery, systemd, status), the `plugins/gateway` sidecar, `gray gateway ...` / `gray send`, and the `telegram` / `discord` / `slack` / `all-platforms` features. Chat returns only as a plugin (workstream C).

**Architecture:** The gateway is a leaf: only `gray` depends on `gray-gateway`, and only in four `main.rs` sites plus CLI types. `gray-cron` is already gateway-free (pure store + math) and stays, with its `--deliver` target kept as an opaque stored string for workstream B's delivery seam. `gray-supervise` loses only its gateway-service-specific `units` module; its generic primitives (`exit/health/heartbeat/lifecycle/watchdog`) stay for B to use or cut. `gray::setup::catalog::gray_home()` already duplicates the home resolution, so no new shared helper is needed.

**Tech Stack:** Rust workspace (edition 2024, resolver 3), GitHub Actions, sh installer.

**Spec:** User decisions 2026-09-11: option 1 → corrected to full deletion ("no native gateway support; that should be a plugin"), decomposition A→B→C, `clipboard` stays. **Supersedes** rev 1 of this file (standalone `gray-gateway` binary — rejected: still native support) and `docs/superpowers/plans/2026-09-11-remove-all-platforms-feature.md` (folded into Task 4). B (core ticker) and C (plugin runtime v2) are out of scope.

## Global Constraints

- No `gray-gateway` anywhere when done: not a member, not a dep, not a feature, no `gray_gateway::` / `gray-gateway` strings outside history. `serde_yaml_ng`, teloxide, twilight-*, slack-morphism leave the lockfile.
- `gray` keeps REPL / `-p` / plugin / cron (store-only) / sessions / update. `gray cron --deliver` keeps accepting (and storing) targets; nothing interprets them yet.
- Default/offline builds stay green: `cargo check -p gray`, `cargo test --workspace`.
- Release ships one binary (`gray`); `install.sh` / `install.ps1` need no change (they only ever installed `gray`).
- No new dependencies. `pub` items in `gray-supervise` keep `clippy -D warnings` green despite losing their only caller.
- Known gap (accepted): the gateway daemon fires cron jobs today, so between A and B `gray cron` manages jobs that never run. B closes it.

---

### Task 1: Delete the crate and unwire the workspace

**Files:**
- Delete: `crates/gray-gateway/` (all 18 src files + Cargo.toml)
- Modify: `Cargo.toml` (members, workspace deps, comment)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `gray-gateway` is not a workspace member or dep.

- [ ] **Step 1: Remove the crate**

Run: `git rm -r crates/gray-gateway`
Expected: stages deletion of `Cargo.toml` + all 18 files under `src/` (`authz, config, daemon, daemon_agent, daemon_boot, daemon_stream, daemon_supervise, delivery, discord, lib, lock, pairing, platform, progress, session, slack, status, systemd, telegram`).

- [ ] **Step 2: Unwire the workspace root**

In `Cargo.toml`, delete the member line:
```toml
    "crates/gray-gateway",
```
Delete the dep line:
```toml
gray-gateway = { path = "crates/gray-gateway" }
```
Replace the stale comment:
```toml
# Phase 1 scope cut: the default build is the harness core only.
# gray-gateway stays a member (built via
# --workspace or -p) but is out of the default tree.
```
with:
```toml
# Minimal core: no gateway member. Chat returns as a plugin, not a crate.
```

- [ ] **Step 3: Verify the workspace resolves without it**

Run: `cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print([p for p in json.load(sys.stdin)['packages'] if 'gateway' in p['name']])"`
Expected: `[]`.

Run: `cargo check --workspace 2>&1 | tail -5`
Expected: fails naming `gray`'s dangling `gray-gateway` dep (fixed in Task 2) — and nothing else. Do not fix forward; continue to Task 2.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml
git commit -m "cut: delete gray-gateway crate, unwire workspace"
```
(`git rm` already staged the deletion.)

---

### Task 2: Strip `gray` of all gateway code

**Files:**
- Modify: `crates/gray/src/lib.rs` (CLI types, system-prompt line, deliver help, moved-away tests)
- Modify: `crates/gray/src/main.rs` (handlers, arms, `cron_store`, comments)
- Modify: `crates/gray/Cargo.toml` (dep + platform features)
- Modify: `crates/gray/src/profile.rs`, `crates/gray/src/repl/dispatch.rs`, `crates/gray/src/repl/mod.rs`, `crates/gray/src/repl/commands.rs` (stale references)

**Interfaces:**
- Consumes: Task 1 (the crate is gone; these references no longer resolve).
- Produces: zero `gateway` strings in `crates/gray/src`; `cargo tree -p gray` is gateway-free.

- [ ] **Step 1: Delete the CLI types from `lib.rs`**

Delete the `Gateway` variant (`lib.rs:267-271`), the `Send` variant (lines 285-291), the `GatewayCmd` enum (lines 346-371), and the `PairingCmd` enum with its doc comment (starts line 417 `/// \`gray gateway pairing ...\`` through the enum's closing brace — read 417-450 first to confirm the boundary).
Delete the `send` parse test (lines 521-528, the `["gray", "send", ...]` block — the subcommand no longer exists; there is no binary to move it to).

- [ ] **Step 2: Reword the agent-facing prompt + deliver help**

Line 53, replace:
```
To schedule recurring work for the user, run `gray cron add "<schedule>" "<prompt>" --deliver <target>` (manage with `gray cron list/show/remove`); the gateway daemon fires jobs and delivers results to chat. Have the job reply `[SILENT]` when there is nothing worth reporting.
```
with:
```
To schedule recurring work for the user, run `gray cron add "<schedule>" "<prompt>"` (manage with `gray cron list/show/remove`).
```
Line 324, replace:
```rust
        /// Delivery target (default `local` = save-only): origin | local | <platform>[:chat[:thread]]
```
with:
```rust
        /// Delivery target, stored with the job (no delivery backend yet): origin | local | <target>
```

- [ ] **Step 3: Delete the handlers from `main.rs`**

Delete the match arms (lines 51-53 `Gateway`, 60-62 `Send`), `run_gateway` (132-167), `run_pairing` (169-180), and `run_send` + `print_invite` (from `async fn run_send` through `print_invite`'s closing brace, just before `/// Log panics`).

- [ ] **Step 4: Repoint `cron_store`, reword its comment**

Change:
```rust
    let home = gray_gateway::config::gray_home_dir()?;
```
to:
```rust
    let home = gray::setup::gray_home()?;
```
Reword the comment above it (line 271 + following `send_once` lines) to:
```rust
/// `gray cron ...` (cron plan Task 5).
///
/// File-only surface: the store lives at `$GRAY_HOME/cron/jobs.json`.
/// Delivery targets ride the record opaquely; no backend interprets them yet.
```
Reword `parse_deliver_flag`'s doc (lines 328-330, `resolves at fire time ... fail safe to save-only in the daemon`) to:
```rust
/// `--deliver` flag: `origin`/`local` keywords (case-insensitive), anything
/// else rides `Deliver::Target`, stored opaquely until a delivery backend exists.
```

- [ ] **Step 5: Drop the dependency and platform features**

In `crates/gray/Cargo.toml`, replace:
```toml
# Phase 1 scope cut: platform adapters are opt-in (default build is the
# harness core with gateway config/status only, no twilight/teloxide/slack).
# The workspace edge disables gateway default features
# out of the default tree; opt back in below.
gray-gateway = { workspace = true }
```
with:
```toml
# No gateway edge: gray ships no messaging adapters.
```
Delete:
```toml
# Platform passthroughs (opt-in; default build carries no platform deps).
telegram = ["gray-gateway/telegram"]
discord = ["gray-gateway/discord"]
slack = ["gray-gateway/slack"]
all-platforms = ["gray-gateway/all-platforms"]
```
(`clipboard` stays.)

- [ ] **Step 6: Fix stale comments and the user-facing REPL string**

`profile.rs:4-5`:
```rust
//! [`gray_plugin::builder`] (lowest common crate — the `gray → gray-gateway`
//! edge forbids a `gray`-owned shared builder). This module keeps gray's
```
to:
```rust
//! [`gray_plugin::builder`] (lowest common crate — keeps the shared builder
//! out of any single binary). This module keeps gray's
```
`dispatch.rs:314` `// The gateway left the TUI (kept as the \`gray gateway\` CLI):` → `// The gateway left the TUI (native gateway deleted; chat returns as a plugin):`
`dispatch.rs:320` `"the TUI gateway is gone — run \`gray gateway …\` outside gray",` → `"the TUI gateway is gone — native chat support was removed",`
`mod.rs:515-516`:
```rust
    // The messaging gateway left gray core (the gray-gateway crate is
    // preserved and still runs via the `gray gateway` CLI): no autostart,
```
to:
```rust
    // The messaging gateway was deleted from gray core (chat returns as a
    // plugin): no autostart,
```
`commands.rs:765` (read ±5 lines first): replace the `` `gray gateway` `` reference with `the deleted native gateway`.

- [ ] **Step 7: Verify zero references and a clean tree**

Run: `rg -n -i 'gateway' crates/gray/src crates/gray/tests`
Expected: no output.

Run: `cargo check -p gray && cargo tree -p gray -e normal | rg -c 'gateway|teloxide|twilight|slack-morphism|serde_yaml_ng' || echo "clean"`
Expected: `clean`.

Run: `cargo test -p gray --lib`
Expected: PASS (cron parse tests stay — `--deliver telegram:123` is still accepted as an opaque string).

- [ ] **Step 8: Commit**

```bash
git add crates/gray/ Cargo.lock
git commit -m "cut: remove native gateway from gray (CLI, dep, features)"
```

---

### Task 3: Cut gateway remnants in `gray-cron`, `gray-plugin`, `gray-supervise`

**Files:**
- Modify: `crates/gray-cron/src/store.rs` (delete lifecycle guard, reword lock comments)
- Modify: `crates/gray-plugin/src/builder.rs`, `crates/gray-plugin/src/host.rs` (comments)
- Delete: `crates/gray-supervise/src/units.rs`; modify `crates/gray-supervise/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 (the referenced crate/paths are gone).
- Produces: no `gateway` strings in these three crates; `units` module gone.

- [ ] **Step 1: Delete the gateway-lifecycle guard from `gray-cron`**

Delete `reject_lifecycle_shape` (store.rs:193-196, the `gateway restart`/`gateway stop` substring block — read 185-200 first for exact boundaries), its call at line 316 (`reject_lifecycle_shape(prompt)?;`), its test (~line 640, the `run gateway restart nightly` / `systemctl restart gray-gateway` case — read first), and the module-doc mention (line 11, `` detection table (`reject_lifecycle_shape`) ``).
Reword the three lock comments that cite the deleted crate (lines 5, 116-118, 154: `` `gray-gateway/src/lock.rs` `` / `` `gray_gateway::delivery::atomic_write_json` ``) to describe the mechanism without the path, e.g. `same flock + 300s-claim shape the daemon used`.

- [ ] **Step 2: Reword plugin-crate comments**

`builder.rs:1` `//! One profile-aware agent builder for every surface (REPL, \`-p\`, gateway, cron).` → `//! One profile-aware agent builder for every surface (REPL, \`-p\`, cron).`
`builder.rs:3-6` (the `gray → gray-gateway edge forbade...` note) → `//! Lives here (not in \`gray\`) so every host shares one builder without depending on the binary.`
`builder.rs` (~line 12, `gateway delivery runs through \`run_agent\``) → drop the gateway clause.
`host.rs:3` `//! Both hosts (REPL/\`-p\` in \`gray\`, daemon in \`gray-gateway\`) serve sidecar` → `//! The host (REPL/\`-p\` in \`gray\`) serves sidecar`.

- [ ] **Step 3: Delete the `units` module from `gray-supervise`**

Run: `git rm crates/gray-supervise/src/units.rs`
In `crates/gray-supervise/src/lib.rs`, delete the `pub mod units;` line. (`exit`, `health`, `heartbeat`, `lifecycle`, `watchdog` stay — currently callerless, `pub` so no warnings; workstream B decides their fate.)

- [ ] **Step 4: Verify**

Run: `rg -n -i 'gateway' crates/gray-cron/src crates/gray-plugin/src crates/gray-supervise/src`
Expected: no output (adjust any code comments the sweep catches — comments only, no behavior change).

Run: `cargo check -p gray-cron -p gray-plugin -p gray-supervise && cargo test -p gray-cron -p gray-supervise --quiet`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gray-cron crates/gray-plugin crates/gray-supervise Cargo.lock
git commit -m "cut: gateway lifecycle guard, comments, systemd units module"
```

---

### Task 4: CI, release, sidecar, index

**Files:**
- Modify: `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- Delete: `plugins/gateway/`
- Modify: `plugins/official.json`

**Interfaces:**
- Consumes: Tasks 1-3 (nothing gateway remains to build or delegate to).
- Produces: CI/release never mention gateway; index has no gateway entry.

- [ ] **Step 1: CI — drop the gateway test line and both `--all-features` lines**

Delete line 29 (`- run: cargo test -p gray-gateway --features all-platforms --quiet`), line 30 (`- run: cargo check -p gray --all-features`), and line 63 (same `--all-features` command in `windows-check`; the job keeps `cargo check -p gray`).

- [ ] **Step 2: Release — build only `gray`, drop the adapter assertion**

Build step — replace:
```bash
          # all-platforms: the default build stubs the Telegram/Discord/Slack
          # adapters; the released binary must carry them or the advertised
          # gateway crash-loops on "not compiled" (audit B1).
          if [ "${{ matrix.plat }}" = "aarch64-linux" ]; then
            cross build --release -p gray --features all-platforms --target ${{ matrix.target }}
          else
            cargo build --release -p gray --features all-platforms --target ${{ matrix.target }}
          fi
```
with:
```bash
          # Single binary. No gateway features exist anymore.
          if [ "${{ matrix.plat }}" = "aarch64-linux" ]; then
            cross build --release -p gray --target ${{ matrix.target }}
          else
            cargo build --release -p gray --target ${{ matrix.target }}
          fi
```
Delete the whole `release build must carry gateway adapters` step (the `cargo tree ... | grep -Eq 'teloxide|...'` block). The `package` step (`tar ... gray`) and smoke test are unchanged.

- [ ] **Step 3: Delete the sidecar, empty the index**

Run: `git rm -r plugins/gateway`
Replace `plugins/official.json` with:
```json
{
  "schema": 1,
  "plugins": {}
}
```

- [ ] **Step 4: Verify**

Run: `rg -n 'all-platforms|all-features|gray-gateway|gray_gateway' .github/ plugins/`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add .github/ plugins/
git commit -m "cut: gateway CI/release steps, sidecar, index entry"
```

---

### Task 5: Docs (README, plugins guide, security, protocol appendix, changelog)

**Files:**
- Modify: `README.md`, `docs/plugins.md`, `SECURITY.md`, `docs/protocol-v1.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: Tasks 1-4 (names, commands, and build lines are final).
- Produces: docs describe one binary with no native gateway; no stale promises.

- [ ] **Step 1: README — product line, feature table, install block**

Line 31, replace the tail `the release binary ships the gateway adapters; from-source builds are feature-gated (see [Install](#install)).` with `no native messaging gateway (chat returns as a plugin).`
Delete the `| **Lives where you do** | ... |` row (line 40) — B re-adds an honest proactivity row when the ticker lands.
Replace the source-build block (lines 52-56):
```bash
cargo build --release -p gray                             # harness core
cargo build --release -p gray --features all-platforms   # what release binaries ship
cargo build --release -p gray --features clipboard        # + image paste in the TUI
```
with:
```bash
cargo build --release -p gray                          # harness core
cargo build --release -p gray --features clipboard     # + image paste in the TUI
```
Replace the table (lines 58-64):
```
| build | adds |
|---|---|
| default | harness core: CLI, TUI, provider, sessions, tools, cron |
| `--features all-platforms` | Telegram + Discord + Slack gateway adapters |
| `--features clipboard` | image/paste attachments (arboard + image) |
```
with:
```
| build | adds |
|---|---|
| default | harness core: CLI, TUI, provider, sessions, tools, cron |
| `--features clipboard` | image/paste attachments (arboard + image) |
```
and delete the `Release binaries ship \`all-platforms\`.` line.

- [ ] **Step 2: README — CLI surface, Gateway section, platform row**

Replace lines 106-117:
```
`gray` itself plus four subcommands — everything else is a slash command away:

| subcommand | what it does |
|---|---|
| `gray resume [--last\|--all] [SESSION_ID]` | resume a conversation — picker, most-recent, or by id/prefix |
| `gray gateway run\|status\|install\|uninstall\|invite\|pairing` | messaging gateway daemon (systemd user service, Linux-only) |
| `gray plugin <list\|search\|install\|remove\|update\|enable\|disable\|check>` | manage plugins |
| `gray update` | update gray to the latest release |
```
with:
```
`gray` itself plus five subcommands — everything else is a slash command away:

| subcommand | what it does |
|---|---|
| `gray resume [--last\|--all] [SESSION_ID]` | resume a conversation — picker, most-recent, or by id/prefix |
| `gray plugin <list\|search\|install\|remove\|update\|enable\|disable\|check>` | manage plugins |
| `gray cron <list\|add\|remove\|show>` | recurring/one-shot jobs (file-only, no daemon needed) |
| `gray sessions prune` | session store maintenance |
| `gray update` | update gray to the latest release |
```
Replace the `## Gateway` section (lines 127-133) with:
```markdown
## Gateway

Removed: gray ships no native messaging gateway — no `gray gateway`, no `gray send`, no platform adapters. Chat (Telegram/Discord/Slack) returns as a plugin.

**Scheduling** — the agent stores recurring work with `gray cron add "<schedule>" "<prompt>"` (manage with `gray cron list/show/remove`); execution and delivery arrive with the core scheduler (in progress).
```
Line 189: replace `` `gray gateway install` (systemd user service) is Linux-only `` with `systemd user service (Linux-only)`.

- [ ] **Step 3: Plugins guide, security policy, protocol appendix**

`docs/plugins.md`: delete the gateway-sidecar bullet (lines 162-165, `Gateway sidecar ... manifest \`commands:["/gateway"]\` + \`capabilities:["exec"]\``); rewrite the official-plugins bullet (lines 158-161) to `Official plugins: none yet (the Gray Index seed, \`plugins/official.json\`, is empty).`; rewrite the cron bullet's send clause (line 173, `` one-shot sends via `gray send <platform[:chat[:thread]]> <text>` ``) to `Delivery targets are stored with the job; no delivery backend exists yet.`
`SECURITY.md`: delete the `## Gateway allowlist` section; in the threat model (line 7), replace `, and\n(over the gateway) untrusted chat users.` with `.` (threat surface becomes model output + tool/plugin results).
`docs/protocol-v1.md` appendix (lines ~175-179), replace:
```
`gray → gray-gateway`, `gray → gray-cron`, `gray-tools → gray-cron`,
`gray-gateway → gray-cron` (F1: cron moves **with** the gateway in ③).
`gray → gray-gateway` pulls twilight into every workspace build today
(13 twilight/teloxide/slack-morphism nodes under `cargo tree -p gray`;
Task 3.0 cuts this edge first).
```
with:
```
`gray → gray-cron`, `gray-tools → gray-cron`.
(The gateway crate and the `gray → gray-gateway` edge were deleted;
no teloxide/twilight/slack-morphism nodes remain in the tree.)
```
(Deeper gateway mentions in that frozen spec — consumers audit, v2 notes — stay as known-stale for workstream C's doc rework.)

- [ ] **Step 4: Changelog**

Under `## [Unreleased]`, insert a `### Changed` section after the `### Added` block:
```markdown
### Changed
- Removed the native messaging gateway: deleted `crates/gray-gateway` (adapters, daemon, pairing, delivery, systemd), the `plugins/gateway` sidecar, `gray gateway ...`/`gray send`, and the `telegram`/`discord`/`slack`/`all-platforms` features. Chat returns as a plugin; `gray cron --deliver` targets are stored opaquely until a delivery backend exists. Dropped the `--all-features` CI checks.
```
(Historical entries stay untouched.)

- [ ] **Step 5: Verify no stale references**

Run: `rg -n -i 'gateway|all-platforms|--all-features|teloxide|twilight|slack-morphism|serde_yaml_ng' README.md docs/plugins.md docs/protocol-v1.md SECURITY.md CHANGELOG.md plugins/ .github/ dist/ crates/ --glob '!crates/gray-supervise/**' | rg -v 'CHANGELOG.md|protocol-v1.md:(10[01]|11[6-9]|12[01])'`
Expected: no output except the intentional `protocol-v1.md` known-stale lines and CHANGELOG history. Triage any other hit: delete or reword, never leave a promise of chat support.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/plugins.md docs/protocol-v1.md SECURITY.md CHANGELOG.md
git commit -m "docs: no native gateway; chat returns as a plugin"
```

---

### Task 6: Whole-workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Full local gate**

Run: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace --quiet && python3 docs/schema/validate.py`
Expected: all PASS.

- [ ] **Step 2: Minimal-core proof**

Run: `cargo tree -p gray -e normal | rg -c 'gateway|teloxide|twilight|slack|serde_yaml_ng|arboard|image' || echo "clean"`
Expected: `clean` (default build carries none of gateway/clipboard).

Run: `cargo tree -p gray --features clipboard -e normal | rg -c arboard`
Expected: nonzero (clipboard still opt-in and working).

- [ ] **Step 3: Lockfile + smoke**

Run: `rg -c 'name = "(teloxide|twilight-gateway|slack-morphism|serde_yaml_ng|gray-gateway)"' Cargo.lock || echo "pruned"`
Expected: `pruned`.

Run: `cargo build --release -p gray && ./target/release/gray --version && ./target/release/gray --help | rg -c 'gateway|send' || echo "no chat surface"`
Expected: version prints; `no chat surface`.

---

## Notes / deliberate scope cuts

- **`gray cron` keeps working store-only** (`add/list/show/remove` + opaque `--deliver`). Nothing fires jobs until workstream B's core ticker — the accepted A→B gap.
- **`exit/health/heartbeat/lifecycle/watchdog` stay** in `gray-supervise` despite losing their only caller (`pub` items, no warnings). B uses or cuts them; only the gateway-specific `units` module goes now.
- **`[SILENT]` convention dies with delivery** (removed from the system prompt with the gateway-delivery promise).
- **Historical docs** (`docs/superpowers/plans|specs`, old CHANGELOG entries, dogfood reports) are records — not rewritten. `dogfood/` harness references to gateway/guard are its own cleanup.
- **Pre-existing staleness untouched**: `SECURITY.md`'s guard section (guard already removed on this branch), `docs/schema/manifest.v1.json`'s legacy `provider` string, `gray.yml`'s providers note.
