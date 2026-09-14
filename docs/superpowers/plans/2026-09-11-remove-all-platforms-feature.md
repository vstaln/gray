# Remove the `all-platforms` Umbrella Feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the `all-platforms` cargo feature (the "all-features" umbrella) from `gray` and `gray-gateway`, and drop the `--all-features` CI checks, so feature selection is explicit and the default build stays minimal.

**Architecture:** `all-platforms` is pure sugar: `gray-gateway`'s `all-platforms = ["telegram","discord","slack"]` and `gray`'s passthrough `all-platforms = ["gray-gateway/all-platforms"]`. Nothing in code does `cfg(feature = "all-platforms")` — it only appears in manifests, CI, the release workflow, and docs. Removing it is mechanical: replace every `--features all-platforms` with the explicit `--features telegram,discord,slack`, delete the two feature definitions, and delete the two `--all-features` CI lines.

**Tech Stack:** Rust workspace (edition 2024, cargo resolver 3), GitHub Actions.

**Spec:** This request (no separate spec doc). Scope confirmed with the user: (A) remove `all-platforms` from the `gray` crate **and** the `gray-gateway` crate it forwards to, and (B) drop the `--all-features` CI checks.

## Global Constraints

- The `telegram`, `discord`, `slack`, and `clipboard` features **stay** — release binaries must still carry the real adapters. Only the umbrella `all-platforms` goes.
- Default build must remain dependency-free/offline: `cargo check -p gray` with no features must still pass.
- Release binaries must still ship real adapters (not stubs); the release workflow's `cargo tree` assertion must keep passing.
- No new dependencies, no code changes: this is manifest + CI + docs only. There is no `cfg(feature = "all-platforms")` anywhere.

---

### Task 1: Remove `all-platforms` from the two manifests

**Files:**
- Modify: `crates/gray-gateway/Cargo.toml:41-42`
- Modify: `crates/gray/Cargo.toml:61-62`

**Interfaces:**
- Consumes: nothing.
- Produces: `gray-gateway` and `gray` still expose `telegram`, `discord`, `slack` (and gray: `clipboard`) features; no `all-platforms` feature exists for later tasks to reference.

- [ ] **Step 1: Delete the feature block lines in `gray-gateway`**

In `crates/gray-gateway/Cargo.toml`, remove:
```toml
# Enable all real adapters: --features all-platforms
all-platforms = ["telegram", "discord", "slack"]
```
The `[features]` section must end at `slack = ["dep:slack-morphism"]`.

- [ ] **Step 2: Delete the passthrough line in `gray`**

In `crates/gray/Cargo.toml`, remove:
```toml
all-platforms = ["gray-gateway/all-platforms"]
```
Keep `clipboard`, `telegram`, `discord`, `slack`. The trailing Phase-1 comment above the `gray-gateway` dependency stays accurate.

- [ ] **Step 3: Verify the no-feature build and the explicit feature build**

Run: `cargo check -p gray && cargo check -p gray --features telegram,discord,slack`
Expected: both PASS. The second must **not** error with "feature `all-platforms` does not exist" (we removed it) and must still pull `teloxide`/`twilight-*`/`slack-morphism`.

- [ ] **Step 4: Verify the umbrella feature is now unknown**

Run: `cargo check -p gray --features all-platforms 2>&1 | head -3`
Expected: FAIL with an unknown-feature error. This is the intended end state (do not "fix" it).

- [ ] **Step 5: Commit**

```bash
git add crates/gray-gateway/Cargo.toml crates/gray/Cargo.toml
git commit -m "build: drop all-platforms umbrella feature from gray/gray-gateway"
```

---

### Task 2: Update CI and the release workflow to name adapters explicitly

**Files:**
- Modify: `.github/workflows/ci.yml:29-30,63`
- Modify: `.github/workflows/release.yml:94,98,100,106`

**Interfaces:**
- Consumes: Task 1 (the `all-platforms` feature no longer exists; explicit lists are the only valid form).
- Produces: CI/release invoke `--features telegram,discord,slack`; no `--all-features` remains.

- [ ] **Step 1: Fix the gateway test line in `ci.yml`**

Change line 29:
```yaml
      - run: cargo test -p gray-gateway --features all-platforms --quiet
```
to:
```yaml
      - run: cargo test -p gray-gateway --features telegram,discord,slack --quiet
```

- [ ] **Step 2: Drop the two `--all-features` checks in `ci.yml`**

Delete line 30 entirely (`- run: cargo check -p gray --all-features`).
Delete line 63 entirely (same command under the `windows-check` job); the job keeps its `cargo check -p gray` line.

- [ ] **Step 3: Fix the release build invocations in `release.yml`**

In the `build` step, change both occurrences (aarch64 cross + native):
```
--features all-platforms
```
to:
```
--features telegram,discord,slack
```
In the `release build must carry gateway adapters` step, change:
```
cargo tree -p gray --features all-platforms -e normal \
```
to:
```
cargo tree -p gray --features telegram,discord,slack -e normal \
```
Update the explanatory comment at line 94: replace `# all-platforms:` with `# adapters:` and keep the "default build stubs …" sentence.

- [ ] **Step 4: Verify no stale references remain in CI/workflows**

Run: `rg -n 'all-platforms|all-features' .github/`
Expected: no output.

Run the same commands CI will run, locally:
Run: `cargo test -p gray-gateway --features telegram,discord,slack --quiet`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml .github/workflows/release.yml
git commit -m "ci: enable adapters explicitly; drop --all-features checks"
```

---

### Task 3: Update docs and the changelog

**Files:**
- Modify: `crates/gray-gateway/src/lib.rs:9`
- Modify: `README.md:40,54,61,64`
- Modify: `CHANGELOG.md` (new `### Changed` entry under `[Unreleased]`)

**Interfaces:**
- Consumes: Task 1 (feature list is now `telegram,discord,slack`).
- Produces: docs advertise the explicit feature list.

- [ ] **Step 1: Fix the crate doc comment**

In `crates/gray-gateway/src/lib.rs`, change:
```rust
//! All:    `cargo check -p gray-gateway --features all-platforms`
```
to:
```rust
//! All:    `cargo check -p gray-gateway --features telegram,discord,slack`
```

- [ ] **Step 2: Fix the README feature references**

- Line 40: `from source add \`--features all-platforms\`` → `from source add \`--features telegram,discord,slack\``
- Line 54: `cargo build --release -p gray --features all-platforms   # what release binaries ship` → `cargo build --release -p gray --features telegram,discord,slack   # what release binaries ship`
- Line 61 table row: `| \`--features all-platforms\` | Telegram + Discord + Slack gateway adapters |` → `| \`--features telegram,discord,slack\` | Telegram + Discord + Slack gateway adapters |`
- Line 64: ``Release binaries ship `all-platforms`.`` → ``Release binaries ship all three adapters (`telegram,discord,slack`).``

- [ ] **Step 3: Add the changelog entry**

Under `## [Unreleased]`, add a new section after the `### Added` block (before `### Fixed`):
```markdown
### Changed
- Build: removed the `all-platforms` umbrella feature from `gray`/`gray-gateway` and the `--all-features` CI checks; enable adapters explicitly with `--features telegram,discord,slack`
```
Do **not** rewrite the historical `[0.1.0]` / older entries that mention `all-platforms`.

- [ ] **Step 4: Verify no stale live references remain**

Run: `rg -n 'all-platforms|all-features' README.md crates/gray-gateway/src/lib.rs .github/ crates/*/Cargo.toml`
Expected: no output.
(Run without excluding `CHANGELOG.md`; historical `all-platforms` mentions there are intentionally left in place.)

- [ ] **Step 5: Commit**

```bash
git add crates/gray-gateway/src/lib.rs README.md CHANGELOG.md
git commit -m "docs: replace all-platforms with explicit adapter features"
```

---

### Task 4: Whole-workspace verification

**Files:**
- None (verification only).

- [ ] **Step 1: Default workspace still green and dependency-free**

Run: `cargo test --workspace --quiet`
Expected: PASS. Confirm the default `gray` build pulled no platform SDKs:
Run: `cargo tree -p gray -e normal | grep -E 'teloxide|twilight|slack-morphism' || echo "clean (expected)"`
Expected: `clean (expected)`.

- [ ] **Step 2: Explicit adapter build still carries the SDKs**

Run: `cargo tree -p gray --features telegram,discord,slack -e normal | grep -E 'teloxide|twilight-gateway|slack-morphism' | wc -l`
Expected: `3` (nonzero), matching the release workflow's assertion.

- [ ] **Step 3: Clipboard build unaffected**

Run: `cargo check -p gray --features clipboard`
Expected: PASS.

- [ ] **Step 4: Full-feature CI equivalents pass**

Run: `cargo clippy --workspace -- -D warnings && cargo test -p gray-gateway --features telegram,discord,slack --quiet`
Expected: PASS.

---

## Notes / deliberate scope cuts

- **`clipboard` loses its `--all-features` coverage.** CI no longer builds it (Task 2 Step 2). If that regression matters, add one line next to the default check: `- run: cargo check -p gray --features clipboard`. Left out here because the user asked to *drop* the checks.
- **Named passthroughs stay.** `gray` keeps `telegram`/`discord`/`slack` because the release workflow builds `-p gray` and must select adapters through it. Only the umbrella is deleted.
- **Historical changelog entries left intact.** Sites in `docs/superpowers/plans|specs` are historical records and are not touched.
