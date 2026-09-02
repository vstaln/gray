# gray.alignment.id Web / Docs / Portal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `gray.alignment.id` as marketing + docs (static export) and `portal.gray.alignment.id` as account/payments, copying the Hermes/Nous Portal route map and section rhythm, rendered in a noir editorial system (Instrument Serif + Newsreader + Departure Mono, one sand accent, form→blur→grain backgrounds), without touching a single installer URL.

**Architecture:** `web/` Next.js 16 App Router with `output: 'export'` → rsynced into `/var/www/gray/` beside the existing `dl/`; docs via Fumadocs MDX in `web/content/docs/`; exactly three vendored React Bits shaders (`Dither`, `Grainient`, one text effect) behind reduced-motion / viewport / intersection gates; `portal/` a second Next app with Better Auth + Drizzle/Postgres + Polar, and metering in gray's own ledger because `crates/gray/src/proxy.rs` is already in the request path.

**Tech Stack:** Next.js 16, React 19, Tailwind v4 (`@tailwindcss/postcss`, no `tailwind.config.js`), shadcn/ui `new-york`/`neutral`, `tw-animate-css`, Fumadocs (`-core`/`-ui`/`-mdx`), `motion`, `ogl`, Better Auth, Drizzle + Neon Postgres, Polar, Plausible, pnpm

**Spec:** `docs/superpowers/specs/2026-09-02-gray-web-portal-design.md`

## Global Constraints

* `https://gray.alignment.id/install.sh`, `/install.ps1`, `/dl/*`, `/dl/latest-{stable,beta}.txt` must stay byte-identical — hardcoded at `dist/install.sh:14`, `dist/install.ps1:35`, `crates/gray/src/update.rs:4`, `scripts/deploy.sh` — one line.
* `scripts/deploy.sh` is not modified by phases 1–4; the export writes only to a path whitelist under `/var/www/gray/` — one line.
* Rust workspace `Cargo.toml:3-12` gains no members; phases 1–3 touch zero `.rs` files — one line.
* Every self-hosted face is SIL OFL 1.1 with its license committed beside the `woff2`; **Redaction never enters the repo** (personal-use-only) — one line.
* Three WebGL shaders site-wide, each gated on `prefers-reduced-motion`, `min-width: 768px`, and IntersectionObserver, loaded via `next/dynamic({ssr:false})` — one line.
* Budgets enforced in CI: route JS ≤ 120KB gz (hero ≤ 180KB), fonts ≤ 140KB, LCP < 1.8s, CLS < 0.02, a11y = 100 — one line.
* Docs reference pages (CLI, slash, env, tools) are generated from Rust source, never hand-maintained — one line.
* **Phase 3 is blocked** until the business model call (A: sell hosting / B: resell inference) is made — one line.

---

## File Structure

**Existing (untouched):** `dist/install.sh`, `dist/install.ps1`, `scripts/deploy.sh`, `crates/**`
**Existing (replaced):** `dist/index.html:1-58` — becomes the static-export artifact

**New:**
```
web/package.json · next.config.ts (output:'export') · postcss.config.mjs · components.json
web/app/layout.tsx · globals.css
web/app/(marketing)/page.tsx · changelog/page.tsx · manifesto/page.tsx
web/app/docs/[[...slug]]/page.tsx · web/app/docs/layout.tsx
web/content/docs/**/*.mdx            ~34 pages (see Task 8 map)
web/components/site/{nav,hero,install-tabs,downloads,panels,terminal-replay,pricing,footer}.tsx
web/components/bits/{Dither,Grainient,BlurText}.tsx   vendored + pinned
web/components/ui/**                 shadcn
web/lib/{tokens.css,fonts.ts,latest.ts,cn.ts}
web/public/fonts/*.woff2 + OFL.txt · web/public/bg/*.avif · web/public/noise-128.png
web/scripts/gen-reference.mjs        Rust → MDX reference generator
portal/**                            phase 4, mirrors web/ conventions
.github/workflows/web.yml            build + budget gate + deploy
```

---

### Task 1: Scaffold `web/` and prove the export path

**Files:**
- Create: `web/package.json`, `web/next.config.ts`, `web/postcss.config.mjs`, `web/tsconfig.json`, `web/app/layout.tsx`, `web/app/(marketing)/page.tsx`, `web/app/globals.css`
- Modify: `.gitignore:1-30` (add `web/node_modules`, `web/.next`, `web/out`, `portal/node_modules`, `portal/.next`)
- Modify: `.hoplite/settings.json` (`scripts.setup` = `cd web && pnpm install`, `scripts.run` = `cd web && pnpm dev --port 3000`)

**Interfaces:**
- Consumes: nothing
- Produces: `web/out/index.html` renderable at `:3000`

- [ ] **Step 1:** `pnpm dlx shadcn@latest init -t next` in `web/`, then set `output: 'export'`, `images.unoptimized: true`, `trailingSlash: true` in `next.config.ts`.
- [ ] **Step 2:** Wire `.hoplite/settings.json` scripts so any future thread's Preview boots the site.
- [ ] **Step 3:** `pnpm build` → assert `web/out/index.html` exists and contains no `/dl/` or `/install.sh` rewrite.

**Verify:** `preview_start` returns ready; `curl -s localhost:3000 | grep -q '<html'`.

---

### Task 2: Design tokens + typography

**Files:**
- Create: `web/lib/tokens.css`, `web/lib/fonts.ts`, `web/public/fonts/{InstrumentSerif-Regular,Newsreader-Variable,DepartureMono-Regular}.woff2`, `web/public/fonts/OFL-*.txt`
- Modify: `web/app/globals.css`, `web/app/layout.tsx`

**Interfaces:**
- Consumes: Task 1
- Produces: `--ink-*`/`--sand-*` OKLCH tokens, `--font-display`/`--font-body`/`--font-mono`

- [ ] **Step 1:** Freeze the ramp from spec §2.1 as OKLCH custom properties inside `@theme` (Tailwind v4 CSS-first; there is no `tailwind.config.js`).
- [ ] **Step 2:** Subset the three OFL faces to latin+punct `woff2`, load with `next/font/local`, `display:'swap'`, preload display only, commit each license file.
- [ ] **Step 3:** Type scale: display 56/80/112/140 `-0.02em` `text-wrap:balance`; body 17/1.75; mono 11/13 uppercase `0.08em`.
- [ ] **Step 4:** Ship a `/manifesto` specimen page that renders every token and face — this is the visual-regression baseline.

**Verify:** `view_image` a screenshot of `/manifesto`; total font payload ≤ 140KB.

---

### Task 3: Background system (form → blur → grain)

**Files:**
- Create: `web/components/site/backdrop.tsx`, `web/public/bg/{hero,pricing,footer}.avif`, `web/public/noise-128.png`
- Create: `web/components/bits/Dither.tsx`, `web/components/bits/Grainient.tsx`

**Interfaces:**
- Consumes: Task 2 tokens
- Produces: `<Backdrop variant="hero"|"band"|"quiet" shader?>` — poster-first, shader-optional

- [ ] **Step 1:** Bake the three blob compositions (studio.zoxilsi / grainient.supply / pryzm) on the ink ramp; export AVIF ≤ 40KB each into `web/public/bg/`.
- [ ] **Step 2:** `Backdrop` layers: AVIF → `blur(80px)` + `mix-blend-mode:screen` at 12–18% → vertical `linear-gradient` mask to `--ink-950` → `noise-128.png` tile at `opacity:.04`.
- [ ] **Step 3:** Vendor `Dither` and `Grainient` via `npx shadcn@latest add https://reactbits.dev/r/Dither-TS-TW` into `components/bits/`, pin the source, strip unused props.
- [ ] **Step 4:** Gate both behind a shared `useShaderAllowed()` hook: `prefers-reduced-motion: reduce` → poster, `< 768px` → poster, off-screen → unmounted, DPR ≤ 1.5, `next/dynamic({ssr:false})`.
- [ ] **Step 5:** Dither the Hokusai plate (`docs/img/hokusai-kajikazawa.jpg`) to 1-bit AVIF for the footer — committed, never client-generated.

**Verify:** Screenshot with and without `prefers-reduced-motion`; confirm zero WebGL context at 375px width via `browser_console`.

---

### Task 4: Marketing page — nav, hero, install, downloads

**Files:**
- Create: `web/components/site/{nav,hero,install-tabs,downloads}.tsx`, `web/lib/latest.ts`
- Modify: `web/app/(marketing)/page.tsx`

**Interfaces:**
- Consumes: Tasks 2–3
- Produces: `/` sections 1–4 of spec §3

- [ ] **Step 1:** Nav: `gray-logo-clean.svg` wordmark + `Docs · Portal · GitHub` + filled `Install`; sticky, `backdrop-blur`, 1px `--ink-700` rule.
- [ ] **Step 2:** Hero: mono eyebrow, 3-line Instrument Serif h1, copyable one-liner, `Dither` backdrop, sub-line `Rust · OpenAI-compatible · JSONL sessions · zero runtime deps`.
- [ ] **Step 3:** `#install` tabs (macOS/Linux · Windows · source) reproducing `README.md` commands verbatim, each with a copy button.
- [ ] **Step 4:** `#downloads` reads `/dl/latest-stable.txt` + `/dl/latest-beta.txt` at build time via `lib/latest.ts`, with a client refresh and a hardcoded fallback string.

**Verify:** Copy buttons yield exactly the strings in `README.md`; `#downloads` shows the live `Cargo.toml` version.

---

### Task 5: Marketing page — panels, terminal proof, footer

**Files:**
- Create: `web/components/site/{panels,terminal-replay,footer}.tsx`, `web/public/cast/gray-demo.json`
- Modify: `web/app/(marketing)/page.tsx`

**Interfaces:**
- Consumes: Task 4
- Produces: `/` sections 5, 6, 9

- [ ] **Step 1:** Six numbered panels (Hermes `#1…#6` rhythm) with gray's real capabilities: One Binary · Any Provider · Sessions That Survive · Delegate · Lives Everywhere · Schedule.
- [ ] **Step 2:** Record a real REPL session (`❯` prompt, streaming, one tool call, `⬡` working indicator) as an asciinema cast; replay it in CSS/JS — no `<video>`.
- [ ] **Step 3:** Footer with the dithered Hokusai plate, MIT line, `/privacy` `/terms`, GitHub/Discord/X.
- [ ] **Step 4:** One `BlurText` reveal per section maximum; single easing `cubic-bezier(.16,1,.3,1)`, durations 180ms/700ms.

**Verify:** Full-page screenshot at 1440px and 375px; both inspected with `view_image`.

---

### Task 6: Fumadocs shell in the noir theme

**Files:**
- Create: `web/app/docs/layout.tsx`, `web/app/docs/[[...slug]]/page.tsx`, `web/source.config.ts`, `web/lib/source.ts`, `web/mdx-components.tsx`
- Modify: `web/app/globals.css`

**Interfaces:**
- Consumes: Tasks 1–2
- Produces: `/docs/*` with sidebar, TOC, search

- [ ] **Step 1:** Install `fumadocs-core`/`-ui`/`-mdx`; copy the UI components into `web/components/docs/` with `fumadocs-cli` so the theme is ours to restyle.
- [ ] **Step 2:** Restyle sidebar/TOC/code blocks onto the ink ramp; Departure Mono for section labels, Newsreader for prose, no shader on doc routes.
- [ ] **Step 3:** Static search index (no hosted search) so the export stays fully static.
- [ ] **Step 4:** Code blocks: copy button, filename chrome, `bash`/`rust`/`json`/`toml`/`yaml` grammars only.

**Verify:** `/docs/getting-started/installation` renders; search finds "context-window".

---

### Task 7: Generate reference docs from Rust source

**Files:**
- Create: `web/scripts/gen-reference.mjs`
- Create (generated): `web/content/docs/reference/{cli-commands,slash-commands,environment-variables,tools}.mdx`
- Modify: `web/package.json` (`predev`/`prebuild` hook)

**Interfaces:**
- Consumes: `crates/gray/src/lib.rs:218-275`, `crates/gray/src/repl/commands.rs:2-35`, `crates/gray-tools/src/`, `README.md` env table
- Produces: four MDX pages regenerated on every build

- [ ] **Step 1:** Parse the `COMMANDS` and `ALIASES` tables out of `repl/commands.rs` → slash-command reference (16 commands, 14 aliases).
- [ ] **Step 2:** Shell out to `cargo run -q -p gray -- --help` (and each subcommand) for the CLI reference; fall back to a committed snapshot when cargo is absent so the web build never needs Rust.
- [ ] **Step 3:** Enumerate `crates/gray-tools/src/*.rs` tool modules → tools reference.
- [ ] **Step 4:** Mark generated files with a `<!-- generated by gen-reference.mjs -->` header and a CI check that they are current.

**Verify:** `node web/scripts/gen-reference.mjs && git diff --exit-code web/content/docs/reference/`.

---

### Task 8: Write the ~34 docs pages

**Files:**
- Create: `web/content/docs/**/*.mdx` per spec §1 sidebar
- Create: `web/content/docs/meta.json` (sidebar order)

**Interfaces:**
- Consumes: Task 6 shell, Task 7 generated pages
- Produces: the full sidebar tree

- [ ] **Step 1:** Getting Started (5): Installation · Quickstart · Updating · Platform Support · Building from Source.
- [ ] **Step 2:** Using gray (9): REPL & Composer · Slash Commands · Configuration · Providers & Models · OAuth (xAI · Codex) · Sessions & Resume · Context Window & Auto-compact · System Prompt (AGENTS.md) · Logging.
- [ ] **Step 3:** Core (4): Tools · Skills · Delegation · Cron Jobs. Surfaces (2): Proxy · Messaging Gateway.
- [ ] **Step 4:** Developer (7): Architecture · Agent Loop · Provider Runtime · Session Storage · Markdown Renderer · Adding a Tool · Adding a Provider / Platform Adapter.
- [ ] **Step 5:** Reference (Task 7's four) + Model Catalog + FAQ.
- [ ] **Step 6:** Every page opens with a one-sentence answer and a runnable command. No page describes behavior that does not exist in `crates/`.

**Verify:** Zero broken internal links (`fumadocs` link check); every documented flag exists in the Rust source.

---

### Task 9: Deploy pipeline + budget gate

**Files:**
- Create: `.github/workflows/web.yml`
- Modify: `dist/index.html` (deleted — replaced by the export)
- Modify: `scripts/deploy.sh` **only** if a separate `deploy-web.sh` proves insufficient; prefer a new `scripts/deploy-web.sh`

**Interfaces:**
- Consumes: Tasks 1–8
- Produces: `/var/www/gray/` updated without touching `dl/`, `install.sh`, `install.ps1`

- [ ] **Step 1:** `scripts/deploy-web.sh`: `rsync --delete` `web/out/` into `/var/www/gray/` with `--exclude dl/ --exclude install.sh --exclude install.ps1`.
- [ ] **Step 2:** CI: `pnpm build`, Lighthouse CI (LCP/CLS/a11y thresholds), bundle-size gate (120/180KB gz), generated-docs freshness check.
- [ ] **Step 3:** Post-deploy smoke test: `curl -fsSL https://gray.alignment.id/install.sh | head -1` equals `#!/bin/sh`, and `/dl/latest-stable.txt` still returns the `Cargo.toml` version.
- [ ] **Step 4:** Deploy only on `main`; PRs get a preview build artifact.

**Verify:** Run the smoke test against production after the first deploy; `gray update` still resolves.

---

### Task 10 (BLOCKED on the model call): Portal + payments

**Files:**
- Create: `portal/**` (second Next app), `portal/db/schema.ts`, `portal/app/api/webhooks/polar/route.ts`
- Modify (phase 4 only): `crates/gray/src/proxy.rs`, `crates/gray/src/oauth.rs`, `crates/gray/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 2–3 (tokens/backdrop reused verbatim)
- Produces: `portal.gray.alignment.id` routes from spec §5

- [ ] **Step 1:** Decide (A) sell hosting / (B) resell inference. **Do not start Step 2 before this is answered.**
- [ ] **Step 2:** Better Auth (email OTP + GitHub); CLI `gray login` reuses the loopback-PKCE flow in `oauth.rs` (`REDIRECT_URI` `:56121`, `REFRESH_LEAD_SECS` 300).
- [ ] **Step 3:** Drizzle/Neon schema: `users`, `subscriptions`, `api_keys` (hashed, prefix-visible), `ledger` (append-only).
- [ ] **Step 4:** Polar checkout + signature-verified webhooks → `subscriptions`; entitlements read from our DB, never from the payment provider.
- [ ] **Step 5:** `/models` from the bundled `crates/gray/src/setup/catalog.rs` models.dev snapshot — static, unauthenticated, SEO.
- [ ] **Step 6:** `/manage-subscription` in the reference-image-3 layout: four tier columns, Departure Mono labels, dithered plate per tier, active tier inverted to `--sand-400` (not Hermes blue).
- [ ] **Step 7:** `/usage`, `/keys`, `/api-docs`, `/help`, `/privacy`, `/terms`.
- [ ] **Step 8:** Model (B) only: `GrayAdapter` in `proxy.rs` + metering writes into `ledger` on every forwarded request.

**Verify:** Webhook replay test; a paid account's entitlement survives a Polar outage; no card data ever reaches our servers.

---

## Sequencing

Tasks 1–3 are the foundation and must land first, in order. Tasks 4–5 and 6–8 are then independently parallelizable (two workers). Task 9 needs 1–8. Task 10 is a separate PR stack and is blocked on the §4 decision.

Suggested PR stack: `web-foundation` (1–3) → `web-marketing` (4–5) → `web-docs` (6–8) → `web-deploy` (9) → `portal-*` (10).
