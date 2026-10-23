Here are the top 5 phase-3 items. I edited nothing and ran nothing from monocode. All gray-ide line numbers refer to `git show ide-monocode:crates/gray-ide/src/app.rs`. All monocode paths are under `~/ref-untrusted/monocode/src`, and gpui-component paths under `~/.cargo/registry/src/index.crates.io-*/gpui-component-0.7.3/src` (`gc/` below).

**1. The palette can't be driven from the keyboard, and its rows aren't grouped**
- **gray-ide:** Enter always runs item 0 (`app.rs:273-276`), and only row 0 is ever highlighted (`render_palette`, `.when(i == 0, …)` at ~4003). There is no up/down handling at all. The right-hand column is a category string like "Thread".
- **monocode** (`ModelPicker.tsx:1664-1690`): the highlighted row follows the arrow keys and the mouse. Rows are `h-8 rounded-lg`: highlighted gets `bg-selection` (10%), hover gets `bg-content/5`. Group headers are `px-2.5 pt-2 pb-1 text-[10px] font-medium uppercase tracking-wide text-content/40`.
- **Fix:**
  - Add a `palette_sel: usize` and use it for the highlight and for Enter.
  - Catch arrow keys with `.capture_action(|_: &input::MoveUp, …)` / `MoveDown` on the palette root (`gc/input/mod.rs:32-34` re-exports the actions; `capture_action` is in gpui-pre `div.rs:435`). Hover sets `palette_sel`.
  - Emit a header row (`text_size(px(10.))`, `muted_foreground.opacity(0.4)`) whenever the category changes.

**2. A real model-picker popover**
- **gray-ide:** the model chip and `ctrl-shift-m` open the full palette pre-filled with "Model:" (`app.rs:3444`, `5395`). Its items are flat strings like `Model: {label} {m}` (`app.rs:1243`).
- **monocode** (`ModelPicker.tsx:1601-1760`):
  - A 44px provider rail (`w-11 border-r p-1.5`, 16px icons) next to the list.
  - Search row `px-3 py-2.5`, 13px, placeholder at 40%.
  - Grouped `h-8` rows, a ✓ at 55% on the current model, and a favorite star shown on hover.
  - Opens with `popover-open`: 170ms, `cubic-bezier(0.16,1,0.3,1)`, scale 0.94→1 (`index.css:3802-3815`).
- **gpui-kit:** `Popover::new(id).anchor(..).offset(..).trigger(chip).open(bool).on_open_change(..).content(|..| ..)` (`gc/popover.rs:136-270`). Wrap the content in `motion::enter(.., POPOVER_MS, 6., ease_pop())`.

**3. Sidebar rows don't show thread state**
- **gray-ide** (`app.rs:2542-2572`): a static 6px dot (`size_1p5`) colored info/danger/success/muted.
- **monocode** (`Sidebar.tsx:5305-5330`, colors at 5246-5262):
  - A working thread spins in the icon slot (`TerminalSpinner`, braille frames every 80ms, accent color, 16px slot).
  - A finished thread shows a ✓ (`size-3.5`, emerald-400) until seen.
  - A thread needing approval gets `bg-content/20` plus a dashed `border-content/30`, and "Needs approval" replaces the time.
- **gpui-kit:** `Spinner::new().color(t.primary).with_size(Size::XSmall)` (`gc/spinner.rs:20-61`; it's an Icon rotated by `.repeat()`). Use `Icon::new(IconName::Check).xsmall().text_color(t.success)` in a `size(px(16.))` slot. The approval state isn't per-thread today; it needs a `turns[id]` approval check.

**4. The live-turn status line still uses the old style**
- **gray-ide** (`app.rs:2990-3005`, visible in s3/s4/midturn): a mono, uppercase line reading "Fake · CHURNING 8.7S". Visual spec P3 called for 12px sans at 55%. It also duplicates the status bar's "Working 8.7s".
- **monocode:** "Working…" / "Worked for …" goes on the group header (`AgentTranscript.tsx:4825-4847`), and the thinking pulse is 0.35↔0.9 over 1.8s (`index.css:3678`).
- **Fix:**
  - Drop `font_family(mono)` and the uppercase.
  - `text_size(px(12.))`, `muted_foreground.opacity(0.55)`.
  - Label `"Working · {elapsed}"`, shown through the existing `motion::shimmer_label` and formatted like the group's "1m 12s".
  - Keep `motion::churner`.

**5. No press feedback, and the overlay styling is unfinished**
- **gray-ide:** 15 `.hover(` calls but only 3 `.active(` calls. Sidebar rows, header icon buttons, composer chips and hint rows have no pressed state.
- **Remaining overlay gaps:**
  - The palette still uses `rounded_xl().shadow_lg()` on a `0.35` scrim (~4040-4048).
  - The settings modal wasn't checked.
  - `render_panel`'s header is still `h(px(48.))` (`app.rs:3720`), not 40.
- **monocode:**
  - 120ms feedback at `cubic-bezier(.22,1,.36,1)` (`index.css:141-145`, 298, 343).
  - Peek shadow (visual spec P5, `index.css` l.619): `0 12px 32px black/28%` plus `0 2px 6px black/16%`, an 8% border, and a 10px radius. Scrim at 0.4 (0.18 in light mode).
- **gpui-kit:**
  - `.active(|s| s.bg(ink.opacity(0.10)))` works only on elements with an `.id()`.
  - Shadows: `.shadow(vec![BoxShadow::new(px(0.),px(12.),black().opacity(0.28)).blur_radius(px(32.)), BoxShadow::new(px(0.),px(2.),black().opacity(0.16)).blur_radius(px(6.))])` (gpui-pre `style.rs:366-389`).

**Other things in the screenshots:**
- s3/s4: the "Checkpoint c610f7add7" row renders above the user bubble it belongs to. That's either an ordering bug or intended; I didn't check.
- s1 (empty state): the "project" header appears twice in the sidebar.
- Not in the top 5:
  - Diff panel: gpui-kit's `Diff` has no hunk hooks, so polishing it means building our own rows.
  - Settings polish: its renderer at `app.rs:4073` was out of scope for this pass.

**Not verified:**
- That `MoveUp`/`MoveDown` reach a capture handler while the single-line Input has focus. If the Input swallows them, use `capture_key_down` (`div.rs:512`) and match `"up"`/`"down"`.
- That several `Spinner`s, all with the internal id `"circle"`, animate independently. They should, because each row's `thread-{id}` scopes them.