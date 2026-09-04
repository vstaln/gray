# gray.alignment.id · Onboarding · Design plan v2

Status: PLAN ONLY. Nothing built. Concept name: **FIRST LIGHT**.

## 0. Corrections to plan v1

- gray is a Rust CLI agent (`vstaln/gray`), not a chat web app. v1 planned chat shells, model tables and auth pages for a product that does not exist. All of that is gone.
- Onboarding for a CLI is exactly three moves: paste one command, run `gray`, type `/connect`. The site has one job: make those three moves unmistakable. Everything that does not serve that is cut.
- v1 said "move the install snippet to docs". Wrong for a CLI. The command is the CTA. It goes in the hero.
- v1 wanted a hand-written WebGL dither. Not needed: `@paper-design/shaders-react` ships `ImageDithering` (zero deps, 8x8 Bayer, animatable cell size).
- The repo already has `web/` (Next.js, live), `redesign-2/` (Vite, 10 sections, ReactBits, Lenis, sonner) and `ui-redesign/` (Next, waitlist, drizzle). Build in `web/`. Delete the other two. Do not start a fourth folder.

## 1. The idea

In astronomy, **first light** is the first image a new telescope captures. In March 2022 JWST published its *telescope alignment evaluation image*: one star, six diffraction spikes, galaxies behind it, taken the moment all 18 mirror segments were aligned into a single instrument.

The product is *gray by alignment*. The onboarding is the moment the tool comes into focus. So the hero plate is the JWST alignment image, dithered to 1-bit, and it **resolves from coarse cells to fine cells** as the page loads, like segments locking into place. That is the single metaphor. It appears twice on the page (hero load, one in-view plate) and nowhere else.

Register: a JPL press kit, not a startup. Monumental scale, quiet copy, real metadata under every image. Nous Research is the reference for *monochrome, text-forward, dithered plates, mono captions with real values*. We drop Nous's hard borders, all-caps paragraphs and generative seed art. We do not need to generate a universe; NASA photographed it.

## 2. What onboarding is (and is not)

**Is:** landing page `/` with install command, three steps, one recording of the real first run, one line of real numbers, footer. Plus a small parity pass on the TUI first-run screen so web and terminal look like one product.

**Is not (cut, with reason):**
- Waitlist form and drizzle DB (ui-redesign). No signup exists to feed.
- Pricing block on the landing page. Tiers are "not open for signup yet". Selling nothing is noise. Link to `/pricing` in the footer.
- Stats band, Manifesto, Providers logo loop, six feature tiles with six images (redesign-2 and live site). Six plates makes each one wallpaper.
- Lenis smooth scroll, sonner toasts, custom cursors, CSS starfield, fake div terminal.
- Any second CTA. "Read the docs" is a nav link, not a button.

## 3. Page: `/` (five blocks, nothing else)

### 3.1 Nav (56px, one line)
Left: `gray` in Instrument Serif 22px, then `by alignment` in mono, dim. Right: `Docs`, `GitHub`. No sign-in, no socials, no version chip.

### 3.2 Hero: first light
Full-bleed `ImageDithering` plate of the JWST alignment image, `colorBack` ink-950, `colorFront` paper, `type="8x8"`, `colorSteps={2}`. Text left-aligned on a 12-col grid, cols 1 to 7. Exactly four text elements:

1. Eyebrow (mono 11px): `Rust · OpenAI-compatible · MIT` (keep, it is true and short)
2. H1 (display 88/80, tracking -0.02em, max 2 lines): `The agent that fits in one binary.` (keep, it is the best line on the current site)
3. One sentence (17/26): `Runs tools, edits code, streams from any provider. Starts at a prompt and gets out of the way.`
4. Install command block: `$ curl -fsSL https://gray.alignment.id/install.sh | sh` with a copy button. Under it, a mono `<details>`: `Windows · beta · from source` that expands three more commands. Not tabs.

Plate caption, bottom-right, mono 11px: `JWST · TELESCOPE ALIGNMENT EVALUATION IMAGE · 2022-03-11 · NASA/STScI`. This is the Nous move (real metadata as ornament), done once.

### 3.3 Three moves
A vertical list, one hairline between rows, left column = number + verb, right column = the thing itself. No cards, no icons.

| | Verb | Copy (one sentence) | Right column |
|---|---|---|---|
| 01 | Install | One command. No prerequisites. | the command (same component as hero, compact) |
| 02 | Run | `gray` drops you at a prompt. Nothing is forced at boot. | asciinema recording of the real first run |
| 03 | Connect | `/connect` when you need a model: free tier, your key, OAuth, or local. | DSN 70m antenna plate, dithered, small, resolves on in-view |

Closing line under the list, display 32px: `That is the whole onboarding.` It is honest and it is the brand ("gets out of the way").

### 3.4 One line of numbers
Three mono numbers with ReactBits `CountUp`, once, on entry. Only if real and measured from the build: binary size (MB, from `dist/`), cold start (ms, measured), providers in the bundled catalog (count from `providers.json`). If any of the three cannot be measured, show two. Never invent.

### 3.5 Footer
One row: `MIT · © 2026 vstaln · Imagery NASA/STScI, NASA/JPL-Caltech` and links `Docs · GitHub · Pricing · Credits · Privacy · Terms`. Credits page already exists in `web/app/credits`.

## 4. Visual system (inherit, then tighten)

### 4.1 Colour (existing tokens in `web/app/globals.css`, keep)
- Ink ramp `#050506 → #3a3a42`, `paper #eceae7`, `dim #8b8a86`, `dimmer #5d5c59`.
- Accent: **adopt the TUI's exact peach `#f6ad7e` (rgb 246,173,126)** instead of the web's muted `sand-400 #d4a373`, so the asciinema recording's highlight matches the page pixel for pixel. Keep `sand-300/200` as hover/pressed derivatives of the new value.
- Accent budget: the `$` prompt glyph, the copied state, the active row marker. Three things. Never a background.
- Dither palette is strictly two-tone: ink-950 and paper.

### 4.2 Type (keep the loaded fonts, use fewer roles)
- Display: Instrument Serif 400. Sizes: 88, 32. Two sizes only.
- Body: Newsreader 17/26. One size.
- Mono: Departure Mono at 11px (labels) and 14px (commands), `tnum`. Departure Mono is a pixel font and only stays crisp at multiples of 11, so labels stay at 11 or 22.
- Sentence case everywhere. Eyebrows are the only uppercase.

### 4.3 Shape and space
- Radius 2px. Hairlines `ink-700`, one edge per group.
- 12-col grid, 24px gutter, 1200 max. Section rhythm 128px desktop, 72px mobile.
- No cards. Tiles only where an image sits (the DSN plate).

### 4.4 Imagery (two plates on the whole page)
| Asset | Source | Where |
|---|---|---|
| JWST telescope alignment evaluation image, 2022 | webbtelescope.org / NASA/STScI, public domain | Hero |
| Deep Space Network 70m antenna (Goldstone DSS-14 or Canberra DSS-43) | images.nasa.gov, NASA/JPL-Caltech | Step 03 Connect |

Rules: never dither behind text, cell size >= 2 device px, every plate ships a pre-dithered PNG poster (the existing `*-dither.png` pipeline in `web/public/space` already does this), credits JSON feeds the footer.

## 5. Motion (one runtime, one metaphor)

Rule: **`motion` is the only animation runtime**. Nothing that pulls gsap, three or postprocessing enters the bundle. This single rule filters ReactBits down to a short list.

### 5.1 Load sequence (hero, once, 1.8s total)
1. `t=0` poster PNG visible (this is the LCP element, no layout shift).
2. `t=0` canvas mounts under the text, `size` animated 16 → 2 over 1600ms with `easeOutExpo` via a `motion` value driving the `ImageDithering` prop (rAF-throttled setState). Canvas fades over the poster in 200ms once the first frame is drawn.
3. `t=400ms` H1 via ReactBits `BlurText` (words unblur, on-metaphor: focus), 60ms stagger.
4. `t=1000ms` plate caption via ReactBits `DecryptedText`, sequential, resolves as the image resolves.
5. `prefers-reduced-motion`: poster only, `size=2` static, no text animation.

### 5.2 Scroll
`whileInView` 12px fade-up, once, on each block. The DSN plate runs the same 16 → 2 resolve when it enters the viewport. No parallax, no pinning, no scroll-linked anything else.

### 5.3 Micro
- Copy button: label `copy` → `copied` with a peach check for 1200ms, plus ReactBits `ClickSpark` in peach at the click point (ignition; the only "delight" on the page).
- Keyboard: Tab lands on the command, Enter copies. `/` focuses the command from anywhere.
- Asciinema: autoplays muted on in-view, pauses out of view, click to pause/resume, controls hidden, loops with a 3s hold on the last frame.

## 6. The recording (this replaces every fake terminal)

`asciinema rec`, 80x24, <= 25s, poster frame on the welcome screen. Script:
1. `$ gray` → logo → `Welcome to gray by alignment`
2. `/connect` → picker → `OpenRouter` → key pasted (masked) → model picker → pick one
3. Prompt: `one-line summary of this repo` → streamed answer → back at the prompt

Custom asciinema theme: ink-950 background, paper foreground, peach for the selection bar, so the terminal and the page are the same object. This recording is also the `og:image` source frame.

## 7. TUI parity (small, cheap, worth it)

`crates/gray/src/setup/mod.rs::run_onboarding` prints logo, two dim welcome lines, provider menu. Changes:
- Welcome line 2 becomes the same sentence as the hero: `Runs tools, edits code, streams from any provider. Starts at a prompt and gets out of the way.` One sentence, two surfaces.
- Add a third dim line: `/connect when you need a model · /help for the rest`, so the picker is not a surprise.
- Nothing else. The peach selection bar and near-black box already match the web after 4.1.

## 8. Libraries (searched, with reasons)

Install into `web/`:

| Package | Why | Weight |
|---|---|---|
| `@paper-design/shaders-react` | `ImageDithering`: 8x8 Bayer, 2-colour, animatable `size`, `minPixelRatio`, `maxPixelCount` caps. Zero deps. | ~40kb |
| `motion` | The one animation runtime. `whileInView`, motion values for the resolve, reduced-motion hook. | ~30kb |
| `asciinema-player` | Real recording of the real first run. | ~120kb, lazy-loaded on in-view |
| ReactBits `BlurText` | H1 reveal. dep: motion | copy-paste |
| ReactBits `DecryptedText` | Plate caption only. dep: motion | copy-paste |
| ReactBits `CountUp` | Three real numbers. dep: motion | copy-paste |
| ReactBits `ClickSpark` | Copy ignition. dep: none | copy-paste |
| ReactBits `Noise` | Replaces `noise.png` grain tile with a canvas grain at 3% opacity, static (`patternRefreshInterval` off). dep: none | copy-paste |

Install ReactBits via `npx shadcn@latest add @react-bits/<Name>-TS-TW` into `web/components/bits/`.

Considered and rejected from ReactBits: `Dither` (three + postprocessing for a 40-line effect), `HalftoneReveal` (ogl, second dither system would fight the first), `FaultyTerminal` / `LetterGlitch` / `ASCIIText` (CRT cliches), `TargetCursor` / `Crosshair` / `SplashCursor` (gimmicks), `TextType` (gsap; the recording is the typing), `Stepper` (form UI, not a reading list), `Galaxy` / `Particles` (the plate is enough), `SplitFlapText` (tempting for a mission-control feel, but it makes the provider list a toy).

Also considered: `@number-flow/react` (fine, but `CountUp` already covers it with the same runtime), Phosphor icons (two icons on the page; inline SVG instead).

Removed from the repo: `redesign-2/`, `ui-redesign/`, and from `web/` any Lenis, sonner, radix that snuck in.

## 9. Performance and accessibility budget

- LCP < 2.0s on 4G: poster PNG is LCP, canvas upgrades in place.
- CLS = 0: canvas and poster share one aspect-ratio box.
- Total JS on `/` < 160kb gz before the lazy asciinema chunk.
- `ImageDithering`: `minPixelRatio={1}`, `maxPixelCount={1_600_000}`, unmount when `!inView`.
- No WebGL: poster only, page is identical minus the resolve.
- Copy button, details toggle and asciinema are all keyboard reachable with visible peach focus rings (existing `:focus-visible` rule).
- Alt text on both plates is the caption text.

## 10. Build order (one person, ~4 days)

1. **Day 1:** Delete `redesign-2/` and `ui-redesign/`. Update tokens (peach). Reduce `web/app/page.tsx` to the five blocks with static placeholders. Ship: the page is already better because it is shorter.
2. **Day 2:** Assets: fetch and pre-dither JWST + DSN plates, write credits JSON. `ImageDithering` hero with poster fallback and load resolve. Reduced-motion path.
3. **Day 3:** Record the asciinema cast, theme it, lazy-load it in step 02. Copy button + `ClickSpark`. `BlurText`, `DecryptedText`, `CountUp` with measured numbers.
4. **Day 4:** TUI parity commit in `setup/mod.rs`. Lighthouse pass against section 9. OG image from the recording's poster frame.

## 11. Copy rules

Sentence case. Plain verbs. No "unleash", "supercharge", "seamless", "blazing". No em dashes. No `//` prefixes, no `#01` boxed labels, no rotated text. One CTA intent per page: install. Every image gets real metadata, never a decorative label.
