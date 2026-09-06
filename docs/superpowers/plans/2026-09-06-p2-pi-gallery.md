# P2 Pi Gallery (preview) Implementation Plan — npm search + install, skills-only

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `/plugin search` fans out to Gray Index + pi universe, and `/plugin install npm:<pkg>` installs pi skills into gray, labeled `Pi Gallery (preview)`, with hash verification and no new state files beyond the R5-decided `.gray/plugins.json`.

**Architecture:** Steal everything: existing `fetch::client/download/unpack_tar_gz` for transport, npm registry as the metadata backend (one JSON GET, no npm CLI, no new deps except `Sha512` from the already-used `sha2` crate), existing skills-loader roots for activation, existing lockfile + `set_enabled` for state. Extensions/themes are listed but never executed (P3 node bridge is a separate plan).

**Tech Stack:** Rust (`gray-pkg`, `gray` repl), npm registry HTTPS API, existing skills dirs.

**Spec:** `docs/superpowers/specs/2026-09-06-p2-pi-gallery-design.md` (AMENDED in-session; this session's locked decisions are source of truth on any divergence).

## Global Constraints

- Work on `feat/plugin-p1` (continue the branch; P1 commits are ancestors). Never push/merge `main`.
- Prerequisite: tree must resolve+build (user's cron rip-out in flight; `gray → gray-gateway/cron` feature flag is theirs to drop — do NOT touch rip-out files; verify `cargo check --workspace` passes before starting Task P2-1, and stop with NEEDS_CONTEXT if it doesn't).
- Reuse-first: no npm CLI, no new registry client, no new state files. Frozen contracts: lock shape, miss-message string, copy rule (`Pi Gallery (preview)` label at display time, `ecosystem: "pi-gallery"` stored).
- Commit discipline: `git add` ONLY listed files. Never push.

---

### Task 1: P2-1 `npm:` spec + registry metadata + tarball install

**Files:**
- Modify: `crates/gray-pkg/src/ops.rs` (`NameOrUrl`/`parse_spec`, `install` dispatch, namer), `crates/gray-pkg/src/fetch.rs` (sha512 verify path)

**Interfaces:**
- Consumes: `fetch::client/download/unpack_tar_gz/redact`, `HashSpec`, lock read/write.
- Produces: `NameOrUrl::Npm { name: String, version: Option<String> }`; `async fn npm_tarball_url(client, name, version) -> anyhow::Result<(String tarball_url, String integrity)>` via `GET https://registry.npmjs.org/<pkg>` (respect `MAX_REDIRECTS`, redact URL in logs); install flow metadata → `download(tarball, integrity)` → unpack to staging dir.

- [ ] **Step 1: Failing test** — `parse_spec("npm:pi-foo")` → `Npm{name:"pi-foo",version:None}`; `parse_spec("npm:@scope/bar@1.2.3")` → version `Some`. Run `cargo test -p gray-pkg` → FAIL (variant missing).
- [ ] **Step 2: Spec enum + registry resolve** — add variant + `parseNpmSpec` (regex-free split on LAST `@` after stripping `npm:`; scoped names keep leading `@`). `npm_tarball_url`: GET metadata, pick `version` exact or `dist-tags.latest`, read `versions[v].dist.tarball` + `dist.integrity` (fallback `dist.shasum` → `sha256:` form). Unit-test resolver against a local stub HTTP server (no live registry in tests; loopback http allowed by `check_url`).
- [ ] **Step 3: Integrity verify** — npm `integrity` is `sha512-<base64>`; `fetch::download` verifies sha256-hex only. Add sha512-base64 verification alongside (same function, algorithm prefix dispatch: `sha256:<hex>` existing, `sha512-<base64>` new, else bail `unsupported hash algorithm`). Test both paths + mismatch bail.
- [ ] **Step 4: Wire `install`** — `Npm` arm: resolve → download (verified) → unpack to staging tempdir → hand path to Task P2-2's extractor (define `pub(crate) async fn stage_npm_package(...) -> anyhow::Result<StagedPkg{dir, name, version, integrity}>` EXACTLY with that name/shape; P2-2 consumes it). Lock write happens in P2-2 after extraction (single write, no half-state).
- [ ] **Step 5: Run + commit** — `cargo test -p gray-pkg --quiet` PASS. `git add crates/gray-pkg/src/ops.rs crates/gray-pkg/src/fetch.rs` + commit `feat(pkg): npm: spec with verified tarball staging`.

### Task 2: P2-2 Skills extraction + lock record

**Files:**
- Modify: `crates/gray-pkg/src/ops.rs` (extractor + lock write), optionally `crates/gray/src/skills/*` (ONLY if discovery misses a dir — verify first, prefer no change)

**Interfaces:**
- Consumes: `StagedPkg` from P2-1.
- Produces: lock entry `{ecosystem:"pi-gallery", version, hash:<integrity>, source:<tarball URL redacted? no — full URL stored, redacted only in logs>, scope}`; installer summary `{taken: Vec<String>, skipped_ext: bool, skipped_themes: bool}` printed by callers.

- [ ] **Step 1: Layout probe (no code)** — download 3 real pi tarballs to /tmp (manual, not committed), list their skills layouts; confirm probe patterns `skills/*/SKILL.md`, `*/SKILL.md`, top-level `*.md`, manifest `pi.skills` globs. Record findings as a comment atop the extractor.
- [ ] **Step 2: Extractor** — copy matched skills into `<plugins_dir>/pi/<name>/` (user scope) preserving relative names; NEVER execute package code (no build scripts, no binaries on PATH — copy `.md` only, ignore everything else). Print taken vs skipped (extensions/themes present → `skipped_ext/skipped_themes=true` + honest line `skipped N extension files (P3)`).
- [ ] **Step 3: Lock + scope** — write ONE lock entry post-extraction (no half-state on failure: extraction errors remove the staging dir and write nothing). Project scope (cwd `.gray/plugins.json` overlay per R5) honored for `enabled` at read time — no new behavior beyond R5.
- [ ] **Step 4: Discovery check** — verify installed skills appear via existing `discover_skills` roots (gray already scans `.pi/skills`; if `<plugins_dir>/pi` isn't scanned, add that ONE root + test). Test: install fixture tarball → skill discoverable by name.
- [ ] **Step 5: Run + commit** — `cargo test -p gray-pkg --quiet` (+ gray skills tests if touched). Commit `feat(pkg): install skills from pi tarballs`.

### Task 3: P2-3 `search` fan-out with (preview) labels

**Files:**
- Modify: `crates/gray/src/repl/plugin_cmds.rs` (`Search` arm), `crates/gray/src/main.rs` (`Search` arm) — replace honest stubs with real fan-out; `crates/gray-pkg/src/index.rs` ONLY if a helper is needed (prefer local fn in callers).

**Interfaces:**
- Consumes: `index::fetch_index/lookup` (gray side), npm search API `GET https://registry.npmjs.org/-/v1/search?text=<q>&size=20` (pi side).
- Produces: merged lines `name version [Gray Index|Pi Gallery (preview)] - description`; gray wins collisions; pi-side failure → single line `Pi Gallery (preview): unreachable`, never an error exit.

- [ ] **Step 1: Probe (no code)** — curl both `/-/v1/search?text=<known-pi-pkg>` and pi.dev packages page; confirm npm search recall for 3 known packages; record decision (expect: npm search wins, pi.dev is SSR HTML).
- [ ] **Step 2: Implement fan-out** in both REPL + CLI arms (shared helper to avoid duplication — put it in `gray-pkg::ops` as `pub async fn search_all(query) -> Vec<SearchHit{name, version, desc, source}>` so both callers share it; test the helper with stub servers).
- [ ] **Step 3: Tests** — collision fixture (same name both sides → gray line only + pi line suppressed), unreachable-pi fixture (timeout/refused → advisory line, exit 0), copy-rule asserts (exact `(preview)` string).
- [ ] **Step 4: Run + commit** — `cargo test -p gray-pkg` + `cargo test -p gray --lib repl::` PASS. Commit `feat(plugin): search fans out to Pi Gallery (preview)`.

### Task 4: P2-4 `git:` specs (phase b)

**Files:** Modify: `crates/gray-pkg/src/ops.rs`, `fetch.rs` (only if a helper is needed).

- [ ] **Step 1:** `NameOrUrl::Git{url, git_ref}` + parse (`git:` prefix or raw `https://`/`ssh://` + optional `@ref`). Shallow clone `--depth 1` (+ `--branch <ref>` when pinned) into staging via `git` CLI (shell out — never reimplement git). No hash verify (honest `unverified` warning mirroring `install_url`); record `hash:<commit-sha>` post-clone.
- [ ] **Step 2:** Reuse P2-2 extractor on the clone (skills only, same taken/skipped honesty).
- [ ] **Step 3:** Tests with local `git` fixture repos (no network). Commit `feat(pkg): git: plugin sources`.

### Task 5: P2-5 Docs + P2 verification

**Files:** Modify: `docs/plugins.md`, plan-adjacent tests only.
- [ ] Document `npm:`/`git:` specs, `(preview)` meaning, skills-only honesty statement, trust note (project installs after trust), P3 pointer for extensions.
- [ ] Full: `cargo test --workspace --quiet && cargo clippy --quiet`; `gray plugin search <q>`, `install npm:<real-skill-pkg>`, skill shows in discovery, `remove` cleans it.
- [ ] Commit `docs(plugins): Pi Gallery (preview) source`.
