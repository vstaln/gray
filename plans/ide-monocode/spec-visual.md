I've written the spec below. The main finding is that monocode's polish comes from one neutral ink color applied at a few fixed opacities, not from a palette. Gray can get the same craft by deriving from its own ink `#eceae7` and keeping its accent `#d4a373` and fonts. Nothing was edited and nothing from monocode was run; every gpui API cited below was checked in the registry source.

# Visual design system & layout spec

## How the two sides are built
- **monocode** (`src/styles/index.css`): the theme hue is 240 with 0% saturation. Dark background is 9% lightness and text ("content") is 92%; light mode is 97% and 18% (lines 141–195, 207–220). Every separator and fill is the text color at a low alpha:
  - lines and dividers 7% (l.40)
  - selection fills: subtle 8%, normal 10%, strong 12%, hover 15%, emphasis 20% (dark); 5/6/7/10/14% in light (l.187–191, 214–218)
  - accent `hsl(211 92% 62%)`
  - motion: 120ms feedback, 160ms reorder, 200ms tab close, ease `cubic-bezier(0.22,1,0.36,1)` (l.141–145)
  - code text 12.5px; the UI runs at 12–13px with 10–11px metadata
- **gray-ide**: `apply_brand` (`app.rs:4518–4570`) sets fixed hex values. Background `#050506`, border `#1c1c20`, muted `#131316`, radius 4/6. Font size is never set, so it stays at gpui-component's default of `px(16.)` (`gpui-component-0.7.3/src/theme/mod.rs:771`). In light mode it returns early and leaves the stock grays.

## P1 — Tokens in `apply_brand`
Add `let ink = hex(0xeceae7);` and derive from it with `Hsla::opacity(f)`, which multiplies alpha (`gpui-pre-0.3.9/src/color.rs:637`):
- `border`, `sidebar_border`, `title_bar_border`, `status_bar_border` = `ink.opacity(0.07)`
- `list_hover` = `ink.opacity(0.05)`
- `list_active`, `tab_active`, `sidebar_accent` = `ink.opacity(0.10)`
- `secondary_hover` = 0.12, `secondary_active` = 0.15, `selection` = `accent.opacity(0.30)` (like monocode's `::selection` at l.2018)
- `muted` = `ink.opacity(0.06)` (monocode's code-block fill, l.2110)
- Sidebar: `hex(0x050506)` darkened about 10% → `hex(0x040405)`, matching `.sidebar-glass`'s 90%-with-black (l.427)
- Size and radius: `t.font_size = px(14.)`, `t.mono_font_size = px(12.5)`, `t.radius = px(6.)`, `t.radius_lg = px(10.)` (monocode's code block radius is 10px, and its rounded-md/lg/xl are 6/8/12)
- Light mode should stop returning early. Use `ink = hex(0x2b2a28)` on `background = hex(0xf7f6f4)`, with alphas 0.05/0.06/0.07/0.10/0.14.
- Keep the accent `#d4a373`, Plus Jakarta Sans and Departure Mono (`fonts/`).

## P2 — Shell layout
1. **Sidebar width bug.** `render_sidebar` sets `.w(px(272.))` (`app.rs:2301`), but its resizable panel is sized 232 (`app.rs:4380`). Change it to `.w_full()`.
2. **Bar heights.** Monocode uses 40px title and sidebar headers (`TitleBar.tsx:1128`, `Sidebar.tsx:2583`) and 36px sub-bars (2573). Change gray's `.h(px(48.))` at `app.rs:2309` and `2443` to `px(40.)`.
3. **Sidebar rows** (`app.rs:2265–2290`). Monocode: 12–13px text, `rounded-md`, `px-2.5 py-1.5`, inactive text at 80%, hover fill 5%, selected fill 10%, plus a 15% accent fill for the focused row (`Sidebar.tsx:5243–5262`). For gray:
   - Thread rows: `.text_size(px(13.))`, `.py(px(6.))`, `.px(px(10.))`, `.text_color(t.foreground.opacity(0.8))`. Active gets `.bg(t.primary.opacity(0.15))`; hover gets `.bg(ink.opacity(0.05))`.
   - Time label: `.text_size(px(11.))` at `opacity(0.4)`.
   - Project headers (`2206–2216`): `.text_size(px(12.))` with `font_medium()` instead of `font_semibold()`, and `muted_foreground` at `opacity(0.5)` when inactive.
4. **Header** (`app.rs:2442–2460`). Monocode's title is 13px medium (`TitleBar.tsx:1385`); tabs are 30px tall, `rounded-md`, `px-2`, with inactive text at 50% (l.409–415). Gray's title becomes `.text_size(px(13.)).font_medium()`, and pills such as the worktree one become `.h(px(20.)).rounded_full().bg(ink.opacity(0.07))`.
5. **Status bar (new).** Monocode's footer is `h-7 border-t px-3` with 10–11px mono text (`UsageFooter.tsx:136, 706`). Add an `h_flex().h(px(28.)).px_3().border_t_1().border_color(t.status_bar_border).text_size(px(11.))` row under `main` in `render` (`app.rs:4394`). It shows provider·model, branch, and the elapsed churn time, which currently sits in the timeline at `2636–2645`.

## P3 — Transcript (`render_item`, `app.rs:4200–4268`; column at `2620–2627`)
- **Column:** monocode uses `max-w-4xl` (896px), `px-4`, with 4px gaps between blocks (`AgentTranscript.tsx:1787–1801`). Gray: `.max_w(px(896.))` at 2623, 2726 and the empty state at 2666; `.px_4()`; `.gap_3()` instead of `gap_4`.
- **User bubble:** monocode is `px-3 py-2`, 10% fill, `rounded-lg`, with a 10% border (`AgentTranscript.tsx:2772–2779`); in chat mode it caps at 576px and becomes fully rounded for a single line. Gray: `.px_3().py_2().rounded(px(10.)).bg(ink.opacity(0.10)).border_1().border_color(ink.opacity(0.10)).max_w(px(576.)).text_size(px(14.))`, replacing `rounded_2xl` + `t.secondary`.
- **Tool rows:** monocode draws these as flat rows (`gap-1.5 py-1`) with a mono command chip: `rounded bg-content/6 px-1 text-[13px]` at 70% (`AgentTranscript.tsx:4705–4747`). Output goes in a `pl-5` mono 12px/20px block, `max-h-32`, at 70%; errors are red-400 at 80%. For gray's Activity:
  - Drop the `KIND.to_uppercase()` label.
  - Row: `h_flex().gap_1p5().py_1()` with a 14px icon at `muted_foreground.opacity(0.5)`.
  - Label chip: `div().px_1().rounded(px(4.)).bg(ink.opacity(0.06)).font_family(mono).text_size(px(13.)).text_color(ink.opacity(0.7))`.
  - Detail: `.text_size(px(12.)).text_color(ink.opacity(0.4))`.
  - Errors use `t.danger.opacity(0.8)`.
- **Code blocks / expanded output:** use `rounded(px(10.))`, a 10% border and a 6% fill (l.2105–2110). For expanded `<pre>` blocks use `rounded_md().bg(ink.opacity(0.05)).p(px(10.))` in 11px mono at 70% (`AgentTranscript.tsx:2848`).
- **Churn line** (2636): 12px sans at 55% (monocode l.5163), not uppercase mono.

## P4 — Composer (`app.rs:2724–2740`)
Monocode uses `rounded-lg border-content/10`, rising to 20% on focus, over a 3% fill (`Composer.tsx:2038–2041`, `index.css:1899–1910`). Light mode gets a shadow of `0 6px 24px` at 9% plus `0 2px 6px` at 6% (l.1955–1962).

Gray: `.rounded(px(10.)).border_1().border_color(ink.opacity(0.10)).bg(ink.opacity(0.03))`. Use `when(focused, |s| s.border_color(ink.opacity(0.20)))`, with `self.composer.read(cx).focus_handle(cx).is_focused(window)`.

Shadows use `.shadow(vec![BoxShadow::new(px(0.),px(6.),ink.opacity(0.09)).blur_radius(px(24.)), BoxShadow::new(px(0.),px(2.),ink.opacity(0.06)).blur_radius(px(6.))])`. The builder is at `gpui-pre-0.3.9/src/style.rs:366–389`, and `.shadow(Vec<BoxShadow>)` is at `gpui-pre-macros/src/styles.rs:390`.

The send button follows `.primary-action` (l.1964–1976): white with black text when enabled, `white.opacity(0.3)` when disabled. To keep gray's identity, use `t.primary` in place of white. Placeholder text is at 40% (l.2028).

## P5 — Overlays and motion
- **Popovers, palette, toast** (`app.rs:3387–3391`, `4469–4473`): replace `shadow_lg` with monocode's peek shadow (l.619): `BoxShadow::new(0,12,black.opacity(0.28)).blur_radius(32)` plus `(0,2,black.opacity(0.16)).blur_radius(6)`, with an 8% border and `rounded(px(10.))`. The scrim at 3377 goes from `0.35` to `hsla(0,0,0,0.4)` (0.18 in light).
- **Motion:** only where it's cheap. Use `.with_animation(id, Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()), …)` (`gpui-pre-0.3.9/src/elements/animation.rs:33,60,526`). That curve is close to `cubic-bezier(0.22,1,0.36,1)`. Fade in new transcript items and the toast.

## Limits and open questions
- **No backdrop blur.** gpui-pre has no backdrop filter; "blur" only appears as shadow `blur_radius`. Monocode's frosted composer, sidebar and queue panels need a translucent fill plus a shadow instead.
- **Inset card layout** (monocode l.486–606): a rounded workspace card at `#060606`/`#141414` with a 4% stroke and `0 1px 3px` at 8%, on a `#0d0d0d` shell. It's optional and would be a fair amount of work: wrap `center_outer` in `.m_1p5().rounded(px(10.)).border_1()`. Whether to do it is a design call for you.
- **Text size after the font change.** I haven't confirmed whether gpui-kit sets the window's base text size from `t.font_size`. Check it in a running window after the P1 change.