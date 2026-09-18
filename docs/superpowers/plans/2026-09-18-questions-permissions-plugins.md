# Questions + permissions sidecar plugins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the day-one release path for the two standalone Rust sidecar plugins so `gray plugin install <release-tarball-url>` and later `gray plugins install questions|permissions` both work.

**Architecture:** Thin Rust sidecars own schema/policy; the gray host owns user I/O via `host/ask`. The remaining work is release automation in both repos, one new `docs/plugins.md` in gray resolving the existing `ask.rs:145` reference, and loopback installation round-trips. No gray behavior changes.

**Tech Stack:** Rust edition 2024 (serde/serde_json, tempfile dev-only in the plugins), gray NDJSON wire v1 over stdio (`plugin/manifest`, `tool/call`, `tool/before`, `command/run`, `prompt/context`, `event/notify`, `plugin/shutdown`, sidecar→host `host/ask`), GitHub Actions (`dtolnay/rust-toolchain@stable`, `shasum`, `gh release create`), gray-pkg install arms (`install_index` hash-verified, `install_url` unverified `replace_archive`).

**Spec:** `docs/superpowers/specs/2026-09-18-questions-permissions-plugins-design.md`

## Global Constraints

- Both plugin repos stay Rust-only with no gray dependencies; runtime deps stay `serde` + `serde_json` only (`tempfile` is dev-only).
- Binary names stay `questions` (from `gray-questions`) and `permissions` (from `gray-permissions`); release assets are `questions-<target>` / `permissions-<target>` plus `.sha256` sidecars.
- Gray host behavior is frozen as committed on `feat/plugin-ask` (`74b95f6`): do not touch `crates/gray-plugin/src/sidecar.rs`, `crates/gray/src/ask.rs`, `crates/gray/src/host.rs`, `composer/`, `repl/`.
- Dirty gray files `crates/gray-tools/src/shell/tools/bash.rs` + `bash/jobs.rs` and untracked helpers (`.agents/`, `calculator.html`, `graydemos/`, `skills-lock.json`, `text_*.json`, `crates/gray/examples/`) belong to a sibling agent: never stage, revert, or include them.
- Public index publishing (`gray.alignment.id`) is ops-only and out of scope; verification uses `GRAY_PLUGIN_INDEX` loopback only.
- Verify narrow per task, then pre-commit: full `cargo test --workspace` + `cargo fmt --check` in gray. In each plugin repo: `cargo test --locked` + `cargo fmt --check`.

---

### Task 1: gray-questions release workflow

**Files:**
- Create: `/home/vstaln/grayplugins/gray-questions/.github/workflows/release.yml`
- Test: existing `/home/vstaln/grayplugins/gray-questions/.github/workflows/test.yml` (unchanged; CI must stay green)

**Interfaces:**
- Consumes: `Cargo.toml` `name = "gray-questions"`, `version = "0.1.0"`, `[[bin]] name = "questions" path = "src/main.rs"`; template `/home/vstaln/grayplugins/gray-background/.github/workflows/release.yml` (4-target matrix, tag==version check, `shasum -a 256`, `gh release create`).
- Produces: tag-triggered `release.yml` publishing `questions-<target>` + `questions-<target>.sha256` assets for `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.

- [ ] **Step 1: Copy the background template with the binary name swapped**

Create `/home/vstaln/grayplugins/gray-questions/.github/workflows/release.yml` with this exact content (template lines verbatim, only `background` → `questions` in the asset name, binary path, and artifact name):

```yaml
name: release
on:
  push:
    tags: ['v*']
permissions:
  contents: read
jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-musl
          - os: ubuntu-24.04-arm
            target: aarch64-unknown-linux-musl
          - os: macos-15-intel
            target: x86_64-apple-darwin
          - os: macos-15
            target: aarch64-apple-darwin
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Check tag matches package version
        shell: bash
        run: |
          version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
          test "$GITHUB_REF_NAME" = "v$version"
      - name: Install Linux musl linker
        if: runner.os == 'Linux'
        run: sudo -n apt-get update && sudo -n apt-get install -y musl-tools
      - name: Test and build
        env:
          CARGO_BUILD_JOBS: '4'
        run: |
          cargo test --locked
          cargo build --release --locked --target ${{ matrix.target }}
      - name: Stage binary and checksum
        shell: bash
        run: |
          mkdir -p dist
          asset=questions-${{ matrix.target }}
          cp target/${{ matrix.target }}/release/questions dist/$asset
          cd dist
          shasum -a 256 "$asset" > "$asset.sha256"
      - uses: actions/upload-artifact@v4
        with:
          name: questions-${{ matrix.target }}
          path: dist/*
  publish:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Publish complete release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release create "$GITHUB_REF_NAME" dist/* --repo "$GITHUB_REPOSITORY" --verify-tag --generate-notes
```

- [ ] **Step 2: Validate YAML shape without pushing**

Run: `python3 -c "import yaml,sys; d=yaml.safe_load(open('/home/vstaln/grayplugins/gray-questions/.github/workflows/release.yml')); print(sorted(d['jobs'].keys())); print(d['jobs']['build']['strategy']['matrix']['include'][0])"` (PyYAML may be missing; if so, run `ruby -ryaml -e "d=YAML.load_file('/home/vstaln/grayplugins/gray-questions/.github/workflows/release.yml'); puts d['jobs'].keys"` instead)
Expected: `['build', 'publish']` plus the first matrix row with `x86_64-unknown-linux-musl`.

- [ ] **Step 3: Run plugin tests + fmt**

Run: `cargo test --locked --manifest-path /home/vstaln/grayplugins/gray-questions/Cargo.toml` then `cargo fmt --check --manifest-path /home/vstaln/grayplugins/gray-questions/Cargo.toml`
Expected: 6 lib + 3 wire tests PASS; fmt exit 0.

- [ ] **Step 4: Commit (do NOT tag, do NOT push)**

```bash
git -C /home/vstaln/grayplugins/gray-questions add .github/workflows/release.yml
git -C /home/vstaln/grayplugins/gray-questions commit -m "ci: add tag-triggered release workflow (4-target, questions assets)"
```

Do NOT create the `v0.1.0` tag and do NOT push: tagging fires the release and pushing needs the user's go-ahead.

---

### Task 2: gray-permissions release workflow

**Files:**
- Create: `/home/vstaln/grayplugins/gray-permissions/.github/workflows/release.yml`
- Test: existing `/home/vstaln/grayplugins/gray-permissions/.github/workflows/test.yml` (unchanged; CI must stay green)

**Interfaces:**
- Consumes: `Cargo.toml` `name = "gray-permissions"`, `version = "0.1.0"`, `[[bin]] name = "permissions" path = "src/main.rs"`; same background template as Task 1.
- Produces: tag-triggered `release.yml` publishing `permissions-<target>` + `permissions-<target>.sha256` assets for the same 4 targets.

- [ ] **Step 1: Copy the background template with the binary name swapped**

Create `/home/vstaln/grayplugins/gray-permissions/.github/workflows/release.yml` with this exact content (identical to Task 1 except `questions` → `permissions` in the asset name, binary path, and artifact name):

```yaml
name: release
on:
  push:
    tags: ['v*']
permissions:
  contents: read
jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-musl
          - os: ubuntu-24.04-arm
            target: aarch64-unknown-linux-musl
          - os: macos-15-intel
            target: x86_64-apple-darwin
          - os: macos-15
            target: aarch64-apple-darwin
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Check tag matches package version
        shell: bash
        run: |
          version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
          test "$GITHUB_REF_NAME" = "v$version"
      - name: Install Linux musl linker
        if: runner.os == 'Linux'
        run: sudo -n apt-get update && sudo -n apt-get install -y musl-tools
      - name: Test and build
        env:
          CARGO_BUILD_JOBS: '4'
        run: |
          cargo test --locked
          cargo build --release --locked --target ${{ matrix.target }}
      - name: Stage binary and checksum
        shell: bash
        run: |
          mkdir -p dist
          asset=permissions-${{ matrix.target }}
          cp target/${{ matrix.target }}/release/permissions dist/$asset
          cd dist
          shasum -a 256 "$asset" > "$asset.sha256"
      - uses: actions/upload-artifact@v4
        with:
          name: permissions-${{ matrix.target }}
          path: dist/*
  publish:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Publish complete release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release create "$GITHUB_REF_NAME" dist/* --repo "$GITHUB_REPOSITORY" --verify-tag --generate-notes
```

- [ ] **Step 2: Validate YAML shape without pushing**

Run: same validator as Task 1 Step 2 against `/home/vstaln/grayplugins/gray-permissions/.github/workflows/release.yml`
Expected: `['build', 'publish']` plus the first matrix row with `x86_64-unknown-linux-musl`.

- [ ] **Step 3: Run plugin tests + fmt**

Run: `cargo test --locked --manifest-path /home/vstaln/grayplugins/gray-permissions/Cargo.toml` then `cargo fmt --check --manifest-path /home/vstaln/grayplugins/gray-permissions/Cargo.toml`
Expected: 9 lib + 4 wire tests PASS; fmt exit 0.

- [ ] **Step 4: Commit (do NOT tag, do NOT push)**

```bash
git -C /home/vstaln/grayplugins/gray-permissions add .github/workflows/release.yml
git -C /home/vstaln/grayplugins/gray-permissions commit -m "ci: add tag-triggered release workflow (4-target, permissions assets)"
```

Do NOT create the `v0.1.0` tag and do NOT push: same reason as Task 1.

---

### Task 3: gray docs/plugins.md (resolves the ask.rs:145 reference)

**Files:**
- Create: `/home/vstaln/gray/docs/plugins.md`
- Read: `/home/vstaln/gray/crates/gray/src/ask.rs:1-200` (wire shapes, TTLs, surfaces), `/home/vstaln/gray/crates/gray-pkg/src/index.rs:1-60` (index entry shape, `GRAY_PLUGIN_INDEX`), `/home/vstaln/gray/crates/gray-pkg/src/ops.rs:1071-1290` (`install_index` vs `install_url` semantics), `/home/vstaln/gray/crates/gray/src/lib.rs:335-345` (`plugins` alias), `/home/vstaln/gray/docs/native-plugin-ui.md:1-60` (doc tone + sidecar contract reference)

**Interfaces:**
- Consumes: spec section 4 (wire shapes, TTLs, `blocking:false` → empty, both install spellings, headless behavior); both plugin READMEs (tarball-then-index path, no changes needed there).
- Produces: `docs/plugins.md` that a plugin author can follow without reading the spec; zero code changes.

- [ ] **Step 1: Write docs/plugins.md**

Create `/home/vstaln/gray/docs/plugins.md` with this exact content (TTL numbers and wire names must match `ask.rs` / `sidecar.rs` / both plugin `main.rs` files verbatim):

```markdown
# Sidecar plugins: questions + permissions (`host/ask`)

Thin Rust sidecars own schema/policy; the gray host owns user I/O via
`host/ask`. Two first-party examples:

- `questions` (`vstaln/gray-questions`): the AI asking you clarifying
  questions (`request_user_input`, 1–3 questions, options + free-form notes).
- `permissions` (`vstaln/gray-permissions`): the guard on tool calls
  ("hey, can I run this command?") via `tool/before` plus
  `/permissions` (`/perms`, `/access`).

## Install

```sh
gray plugin install <release-tarball-url>   # day one (unverified, warned)
gray plugins install questions              # after index publish (verified)
gray plugins install permissions            # after index publish (verified)
```

`plugin` and `plugins` are the same command (`visible_alias`). `git:`
installs do NOT carry sidecars (that arm only extracts pi skills), so a
release tarball URL or an index name is the only day-one path.

Index entries are `gray-native`/`tarball` with `sha256:<hex>`:

```json
{"ecosystem": "gray-native", "version": "0.1.0",
 "source": {"type": "tarball", "url": "https://…/questions-<target>"},
 "hash": "sha256:<hex>", "scope": ""}
```

Local verification points `GRAY_PLUGIN_INDEX` at a loopback index serving
the same shape.

## Wire (v1)

Host→sidecar is NDJSON over stdio: `plugin/manifest`, `tool/call`,
`tool/before`, `command/run`, `prompt/context`, `event/notify` (no reply),
`plugin/shutdown` (clean exit). Sidecar→host asking is one method:

```json
{"id": "q1", "method": "host/ask",
 "params": {"questions": [{"id": "color", "header": "Color",
   "question": "Which color?",
   "options": [{"label": "Red", "description": "warm"}]}],
  "blocking": true}}
```

The host replies `{"id": "q1", "result": {"answers": …}}` or
`{"id": "q1", "error": …}`. Asking sidecars claim protocol `"1.1"` in
`plugin/manifest`; pre-1.1 sidecars keep the 30s fail-fast.

## TTLs

- Plugin inner TTL: 300s (both sidecars `recv_timeout(300s)` so the plugin
  reports its own timeout, never the host's generic one).
- Host handler TTL (`ASK_HANDLER_TTL`): 300s per `host/ask` task.
- Host outer TTL (`ASK_TTL`): 330s on `tool/call`/`tool/before` for
  protocol-1.1 sidecars; everything else keeps `HOST_TTL` 30s.

## Semantics

- `blocking: false` resolves empty immediately in v1 (no follow-up
  message injection).
- Bad tool args are rejected without asking.
- No host handler is a loud `{"error": …}`, never a hang.
- Empty/timeout answers deny: permissions fails closed, `request_user_input`
  reports no user reachable.

## Headless (cron/gateway, piped stdin, `-p`)

No TTY prompt exists there: piped stdin takes one number-or-free-text
line per question (blank/EOF skips); anything else resolves empty
immediately. Permissions denies; questions reports no user reachable.
```

- [ ] **Step 2: Confirm the ask.rs reference now resolves**

Run: `grep -rn "docs/plugins.md" /home/vstaln/gray/crates/gray/src/ && ls /home/vstaln/gray/docs/plugins.md`
Expected: `ask.rs:145` still references it and the file exists.

- [ ] **Step 3: Narrow gray check (docs-only, no behavior change)**

Run: `cargo test -p gray --lib ask` in `/home/vstaln/gray`
Expected: PASS (ask unit tests unaffected by a docs addition).

- [ ] **Step 4: Commit docs only**

```bash
git -C /home/vstaln/gray add docs/plugins.md
git -C /home/vstaln/gray commit -m "docs: sidecar plugins + host/ask contract (questions, permissions)"
```

Stage ONLY `docs/plugins.md` — never the sibling agent's dirty/untracked files.

---

### Task 4: Loopback install round-trip + check for both plugins

**Files:**
- Modify: none (verification only; no repo files change).
- Read: `/home/vstaln/gray/crates/gray-pkg/src/ops.rs:1170-1290` (`replace_archive` + `install_url` semantics), `/home/vstaln/gray/crates/gray-pkg/src/ops_tests.rs:210-250` (`tiny_tgz` + `spawn_tarball` fixture shape), `/home/vstaln/gray/crates/gray-pkg/tests/index.rs:1-80` (loopback index fixture shape)

**Interfaces:**
- Consumes: release-mode `questions` + `permissions` binaries built from the Task 1/2 repos; loopback HTTP tarball + index servers (127.0.0.1 only, per `fetch.rs check_url`); `gray plugin check` conformance (`tool/before` allow/deny-without-host pass, hang fails; `command/run` resolves).
- Produces: PASS lines for both plugins via both install arms, with a fresh `GRAY_HOME` so the real home is untouched.

- [ ] **Step 1: Build both release binaries**

Run: `cargo build --release --locked --manifest-path /home/vstaln/grayplugins/gray-questions/Cargo.toml` and `cargo build --release --locked --manifest-path /home/vstaln/grayplugins/gray-permissions/Cargo.toml`
Expected: `~/grayplugins/gray-questions/target/release/questions` and `~/grayplugins/gray-permissions/target/release/permissions` both exist and are executable.

- [ ] **Step 2: Build the gray debug binary under test**

Run: `cargo build -p gray` in `/home/vstaln/gray`
Expected: `target/debug/gray` exists. (Uses the `feat/plugin-ask` worktree state including the Task 3 docs; behavior unchanged.)

- [ ] **Step 3: URL-arm round-trip (unverified tarball path)**

Run (fresh temp `GRAY_HOME` per plugin; serve the release binary as a `.tar.gz` whose top-level unpack yields the executable — mirror the `tiny_tgz` + `replace_archive` layout so `resolve_argv` finds exactly one executable):

```bash
export GRAY_HOME=$(mktemp -d)
# questions: pack target/release/questions as questions.tar.gz, serve on 127.0.0.1, then:
/home/vstaln/gray/target/debug/gray plugin install http://127.0.0.1:<port>/questions.tar.gz
/home/vstaln/gray/target/debug/gray plugin check $GRAY_HOME/plugins/questions
# permissions: same with permissions.tar.gz, then:
/home/vstaln/gray/target/debug/gray plugin install http://127.0.0.1:<port>/permissions.tar.gz
/home/vstaln/gray/target/debug/gray plugin check $GRAY_HOME/plugins/permissions
```

Expected: both installs print the unverified-URL warning and succeed; both `plugin check` runs print PASS for `manifest`, `tool/call` (questions) / `tool/before` (permissions), `command/run` (permissions), `notify`, `shutdown` with zero FAIL lines. The permissions `tool/before` PASS here is the fail-closed proof (spec §5): with no host handler in check mode the Ask-shaped verdict denies fast instead of hanging to the ask TTL. Corrupt-mode fallback needs no new step either (spec §5): `load_mode` falls back to the `GRAY_PERMISSION` seed, then `auto` — exercised by setting `GRAY_PERMISSION=bogus` and confirming `command/run` on `/permissions` still prints a valid mode. Clean up the temp `GRAY_HOME` dirs afterwards.

- [ ] **Step 4: Index-arm round-trip (hash-verified path)**

Run (fresh temp `GRAY_HOME`; loopback serves `/index.json` shaped like `crates/gray-pkg/tests/index.rs minimal_index` with entries `questions` + `permissions` as `{"ecosystem": "gray-native", "version": "0.1.0", "source": {"type": "tarball", "url": "http://127.0.0.1:<port>/questions.tar.gz"}, "hash": "sha256:<hex of served tarball>"}` and the same for permissions, with `GRAY_PLUGIN_INDEX=http://127.0.0.1:<port>/index.json`):

```bash
export GRAY_HOME=$(mktemp -d) GRAY_PLUGIN_INDEX=http://127.0.0.1:<port>/index.json
/home/vstaln/gray/target/debug/gray plugins install questions
/home/vstaln/gray/target/debug/gray plugins install permissions
/home/vstaln/gray/target/debug/gray plugin check $GRAY_HOME/plugins/questions
/home/vstaln/gray/target/debug/gray plugin check $GRAY_HOME/plugins/permissions
```

Expected: both installs succeed WITHOUT the unverified warning (hash-verified); both checks PASS with zero FAIL lines. This also proves the `plugins` alias spelling. Clean up afterwards.

- [ ] **Step 5: Record the outcome**

No commit (verification only). Append the PASS/FAIL lines to the execution report; any FAIL stops the run and becomes the report's headline.

---

### Task 5: Full workspace verification + alias proof

**Files:**
- Modify: none (verification + report only).

**Interfaces:**
- Consumes: all prior tasks green.
- Produces: pre-commit gate result plus the `gray plugins --help` alias proof.

- [ ] **Step 1: Alias proof**

Run: `/home/vstaln/gray/target/debug/gray plugins --help`
Expected: shows the plugin subcommands (`install`, `check`, …) — same as `gray plugin --help`.

- [ ] **Step 2: Plugin repos final gate**

Run: `cargo test --locked` + `cargo fmt --check` in both `/home/vstaln/grayplugins/gray-questions` and `/home/vstaln/grayplugins/gray-permissions`
Expected: questions 6+3 PASS, permissions 9+4 PASS, both fmt clean.

- [ ] **Step 3: Gray pre-commit gate**

Run in `/home/vstaln/gray`: `cargo test --workspace` then `cargo fmt --check`
Expected: full workspace green + fmt clean. Known flake: parallel full-workspace runs have intermittently reported a pre-existing gateway filesystem permission failure (see `docs/native-plugin-ui.md` Verification); on exactly that failure, re-run serially with `-- --test-threads=1` before calling it red.

- [ ] **Step 4: Report, do NOT push, do NOT tag, do NOT open a PR**

Write the final report (per-task PASS/FAIL, alias proof, workspace result). Then STOP and ask the user about: pushing the two `release.yml` commits, tagging `v0.1.0` in both repos (fires the releases), and opening a gray PR for `feat/plugin-ask` + `docs/plugins.md`. Never push `main`, never push gray, never tag, never open a PR unasked. PTY modal smoke stays manual (noted, not run).
