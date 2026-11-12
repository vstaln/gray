use super::*;

/// Hairline for floating surfaces — a step up from the page hairline.
pub(super) fn float_border(t: &Theme) -> Hsla {
    t.foreground.opacity(0.24)
}

pub(super) fn glass_bg(t: &Theme) -> Hsla {
    // Nearly opaque: without a backdrop blur (X11), lower alphas let the
    // transcript text read through palettes and popovers.
    if t.mode.is_dark() {
        hex(0x232325).opacity(0.95)
    } else {
        hex(0xf7f6f4).opacity(0.96)
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

/// Lineless divider for the main split: paints nothing at rest, a soft pill
/// when the pointer owns the handle (hit band + cursor stay with gpui-base).
pub(super) fn lineless_handle() -> base::ResizeHandleRenderer {
    std::rc::Rc::new(|handle, _w, cx| {
        let engaged = handle.state() != base::ResizeHandleState::Idle;
        Some(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .when(engaged, |s| {
                    s.child(
                        div()
                            .w(px(3.))
                            .h(px(24.))
                            .rounded_full()
                            .bg(cx.theme().foreground.opacity(0.18)),
                    )
                })
                .into_any_element(),
        )
    })
}

/// Palette group (sort rank, header) for a command.
pub(super) fn cmd_group(cmd: &Cmd) -> (u8, &'static str) {
    use Cmd::*;
    match cmd {
        NewThread | SearchThreads | Settle | Snooze(_) | Pin | Archive | Delete | Rename(_)
        | Stop | ShowArchived | Approve(_) | AskPick(..) | AskSubmit => (0, "Thread"),
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
        | SetTerminalPlacement(_)
        | SplitPane
        | SplitTo(_)
        | ClosePane
        | FocusPaneDir(_)
        | SwapPaneDir(_)
        | ToggleZoom
        | ReopenClosed
        | GoBack
        | GoForward => (4, "View"),
        CloseFile(..) | GoToLine(..) | OpenFileAt(..) => (10, "Files"),
        Checkpoint | Restore | TurnDiff | HeadDiff | Branch(_) | NewBranch(_) | Worktree(_)
        | DiscardChanges => (5, "Git"),
        Settings | SettingsSection(_) | SetNotify(_) | SetAutoAccept(_) | SetAutosave(_)
        | ToggleTheme | SetBackground(_) => (6, "Settings"),
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

/// Rank a path for a lowercase query; higher is better, None = no match.
/// Filename hits beat directory hits, contiguous beats scattered, and a
/// subsequence that falls apart into many fragments is no match at all (a
/// loose subsequence matched "session" against `assets/app-icon.png`).
pub(super) fn path_score(path: &str, q: &str) -> Option<i32> {
    let q: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let short = -(lower.len().min(200) as i32);
    if name == q || name.split('.').next() == Some(q.as_str()) {
        return Some(10_000 + short);
    }
    if name.starts_with(&q) {
        return Some(8_000 + short);
    }
    if name.contains(&q) {
        return Some(6_000 + short);
    }
    if let Some(at) = lower.find(&q) {
        let boundary = at == 0 || matches!(lower.as_bytes()[at - 1], b'/' | b'_' | b'-' | b'.');
        return Some(4_000 + if boundary { 500 } else { 0 } + short);
    }
    // Subsequence: try each start of the first char, extend preferring the
    // adjacent char, then a word-boundary hit, then the nearest; keep the
    // attempt with the fewest fragments.
    let hay: Vec<char> = lower.chars().collect();
    let qc: Vec<char> = q.chars().collect();
    let name_start = hay.len() - name.chars().count();
    let boundary = |i: usize| i == 0 || matches!(hay[i - 1], '/' | '_' | '-' | '.' | ' ');
    let mut best: Option<(usize, i32)> = None;
    for start in (0..hay.len()).filter(|&i| hay[i] == qc[0]) {
        let mut runs = 1;
        let mut bonus = if boundary(start) { 40 } else { 0 };
        let mut last = start;
        let mut ok = true;
        for &c in &qc[1..] {
            let pos = if hay.get(last + 1) == Some(&c) {
                last + 1
            } else {
                let next = (last + 1..hay.len()).filter(|&i| hay[i] == c);
                match next
                    .clone()
                    .find(|&i| boundary(i))
                    .or_else(|| next.clone().next())
                {
                    Some(p) => {
                        runs += 1;
                        if boundary(p) {
                            bonus += 40;
                        }
                        p
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            };
            if pos >= name_start {
                bonus += 10;
            }
            last = pos;
        }
        if ok && best.is_none_or(|(r, b)| runs < r || (runs == r && bonus > b)) {
            best = Some((runs, bonus));
        }
    }
    let (runs, bonus) = best?;
    let qlen = qc.len();
    if qlen >= 3 && runs > qlen.div_ceil(2) {
        return None;
    }
    Some(2_000 - runs as i32 * 100 + bonus + short)
}

/// Best `limit` paths for `q`, best first.
pub(super) fn rank_paths(paths: Vec<String>, q: &str, limit: usize) -> Vec<String> {
    let mut scored: Vec<(i32, String)> = paths
        .into_iter()
        .filter_map(|p| path_score(&p, q).map(|s| (s, p)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, p)| p).collect()
}

pub(super) fn provider_brand(provider: &str) -> Option<&'static str> {
    let p = provider.to_ascii_lowercase();
    let has = |f: &str| p.contains(f);
    Some(match () {
        _ if has("antigravity") => "icons/antigravity.svg",
        _ if has("claude") || has("anthropic") => "icons/claude.svg",
        _ if has("codex") => "icons/codex.svg",
        _ if has("openai") || has("chatgpt") => "icons/chatgpt.svg",
        _ if has("gemini") || has("google") => "icons/gemini.svg",
        _ if has("devin") => "icons/devin.svg",
        _ if has("deepseek") => "icons/deepseek.svg",
        _ if has("openrouter") => "icons/openrouter.svg",
        _ if has("ollama") => "icons/ollama.svg",
        _ if has("qwen") || has("dashscope") => "icons/qwen.svg",
        _ if has("grok") || has("xai") => "icons/grok.svg",
        _ if has("kimi") || has("moonshot") => "icons/kimi.svg",
        _ if has("zai") || has("zhipu") || has("glm") => "icons/zai.svg",
        _ if has("mistral") => "icons/mistral.svg",
        _ if has("meta") || has("llama") => "icons/meta.svg",
        _ if has("commandcode") => "icons/commandcode.svg",
        _ if has("opencode") => "icons/opencode.svg",
        _ if has("stepfun") || has("step-") => "icons/stepfun.svg",
        _ if has("cursor") => "icons/cursor.svg",
        _ if has("copilot") => "icons/copilot.svg",
        _ if has("github") => "icons/g/github.svg",
        _ if has("lmstudio") || has("lm-studio") => "icons/lmstudio.svg",
        _ if has("nous") => "icons/nous.svg",
        _ if has("muse") => "icons/muse.svg",
        _ if has("gmi") => "icons/gmi-cloud.svg",
        // The gray CLI itself gets the gray mark; any other provider it routes
        // to without a logo of its own gets a neutral chip, so unknown
        // providers don't all look like gray.
        _ if p == "gray" => "icons/gray-logo.svg",
        _ if crate::provider::is_gray_provider(provider) => "icons/provider.svg",
        _ => return None,
    })
}

pub(super) fn provider_chip(
    id: impl Into<ElementId>,
    provider: &str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let icon = match provider_brand(provider) {
        Some(path) => svg()
            .path(path.to_string())
            .size_3()
            .text_color(t.muted_foreground)
            .into_any_element(),
        None => Icon::new(GIcon::Bot).xsmall().into_any_element(),
    };
    chip(id, icon, label, cx)
}

pub(super) fn chip(
    id: impl Into<ElementId>,
    icon: impl IntoElement,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    h_flex()
        .id(id)
        .gap_1()
        .h(px(20.))
        .px(px(6.))
        .text_size(px(11.5))
        .text_color(t.muted_foreground.opacity(faint(t, 0.9)))
        .cursor_pointer()
        .hover(|s| s.bg(t.foreground.opacity(0.06)).text_color(t.foreground))
        .active(|s| s.bg(t.foreground.opacity(0.10)))
        .child(icon)
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
    t.mono_font_family = "CommitMono".into();
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
            hex(0x1c1c1d),
            hex(0xeceae7),
            hex(0x18181a),
            hex(0x232325),
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
