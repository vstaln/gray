# Phase 4 — next-tier monocode feel (from mono-gap2 scout)

Source refs: monocode ~/ref-untrusted/monocode/src. Current code: ~/gray branch ide-monocode, crates/gray-ide/src/{app.rs,motion.rs}. gpui-kit 0.7.3 in ~/.cargo/registry.

## Lane A — motion cluster
1. **Turn celebration burst** — one-shot decor on a fresh prompt turn: halo ring + sheen sweep + ember glow. monocode: features/sessions/ui/turnCelebration.ts, keyframes index.css:1231-1299 (turn-mode-halo/sheen, monocode-ember-glow), plan-step-* 1329-1424. Gate by freshness (monocode FRESH_MS=4000 — only on just-submitted turns, never on remount/old threads).
2. **In-app toast card + slide-in** — render self.toast as a card (slide+overshoot, approval-toast-in index.css:2552; linked-panel-slide/reveal 2586-2623). Data plumbing exists (app.rs:540-547,2363 — currently notifications only). Auto-dismiss.
3. **Backdrop+panel enter for every scrim overlay** — scrim fade + panel scale-in (modal-backdrop-in/modal-panel-in index.css:4205-4230). Apply to palette, settings modal, sessions/search overlays. model picker already has enter(); unify via motion::enter.
4. **Composer dock-slide** — first send slides composer from centered empty-state slot to docked position (~480ms, useComposerDockMotion.ts; FLIP measured transform).
5. **SPIKE (timebox ~1h, bail cleanly if blocked): paced word-fade** on streaming assistant text — per-word fade, fade len = 3x avg burst gap clamped 120-400ms (wordFade.tsx, word-fade-in index.css:3666). Needs word-granular spans in the markdown/TextView path — if gpui can't do it cheaply, note why and skip.

## Lane B — composer popovers
6. **@-mention file picker** — typing @ in composer opens an anchored popover listing fuzzy-ranked files (monocode FileMentionPicker.tsx + Composer.tsx:484-606,987 slashTokenAt/rankSkills). Selecting inserts the mention; feeds existing expand_mentions (app.rs ~6003). gpui-kit popover + existing fuzzy search.
7. **Slash-command inline popover** — typing / opens command menu above composer (modeCommands.tsx defs; reuse palette row rendering/selection incl. group headers + arrow nav from phase 3). composer-mode-shimmer on switch is optional garnish (index.css:2666).

## Rules (both lanes)
- Branch off ide-monocode in your own worktree; commit on your lane branch. Build on maid per BRIEF.md (flock ~/build/gray-ide.lock; zigbuild -j6); clippy -D warnings + fmt clean. Xvfb screenshots per recipe; e2e if you touch state keys.
- Respect existing motion.rs primitives (enter/fade_in/breathe/shimmer); add new helpers there, don't inline ad-hoc timers. Animations must be one-shot or visibility-gated — no idle repaints (review-1.md found a 30Hz idle redraw; don't reintroduce).
- Follow existing code conventions; compact code; no comments unless needed.
