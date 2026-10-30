use super::*;

impl GrayApp {
    /// Welcome state: no items and no running turn; the composer sits in the
    /// centered slot instead of the dock.
    pub(super) fn timeline_empty(&self) -> bool {
        match self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id))
        {
            Some(th) => th.items.is_empty() && !self.turns.contains_key(&th.id),
            None => true,
        }
    }

    pub(super) fn render_timeline(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let base = {
            let mut b = self.rise_base.borrow_mut();
            match (thread, b.as_ref()) {
                (Some(th), Some((id, n))) if *id == th.id => *n,
                (Some(th), _) => {
                    *b = Some((th.id.clone(), th.items.len()));
                    th.items.len()
                }
                (None, _) => {
                    *b = None;
                    usize::MAX
                }
            }
        };
        let empty = self.timeline_empty();
        if empty {
            let welcome_composer = self.render_composer(window, cx);
            let project = self
                .active_project
                .as_ref()
                .and_then(|id| self.store.state.project(id))
                .map(|p| p.name.clone());
            return motion::enter(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .pb(px(40.))
                    .child(
                        svg()
                            .path("icons/gray-logo.svg")
                            .size(px(52.))
                            .text_color(t.foreground.opacity(0.9)),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(26.))
                            .font_family("Brygada 1918")
                            .child(motion::decode("welcome-title", "Gray", motion::WELCOME_MS)),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(t.foreground.opacity(0.55))
                            .child(match project {
                                Some(p) => format!("What should we build in {p}?"),
                                None => "Open a project to get started".to_string(),
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(640.))
                            .mt(px(14.))
                            .child(welcome_composer),
                    ),
                "welcome-in",
                motion::WELCOME_MS,
                -8.,
                motion::ease_out(),
            )
            .into_any_element();
        }
        let thread = thread.unwrap();
        let mut col = v_flex()
            .id("timeline-items")
            .track_scroll(&self.items_scroll)
            .w_full()
            .max_w(px(896.))
            .mx_auto()
            .px_4()
            .pt(px(20.))
            .pb(px(28.))
            .gap(px(14.));
        let running = self.turns.contains_key(&thread.id);
        for row in timeline_rows(&thread.items) {
            col = col.child(match row {
                Row::Item(ix) => {
                    let item = &thread.items[ix];
                    let assistant =
                        matches!(item, Item::Message { role, .. } if *role != Role::User);
                    // The final reply replaces already-visible streamed text: no rise.
                    let new = ix >= base && !assistant;
                    let fresh = new && now() - item_at(item) <= motion::FRESH_MS;
                    rise(render_item(item, &t, fresh), item, new)
                }
                Row::Group(r) => self.render_group(&thread.items, r, running, base, &t, cx),
            });
        }
        if let Some(a) = self.turns.get(&thread.id) {
            col = col.child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(t.muted_foreground.opacity(faint(&t, 0.55)))
                            .child(motion::churner(&t))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(motion::shimmer_label("churn-label", "Working", &t))
                                    .child(motion::flap(
                                        &format!(
                                            "churn-elapsed-{}",
                                            a.started.elapsed().as_secs() / 60
                                        ),
                                        &format!(
                                            "· {}",
                                            fmt_elapsed(a.started.elapsed().as_secs() as i64)
                                        ),
                                        &t,
                                    )),
                            ),
                    )
                    .map(|s| {
                        if a.text.is_empty() {
                            s.child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(motion::typing_dots(&t))
                                    .child(motion::breathe(
                                        div()
                                            .text_sm()
                                            .text_color(t.muted_foreground)
                                            .child("Thinking"),
                                        "think-pulse",
                                    )),
                            )
                        } else {
                            s.child(
                                TextView::markdown("streaming", a.text.clone())
                                    .font_family("Plus Jakarta Sans")
                                    .motion(motion::word_fade())
                                    .selectable(true),
                            )
                        }
                    }),
            );
        }
        let bg = t.background;
        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("timeline")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(col),
            )
            .when(!self.follow, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(48.))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(bg.opacity(0.), 0.),
                            linear_color_stop(bg, 1.),
                        )),
                )
                .child(
                    h_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(14.))
                        .justify_center()
                        .child(motion::enter(
                            h_flex()
                                .id("jump-latest")
                                .h(px(28.))
                                .px(px(11.))
                                .gap(px(6.))
                                .cursor_pointer()
                                .bg(t.popover)
                                .border_1()
                                .border_color(float_border(&t))
                                .shadow(hard_shadow(&t))
                                .text_size(px(12.))
                                .text_color(t.foreground.opacity(0.75))
                                .hover(|s| s.bg(t.secondary_hover))
                                .active(|s| s.bg(t.foreground.opacity(0.10)))
                                .child(Icon::new(GIcon::ArrowDown).size(px(13.)))
                                .child("Jump to latest")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.follow = true;
                                    this.scroll.scroll_to_bottom();
                                    cx.notify();
                                })),
                            "jump-latest-in",
                            motion::CARD_MS,
                            -6.,
                            motion::ease_out(),
                        )),
                )
            })
            .into_any_element()
    }

    /// A run of tool activity folded under one "Worked for …" header.
    pub(super) fn render_group(
        &self,
        items: &[Item],
        r: std::ops::Range<usize>,
        running: bool,
        base: usize,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let g = group_info(items, &r, running);
        let open = self.group_open.get(&g.id).copied().unwrap_or(g.live);
        let ink = t.foreground;
        let color = if g.err {
            t.danger.opacity(0.85)
        } else {
            ink.opacity(0.55)
        };
        let label = if g.live {
            motion::shimmer_label("group-live", g.label(), t).into_any_element()
        } else {
            motion::decode(
                SharedString::from(format!("grp-done-{}", g.id)),
                &g.label(),
                motion::RISE_MS,
            )
            .into_any_element()
        };
        let gid = g.id.clone();
        let header = h_flex()
            .id(SharedString::from(format!("{}-head", g.id)))
            .gap(px(6.))
            .py(px(2.))
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(color)
            .hover(|s| s.text_color(if g.err { t.danger } else { ink.opacity(0.8) }))
            .active(|s| s.bg(t.foreground.opacity(0.10)))
            .child(
                Icon::new(if open {
                    GIcon::ChevronDown
                } else {
                    GIcon::ChevronRight
                })
                .size(px(14.)),
            )
            .when(g.err, |h| h.child(Icon::new(GIcon::CircleX).size(px(13.))))
            .child(label)
            .child(div().text_color(ink.opacity(faint(t, 0.32))).child(format!(
                "· {} {}",
                g.steps,
                if g.steps == 1 { "step" } else { "steps" }
            )))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.group_open.insert(gid.clone(), !open);
                cx.notify();
            }));
        let mut body = v_flex()
            .gap(px(4.))
            .ml(px(6.))
            .mt(px(4.))
            .pl(px(13.))
            .border_l_1()
            .border_color(ink.opacity(0.08));
        for ix in r.clone() {
            body = body.child(rise(
                render_item(&items[ix], t, false),
                &items[ix],
                ix >= base,
            ));
        }
        let header = rise(header.into_any_element(), &items[r.start], r.start >= base);
        Collapsible::new()
            .open(open)
            .motion_id(SharedString::from(g.id.clone()))
            .child(header)
            .content(body)
            .into_any_element()
    }
}
