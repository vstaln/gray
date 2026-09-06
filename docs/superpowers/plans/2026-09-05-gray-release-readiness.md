# Gray release-readiness fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear all 12 items from the release-readiness review so gray can be re-released as v1.0.0 with verified artifacts, documented safety model, and no internal code-names in shipped output.

**Architecture:** Small, independent fixes across release plumbing (workflow, deploy, installer), two Rust source changes (gateway config, update check), one dependency swap, README docs, and a file-split refactor — finished by a version bump plus a tagged, verified release.

**Tech Stack:** Rust (edition 2024, 9-crate workspace), GitHub Actions, POSIX sh installer scripts, keep-a-changelog CHANGELOG.md.

**Spec:** `/tmp/opencode/gray-eval/src/data/findings.ts` (findings R1–R3, D1–D4, S1–S4, C1–C3, N1, L1–L5, P1–P6) and checklist `releaseChecklist`; live repo state verified 2026-09-05 (Cargo.toml `0.1.0`, tag `v0.1.0` exists, 719 commits since tag, single Linux asset on the release, `latest-stable.txt` → 404).

## Global Constraints

- Rust edition 2024, resolver 3 — do not change.
- `dist/install.sh` and `scripts/deploy.sh` are POSIX `sh` (`#!/bin/sh`) — no bashisms (`[[ ]]`, `local`, arrays).
- No new runtime dependencies except the `serde_yaml` → `serde_yaml_ng` swap.
- Commit messages follow repo style (`feat|fix|chore|docs|release: ...`).
- NEVER push to `main`, tag, or touch the GitHub Release without explicit human approval (Task 11 is approval-gated).
- Every Rust change must pass `cargo test --workspace --quiet` and `cargo clippy --workspace --quiet`.

---

### Task 1: Scrub internal code-names and fix the false PI_* guideline (C1)

**Files:**
- Modify: `crates/gray-tools/src/bash.rs:114` (BASH_GUIDELINES)
- Modify: `crates/gray-gateway/src/config.rs:3,44,50` (doc comments only)
- Modify: `crates/gray/src/lib.rs` (GatewayCmd/PairingCmd comments — the `(hermes/openclaw parity)` line)
- Modify: other `*.rs` hits from the census grep (rewrite or delete; keep behavior identical)
- Out of scope: `crates/gray-plugin/src/profile.rs:12-13` (owned by Task 2), git branch names (history, leave alone)

**Interfaces:**
- Consumes: nothing.
- Produces: `BASH_GUIDELINES` no longer mentions `PI_*`; repo-wide `grep -rniE "PI_\*|ponytail|hermes|openclaw|codex tri-state|dcg idea|pi parity" crates/ --include='*.rs'` is empty except `profile.rs:12-13`; attribution lives in README Acknowledgements (Task 8).

- [ ] **Step 1: Census the hits**

Run: `grep -rniE "PI_\*|ponytail|hermes|openclaw|codex tri-state|dcg idea|pi parity" crates/ --include='*.rs'`
Expected: list of ~40 files; each hit is a comment/string, none is logic.

- [ ] **Step 2: Fix the real bug — delete the false PI_* guideline**

```rust
// crates/gray-tools/src/bash.rs:114 — before:
pub const BASH_GUIDELINES: &[&str] = &["You can inspect PI_* environment variables for current model and session details."];
// after:
pub const BASH_GUIDELINES: &[&str] = &[];
```

- [ ] **Step 3: Rewrite the named comment sites**

```rust
// config.rs:3 — before:
//! Security model (OpenClaw-style "trusted gateway, explicit operator allowlist"):
// after:
//! Security model ("trusted gateway, explicit operator allowlist"):
// config.rs:44 — before:
/// Env var consulted for the user allowlist (hermes `TELEGRAM_ALLOWED_USERS` style).
// after:
/// Env var consulted for the user allowlist.
// config.rs:50 — before:
/// How unknown DM senders are treated (OpenClaw `dmPolicy`).
// after:
/// How unknown DM senders are treated.
// lib.rs — before:
/// `gray gateway pairing ...` — runtime owner binding (hermes/openclaw parity).
// after:
/// `gray gateway pairing ...` — runtime owner binding without editing gateway.yaml.
```
Apply the same treatment (explain the *why* or delete) to every remaining census hit.

- [ ] **Step 4: Verify scrub + tests**

Run: `grep -rniE "PI_\*|ponytail|hermes|openclaw|codex tri-state|dcg idea|pi parity" crates/ --include='*.rs'` (expect only `profile.rs:12-13`), then `cargo test -p gray-tools --quiet`
Expected: PASS (no test asserts on guideline text — `BASH_GUIDELINES` is referenced only at `bash.rs:114,145`).

- [ ] **Step 5: Commit**

```bash
git add crates/
git commit -m "fix(review): drop false PI_* guideline, scrub internal code-names from comments"
```

---

### Task 2: Swap unmaintained serde_yaml → serde_yaml_ng, add audit to CI (C2)

**Files:**
- Modify: `crates/gray-gateway/Cargo.toml:12`
- Modify: `crates/gray-gateway/src/config.rs` (all `serde_yaml::` paths + tests)
- Modify: `crates/gray-plugin/src/profile.rs:12-13` (comment references serde_yaml)
- Modify: `.github/workflows/ci.yml` (audit step)
- Modify: `Cargo.lock` (via cargo, do not hand-edit)

**Interfaces:**
- Consumes: Task 1 leaves `profile.rs:12-13` untouched.
- Produces: `grep -rn "serde_yaml" crates/ Cargo.toml` is empty (excluding `serde_yaml_ng`); `cargo audit` runs in CI. Verified 2026-09-05: `serde_yml` is ALSO deprecated — use `serde_yaml_ng 0.10`, a drop-in (`from_str`/`to_string` identical).

- [ ] **Step 1: Swap the dependency**

```toml
# crates/gray-gateway/Cargo.toml — before:
serde_yaml = "0.9"
# after:
serde_yaml_ng = "0.10"
```

- [ ] **Step 2: Rename all use sites (7 code + 6 test)**

Run: `grep -rn "serde_yaml" crates/gray-gateway/src/config.rs` then replace every `serde_yaml::` with `serde_yaml_ng::` (lines ~158, 163, 186, 197–198, 206, 212–222). Update the profile comment:

```rust
// crates/gray-plugin/src/profile.rs:12-13 — before:
// ponytail: hand-rolled line parser instead of a serde_yaml dependency;
// switch to serde_yaml if profiles grow beyond a flat entry list.
// after:
// Hand-rolled line parser instead of a YAML dependency;
// switch to serde_yaml_ng if profiles grow beyond a flat entry list.
```

- [ ] **Step 3: Add audit to CI**

```yaml
# .github/workflows/ci.yml — append after the clippy line:
      - uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 4: Verify build + tests + lockfile**

Run: `cargo test -p gray-gateway --quiet && grep -rn "serde_yaml[^_]" crates/ Cargo.toml; echo "leftover-check rc=$?"`
Expected: tests PASS; grep finds nothing (rc=1). `Cargo.lock` updated by cargo automatically.

- [ ] **Step 5: Commit**

```bash
git add crates/gray-gateway/ crates/gray-plugin/ Cargo.lock .github/workflows/ci.yml
git commit -m "fix(deps): serde_yaml (unmaintained) -> serde_yaml_ng, audit in CI"
```

---

### Task 3: Gateway autostart off by default, loud parse errors (S2, S3)

**Files:**
- Modify: `crates/gray-gateway/src/config.rs:142,146,156-159` (`serde_yaml_ng::` paths — requires Task 2)
- Modify: `crates/gray/src/repl/mod.rs:2291` (comment says "default on"), `:3424-3425` (test asserts default-on)

**Interfaces:**
- Consumes: Task 2 (`serde_yaml_ng::` paths in config.rs).
- Produces: fresh configs do NOT autostart; corrupt `gateway.yaml` prints a warning with path + serde error; boot notice already exists (`begin_gateway_boot("Gateway autostarted", …)` at `repl/mod.rs:2300` — no change needed).

- [ ] **Step 1: Write the failing test**

```rust
// crates/gray-gateway/src/config.rs, in mod tests:
#[test]
fn autostart_defaults_off_and_parse_errors_are_loud() {
    assert!(!GatewayConfig::default().autostart);
    let yaml = "platforms:\n  telegram:\n    enabled: true\n    token: 123:abc\n";
    let cfg: GatewayConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert!(!cfg.autostart);
}
```
Run: `cargo test -p gray-gateway autostart_defaults_off --quiet`
Expected: FAIL (`autostart` is `true`).

- [ ] **Step 2: Flip the default (two sites: serde default fn + manual Default impl)**

```rust
// config.rs:142 — before:
fn default_autostart() -> bool { true }
// after:
fn default_autostart() -> bool { false }
// config.rs:146 Default impl — before: autostart: true — after: autostart: false
```

- [ ] **Step 3: Warn loudly on parse errors instead of silently defaulting**

```rust
// config.rs:156-159 — before:
pub fn load_gateway_config() -> GatewayConfig {
    let Ok(path) = gray_gateway_path() else { return GatewayConfig::default(); };
    std::fs::read_to_string(&path).ok().and_then(|s| serde_yaml_ng::from_str(&s).ok()).unwrap_or_default()
}
// after:
pub fn load_gateway_config() -> GatewayConfig {
    let Ok(path) = gray_gateway_path() else { return GatewayConfig::default(); };
    let Ok(text) = std::fs::read_to_string(&path) else { return GatewayConfig::default(); };
    match serde_yaml_ng::from_str(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: ignoring {}: failed to parse gateway config: {e}", path.display());
            GatewayConfig::default()
        }
    }
}
```

- [ ] **Step 4: Fix the stale comment + default-on test in repl**

```rust
// repl/mod.rs:2291 — before:
    // Gateway autostart (default on): boot the in-process daemon when any
// after:
    // Gateway autostart (default off; /gateway autostart on to enable): boot the
    // in-process daemon when any
// repl/mod.rs:3424-3425 — before:
        // default-on: fresh config autostarts
        assert!(gray_gateway::config::GatewayConfig::default().autostart);
// after:
        // default-off: fresh config does not autostart (S2)
        assert!(!gray_gateway::config::GatewayConfig::default().autostart);
```

- [ ] **Step 5: Verify + commit**

Run: `cargo test -p gray-gateway --quiet && cargo test -p gray --lib repl --quiet`
Expected: PASS.
```bash
git add crates/gray-gateway/src/config.rs crates/gray/src/repl/mod.rs
git commit -m "fix(gateway): autostart defaults off, loud warning on corrupt gateway.yaml"
```

---

### Task 4: Update-check opt-out + 24h cache (L4)

**Files:**
- Modify: `crates/gray/src/update.rs` (`startup_check`, new `update_check_due` + cache path helper, tests)
- Modify: `README.md` env table (deferred to Task 8 — note the new var name `GRAY_NO_UPDATE_CHECK` there)

**Interfaces:**
- Consumes: nothing.
- Produces: `GRAY_NO_UPDATE_CHECK=1` disables the startup network call; checks cached for 24h in `<gray-home>/logs/last_update_check` (epoch secs).

- [ ] **Step 1: Write the failing test**

```rust
// update.rs, in mod tests:
#[test]
fn check_due_logic() {
    assert!(update_check_due(None, 1_000_000));
    assert!(!update_check_due(Some(1_000_000), 1_000_000 + 3600));
    assert!(update_check_due(Some(1_000_000), 1_000_000 + 24 * 3600));
    assert!(update_check_due(Some(2_000_000), 1_000_000)); // clock skew never blocks
}
```
Run: `cargo test -p gray --lib check_due_logic --quiet`
Expected: FAIL (`update_check_due` not defined).

- [ ] **Step 2: Implement pure helper + cache I/O**

```rust
/// Seconds between update checks.
const CHECK_INTERVAL_SECS: u64 = 24 * 3600;

fn update_check_due(last_check_secs: Option<u64>, now_secs: u64) -> bool {
    match last_check_secs {
        None => true,
        Some(t) => now_secs.saturating_sub(t) >= CHECK_INTERVAL_SECS,
    }
}

fn last_check_path() -> Option<PathBuf> {
    crate::setup::gray_home().ok().map(|h| h.join("logs").join("last_update_check"))
}

fn read_last_check() -> Option<u64> {
    let p = last_check_path()?;
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn write_last_check(now_secs: u64) {
    if let Some(p) = last_check_path() {
        if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(p, now_secs.to_string());
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
```

- [ ] **Step 3: Wire into startup_check (opt-out first, cache second)**

```rust
// startup_check — after `let current = ...`, before the debug check:
    if std::env::var("GRAY_NO_UPDATE_CHECK").as_deref() == Ok("1") {
        return;
    }
// ... keep existing `if cfg!(debug_assertions) ...` ...
    let now = now_secs();
    if !update_check_due(read_last_check(), now) {
        return;
    }
    write_last_check(now);
```

- [ ] **Step 4: Verify**

Run: `cargo test -p gray --lib update --quiet`
Expected: PASS (existing semver/receipt/lock tests + new `check_due_logic`).

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/update.rs
git commit -m "feat(update): GRAY_NO_UPDATE_CHECK opt-out, cache check for 24h"
```

---

### Task 5: Atomic publish job + SHA256SUMS + notes + channel + host-key pin (D4, S1-part1, R3-wiring, L2, GRAY_CHANNEL fix)

**Files:**
- Modify: `.github/workflows/release.yml` (remove per-matrix `gh release create||upload`, add artifact upload, `publish` job, `GRAY_CHANNEL` build env, `cancel-in-progress: false`, pinned known_hosts)
- Create: `.github/deploy_known_hosts` (one-time `ssh-keyscan` output, committed)

**Interfaces:**
- Consumes: CHANGELOG.md heading convention is DEFINED here — Task 10 must write `## [1.0.0] - YYYY-MM-DD` headings (keep-a-changelog) for the awk extractor below.
- Produces: tags trigger one `publish` job that creates the GitHub Release exactly once with 4 tarballs + `SHA256SUMS` + changelog notes; each matrix build embeds the correct `GRAY_CHANNEL` (fixes beta binaries reporting `stable`, found while planning: `build.rs` defaults to `stable` and no step exports it); deploys pin the host key.

- [ ] **Step 1: Pin the deploy host key (L2)**

Run locally (needs network to the deploy host): `ssh-keyscan -H 168.110.210.65 > .github/deploy_known_hosts && cat .github/deploy_known_hosts`
Expected: 3+ lines (rsa/ecdsa/ed25519). Commit the file; never regenerate in CI.

- [ ] **Step 2: Harden each matrix build — channel env, checksum, artifact**

```yaml
      - name: build
        env:
          GRAY_CHANNEL: ${{ steps.meta.outputs.channel }}
        shell: bash
        run: |
          if [ "${{ matrix.plat }}" = "aarch64-linux" ]; then
            cross build --release --target ${{ matrix.target }}
          else
            cargo build --release --target ${{ matrix.target }}
          fi
      - name: package
        shell: bash
        run: |
          TAR=gray-${{ steps.meta.outputs.channel }}-${{ matrix.plat }}.tar.gz
          tar czf "$TAR" -C target/${{ matrix.target }}/release gray
          sha256sum "$TAR" > "SHA256SUMS-${{ matrix.plat }}"
          echo "TAR=$TAR" >> "$GITHUB_ENV"
      - uses: actions/upload-artifact@v4
        with:
          name: dist-${{ matrix.plat }}
          path: |
            gray-*.tar.gz
            SHA256SUMS-*
```
Delete the old `github release (tags only)` step entirely (it moves to the publish job).

- [ ] **Step 3: Add the single-writer publish job (D4+S1+R3)**

```yaml
  publish:
    needs: build-deploy
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          path: dist-all
          merge-multiple: true
      - name: assemble sums + notes
        shell: bash
        run: |
          cat dist-all/SHA256SUMS-* > dist-all/SHA256SUMS
          sha256sum -c dist-all/SHA256SUMS
          awk '/^## \[/{if (++n==2) exit} n==1' CHANGELOG.md | sed '1d' > dist-all/NOTES.md
          [ -s dist-all/NOTES.md ] || { echo "::error::no CHANGELOG entry for $GITHUB_REF_NAME"; exit 1; }
      - name: create release once
        env:
          GH_TOKEN: ${{ github.token }}
        shell: bash
        run: |
          PRERELEASE=""; case "$GITHUB_REF_NAME" in *-*) PRERELEASE="--prerelease" ;; esac
          gh release create "$GITHUB_REF_NAME" dist-all/gray-*.tar.gz dist-all/SHA256SUMS \
            $PRERELEASE --title "$GITHUB_REF_NAME" --notes-file dist-all/NOTES.md
```
Note: the awk prints the section under the FIRST `## [` heading (the newest entry); the `publish` job assumes Task 10's CHANGELOG puts `## [1.0.0]` on top.

- [ ] **Step 4: Pin known_hosts everywhere, stop cancelling deploys**

Replace both `ssh-keyscan -H 168.110.210.65 >> ~/.ssh/known_hosts 2>/dev/null` lines with `cat .github/deploy_known_hosts >> ~/.ssh/known_hosts`, and flip `cancel-in-progress: true` → `false` (a follow-up push to main must not kill a half-deployed beta).

- [ ] **Step 5: Validate the YAML + commit**

Run: `python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/release.yml')); print(sorted(d['jobs']))"`
Expected: `['build-deploy', 'publish']`.
```bash
git add .github/
git commit -m "fix(release): atomic publish job, SHA256SUMS, changelog notes, pinned host key"
```

---

### Task 6: deploy.sh ships SUMS + verifies manifests post-deploy (S1-part2, D1-guard)

**Files:**
- Modify: `scripts/deploy.sh` (StrictHostKeyChecking yes, manifest curl check)
- Note: `SHA256SUMS` reaches the CDN via the publish job's deploy path — the per-tarball `deploy.sh` stays single-tarball; the publish job scp's the assembled `SHA256SUMS` alongside (add that scp to the existing `deploy to gray.alignment.id` step or a publish-only deploy step — implementer: extend the `sync installers` pattern, not deploy.sh's arg contract).

**Interfaces:**
- Consumes: Task 5 (`SHA256SUMS-<plat>` artifacts, publish job).
- Produces: every deploy fails loudly if `latest-$CHANNEL.txt` doesn't serve HTTP 200 afterwards; no more `accept-new` TOFU.

- [ ] **Step 1: Harden host-key checking**

```sh
# scripts/deploy.sh:18-19 — before:
SSH() { ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$HOST" "$@"; }
SCP() { scp -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$@"; }
# after:
SSH() { ssh -i "$KEY" -o StrictHostKeyChecking=yes -o BatchMode=yes "$HOST" "$@"; }
SCP() { scp -i "$KEY" -o StrictHostKeyChecking=yes -o BatchMode=yes "$@"; }
```
(One-time local setup: `ssh-keyscan -H 168.110.210.65 >> ~/.ssh/known_hosts`.)

- [ ] **Step 2: Post-deploy manifest check (D1 can never regress silently)**

```sh
# scripts/deploy.sh — append after the `echo "✓ latest-$CHANNEL.txt = $VER"` line:
for f in "latest-$CHANNEL.txt"; do
  code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 15 "https://gray.alignment.id/dl/$f")
  [ "$code" = "200" ] || { echo "deploy check failed: $f returned $code"; exit 1; }
done
```

- [ ] **Step 3: Verify + commit**

Run: `sh -n scripts/deploy.sh && echo SYNTAX-OK`
Expected: `SYNTAX-OK`.
```bash
git add scripts/deploy.sh
git commit -m "fix(deploy): strict host keys, fail deploy when manifest check fails"
```

---

### Task 7: install.sh verifies checksums, dest prefers ~/.local/bin (S1-part3, L3)

**Files:**
- Modify: `dist/install.sh` (checksum verify after download; dest selection; `--system` / `GRAY_INSTALL_DIR`)

**Interfaces:**
- Consumes: Task 5 (CDN serves `SHA256SUMS` next to tarballs).
- Produces: `curl|sh` refuses to extract on checksum mismatch; default install is `~/.local/bin` unless `--system` or `GRAY_INSTALL_DIR` (documented in Task 8's env table). `update.rs::run_installer` inherits verification with no code change.

- [ ] **Step 1: Verify checksum before extracting**

```sh
# dist/install.sh — insert after the curl/wget download block, before `tar xzf`:
echo "→ verifying checksum..."
curl -fsSL "${REPO_URL}/SHA256SUMS" -o "${TMP}/SHA256SUMS"
if have_cmd sha256sum; then
    ( cd "$TMP" && grep " ${TARBALL}\$" SHA256SUMS | sha256sum -c - )
elif have_cmd shasum; then
    ( cd "$TMP" && grep " ${TARBALL}\$" SHA256SUMS | shasum -a 256 -c - )
else
    echo "need sha256sum or shasum to verify download"; exit 1
fi || { echo "checksum mismatch for ${TARBALL} — refusing to install"; exit 1; }
```

- [ ] **Step 2: Prefer ~/.local/bin (L3 — code and comment agree again)**

```sh
# dist/install.sh — replace the DEST block:
# install dir: ~/.local/bin by default; --system or GRAY_INSTALL_DIR for system-wide
SYSTEM=0
for a in "$@"; do [ "$a" = "--system" ] && SYSTEM=1; done
if [ -n "${GRAY_INSTALL_DIR:-}" ]; then
    DEST="${GRAY_INSTALL_DIR}"
elif [ "$SYSTEM" = "1" ] || [ "$(id -u)" = "0" ]; then
    DEST="/usr/local/bin"
else
    DEST="$HOME/.local/bin"
    mkdir -p "$DEST"
fi
```

- [ ] **Step 3: Test the verify snippet for real (no network)**

Run:
```bash
T=$(mktemp -d) && cd "$T" && echo hi > gray && tar czf fake.tar.gz gray && sha256sum fake.tar.gz > SHA256SUMS && grep " fake.tar.gz\$" SHA256SUMS | sha256sum -c - && echo VERIFY-OK; rm -rf "$T"
```
Expected: `fake.tar.gz: OK` + `VERIFY-OK`. Then `sh -n dist/install.sh && echo SYNTAX-OK`.

- [ ] **Step 4: Commit**

```bash
git add dist/install.sh
git commit -m "fix(install): verify SHA256SUMS before extract, default to ~/.local/bin"
```

---

### Task 8: README — Safety, Subcommands, Platform matrix, gateway docs, claims, credits (D2, D3, S4, L5, N1)

**Files:**
- Modify: `README.md` (only file; appends new sections + small corrections)

**Interfaces:**
- Consumes: new env vars `GRAY_NO_UPDATE_CHECK` (Task 4), `GRAY_INSTALL_DIR`/`--system` (Task 7), autostart default off (Task 3).
- Produces: Safety section, Subcommands table, Platform matrix, gateway/cron/plugin/proxy docs (or `docs/*.md` pages), corrected claims, Acknowledgements, crates.io decision record (N1 = ship as "gray" via installer only).

- [ ] **Step 1: Capture ground truth for the Subcommands table**

Run: `cargo run -q -p gray -- --help && cargo run -q -p gray -- gateway --help && cargo run -q -p gray -- cron --help`
Expected: full flag/subcommand list; paste real output into the table (do not hand-write from memory).

- [ ] **Step 2: Add the Safety section (S4 — guard is best-effort, no sandbox)**

```markdown
## Safety

`gray` executes shell commands from the model. The destructive-command guard
(`crates/gray-tools/src/bash.rs`) blocks obvious foot-guns (`rm -rf /`, `mkfs`,
fork bombs, `git reset --hard`) after an allow-prompt — it is prefix-based and
**not a sandbox**: pipes, `&&` chains, `$(...)`, `eval`, `xargs rm`,
`find -delete`, `python -c 'shutil.rmtree(...)'` and `curl … | sh` pass through.
`GRAY_GUARD_BYPASS=1` disables it entirely. There is no container or VM isolation:
run gray in a container/VM for untrusted work.
```

- [ ] **Step 3: Add Subcommands table + CLI tools list + Platform matrix (D2, D3)**

Subcommands table from Step 1 output (`resume`, `cron`, `proxy`, `update`, `gateway run/status/install/uninstall/invite/pairing`; flags `--dump-manifest`, `--context-window`, `--session`, `-p`, `-c`). Tools line: extend `gray-tools` Shape entry to `read · write · edit · bash · find · grep · ls · cron_tool · plugin loader (gray.yml profiles)`. Platform matrix:

```markdown
## Platform support

| OS/arch | binary | notes |
|---|---|---|
| Linux x86_64 / aarch64 | musl-static | fully supported |
| macOS arm64 / x86_64 | Rust-static, **not notarized** | curl-installed binaries run fine; browser downloads may hit Gatekeeper quarantine |
| Windows | via WSL only | native Windows unsupported |

`gray gateway install` (systemd user service) is Linux-only. Single static binary —
"zero runtime deps" means no sidecar services; you still need `sh`, `curl`/`wget`,
`tar`, and `sha256sum`/`shasum` for the installer.
```
Apply the line-5 claim fix (`zero runtime deps beyond the toolchain` → `single static binary`) and the Shape `gray-gateway` line (`Discord gateway daemon` → `Telegram/Discord/Slack gateway daemon`).

- [ ] **Step 4: Document the gateway (S2), env additions, credits, naming (L5, N1)**

Gateway docs: config keys (`platforms.<p>.{enabled,token,allowed_users,dm_policy}`, `group_per_user`, `autostart` default **off**, `denied_tools`, `streaming`, `cron_delivery`), pairing flow (`gray gateway pairing approve/list/revoke`), `gateway.yaml` is `0600`, deny-by-default model. Env rows: `GRAY_NO_UPDATE_CHECK=1` (disable startup check), `GRAY_AUTO_UPDATE=1` (background self-update, no prompt), `GRAY_INSTALL_DIR` / `--system` (installer dest). Acknowledgements: ideas/designs informed by pi, Codex, OpenClaw, hermes, dcg — plus naming note: install path is the script/source build (`cargo install gray` belongs to another crate); the binary stays `gray`.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: safety model, subcommands, platforms, gateway, credits"
```

---

### Task 9: Split agent.rs (72 KB) and daemon.rs (57 KB) (C3)

**Files:**
- Modify: `crates/gray-core/src/agent.rs` (~72 KB) → `agent.rs` (types + constructor) + `agent_loop.rs` + `agent_tools.rs` (tool dispatch) + `agent_compact.rs` (compaction/recovery)
- Modify: `crates/gray-gateway/src/daemon.rs` (~57 KB) → `daemon.rs` (routing core) + `daemon_telegram.rs` + `daemon_discord.rs` + `daemon_slack.rs`
- Modify: `crates/gray-core/src/lib.rs`, `crates/gray-gateway/src/lib.rs` (`mod` declarations)

**Interfaces:**
- Consumes: Tasks 1–3 (comment scrub, serde rename — split AFTER them so moved code is already clean).
- Produces: same public API, `cargo test --workspace` + clippy green; no behavior change (existing tests are the pin).

- [ ] **Step 1: Map the sections (mover's guide, not a guess)**

Run: `grep -n "^impl \|^pub \|^fn \|^struct \|^enum " crates/gray-core/src/agent.rs` and the same for `daemon.rs`.
Expected: section inventory; assign each item to exactly one target module from the file list above; shared helpers stay in the parent file and are `pub(crate)`.

- [ ] **Step 2: Move code, keep diffs move-only**

Move each section verbatim (`pub(crate)`-ify items the new modules share; add `use super::…` / `use crate::…` imports the compiler asks for). No logic edits in this task.

- [ ] **Step 3: Verify**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --quiet`
Expected: PASS with zero new warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/gray-core/ crates/gray-gateway/
git commit -m "refactor: split agent.rs and daemon.rs into focused modules (no behavior change)"
```

---

### Task 10: Bump to 1.0.0 + CHANGELOG, commit `release: v1.0.0` (R1, R3, L1)

**Files:**
- Modify: `Cargo.toml:16` (`[workspace.package] version`)
- Create: `CHANGELOG.md` (keep-a-changelog; newest entry on top as `## [1.0.0] - YYYY-MM-DD` — contract required by Task 5's notes extractor)
- `Cargo.lock` (via cargo)

**Interfaces:**
- Consumes: all code tasks above (this commit must contain the full release); Task 5's awk contract.
- Produces: single commit `release: v1.0.0` that the v1.0.0 tag will point at (L1: never tag CI-plumbing again). After this commit, `gray --version` = 1.0.0 and `is_newer("1.0.0","0.1.0")` fires for existing users.

- [ ] **Step 1: Bump the workspace version**

```toml
# Cargo.toml [workspace.package] — before:
version = "0.1.0"
# after:
version = "1.0.0"
```
(All crates use `version.workspace = true`; single-line bump. Verify: `grep -rn '^version' Cargo.toml crates/*/Cargo.toml | grep -v workspace | grep -v gray-gateway || true` shows no stray pins.)

- [ ] **Step 2: Write CHANGELOG.md (human notes, not commit titles)**

```markdown
# Changelog

## [1.0.0] - 2026-09-05

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

### Known issues
- macOS binaries are not notarized (curl-install unaffected) (D3)
- Destructive-command guard is best-effort, not a sandbox — see README Safety (S4)
```
Adjust entries to match what actually landed.

- [ ] **Step 3: Verify extractor contract + tests**

Run: `awk '/^## \[/{if (++n==2) exit} n==1' CHANGELOG.md | head -n 5 && cargo test --workspace --quiet`
Expected: non-empty notes preview; PASS (`gray --version` now reports 1.0.0 via `CARGO_PKG_VERSION`).

- [ ] **Step 4: Commit (exact message — the tag lands here)**

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release: v1.0.0"
```

---

### Task 11: Re-cut the release and verify end-to-end (R2, D1) — APPROVAL-GATED

**Files:** none (git tag + GitHub Release + CDN state + local smoke test).

**Interfaces:**
- Consumes: Task 10's `release: v1.0.0` commit on the release branch/PR.
- Produces: v1.0.0 GitHub Release with 4 tarballs + SHA256SUMS; `latest-stable.txt` = 1.0.0 on CDN; smoke-tested installs.

- [ ] **Step 1: Get human approval for push + tag (STOP if not granted)**

Per repo rules: PR against the agreed branch first, `test` check green, then tag `v1.0.0` on the `release: v1.0.0` commit. NEVER push to `main` or create the tag without explicit approval.

- [ ] **Step 2: Verify all four assets + sums on both surfaces**

Run: `gh release view v1.0.0 --json assets --jq '.assets[].name'` (expect 4× `gray-stable-<plat>.tar.gz` + `SHA256SUMS`) and for each plat `curl -fsSI https://gray.alignment.id/dl/gray-stable-<plat>.tar.gz | head -n 1` (expect 200).

- [ ] **Step 3: Verify both manifests (D1)**

Run: `curl -fsSL --max-time 15 https://gray.alignment.id/dl/latest-stable.txt; echo; curl -fsSL --max-time 15 https://gray.alignment.id/dl/latest-beta.txt`
Expected: both print `1.0.0`.

- [ ] **Step 4: Fresh-machine smoke test (R2)**

On Linux x86_64 (and macOS arm64 if available): `curl -fsSL https://gray.alignment.id/install.sh | sh` into a clean `$HOME`, then `gray --version` (expect `gray 1.0.0`), `gray update` exit code, and a tampered-tarball run proving install.sh refuses (checksum path from Task 7).

---

### Task 12: TUI bottom status line must span full width, no right margin (UX)

**Files:**
- Modify: TUI status-bar rendering code only (find via strings below; likely under `crates/gray/src/` composer/tui/repl draw code)
- Forbid: any file owned by Tasks 1–11 (`update.rs`, `config.rs`, `bash.rs`, `install.sh`, `deploy.sh`, `release.yml`, `ci.yml`, `README.md`, `agent.rs`, `daemon.rs`, `Cargo.toml`, `Cargo.lock`, `profile.rs`)

**Interfaces:**
- Consumes: nothing.
- Produces: the bottom status line (`<used>/<window> · <cache%> cache ... <model> · max`) renders edge-to-edge like the panes above it — no right-hand gap before the right panel border.

- [ ] **Step 1: Locate the status-line render**

Run: `grep -rn "est to interrupt\|% cache\|cache" crates/gray/src/ --include='*.rs' | head -n 20`
Expected: the draw function emitting the thinking-status and bottom status spans (width/padding/margin arithmetic nearby — look for `saturating_sub`, `width -`, `margin`, `pad`, `Area` splits).

- [ ] **Step 2: Reproduce by reading**

Read the layout function: identify where the status line's width is computed (parent area minus something — panel border, padding, or a hardcoded margin) and why the panes above don't share it. The panes above span full width; the status line must use the same full-width area.

- [ ] **Step 3: Minimal fix**

Remove/zero the extra margin so the status line spans the same width as the content above. One-spot change; do not restyle anything else.

- [ ] **Step 4: Verify**

Run: `cargo build -p gray --quiet && cargo test -p gray --lib --quiet`
Expected: clean build, PASS. Correctness evidence is code reasoning (TUI layout has no harness test): name the area/width before/after in the report.

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/<tui-files-only>
git commit -m "fix(tui): bottom status line spans full width, no right margin"
```

### Task 13: Diff overlay panel must extend fully to the right (UX)

**Files:**
- Modify: TUI overlay/panel rendering code only (the diff-view overlay with red/green lines and line-number gutter that currently stops short of the right edge, leaving a dark strip)
- Forbid: any file owned by Tasks 1–11; prefer not to touch `crates/gray/src/composer/draw.rs` status-line lines from Task 12 unless the overlay shares the width helper (then extend it, don't duplicate)

**Interfaces:**
- Consumes: Task 12 (committed `3a2d502` — status-line band bg; reuse its approach for unstyled cells if the gap is the same cause).
- Produces: the diff overlay renders edge-to-edge — no dark strip on the right.

- [ ] **Step 1: Locate the overlay render**

Run: `grep -rn "render_widget" crates/gray/src/composer/draw.rs | head -n 30` and look for the overlay/popup `Rect` (likely `Rect::new(area.x, …, width …)` with a width narrower than `area.width`, or a `Block` without full-width fill).
Expected: the overlay's Rect and whether its unfilled cells are styled.

- [ ] **Step 2: Reproduce by reading**

Read the overlay layout: identify why its right edge stops early (narrower Rect, missing fill, or parent area already inset) while the underlying panes span full width.

- [ ] **Step 3: Minimal fix**

Widen/fill the overlay to the full width edge-to-edge. One-spot change; do not restyle anything else.

- [ ] **Step 4: Verify**

Run: `cargo build -p gray --quiet && cargo test -p gray --lib --quiet`
Expected: clean build, PASS. Name the Rect/width before/after in the report.

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/<tui-files-only>
git commit -m "fix(tui): diff overlay extends fully to the right edge"
```

---

## Self-Review

**1. Spec coverage:** R1→T10, R2→T5+T11, R3→T5+T10, D1→T5+T6+T11, D2→T8, D3→T8, D4→T5, S1→T5+T6+T7, S2→T3+T8, S3→T3, S4→T8, C1→T1, C2→T2, C3→T9, N1→T8, L1→T10+T11, L2→T5+T6, L3→T7, L4→T4+T8, L5→T8. All 12 checklist items + 5 lows covered.
**2. Placeholder scan:** every step has exact file:line targets, real code blocks, and concrete commands; input-dependent work (T9 section map, T8 `--help` paste, T5 host-key bytes) is scoped as explicit discovery steps with exact commands.
**3. Type consistency:** `serde_yaml_ng::` used throughout post-T2; CHANGELOG `## [x.y.z]` contract shared by T5/T10; env var names (`GRAY_NO_UPDATE_CHECK`, `GRAY_INSTALL_DIR`, `--system`) shared by T4/T7/T8; `SHA256SUMS[-<plat>]` filenames shared by T5/T6/T7.
