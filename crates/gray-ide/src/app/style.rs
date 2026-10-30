use super::*;

/// Neobrutalist lift for floating surfaces: a solid offset ink block, no
/// blur — replaces soft shadows entirely.
pub(super) fn hard_shadow(t: &Theme) -> Vec<BoxShadow> {
    vec![BoxShadow::new(
        px(4.),
        px(4.),
        if t.mode.is_dark() {
            hsla(0., 0., 0., 0.85)
        } else {
            t.foreground
        },
    )]
}

/// Hairline for floating surfaces — a step up from the page hairline so the
/// hard shadow has a real edge to read against.
pub(super) fn float_border(t: &Theme) -> Hsla {
    t.foreground.opacity(0.24)
}

/// Liquid-glass fill: ink-850 at ~78% so the page ghosts through. Paper in
/// light mode. Corners stay sharp — the sheen and edges do the selling.
pub(super) fn glass_bg(t: &Theme) -> Hsla {
    if t.mode.is_dark() {
        hex(0x131316).opacity(0.78)
    } else {
        hex(0xf7f6f4).opacity(0.85)
    }
}

/// Absolute overlays that make a sharp box read as glass: bright top + left
/// catch-light hairlines and a faint sheen fading over the top ~90px.
pub(super) fn glass_fx(t: &Theme) -> Div {
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(t.foreground.opacity(0.13)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(1.))
                .bg(t.foreground.opacity(0.10)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(88.))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(t.foreground.opacity(0.05), 0.),
                    linear_color_stop(transparent_black(), 1.),
                )),
        )
}

/// Overlay scrim: lighter in light mode so the page doesn't go muddy.
pub(super) fn scrim(t: &Theme) -> Hsla {
    hsla(0., 0., 0., if t.mode.is_dark() { 0.4 } else { 0.18 })
}

/// Palette group (sort rank, header) for a command.
pub(super) fn cmd_group(cmd: &Cmd) -> (u8, &'static str) {
    use Cmd::*;
    match cmd {
        NewThread | SearchThreads | Settle | Snooze(_) | Pin | Archive | Delete | Rename(_)
        | Stop | ShowArchived | Approve(_) => (0, "Thread"),
        Thread(_) => (1, "Switch thread"),
        OpenMessage(_, None) => (1, "Threads"),
        OpenMessage(_, Some(_)) => (2, "Messages"),
        Provider(_) | Model(_) | Effort(_) | Mode(_) => (3, "Model"),
        ToggleSidebar
        | ToggleTerminal
        | ToggleDiff
        | ToggleFiles
        | TogglePlugins
        | ToggleViewer
        | ToggleSessions
        | ToggleSkills
        | SetLayout(_)
        | SetTerminalPlacement(_) => (4, "View"),
        Checkpoint | Restore | TurnDiff | HeadDiff | Branch(_) | NewBranch(_) | Worktree(_)
        | DiscardChanges => (5, "Git"),
        Settings | SettingsSection(_) | SetNotify(_) | SetAutoAccept(_) | ToggleTheme
        | SetBackground(_) => (6, "Settings"),
        Project(_) | AddProject(_) => (7, "Project"),
        UseSkill(_) | ResumeSession(_) | Gray(_) => (8, "Gray"),
        PluginInstall(_) | PluginRemove(_) | PluginEnable(..) => (9, "Plugins"),
        OpenFile(_) | PreviewFile(_) | AttachImage(_) | RemoveAttachment(_) => (10, "Files"),
    }
}

pub(super) fn fuzzy(hay: &str, needle: &str) -> bool {
    if hay.contains(needle) {
        return true;
    }
    let mut it = hay.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| it.any(|h| h == c))
}

pub(super) fn provider_brand(provider: &str) -> Option<&'static str> {
    match provider {
        "gray" => Some("icons/gray-logo.svg"),
        "claude" => Some("icons/claude.svg"),
        "codex" => Some("icons/openai.svg"),
        _ => None,
    }
}

pub(super) fn provider_chip(
    id: impl Into<ElementId>,
    provider: &str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let mut c = h_flex()
        .id(id)
        .gap_1()
        .h(px(22.))
        .px(px(7.))
        .text_size(px(12.))
        .text_color(t.muted_foreground)
        .cursor_pointer()
        .hover(|s| s.bg(t.foreground.opacity(0.06)).text_color(t.foreground))
        .active(|s| s.bg(t.foreground.opacity(0.10)));
    c = match provider_brand(provider) {
        Some(path) => c.child(
            svg()
                .path(path.to_string())
                .size_3()
                .text_color(t.muted_foreground),
        ),
        None => c.child(Icon::new(GIcon::Bot).xsmall()),
    };
    c.child(label.into())
}

pub(super) fn chip(
    id: impl Into<ElementId>,
    icon: GIcon,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    h_flex()
        .id(id)
        .gap_1()
        .h(px(22.))
        .px(px(7.))
        .text_size(px(12.))
        .text_color(t.muted_foreground)
        .cursor_pointer()
        .hover(|s| s.bg(t.foreground.opacity(0.06)).text_color(t.foreground))
        .active(|s| s.bg(t.foreground.opacity(0.10)))
        .child(Icon::new(icon).xsmall())
        .child(label.into())
}

/// Message rise-in (opacity + 10px), only for items that arrived after the
/// thread was opened. Keyed by item id so it plays once per mount.
pub(super) fn rise(el: AnyElement, item: &Item, new: bool) -> AnyElement {
    if !new {
        return el;
    }
    motion::enter(
        v_flex().w_full().child(el),
        SharedString::from(format!("rise-{}", item_id(item))),
        motion::RISE_MS,
        -10.,
        motion::ease_out(),
    )
    .into_any_element()
}

/// Opacity for secondary text: dark mode keeps the subtle value; light mode
/// floors it so it stays readable (>= ~3:1) on a white background.
pub(super) fn faint(t: &Theme, dark: f32) -> f32 {
    if t.mode.is_dark() {
        dark
    } else {
        dark.max(0.6)
    }
}

/// The keystroke actually bound to `action`, formatted for display
/// ("Ctrl+Shift+F"), so hints can't drift from `bind_keys`.
pub(super) fn key_hint(action: &dyn Action, cx: &App) -> Option<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap.bindings_for_action(action).last()?;
    let ks = binding.keystrokes().first()?;
    Some(Kbd::format(ks.as_keystroke()))
}

/// A keycap chip for a formatted binding ("Ctrl+Shift+F" -> Ctrl Shift F).
/// `on_accent` inverts it for the solid amber selection bar.
pub(super) fn key_chip(keys: &str, t: &Theme, on_accent: bool) -> Div {
    let mut row = h_flex().gap_0p5().flex_none();
    for part in keys.split('+') {
        row = row.child(
            div()
                .min_w(px(18.))
                .px(px(5.))
                .bg(if on_accent {
                    t.primary_foreground.opacity(0.12)
                } else {
                    t.foreground.opacity(0.06)
                })
                .border_1()
                .border_color(if on_accent {
                    t.primary_foreground.opacity(0.25)
                } else {
                    t.foreground.opacity(0.07)
                })
                .font_family(t.mono_font_family.clone())
                .text_size(px(11.))
                .text_center()
                .text_color(if on_accent {
                    t.primary_foreground
                } else {
                    t.primary
                })
                .child(part.to_string()),
        );
    }
    row
}

pub(super) fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

pub fn apply_brand(cx: &mut App) {
    let dark = cx.theme().mode.is_dark();
    let t = Theme::global_mut(cx);
    t.font_family = "Plus Jakarta Sans".into();
    t.mono_font_family = "Departure Mono".into();
    t.font_size = px(14.);
    t.mono_font_size = px(12.5);
    t.radius = px(0.);
    t.radius_lg = px(0.);
    let accent = hex(0xd4a373);
    t.primary = accent;
    t.primary_hover = hex(0xdfb891);
    t.primary_active = hex(0xe8cdb0);
    t.ring = accent;
    t.caret = accent;
    t.button_primary = accent;
    t.button_primary_hover = hex(0xdfb891);
    t.button_primary_active = hex(0xe8cdb0);
    t.danger = hex(0xb7513f);
    t.warning = accent;
    t.primary_foreground = hex(0x050506);
    t.button_primary_foreground = hex(0x050506);
    // One neutral ink at fixed alphas for every line and fill (monocode's trick).
    let (bg, ink, side, pop, a) = if dark {
        (
            hex(0x050506),
            hex(0xeceae7),
            hex(0x040405),
            hex(0x0b0b0c),
            [0.05, 0.06, 0.07, 0.10, 0.12, 0.15],
        )
    } else {
        (
            hex(0xf7f6f4),
            hex(0x2b2a28),
            hex(0xefeeeb),
            hex(0xfdfcfb),
            [0.05, 0.06, 0.07, 0.10, 0.12, 0.14],
        )
    };
    let solid = |f: f32| bg.blend(ink.opacity(f));
    t.background = bg;
    t.foreground = ink;
    t.muted = solid(a[1]);
    t.muted_foreground = if dark { hex(0x8b8a86) } else { hex(0x75736f) };
    t.border = ink.opacity(a[2]);
    t.input = ink.opacity(a[3] + 0.04);
    t.popover = pop;
    t.popover_foreground = ink;
    t.secondary = ink.opacity(a[1]);
    t.secondary_hover = ink.opacity(a[4]);
    t.secondary_active = ink.opacity(a[5]);
    t.secondary_foreground = ink;
    t.accent = ink.opacity(a[3]);
    t.accent_foreground = ink;
    t.sidebar = side;
    t.sidebar_foreground = ink;
    t.sidebar_border = ink.opacity(a[2]);
    t.sidebar_accent = ink.opacity(a[3]);
    t.sidebar_accent_foreground = ink;
    t.title_bar = bg;
    t.title_bar_border = ink.opacity(a[2]);
    t.status_bar = side;
    t.status_bar_border = ink.opacity(a[2]);
    t.list_hover = ink.opacity(a[0]);
    t.list_active = ink.opacity(a[3]);
    t.list_active_border = accent;
    t.tab_active = ink.opacity(a[3]);
    t.tab_active_foreground = ink;
    t.selection = accent.opacity(0.30);
    t.tokens = gpui_kit::component::ThemeTokens::from(&t.colors);
}
