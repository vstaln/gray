# Phase 5 — graysite brutalist restyle (user-directed, overrides monocode softness)

Target look = gray's own TUI sessions picker + ~/graysite/app/globals.css. Sharp, dark, mono-forward, ONE amber accent. "no more rounded anything".

## Palette (from ~/graysite/app/globals.css)
- Inks: #050506 #0a0a0b #0e0e10 #131316 #1c1c20 #2a2a30 #3a3a42 (950→500)
- Text: paper #eceae7, dim #8b8a86, dimmer #5d5c59
- Accent: sand #d4a373 (400), #dfb891 (300), #e8cdb0 (200) — the ONLY accent; use for selection/highlight/active/focus
- Signal (error): #b7513f
- Ease stays cubic-bezier(0.16,1,0.3,1)
- Light mode: keep it working — paper base, same sharp rules + amber accent.

## Rules
1. **Zero border-radius everywhere** (user: "no more rounded anything"): composer, bubbles, cards, palette, model picker, popovers, chips, buttons, toasts, status pill, jump-to-latest, toasts. If a spot truly needs separation use a 1px hairline in ink-600/700 or a subtle bg step, never radius.
2. **Selection = solid sand-400 bar, dark text** (#0a0a0b on it) — palette rows, sidebar thread rows, model-picker rows, search results, mention/slash popovers. Exactly like the TUI picker. No soft alpha selection tint.
3. **Typography**: Departure Mono for rows/labels/hints/footer/keys (TUI feel), Brygada for the "Gray" wordmark, Jakarta for body. Footer hint style like the picker: dim text with amber key labels (`enter resume`, `esc cancel`).
4. **Borders/shadows**: 1px hairlines ink-500..700; flat — no soft shadows at all; if a floating surface needs lift use a hard offset edge (e.g. 1px ink-500 + deeper bg), not blur.
5. **Icons — replace the set** (user: not lucide/phosphor, "not super obvious"). Investigate how IconName renders today; preferred = IBM Carbon-style geometric glyphs vendored as SVGs via assets.rs (Apache-2.0, fetch a handful needed), fallback = small custom geometric icon component drawn on a square grid (lines/rects/circles, 1.5px stroke, square terminals — brutalist). Do NOT ship lucide-derived paths. Audit every icon usage site (they're few) and swap wholesale.
6. Keep ALL motion working (celebration burst, enters, shimmer, word-fade) — restyle it, don't remove it. Animations stay one-shot/visibility-gated.
7. Welcome/empty state: keybind card becomes sharp-edged mono list, amber key labels. Buttons square. The diamond logo can stay (it's gray's mark).

## Verify
maid zigbuild + clippy -D warnings + fmt per BRIEF.md (flock, -j6, zigbuild .2.41). Xvfb :83 screenshots: welcome (sharp card), palette open w/ amber selection mid-list, model popover open, sidebar w/ thread states, transcript w/ user bubble + tool group + toast card, @-picker open, light mode welcome. Dir /tmp/ide-brutalist/. e2e if state keys change.
