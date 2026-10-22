# Motion spec for gray-ide: 10 monocode effects, in priority order

I only read files; nothing was edited and nothing was built. All monocode paths below are under `~/ref-untrusted/monocode/src`. The gray-ide line numbers refer to `crates/gray-ide/src/app.rs`.

## The gpui animation API (checked in the source)
- **Animating an element:** call `el.with_animation(id, Animation, |el, t| el…)` (`AnimationExt`, gpui-pre-0.3.9 `elements/animation.rs:82`). It is available through `use gpui_kit::*`. There is also `with_animations(id, Vec<Animation>, |el, idx, t|)` for chained steps, and `with_spring`.
- **Building an `Animation`:** `Animation::new(Duration)` plays once; add `.repeat()`, `.repeat_synced()` (phase-locked to one app clock), `.with_easing(f)` or `.with_max_fps(f32)`.
- **Easing functions:**
  - Built in: `linear`, `quadratic`, `ease_in_out`, `ease_out_quint()`, `bounce(f)`, `pulsating_between(min, max)` (`animation.rs:506-542`).
  - CSS-style curves: `gpui_kit::base::animation::cubic_bezier(x1, y1, x2, y2)` (gpui-base `animation.rs:14`).
  - `ease_out_cubic` and `ease_in_out_cubic` live in the same module (popover.rs imports them).
- **Reduced motion is free:** `with_animation` honors `App::reduce_motion()` (`app.rs:1293`). One-shot animations jump to their end state and repeating ones stay at their start state, so we don't need our own flag.
- **The id controls replay:** the animation restarts whenever its `ElementId` is first mounted. Key ids by message or overlay id to get enter animations without storing any state.
- **Ready-made widgets:** `ShimmerText::new(text).duration(..)` (gpui-component `shimmer.rs:141`, default 2 s sweep, spread 0.3), `Spinner`, `Skeleton`. I didn't confirm how `ShimmerText` is re-exported under `gpui_kit::component::…`; check before using it.
- **Reference implementations:** the component popover uses 150 ms with `ease_out_cubic`, sliding `top(travel*(1-t))` with `opacity(t)` (popover.rs:94). The default motion tokens are 120/180/280 ms, enter `(0.16,1,0.3,1)`, exit `(0.4,0,1,1)` (`theme/motion.rs`).

## What gray-ide animates today
- Only `churner()` (line 1964): 8 squares of 4 px, driven by `self.ticks`.
- `ticks` advances in a 33 ms poll loop (line 362 → `poll`, line 637).
- **Bug:** `poll` calls `cx.notify()` only when `changed` (line 832). The churner and the "CHURNING x.xS" timer therefore freeze whenever no stream events arrive. Replacing them with `with_animation` (item 1) fixes this.
- Hover effects exist (e.g. lines 1996, 2221, 3223), but they are instant colour swaps.
- The palette and settings overlays (lines 3348 and 3403, mounted at 4457-4458) and the toast (4459) have no enter motion.

## Shared constants
Add these to a `motion` section near `apply_brand` (around line 4518):
- `EASE_OUT = cubic_bezier(0.22, 1.0, 0.36, 1.0)` — monocode `--motion-ease-out`, index.css:144
- `EASE_POP = cubic_bezier(0.16, 1.0, 0.3, 1.0)`
- `FEEDBACK_MS = 120`
- Accent: the existing `primary` value `#d4a373`

## The effects, in priority order

**1. Live churner and "thinking" pulse** (lines 1964 and 2636-2650)
- monocode: `zen-thinking-pulse` breathes opacity 0.35↔0.9 over 1.8 s ease-in-out (index.css:3678, 3729); `TerminalSpinner` steps braille frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` every 80 ms (TerminalSpinner.tsx).
- Wrap each square: `div().size(px(4.)).bg(t.primary).with_animation(("churn", i), Animation::new(ms(1100)).repeat_synced(), move |d, x| d.opacity(…phase-shifted by i/8…))`.
- Wrap the label: `.with_animation("think", Animation::new(ms(1800)).repeat().with_easing(pulsating_between(0.35, 0.9)), |d, v| d.opacity(v))`.
- Separately, make the elapsed-seconds counter re-render on its own: notify every poll while `!self.turns.is_empty()`, or limit the timer element with `with_max_fps(10.)`.

**2. Typing-dots wave before the first token** (line 2648, the branch where `a.text.is_empty()`)
- monocode: `agent-chat-typing`, 1.1 s ease-in-out infinite, opacity 0.35→1 with translateY −3 px at 30% (index.css:2834).
- Use 3 dots of `size(px(5.))`, `rounded_full`, `muted_foreground`, with per-dot phase offsets of 0, 0.15 and 0.3.
- Use `Animation::new(ms(1100)).repeat_synced()` and map `x` to a bump: `opacity(0.35 + 0.65*b)` and `.top(px(-3.*b))` inside a `relative()` parent.

**3. Message rise-in** (`render_item`, line 4200, keyed `msg-{id}`)
- monocode: `prompt-turn-reveal`, 320 ms `--motion-ease-out`, opacity 0→1 and translateY 10 px→0 (index.css:1706).
- `.with_animation(SharedString::from(format!("rise-{id}")), Animation::new(ms(320)).with_easing(EASE_OUT), |d, t| d.opacity(t).top(px(10.*(1.-t))))`.
- The parent must be `relative()`.
- Uncertainty: on first paint after loading a thread this replays for every item. Only animate items whose id is newer than the id recorded when the thread opened.

**4. Palette and model-picker popover open** (dialog at line 3381)
- monocode: `popover-open`, 170 ms `cubic-bezier(0.16,1,0.3,1)`, scale 0.94→1 with opacity (index.css:3802).
- gpui divs have no scale, so substitute a 6 px lift: `with_animation("palette-in", Animation::new(ms(170)).with_easing(EASE_POP), |d, t| d.opacity(t).mt(px(-6.*(1.-t))))`.
- Backdrop (line 3377): `modal-backdrop-in`, 160 ms ease-out, `opacity(t)` on `hsla(0,0,0,0.35)`.

**5. Settings modal** (line 3403)
- monocode: `modal-panel-in`, 200 ms `(0.16,1,0.3,1)`, translateY 8 px with opacity (index.css:4214).
- Same pattern as item 4, with an 8 px offset.

**6. Approval card and toast slide-in** (lines 2662 and 4459)
- monocode: `approval-toast-in`, 180 ms `cubic-bezier(0.22,1,0.36,1)`, translateY −8 px and opacity (index.css:2552).
- Apply to both, keyed by the label or message text so each new one replays.

**7. Shimmer on the "CHURNING" status label** (line 2641)
- monocode: `.shimmer-text`, 2 s linear infinite sweep, with the base text at 40% alpha (index.css:3116).
- Use `ShimmerText::new(label).duration(Duration::from_secs(2))` in place of the plain div.

**8. Running-terminal bars in the status bar or terminal header** (around line 3038)
- monocode: three bars of 4×8 px in `#e39b4a`, opacity 0.4 lighting to 0.85 in steps at 25/50/75% of a 3.2 s linear cycle (index.css:4248-4300).
- Use `Animation::new(ms(3200)).repeat_synced()`; bar `i` is lit when `x >= 0.25*(i+1)`.

**9. Welcome screen fade and glow** (empty-state block, lines 2565-2619)
- monocode: `OpusWelcome` runs for 7000 ms total; the glow is `radial-gradient(#d9775724…)` with `opus-glow-breathe` (0→1 at 22-58%, then back to 0); motes rise 46 px over 2.6-4.2 s (OpusWelcome.css:1-240, OpusWelcome.tsx:12).
- Do the cheap version: the logo/title column gets a 600 ms `EASE_OUT` with `opacity(t)` and an 8 px rise.
- Stagger the hint rows by `with_animations(vec![delay, enter])`, giving row `i` a 60 ms·i hold first.
- Optionally add a breathing accent glow: a `div().absolute()` with `bg(t.primary.opacity(0.08))` and `pulsating_between(0., 1.)` over 4 s.
- Skip the staff lines and notes; they depend on mask, SVG and CSS features we don't have.

**10. Hover feedback that fades instead of snapping** (sidebar 2221/2282, palette 3364, list rows)
- monocode: 120 ms `--motion-ease-out` on background-color (index.css:298, 343).
- gpui's `.hover()` style refinement can't be interpolated. Fading needs per-row hover state plus `with_spring` / `MotionTokens.spring_control` (Spring 180 ms), which adds complexity.
- Lowest priority; I'd skip it unless 1-9 still feel flat.

## Deliberately skipped
- Word-by-word fade on streamed text (wordFade.tsx: 320 ms, cadence-adaptive 120-400 ms, `cubic-bezier(0.33,0.53,0.67,1)`). `TextView::markdown` renders whole spans, so this would need a custom text element.
- Mascot, orchestrator and plan "bloom" bursts, and the ModelPicker effort-tile ripples (ModelPicker.css:92-140). They're decorative and depend on CSS masks or box-shadow tricks.
- Pane slide (`linked-panel-slide`, 260 ms, index.css:2586). The panels are inside `h_resizable`, so animating their width would fight the resize logic.

## Verification
To check these after implementing: run `cargo clippy -p gray-ide -- -D warnings`, start the IDE, then send a turn, open the palette and settings, and toggle reduced motion.