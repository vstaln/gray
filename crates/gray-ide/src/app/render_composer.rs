use super::*;

impl GrayApp {
    pub(super) fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let (dp, dm, dmode) = self.defaults();
        let provider = thread.map(|t| t.provider.clone()).unwrap_or(dp);
        let model = thread.map(|t| t.model.clone()).unwrap_or(dm);
        let mode = thread.map(|t| t.mode.clone()).unwrap_or(dmode);
        let effort = thread
            .map(|t| t.effort.clone())
            .unwrap_or_else(|| "medium".into());
        let running = thread
            .map(|t| self.turns.contains_key(&t.id))
            .unwrap_or(false);
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let pop_open = self.composer_pop.is_some() && self.composer_token(cx).is_some();
        let card = div()
            .w_full()
            .max_w(px(896.))
            .mx_auto()
            .px_4()
            .pb(px(14.))
            .child(
                v_flex()
                    .relative()
                    .when(self.model_open, |s| s.child(self.render_model_picker(cx)))
                    .when(pop_open, |s| {
                        s.capture_action(
                            cx.listener(|this, _: &MoveUp, _, cx| this.pop_move(-1, cx)),
                        )
                        .capture_action(
                            cx.listener(|this, _: &MoveDown, _, cx| this.pop_move(1, cx)),
                        )
                        .capture_action(cx.listener(|this, _: &Dismiss, _, cx| {
                            this.composer_pop = None;
                            cx.notify();
                            cx.stop_propagation();
                        }))
                        .child(self.render_composer_pop(cx))
                    })
                    .border_1()
                    .border_color(t.foreground.opacity(if focused { 0.20 } else { 0.10 }))
                    .bg(t.popover)
                    .children(
                        self.attachments
                            .iter()
                            .enumerate()
                            .map(|(i, path)| {
                                h_flex()
                                    .id(SharedString::from(format!("attachment-{i}")))
                                    .px_2()
                                    .pt_2()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .h(px(44.))
                                            .border_1()
                                            .border_color(t.border)
                                            .overflow_hidden()
                                            .child(
                                                img(path.clone())
                                                    .w(px(44.))
                                                    .h(px(44.))
                                                    .object_fit(ObjectFit::Cover),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(t.muted_foreground)
                                            .child(
                                                path.file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_default(),
                                            ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "attach-remove-{i}"
                                        )))
                                        .ghost()
                                        .xsmall()
                                        .icon(GIcon::Close)
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.run(Cmd::RemoveAttachment(i), window, cx)
                                            }),
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>(),
                    )
                    .child(
                        div()
                            .px_2()
                            .pt_2()
                            .child(Textarea::new(&self.composer).appearance(false)),
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .pb_2()
                            .gap_1()
                            .child(
                                provider_chip(
                                    "composer-model",
                                    &provider,
                                    format!(
                                        "{} · {} · {}",
                                        provider::provider_label(&provider),
                                        model,
                                        effort
                                    ),
                                    cx,
                                )
                                .when(self.model_open, |c| {
                                    c.bg(t.foreground.opacity(0.08)).text_color(t.foreground)
                                })
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        // A click outside just closed the picker: that same
                                        // click landing on the chip must not reopen it.
                                        if this.model_closed_at.take().is_some_and(|at| {
                                            at.elapsed() < Duration::from_millis(400)
                                        }) {
                                            return;
                                        }
                                        this.open_model_picker(window, cx)
                                    },
                                )),
                            )
                            .child(
                                chip(
                                    "composer-mode",
                                    GIcon::TriangleAlert,
                                    provider::mode_label(&mode),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.open_palette(
                                            PaletteMode::Commands,
                                            "Mode:",
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .when_some(self.branch.clone(), |s, b| {
                                s.child(chip("composer-branch", GIcon::Github, b, cx).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.open_palette(
                                            PaletteMode::Commands,
                                            "Branch:",
                                            window,
                                            cx,
                                        )
                                    }),
                                ))
                            })
                            .child(div().flex_1())
                            .child(if running {
                                Button::new("stop")
                                    .small()
                                    .danger()
                                    .icon(GIcon::Square)
                                    .tooltip("Stop (Ctrl+Shift+X)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run(Cmd::Stop, window, cx)
                                    }))
                            } else {
                                Button::new("send")
                                    .small()
                                    .primary()
                                    .icon(GIcon::ArrowUp)
                                    .tooltip("Send (Enter)")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                                    )
                            }),
                    ),
            );
        // First send: slide from the centered welcome slot down into the dock.
        if self
            .dock_slide_at
            .is_some_and(|at| at.elapsed() < motion::ms(motion::DOCK_MS + 200))
        {
            let tid = self.active_thread.clone().unwrap_or_default();
            let travel = (f32::from(window.viewport_size().height) * 0.35).clamp(160., 420.);
            motion::enter(
                card,
                SharedString::from(format!("dock-in-{tid}")),
                motion::DOCK_MS,
                travel,
                motion::ease_out(),
            )
            .into_any_element()
        } else {
            card.into_any_element()
        }
    }

    /// Anchored popover above the composer for the `@`/`/` token under the
    /// caret. Same row styling and arrow-key nav as the palette.
    pub(super) fn render_composer_pop(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut list = v_flex()
            .id("composer-pop-list")
            .track_scroll(&self.pop_scroll)
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1();
        let empty = |msg: &'static str| {
            div()
                .px_2()
                .py_3()
                .text_sm()
                .text_color(t.muted_foreground)
                .child(msg)
        };
        let row = |i: usize, selected: bool, cx: &mut Context<Self>| {
            h_flex()
                .id(("pop-item", i))
                .h(px(32.))
                .flex_shrink_0()
                .px(px(10.))
                .gap_2()
                .text_size(px(13.))
                .cursor_pointer()
                .when(selected, |s| {
                    s.bg(t.primary).text_color(t.primary_foreground)
                })
                .when(!selected, |s| s.hover(|s| s.bg(t.foreground.opacity(0.05))))
                .active(|s| s.bg(t.primary_active))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.pop_sel != i {
                        this.pop_sel = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pop_sel = i;
                    this.accept_pop(window, cx);
                }))
        };
        let mut child_ix = 0usize;
        let mut sel_child = 0usize;
        match self.composer_token(cx) {
            Some((PopKind::Mention, _, q)) => {
                let items = self.mention_items(&q);
                if items.is_empty() {
                    list = list.child(empty("No matching files"));
                }
                let sel = self.pop_sel.min(items.len().saturating_sub(1));
                for (i, path) in items.into_iter().enumerate() {
                    if i == sel {
                        sel_child = child_ix;
                    }
                    child_ix += 1;
                    list = list.child(
                        row(i, i == sel, cx)
                            .child(
                                Icon::new(GIcon::File)
                                    .small()
                                    .text_color(t.muted_foreground),
                            )
                            .child(div().flex_1().truncate().child(path)),
                    );
                }
            }
            Some((PopKind::Slash, _, q)) => {
                let items = self.slash_items(&q);
                if items.is_empty() {
                    list = list.child(empty("No matching commands"));
                }
                let sel = self.pop_sel.min(items.len().saturating_sub(1));
                let mut last_group = None;
                for (i, cmd) in items.into_iter().enumerate() {
                    if last_group != Some(cmd.group) {
                        last_group = Some(cmd.group);
                        list = list.child(
                            div()
                                .px(px(10.))
                                .pt_2()
                                .pb_1()
                                .text_size(px(10.))
                                .font_medium()
                                .text_color(t.muted_foreground.opacity(faint(&t, 0.55)))
                                .child(cmd.group.to_uppercase()),
                        );
                        child_ix += 1;
                    }
                    if i == sel {
                        sel_child = if i == 0 { 0 } else { child_ix };
                    }
                    child_ix += 1;
                    list = list.child(
                        row(i, i == sel, cx)
                            .child(div().flex_1().truncate().child(format!("/{}", cmd.name)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .max_w(px(220.))
                                    .truncate()
                                    .child(cmd.hint),
                            ),
                    );
                }
            }
            None => {}
        }
        if self.pop_reveal.take() {
            self.pop_scroll.scroll_to_item(sel_child);
        }
        let panel = v_flex()
            .id("composer-pop")
            .occlude()
            .w(px(420.))
            .max_h(px(340.))
            .relative()
            .border_1()
            .border_color(float_border(&t))
            .bg(glass_bg(&t))
            .shadow(hard_shadow(&t))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.composer_pop = None;
                cx.notify();
            }))
            .child(list)
            .child(glass_fx(&t));
        div()
            .absolute()
            .bottom_full()
            .left(px(8.))
            .pb(px(6.))
            .child(
                deferred(motion::enter(
                    panel,
                    "composer-pop-in",
                    motion::POPOVER_MS,
                    -6.,
                    motion::ease_pop(),
                ))
                .with_priority(1),
            )
    }
}
