# gray Website Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vstaln/gray` contains only the Rust software; all website variants live in private `vstaln/graysite`, served as `/` (live, unchanged) plus `/v1`, `/v2`, `/v3`, `/v4` previews.

**Architecture:** Each gray folder is `git subtree split` to its own branch (`site-v1`/`site-v2`/`site-v3`), then assembled under `graysite/v1|v2|v3` via init + `git mv` + `git subtree add`. The downloaded `gray-alignment-design-plan.zip` (portal variant) is extracted to `/tmp/v4src`, committed as a throwaway import repo, and grafted as `v4` the same way. `scripts/build-all.sh` builds each variant with its subpath base (`/v1`…`/v4`) plus the unprefixed live build, merges them into one `out/`, and `scripts/deploy.sh` rsyncs it (same installer protect-filters as today's `deploy-web.sh`). `gray` then deletes all website trees, `scripts/deploy-web.sh`, the `web` CI job and the `deploy-site` release job (keeping its installer-sync lines).

**Tech Stack:** git subtree, GitHub, Next.js 16 static export (Node 22, v1 pnpm 10.26.0 frozen / v2 pnpm no-frozen, no lockfile in repo), Vite 7 singlefile (v3 + v4, npm ci via `package-lock.json`), Rust cargo (verify-only).

**Spec:** User directives 2026-09-04: (1) "the gray repo should only be for the software NOT THE WEBSITE"; (2) "add all variants. just make it so that its /v1 /v2 etc" — compare and pick the best; (3) "check the latest downloads. i downloaded another variant" — `~/Downloads/gray-alignment-design-plan.zip` becomes `/v4`. Context: `docs/superpowers/plans/2026-09-02-gray-web-portal-plan.md` (already-implemented site plan, stays in git history, not migrated).

## Global Constraints

- Variant map (fixed): `/v1` = `web/` (current live design), `/v2` = `ui-redesign/`, `/v3` = `redesign-2/`, `/v4` = `gray-alignment-design-plan.zip` (portal: Landing/Chat/Models/Docs/Pricing/Account/SignIn). `/` stays the `web/` build pixel-identical until the user promotes a winner.
- v4 is subpath-safe by construction: `HashRouter` (routes live in `#/...`, no server rewrites) + singlefile bundle + media imported under `src/assets/media` (inlined). Only `base: "/v4/"` is added; no ref rewrites needed. (`ponytail-audit-for-gray.zip` in Downloads is an audit doc + viewer shell, not a site variant — not migrated.)
- Installer URLs must never break: merged `out/` must never contain `install.sh`, `install.ps1`, or `dl` (CI guard enforces this).
- rsync protect/exclude filters for `/dl/`, `/install.sh`, `/install.ps1` must survive the move (deploy script lines 26-29 byte-identical apart from paths).
- `README.md` and `dist/install.sh` in gray keep their `https://gray.alignment.id` URLs as-is (links, not website source).
- No source edits to variant components for the preview, except: base-path config (`v1/next.config.ts`, `v2/next.config.ts`, `v3/vite.config.ts`) and v3's 12 absolute `/space/` refs. v2's missing `/space/*.png` art is a pre-existing gap (no `public/` dir in gray) — the build script backfills it from v1 art, documented in the site README.
- v2's waitlist form POSTs to `/api/waitlist`, which cannot exist in a static export — preview shows the form, submissions fail; README says so. No stub code.
- New repo needs a `DEPLOY_KEY` secret before its deploy workflow can run.
- Node 22. Local commits only — do NOT push to `gray` main (push = beta binary release) or `graysite` main (push = live docroot deploy); the user pushes.

---

### Task 1: Assemble graysite with all variants + history

**Files:**
- Consumes: `web/`, `ui-redesign/`, `redesign-2/` in `/home/vstaln/gray` on branch `main`, plus `~/Downloads/gray-alignment-design-plan.zip`
- Produces: local repo at `/tmp/graysite` on branch `main` with `v1/` (= old `web/`), `v2/` (= old `ui-redesign/`), `v3/` (= old `redesign-2/`), `v4/` (= zip content), full histories for v1-v3 + one import commit for v4; GitHub repo `vstaln/graysite` (private) created, nothing pushed yet

- [ ] **Step 1: Split each variant to its own branch (in gray, no working-tree changes)**

Run: `git subtree split -P web -b site-v1 && git subtree split -P ui-redesign -b site-v2 && git subtree split -P redesign-2 -b site-v3`
Expected: three `Created branch 'site-v<N>' <sha>` lines; `git log --oneline site-v1 | head -3` shows web commits; `git ls-tree --name-only site-v2` shows `src/ package.json next.config.ts`; `git ls-tree --name-only site-v3` shows `src/ index.html vite.config.ts`.

- [ ] **Step 2: Init graysite from site-v1 and move it under v1/**

Run: `git init -qb main /tmp/graysite && git -C /tmp/graysite fetch /home/vstaln/gray site-v1:site-v1 site-v2:site-v2 site-v3:site-v3 && git -C /tmp/graysite checkout -qb main site-v1 && mkdir -p /tmp/graysite/v1 && git -C /tmp/graysite mv $(git -C /tmp/graysite ls-files | cut -d/ -f1 | sort -u | tr '\n' ' ') v1/ && git -C /tmp/graysite status --short | head -5`
Expected: `ls /tmp/graysite` shows only `v1/`; `ls /tmp/graysite/v1` shows `app/ components/ package.json pnpm-lock.yaml public/`. (Do NOT fetch straight into `:main` — git refuses to fetch into the checked-out branch even when unborn; fetch to same-name branches then `checkout -b main site-v1`.)

- [ ] **Step 3: Graft v2 and v3 with history**

Run: `git -C /tmp/graysite commit -qm "chore: v1 from gray/web history" && git -C /tmp/graysite subtree add -P v2 /home/vstaln/gray site-v2 -m "chore: v2 from gray/ui-redesign history" && git -C /tmp/graysite subtree add -P v3 /home/vstaln/gray site-v3 -m "chore: v3 from gray/redesign-2 history" && ls /tmp/graysite`
Expected: `v1/ v2/ v3/`; `git -C /tmp/graysite log --oneline | head -5` shows the two graft commits on top.

- [ ] **Step 4: Import the v4 zip as a throwaway repo and graft it**

Run: `rm -rf /tmp/v4src && mkdir -p /tmp/v4src && unzip -q "$HOME/Downloads/gray-alignment-design-plan.zip" -d /tmp/v4src && git -C /tmp/v4src init -qb main && git -C /tmp/v4src add -A && git -C /tmp/v4src -c user.name=gray -c user.email=gray@local commit -qm "import gray-alignment-design-plan.zip" && git -C /tmp/graysite subtree add -P v4 /tmp/v4src main -m "chore: v4 portal variant import" && ls /tmp/graysite`
Expected: `v1/ v2/ v3/ v4/`; `ls /tmp/graysite/v4` shows `src/ index.html vite.config.ts package-lock.json`.

- [ ] **Step 5: Create the private GitHub repo (no push)**

Run: `gh repo create vstaln/graysite --private --description "gray website variants v1/v2/v3 (gray.alignment.id)"`
Expected: `gh repo view vstaln/graysite --json name,visibility | grep private` succeeds.

- [ ] **Step 6: Verify no Rust or installer state leaked in**

Run: `git -C /tmp/graysite ls-files | grep -E "^(crates/|Cargo\.toml|dist/|scripts/deploy\.sh)" || echo CLEAN`
Expected: `CLEAN`.

---

### Task 2: Base-path configs, build-all, deploy, CI, verify

**Files:**
- Modify (in `/tmp/graysite`): `v1/next.config.ts`, `v2/next.config.ts`, `v3/vite.config.ts`, 5 v3 files with `"/space/` refs, create `scripts/build-all.sh`, `scripts/deploy.sh`, `.github/workflows/ci.yml`, `.github/workflows/deploy.yml`, `README.md`
- Test: `/tmp/graysite/out/` merged tree with `/`, `/v1`, `/v2`, `/v3`

**Interfaces:**
- Consumes: Task 1's `/tmp/graysite` repo
- Produces: committed graysite that builds all variants, merges, guards installer paths, deploys (on user push)

- [ ] **Step 1: v1 base-path support (live build unaffected)**

In `/tmp/graysite/v1/next.config.ts`, change `const nextConfig: NextConfig = {` block to add after `trailingSlash: true,`:

```ts
  // Preview builds set NEXT_BASE_PATH=/v1; the live / build leaves it unset.
  ...(process.env.NEXT_BASE_PATH ? { basePath: process.env.NEXT_BASE_PATH } : {}),
```

Verify: `grep -n "NEXT_BASE_PATH" /tmp/graysite/v1/next.config.ts` prints the added lines.

- [ ] **Step 2: v2 export config (overwrites bare config)**

Replace `/tmp/graysite/v2/next.config.ts` entire content with:

```ts
import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static preview at /v2. API routes (src/app/api) are excluded at build
  // time by scripts/build-all.sh — the waitlist form is visual-only here.
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
  ...(process.env.NEXT_BASE_PATH ? { basePath: process.env.NEXT_BASE_PATH } : {}),
};

export default nextConfig;
```

- [ ] **Step 3: v3 base + absolute asset refs**

In `/tmp/graysite/v3/vite.config.ts`, add `base: "/v3/",` inside `defineConfig({` (first key). Then rewrite the 12 absolute refs so they survive the subpath:

Run: `grep -rl '"/space/' /tmp/graysite/v3/src | xargs sed -i 's|"/space/|"/v3/space/|g' && grep -rn '"/space/' /tmp/graysite/v3/src || echo V3_REFS_FIXED`
Expected: `V3_REFS_FIXED`. (Files touched: `Hero.tsx`, `Install.tsx`, `Pricing.tsx`, `Features.tsx`, `Loop.tsx`.)

- [ ] **Step 4: v4 base (HashRouter needs no ref rewrites)**

In `/tmp/graysite/v4/vite.config.ts`, add `base: "/v4/",` as the first key inside `defineConfig({`. No other v4 source changes: routes live in `#/...` and media is bundled from `src/assets`.

Verify: `grep -n 'base:' /tmp/graysite/v4/vite.config.ts` prints the added line.

- [ ] **Step 5: Write scripts/build-all.sh**

Create `/tmp/graysite/scripts/build-all.sh` with this exact content:

```sh
#!/bin/sh
# Builds / (live, from v1), /v1, /v2, /v3, /v4 and merges them into out/.
# usage: scripts/build-all.sh   (run from repo root)
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/out"
rm -rf "$OUT" "$ROOT/v1/out" "$ROOT/v1/out-v1" "$ROOT/v2/out" "$ROOT/v3/dist" "$ROOT/v4/dist"
mkdir -p "$OUT"

echo "→ live / from v1..."
(cd "$ROOT/v1" && pnpm install --frozen-lockfile && pnpm build)
cp -r "$ROOT/v1/out/." "$OUT/"

echo "→ /v1 from v1..."
(cd "$ROOT/v1" && NEXT_BASE_PATH=/v1 pnpm build)
mkdir -p "$OUT/v1"
if [ -d "$ROOT/v1/out/v1" ]; then cp -r "$ROOT/v1/out/v1/." "$OUT/v1/"; else cp -r "$ROOT/v1/out/." "$OUT/v1/"; fi

echo "→ /v2 from v2 (api routes excluded, space art backfilled)..."
mkdir -p "$ROOT/v2/public/space"
cp "$ROOT/v1/public/space/moon-dither.png" "$ROOT/v2/public/space/moon.png"
cp "$ROOT/v1/public/space/jupiter-dither.png" "$ROOT/v2/public/space/jupiter.png"
cp "$ROOT/v1/public/space/saturn-dither.png" "$ROOT/v2/public/space/saturn.png"
cp "$ROOT/v1/public/space/aurora-dither.png" "$ROOT/v2/public/space/aurora.png"
cp "$ROOT/v1/public/space/eclipse-dither.png" "$ROOT/v2/public/space/eclipse.png"
cp "$ROOT/v1/public/space/carina-plate.jpg" "$ROOT/v2/public/space/hero.png"
mv "$ROOT/v2/src/app/api" /tmp/v2-api-$$
(cd "$ROOT/v2" && npm install --no-audit --no-fund && NEXT_BASE_PATH=/v2 npm run build)
mv /tmp/v2-api-$$ "$ROOT/v2/src/app/api"
mkdir -p "$OUT/v2"
if [ -d "$ROOT/v2/out/v2" ]; then cp -r "$ROOT/v2/out/v2/." "$OUT/v2/"; else cp -r "$ROOT/v2/out/." "$OUT/v2/"; fi

echo "→ /v3 from v3..."
(cd "$ROOT/v3" && npm ci && npx vite build)
mkdir -p "$OUT/v3"
cp -r "$ROOT/v3/dist/." "$OUT/v3/"

echo "→ /v4 from v4..."
(cd "$ROOT/v4" && npm ci && npx vite build)
mkdir -p "$OUT/v4"
cp -r "$ROOT/v4/dist/." "$OUT/v4/"

echo "✓ merged:"
ls "$OUT"
test -f "$OUT/v1/index.html" && test -f "$OUT/v2/index.html" && test -f "$OUT/v3/index.html" && test -f "$OUT/v4/index.html" && echo "✓ /v1 /v2 /v3 /v4 present"
```

Run: `chmod +x /tmp/graysite/scripts/build-all.sh && sh -n /tmp/graysite/scripts/build-all.sh && echo SYNTAX_OK`
Expected: `SYNTAX_OK`.

- [ ] **Step 6: Write scripts/deploy.sh (moved deploy-web.sh, OUT fixed)**

Copy `/home/vstaln/gray/scripts/deploy-web.sh` to `/tmp/graysite/scripts/deploy.sh`, then apply exactly these edits:

```bash
# line 2:  Deploys the static site (web/out)  ->  Deploys the merged site (out/ with /, /v1, /v2, /v3)
# line 3:  usage: scripts/deploy-web.sh       ->  usage: scripts/deploy.sh (build first: scripts/build-all.sh)
# line 14: OUT="${REPO_ROOT}/web/out"         ->  OUT="${REPO_ROOT}/out"
# line 19: run: cd web && pnpm build          ->  run: scripts/build-all.sh
# line 23: deploying web/out to ...           ->  deploying out/ to ...
```

Rsync filters (lines 26-29) and smoke-test block stay byte-identical. Verify: `grep -n 'P /dl/' /tmp/graysite/scripts/deploy.sh` prints the filter lines.

- [ ] **Step 7: Add CI + deploy workflows**

Create `/tmp/graysite/.github/workflows/ci.yml`:

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  site:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: v1/pnpm-lock.yaml
      - run: scripts/build-all.sh
      - name: guard installer paths
        run: |
          for p in install.sh install.ps1 dl; do
            if [ -e "out/$p" ] || [ -e "out/v1/$p" ] || [ -e "out/v2/$p" ] || [ -e "out/v3/$p" ] || [ -e "out/v4/$p" ]; then
              echo "::error::export contains $p, which would shadow the installer artifact"
              exit 1
            fi
          done
```

Create `/tmp/graysite/.github/workflows/deploy.yml`:

```yaml
name: deploy

on:
  push:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: deploy-site
  cancel-in-progress: true

jobs:
  deploy-site:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: v1/pnpm-lock.yaml
      - run: scripts/build-all.sh
      - name: deploy site to gray.alignment.id
        env:
          DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}
        shell: bash
        run: |
          [ -n "$DEPLOY_KEY" ] || { echo "::error::DEPLOY_KEY secret not set"; exit 1; }
          mkdir -p ~/.ssh ~/.config/gray-deploy
          install -m 600 /dev/null ~/.config/gray-deploy/key
          printf '%s\n' "$DEPLOY_KEY" > ~/.config/gray-deploy/key
          ssh-keyscan -H 168.110.210.65 >> ~/.ssh/known_hosts 2>/dev/null
          DEPLOY_KEY=~/.config/gray-deploy/key scripts/deploy.sh
```

Note: old `release.yml` `deploy-site` also scp'd `dist/install.sh` + `dist/install.ps1` — those lines do NOT move here; Task 3 keeps them in gray.

- [ ] **Step 8: Write README.md**

Create `/tmp/graysite/README.md` with this exact content:

```md
# graysite — gray.alignment.id variants

- `/` — live site (source: `v1/`)
- `/v1` — current live design (source: `v1/`, from `gray/web`)
- `/v2` — redesign exploration (source: `v2/`, from `gray/ui-redesign`)
- `/v3` — motion-heavy exploration (source: `v3/`, from `gray/redesign-2`)
- `/v4` — portal exploration: Landing/Chat/Models/Docs/Pricing/Account (source: `v4/`, from `gray-alignment-design-plan.zip`)

Build: `scripts/build-all.sh` → `out/`. Deploy: `scripts/deploy.sh`.
Needs `DEPLOY_KEY` secret for the deploy workflow.

Preview caveats: `/v2` space art is backfilled from v1 placeholders and its
waitlist form is visual-only (no API in a static export).
```

- [ ] **Step 9: Build everything and verify the merged tree**

Run: `scripts/build-all.sh`
Expected: ends with `✓ /v1 /v2 /v3 /v4 present`; `ls out` shows `index.html v1 v2 v3 v4` (+ Next static dirs); base-path checks: `href="/docs/"` in `out/index.html` (live unprefixed), `href="/v1/docs/"` in `out/v1/index.html`, `/v2/_next/` asset prefix in `out/v2/index.html`, `/v3/space/` refs and zero `src="/space/` in `out/v3/index.html`. (Trailing slashes: the export uses `trailingSlash: true`.)

- [ ] **Step 10: Commit (do NOT push — push deploys to the live docroot)**

```bash
git add -A
git commit -m "chore: v1/v2/v3/v4 previews with merged static build"
git log --oneline | head -3
git status --short | head -3
```

Expected: clean tree, 3+ commits visible. Tell the user: `git push` when ready (needs `DEPLOY_KEY` secret set first).

---

### Task 3: Strip the website out of gray

**Files:**
- Delete: `web/`, `ui-redesign/`, `redesign-2/`, `scripts/deploy-web.sh`
- Modify: `.github/workflows/ci.yml` (delete whole `web:` job), `.github/workflows/release.yml` (delete `deploy-site` job lines 121-157, add installer-sync step), `.gitignore` (delete lines 23-28, fix line 39 comment)

**Interfaces:**
- Consumes: Task 2's `/tmp/graysite` (rollback safety — verify before deleting)
- Produces: gray repo with zero website source, green Rust CI, installers still shipping

- [ ] **Step 1: Verify graysite holds all four variants before deleting anything**

Run: `ls -d /tmp/graysite/v1 /tmp/graysite/v2 /tmp/graysite/v3 /tmp/graysite/v4 && git -C /tmp/graysite log --oneline | grep -q "v1/v2/v3/v4 previews" && echo SITE_SAFE || echo SITE_MISSING`
Expected: `SITE_SAFE`. If `SITE_MISSING`, stop — do not proceed.

- [ ] **Step 2: Remove the website trees and site deploy script (scoped paths only — the tree has unrelated uncommitted work)**

Run: `git rm -r web ui-redesign redesign-2 scripts/deploy-web.sh && git status --short | head -12`
Expected: deletions staged for `web/*`, `ui-redesign/*`, `redesign-2/*`, `scripts/deploy-web.sh`; pre-existing modifications under `crates/` etc. left untouched and uncommitted.

- [ ] **Step 3: Delete the web job from ci.yml**

In `.github/workflows/ci.yml`, delete the entire `web:` job block (from `  web:` through the `done` of the guard loop), leaving the `test:` job intact. The file must end after the clippy lines:

```yaml
      - run: cargo test --workspace --quiet
      - run: cargo clippy --workspace --quiet
```

- [ ] **Step 4: Replace deploy-site in release.yml with installer sync**

Delete lines 121-157 (`deploy-site:` job through the end of file). Then append this step at the end of the `build-deploy` job (after the `deploy to gray.alignment.id` step, same `- name:` indentation), guarded to run once:

```yaml
      - name: sync installers to gray.alignment.id (once)
        if: matrix.plat == 'x86_64-linux'
        env:
          DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}
        shell: bash
        run: |
          [ -n "$DEPLOY_KEY" ] || { echo "::error::DEPLOY_KEY secret not set"; exit 1; }
          mkdir -p ~/.ssh ~/.config/gray-deploy
          install -m 600 /dev/null ~/.config/gray-deploy/key
          printf '%s\n' "$DEPLOY_KEY" > ~/.config/gray-deploy/key
          ssh-keyscan -H 168.110.210.65 >> ~/.ssh/known_hosts 2>/dev/null
          scp -i ~/.config/gray-deploy/key -o StrictHostKeyChecking=accept-new -o BatchMode=yes dist/install.sh dist/install.ps1 opc@168.110.210.65:/tmp/
          ssh -i ~/.config/gray-deploy/key -o StrictHostKeyChecking=accept-new -o BatchMode=yes opc@168.110.210.65 "sudo install -m 644 /tmp/install.sh /var/www/gray/install.sh && sudo install -m 644 /tmp/install.ps1 /var/www/gray/install.ps1 && rm -f /tmp/install.sh /tmp/install.ps1 && echo '✓ installers live: https://gray.alignment.id'"
```

This preserves the two `scp`/`ssh` installer lines from the deleted job (old lines 156-157) without the site deploy.

- [ ] **Step 5: Clean .gitignore**

Delete lines 23-28 (`# web (Next.js site)` block). Change line 39 from `# python bytecode from web/scripts/*.py` to `# python bytecode`. Final file keeps `/target/`, `/reference/`, `dist/dl/`, `__pycache__/`, `*.pyc`.

- [ ] **Step 6: Verify zero website references remain**

Run: `grep -rn "deploy-web\|ui-redesign\|redesign-2\|working-directory: web\|package_json_file: web" .github/ scripts/ .gitignore 2>/dev/null || echo CLEAN; ls web ui-redesign redesign-2 2>&1 | head -3`
Expected: `CLEAN` plus three `No such file or directory` lines. (`README.md`/`dist/install.sh` `gray.alignment.id` URLs intentionally remain.)

- [ ] **Step 7: Verify the Rust workspace still passes**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --quiet`
Expected: both exit 0.

- [ ] **Step 8: Commit scoped paths only (do NOT push — push cuts a beta release)**

```bash
git add web ui-redesign redesign-2 scripts/deploy-web.sh .github/workflows/ci.yml .github/workflows/release.yml .gitignore docs/superpowers/plans/2026-09-04-gray-website-split.md
git commit -m "chore: extract website variants to vstaln/graysite (software-only repo)"
git status --short | head -8
```

Expected: commit succeeds; pre-existing `crates/` modifications still show as uncommitted (not swept in).

---

## Self-Review

1. **Spec coverage:** software-only gray → Task 3 deletions + CI cleanup. All variants at /v1–/v4 → Tasks 1-2 (map in Global Constraints). Live / unchanged → unprefixed v1 build to root, no live-page edits. Installer URLs never break → guard in Task 2 Step 7 (all five trees), protect filters in Task 2 Step 6, installer-sync in Task 3 Step 4. DEPLOY_KEY → user told in Task 2 Step 10. No-push rule → Task 2 Step 10 + Task 3 Step 8.
2. **Placeholder scan:** no TBD/TODO/"appropriate handling"; workflow YAMLs and build script fully written; every Run has an Expected; v3 sed covers exactly the 5 files with 12 refs found by grep.
3. **Type consistency:** repo `vstaln/graysite` (private) everywhere; branches `site-v1..v3`; checkout `/tmp/graysite`; `SITE_SAFE` gate precedes deletions; Task 3 commits explicit pathspecs (never `git add -A`) so the dirty `crates/` work is not swept in.
