# gray.alignment.id — Web / Docs / Portal Design

**Date:** 2026-09-02
**Status:** Draft for approval (no code yet)
**Reference steal:** `hermes-agent.nousresearch.com` (marketing + docs), `portal.nousresearch.com` (account/billing) — route maps harvested live, see §1
**Goal:** Replace the 58-line `dist/index.html` with a three-surface web presence — marketing, docs, portal — that keeps every existing installer URL byte-identical, and looks like a Klim/Wolff-Olins noir editorial site rather than a shadcn template.

---

## 0. Constraints (do not break these)

* `https://gray.alignment.id/install.sh`, `/install.ps1`, `/dl/*`, `/dl/latest-{stable,beta}.txt` are hardcoded in `dist/install.sh:14`, `dist/install.ps1:35`, `crates/gray/src/update.rs:4`, `scripts/deploy.sh` → nginx paths on oracle-new stay exactly as they are. The site is additive.
* `scripts/deploy.sh` writes into `/var/www/gray/` and must keep working unmodified.
* Rust workspace (`Cargo.toml`, `resolver = "3"`, 8 crates) is untouched by phases 1–3. Portal-side CLI auth (phase 4) inserts one adapter into `crates/gray/src/proxy.rs` and reuses `crates/gray/src/oauth.rs` PKCE machinery (`REDIRECT_URI` loopback `:56121`, `REFRESH_LEAD_SECS = 300`).
* No font may ship without a redistributable license. **Redaction (mckltype) is free for _personal_ use only — it cannot be used on a site that sells subscriptions.** Everything we self-host is SIL OFL 1.1.
* Docs content is derived from what gray actually does. No aspirational pages: 208 Hermes doc pages map onto ~34 real gray pages.

---

## 1. Information architecture — harvested from Hermes, remapped to gray

Hermes splits into two hosts. We copy that split exactly.

**`hermes-agent.nousresearch.com`** (marketing + docs + installers)
`/` · `/desktop` · `/docs` · `#install` · `#downloads` → github · discord · x · portal/manage-subscription

**`portal.nousresearch.com`** (account)
`/` · `/login` · `/models` · `/manage-subscription` · `/cloud` · `/api-docs` · `/help` · `/privacy` · `/terms`

### gray route map

| Surface | Host | Routes |
|---|---|---|
| **Marketing** | `gray.alignment.id` | `/` · `/#install` · `/#downloads` · `/changelog` · `/manifesto` |
| **Docs** | `gray.alignment.id/docs` | see sidebar below |
| **Portal** | `portal.gray.alignment.id` | `/` · `/login` · `/models` · `/manage-subscription` · `/keys` · `/usage` · `/api-docs` · `/help` · `/privacy` · `/terms` |
| **Artifacts** | `gray.alignment.id` | `/install.sh` · `/install.ps1` · `/dl/*` — **unchanged, nginx** |

Marketing + docs are a static export (`output: 'export'`) rsynced into `/var/www/gray/` beside `dl/`; zero new infra, zero installer risk. Portal is the only surface that needs a server, so it is a separate deploy.

### Docs sidebar (Hermes-shaped, gray-real)

```
Getting Started   Installation · Quickstart · Updating · Platform Support · Building from Source
Using gray        REPL & Composer · Slash Commands · Configuration · Providers & Models
                  OAuth (xAI · Codex) · Sessions & Resume · Context Window & Auto-compact
                  System Prompt (AGENTS.md) · Logging
Core              Tools (bash·read·write·edit·grep·find·ls·request_user_input) · Skills
                  Delegation (delegate_task) · Cron Jobs
Surfaces          Proxy (127.0.0.1:8645) · Messaging Gateway (Telegram·Discord·Slack)
Reference         CLI Commands · Slash Commands · Environment Variables · Tools Reference
                  Model Catalog · FAQ & Troubleshooting
Developer         Architecture (crate map) · Agent Loop · Provider Runtime (SSE)
                  Session Storage (JSONL) · Markdown Renderer · Adding a Tool
                  Adding a Provider · Adding a Platform Adapter
```

Sourced from `crates/gray/src/repl/commands.rs:2-35` (16 slash commands + 14 aliases), `crates/gray/src/lib.rs:218-275` (subcommands), `crates/gray-tools/src/` (13 tool modules), `README.md` env table.

---

## 2. Aesthetic system

The brief: *legendary · serene · peaceful · noir · minimalist · non-generic typefaces · proper backgrounds*. Translated into enforceable rules.

### 2.1 Palette — noir + one warm thread

Keep the sand accent already in `dist/index.html:8` (`#d4a373`) as the brand thread; it separates us from Hermes' electric blue and rhymes with the Hokusai print in the README. Ramp generated once in ramps.studio, frozen as OKLCH tokens in `globals.css` (Tailwind v4 has no `tailwind.config.js`).

```
--ink-950 #06070800   page          --sand-400 #d4a373  accent / links
--ink-900 #0a0b0c     surface       --sand-200 #e8cdb0  accent hover
--ink-800 #121314     card          --paper    #e8e6e3  primary text
--ink-700 #1c1e20     border        --dim      #8a8782  secondary text
--ink-600 #2a2d30     border-hi     --signal   #b7513f  destructive only
```

One accent. No second hue anywhere except `--signal`. Every "gradient" is a *value* gradient inside the ink ramp plus grain — that is what reads as serene instead of AI-slop.

### 2.2 Typography — three faces, all OFL, all self-hosted

| Role | Face | License | Use |
|---|---|---|---|
| Display | **Instrument Serif** | OFL 1.1 | h1/h2 only, 56–140px, `letter-spacing: -0.02em`, `text-wrap: balance` |
| Body | **Newsreader** (variable, optical size) | OFL 1.1 | prose, docs body, 17px/1.75 |
| Mono / eyebrow | **Departure Mono** | OFL 1.1 | eyebrows, labels, code chrome, tabular numerals at 11px increments |

Instrument Serif is the exact register of the reference images (the condensed high-contrast display serif in "Move Every AI Workflow" and "Manage Subscription"). Departure Mono handles the `#1 CONNECT` / `PER MONTH` / `200+ MODELS` uppercase-mono labels that make the Hermes pricing table read as instrument panel rather than SaaS card. Code blocks keep the terminal face (JetBrains Mono) so copied commands look like a terminal.

Subset to latin + punctuation, `woff2`, `next/font/local`, `display: swap`, preload display only. Rule: **no more than two faces visible in one viewport.**

### 2.3 Backgrounds — the three-layer recipe

From reference image 1 (`shapes → blur → final`), promoted to a house rule. Every hero-class background is exactly three layers:

1. **Form** — 2–3 soft blobs on the ink ramp, composed in studio.zoxilsi / grainient.supply, exported and committed as AVIF (~40KB). Static.
2. **Diffusion** — `filter: blur(80px)` + `mix-blend-mode: screen` at 12–18% opacity, masked with a vertical `linear-gradient` to `--ink-950` so the bottom two-thirds go black. This is what makes it peaceful; the blur must eat the shape, not decorate it.
3. **Matter** — grain/dither. Either a tiling 128×128 noise PNG at `opacity: .04` (cheap, everywhere) or, on the hero only, React Bits `Dither` (OGL shader) with `pixelSize`, low `waveSpeed`, monochrome palette.

Dithered halftone imagery (the Hermes pricing cards) is produced in ditther.com / React Bits Texture Lab from public-domain plates — Hokusai is already in `docs/img/` — exported 1-bit AVIF and committed. Never generated client-side.

**React Bits budget — exactly three shaders on the whole site:**

| Component | Where | Fallback |
|---|---|---|
| `Dither` | marketing hero | committed AVIF poster |
| `Grainient` | pricing section band | static gradient + noise |
| `LetterGlitch` or `DecryptedText` | one-line install command reveal | plain text |

Install via `npx shadcn@latest add https://reactbits.dev/r/Dither-TS-TW`, source vendored into `web/components/bits/` and pinned — React Bits is copy-paste, so it becomes our code and gets reviewed like our code.

**Hard gates on every shader:** `prefers-reduced-motion: reduce` → poster; viewport width < 768px → poster; not intersecting → unmounted; `next/dynamic` with `ssr: false`; DPR capped at 1.5.

### 2.4 Motion

Serene means slow and few. One easing (`cubic-bezier(.16,1,.3,1)`), two durations (180ms interface, 700ms reveal). Text reveals are blur+translate (React Bits `BlurText`) at most once per section. No parallax, no scroll-jacking, no marquee, no card tilt.

### 2.5 Performance budget (fail the build if exceeded)

LCP < 1.8s on 4G/mid Android · CLS < 0.02 · route JS < 120KB gz (hero route < 180KB with the shader chunk lazy) · fonts < 140KB total · zero WebGL below 768px · Lighthouse a11y = 100.

---

## 3. Marketing page composition

Sections, top to bottom — Hermes' rhythm, gray's content:

1. **Nav** — wordmark (`docs/assets/gray-logo-clean.svg`), `Docs · Portal · GitHub`, one filled CTA `Install`. Sticky, `backdrop-blur`, 1px `--ink-700` bottom rule.
2. **Hero** — eyebrow `A MINIMAL AGENT HARNESS`; h1 in Instrument Serif, three lines, ~112px: *"The agent that / fits in one / binary."*; one-line copyable install command; `Dither` background with the diffusion mask. Sub-line: `Rust · OpenAI-compatible · JSONL sessions · zero runtime deps`.
3. **Install** (`#install`) — three tabs (macOS/Linux · Windows · from source), each one command, copy button, checksum link into `/dl/`.
4. **Downloads** (`#downloads`) — stable/beta channel cards reading live from `/dl/latest-stable.txt` and `/dl/latest-beta.txt` at build time, plus a client-side refresh. Mirrors Hermes' `#downloads`.
5. **Six capability panels** — Hermes' numbered `#1 Connect … #6 Experiment` structure, gray's truths: `#1 One Binary` · `#2 Any Provider` (OAuth xAI/Codex, OpenRouter, local) · `#3 Sessions That Survive` (JSONL + `-c`) · `#4 Delegate` (`delegate_task`, semaphore 10) · `#5 Lives Everywhere` (gateway: Telegram/Discord/Slack) · `#6 Schedule` (cron). Each panel: mono eyebrow, serif h2, 2-line body, one still frame.
6. **Terminal proof** — a real recorded REPL session (asciinema cast → CSS-styled replay, no video), showing `❯` prompt, streaming tokens, tool call, `⬡` working indicator. This is the strongest asset we have; give it a full viewport.
7. **Pricing band** — see §4. `Grainient` background.
8. **Docs teaser** — three cards into Quickstart / Slash Commands / Gateway.
9. **Footer** — dithered Hokusai plate, MIT line, links, `/privacy` `/terms`.

---

## 4. Payments — the honest version

The Hermes pricing table (reference image 3) is `$0 / $20 / $100 / $200`, monthly credits ≈ 110% of price, a rollover cap, `200+ MODELS`, `HOSTED TOOL USAGE`, `HIGH RATE LIMITS`. That shape only works because Nous resells inference.

**gray today does not resell anything.** `crates/gray/src/proxy.rs` forwards to the user's *own* OpenRouter key / xAI OAuth / Codex OAuth. So a credits tier cannot be copied as-is; it has to be earned by something we actually host. Two candidate models — **this is the one decision the plan needs from you before phase 3 starts:**

* **(A) BYOK stays free forever; sell hosting.** Free = the binary, all providers, all tools, forever. Paid = hosted gateway (we run the Telegram/Discord/Slack daemon so no VPS), cloud agent + cron, session sync, priority builds. Flat $/mo, no metering, no inference resale, no model-catalog liability. Cheapest to build, most defensible for an MIT tool.
* **(B) Hermes parity.** We become the upstream: a `gray` adapter in the proxy points at our own gateway, we buy inference wholesale and sell credits. Needs a credit ledger, real-time burndown, per-model rate cards, abuse controls, and margin management. 10× the work and the operational risk.

### Recommended provider path

The important architectural point either way: **the proxy is already in the request path, so gray meters itself.** Do not outsource metering to the payment provider — the payment provider only needs to answer *"is this account paid, and did they buy a top-up."*

That collapses the Stripe-vs-Polar question. With metering in our own ledger, we need flat subscriptions + one-off purchases + tax handling, which is exactly Polar's sweet spot: merchant-of-record, so no EU VAT registration exposure for a solo maintainer, ~5% + $0.40 vs Stripe's 2.9% + $0.30 plus us owning global tax. Polar's known weaknesses (no entitlements API, all-time-only usage meters, portal-only plan changes, no trial webhooks) don't bite us because entitlements and usage live in our Postgres.

* **Phase 3 (now):** Polar checkout + webhooks → `subscriptions` table → portal reads entitlements. Tiers per model (A).
* **Phase 5 (only if model B is chosen):** revisit. Real-time credit burndown at scale is Stripe Billing + Metronome territory; Stripe's own docs now steer new usage-based integrations to Metronome rather than the Billing Meters API.

Never store card data, never proxy card data, no secrets in the static bundle. Webhook signature verification is mandatory; the ledger is append-only.

---

## 5. Portal composition

Route-for-route with Nous Portal, minus what we don't sell.

* `/` — overview: account state, current plan, install command, quick links.
* `/login` — email OTP or GitHub OAuth (Better Auth). The CLI's `gray login` uses the existing loopback-PKCE flow from `oauth.rs`.
* `/models` — the bundled models.dev catalog (`crates/gray/src/setup/catalog.rs`) rendered as a filterable table with in/out $/1M. Static data, no auth. This page is pure SEO value and costs us nothing.
* `/keys` — BYOK guidance + (model B) issued `gray` API keys, prefix-visible only, hashed at rest.
* `/manage-subscription` — the reference-image-3 table: four tier columns, Departure Mono labels, dithered plate per tier, the active tier inverted to the sand accent instead of Hermes blue.
* `/usage` — read-only ledger view.
* `/api-docs` · `/help` · `/privacy` · `/terms`.

---

## 6. Stack

| Concern | Choice | Why |
|---|---|---|
| Framework | Next.js 16 App Router, React 19 | static export for marketing/docs, server routes for portal, one mental model |
| CSS | Tailwind v4 + `@tailwindcss/postcss` | CSS-first config, OKLCH tokens; no `tailwind.config.js` exists in v4 |
| Primitives | shadcn/ui `new-york`, base `neutral`, `tw-animate-css` | `tailwindcss-animate` is dead in v4 |
| Docs | **Fumadocs** (`fumadocs-core` + `-ui` + `-mdx`) | headless, components copied into our tree so the noir theme survives; Nextra's theme would fight the art direction |
| Motion | `motion` | React Bits' own peer dep |
| Shaders | `ogl` (+`three` only if a component demands it) | React Bits backgrounds are OGL-based |
| Auth | Better Auth | owns the CLI device/PKCE flow too |
| DB | Postgres (Neon) + Drizzle | append-only ledger needs SQL |
| Payments | Polar | MoR; see §4 |
| Analytics | Plausible, self-hosted or EU | no cookie banner, no ad tags (Hermes ships gtag; we won't) |

### Repo layout

```
web/                        pnpm workspace, gitignored node_modules
├─ app/(marketing)/         /  changelog  manifesto
├─ app/docs/[[...slug]]/    Fumadocs route
├─ content/docs/**/*.mdx    ~34 pages
├─ components/bits/         vendored + pinned React Bits sources
├─ components/ui/           shadcn
├─ lib/tokens.css           the ink/sand ramp
└─ public/bg/*.avif         committed baked backgrounds
portal/                     second Next app (server), deployed separately
dist/                       UNCHANGED installers; index.html becomes the export target
```

`web` builds to `dist/` (or `web/out` rsynced into `/var/www/gray/`) so `scripts/deploy.sh` and every installer URL keep working. `.gitignore` gains `web/node_modules`, `web/.next`, `web/out`, `portal/.next`.

---

## 7. Risks

1. **Shader tax.** Three OGL canvases can cost more than the rest of the site. Mitigation: the §2.3 gates are non-negotiable and enforced in CI by a bundle-size check.
2. **Font licensing.** Redaction is personal-use-only and must not appear in the repo. All three shipped faces are OFL 1.1 with license files committed next to the `woff2`.
3. **Installer regression.** Any deploy that touches `/install.sh` or `/dl/` breaks `gray update` for every existing user. Mitigation: the static export writes only to a whitelist of paths; `scripts/deploy.sh` stays untouched; smoke-test `curl -fsSL .../install.sh | head -1` post-deploy.
4. **Docs drift.** 34 pages will rot. Mitigation: generate the CLI/slash/env/tools reference pages from the Rust source (`clap` help + `COMMANDS` table + `Tool::name`) rather than hand-writing them.
5. **Business model.** Building §4 before choosing (A) or (B) wastes the most expensive phase. Phase 3 is blocked on that call.
6. **Generic drift.** The failure mode of shadcn + React Bits is looking like everyone else. Guardrails: one accent, three faces, three shaders, no card-tilt/marquee/parallax, and every background follows form→blur→grain.
