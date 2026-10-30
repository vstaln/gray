use super::*;

impl GrayApp {
    pub(super) fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut icons = v_flex()
            .id("rail-scroll")
            .flex_1()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .items_center();
        for p in &self.store.state.projects {
            let pid = p.id.clone();
            let active = Some(&p.id) == self.active_project.as_ref();
            icons = icons.child(
                Button::new(SharedString::from(format!("rail-project-{}", p.id)))
                    .ghost()
                    .small()
                    .icon(if active {
                        GIcon::FolderOpen
                    } else {
                        GIcon::Folder
                    })
                    .tooltip(p.name.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Project(pid.clone()), window, cx)
                    })),
            );
            if active {
                let threads: Vec<_> = self
                    .store
                    .state
                    .threads_for(&p.id, self.show_archived)
                    .into_iter()
                    .filter(|t| self.show_archived || t.snoozed_until <= now())
                    .collect();
                for th in threads {
                    let id = th.id.clone();
                    let sel = Some(&th.id) == self.active_thread.as_ref();
                    let color = if self.turns.contains_key(&th.id) {
                        t.info
                    } else if th.last_status == "error" {
                        t.danger
                    } else if th.last_status == "completed" {
                        t.success
                    } else {
                        t.muted_foreground
                    };
                    icons = icons.child(
                        div()
                            .id(SharedString::from(format!("rail-thread-{}", th.id)))
                            .w(px(10.))
                            .h(px(10.))
                            .bg(if sel { color } else { color.opacity(0.55) })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::Thread(id.clone()), window, cx)
                            })),
                    );
                }
            }
        }
        let icon_btn =
            |id: &'static str, icon: GIcon, tip: &'static str, cmd: Cmd, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .small()
                    .icon(icon)
                    .tooltip(tip)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)),
                    )
            };
        v_flex()
            .w(px(52.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .child(icons)
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .items_center()
                    .border_t_1()
                    .border_color(t.sidebar_border)
                    .child(icon_btn(
                        "rail-new",
                        GIcon::Plus,
                        "New thread",
                        Cmd::NewThread,
                        cx,
                    ))
                    .child(
                        Button::new("rail-palette")
                            .ghost()
                            .small()
                            .icon(GIcon::Search)
                            .tooltip("Commands (Ctrl+K)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_palette(PaletteMode::Commands, "", window, cx)
                            })),
                    )
                    .child(icon_btn(
                        "rail-files",
                        GIcon::FileText,
                        "Files (Ctrl+E)",
                        Cmd::ToggleFiles,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-diff",
                        GIcon::Copy,
                        "Diff (Ctrl+D)",
                        Cmd::ToggleDiff,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-terminal",
                        GIcon::SquareTerminal,
                        "Terminal (Ctrl+J)",
                        Cmd::ToggleTerminal,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-plugins",
                        GIcon::Star,
                        "Plugins (Ctrl+Shift+P)",
                        Cmd::TogglePlugins,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-sessions",
                        GIcon::Inbox,
                        "Sessions",
                        Cmd::ToggleSessions,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-skills",
                        GIcon::BookOpen,
                        "Skills",
                        Cmd::ToggleSkills,
                        cx,
                    ))
                    .child(icon_btn(
                        "rail-settings",
                        GIcon::Settings,
                        "Settings (Ctrl+,)",
                        Cmd::Settings,
                        cx,
                    )),
            )
    }

    pub(super) fn render_terminal(&self, side: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut lines = v_flex().font_family(t.mono_font_family.clone()).text_xs();
        for l in self.terminal_lines.iter().rev().take(300).rev() {
            lines = lines.child(div().child(l.clone()));
        }
        let mut outer = v_flex().flex_none().border_color(t.border).bg(t.background);
        outer = if side {
            outer.w(px(300.)).h_full().border_l_1()
        } else {
            outer.h(px(240.)).border_t_1()
        };
        outer
            .child(
                h_flex()
                    .h(px(32.))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .border_b_1()
                    .border_color(t.border)
                    .child(Icon::new(GIcon::SquareTerminal).xsmall())
                    .child("Terminal")
                    .when(!self.terminal_rx.is_empty(), |s| {
                        s.child(motion::running_bars(&t))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("close-terminal")
                            .ghost()
                            .xsmall()
                            .icon(GIcon::Close)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ToggleTerminal, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("terminal-output")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .child(lines),
            )
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .child(Input::new(&self.terminal_input).prefix(div().text_xs().child("$"))),
            )
    }

    pub(super) fn render_panel(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let header = h_flex()
            .h(px(40.))
            .px_2()
            .gap_1()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .child(
                Button::new("panel-files")
                    .ghost()
                    .small()
                    .label("Files")
                    .when(panel == Panel::Files, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Files);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-sessions")
                    .ghost()
                    .small()
                    .label("Sessions")
                    .when(panel == Panel::Sessions, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Sessions);
                        this.gray_sessions = sessions::list();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-viewer")
                    .ghost()
                    .small()
                    .label("Viewer")
                    .when(panel == Panel::Viewer, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Viewer);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-plugins")
                    .ghost()
                    .small()
                    .label("Plugins")
                    .when(panel == Panel::Plugins, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Plugins);
                        plugins::refresh(this.plugin_tx.clone());
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-diff")
                    .ghost()
                    .small()
                    .label("Diff")
                    .when(panel == Panel::Diff, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Some(Panel::Diff);
                        this.refresh_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("panel-refresh")
                    .ghost()
                    .small()
                    .icon(GIcon::RefreshCw)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("panel-close")
                    .ghost()
                    .small()
                    .icon(GIcon::Close)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = None;
                        cx.notify();
                    })),
            );
        let body: AnyElement = match panel {
            Panel::Sessions => self.render_sessions(cx).into_any_element(),
            Panel::Skills => self.render_skills(cx).into_any_element(),
            Panel::Viewer => self.render_viewer(cx).into_any_element(),
            Panel::Plugins => self.render_plugins(cx).into_any_element(),
            Panel::Diff => match &self.diff {
                Some(state) => Diff::new(state).flex_1().min_h_0().into_any_element(),
                None => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No changes")
                    .into_any_element(),
            },
            Panel::Files => {
                let mut tree = v_flex()
                    .id("file-tree")
                    .w(px(220.))
                    .h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .py_1()
                    .border_r_1()
                    .border_color(t.border);
                for e in &self.tree {
                    let rel = e.rel.clone();
                    let is_dir = e.is_dir;
                    let open = self.expanded.contains(&e.rel);
                    let active = self
                        .active_tab
                        .and_then(|i| self.tabs.get(i))
                        .map(|t| t.path == e.rel)
                        .unwrap_or(false);
                    tree = tree.child(
                        h_flex()
                            .id(SharedString::from(format!("file-{}", e.rel)))
                            .pl(px(8. + e.depth as f32 * 12.))
                            .pr_2()
                            .py_0p5()
                            .gap_1p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(active, |s| s.bg(t.primary).text_color(t.primary_foreground))
                            .hover(|s| s.bg(t.list_hover))
                            .active(|s| s.bg(t.foreground.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if is_dir {
                                    if !this.expanded.remove(&rel) {
                                        this.expanded.insert(rel.clone());
                                    }
                                    this.refresh_workspace(cx);
                                } else {
                                    this.preview_file(&rel, window, cx);
                                }
                                cx.notify();
                            }))
                            .child(
                                Icon::new(if is_dir {
                                    if open {
                                        GIcon::FolderOpen
                                    } else {
                                        GIcon::Folder
                                    }
                                } else {
                                    GIcon::File
                                })
                                .xsmall(),
                            )
                            .child(div().truncate().child(e.name.clone())),
                    );
                }
                let mut tabs = h_flex()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(t.border)
                    .overflow_x_hidden();
                for (i, tab) in self.tabs.iter().enumerate() {
                    let dirty = tab.editor.read(cx).value().as_ref() != tab.saved.as_str();
                    let name = Path::new(&tab.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let active = Some(i) == self.active_tab;
                    tabs = tabs.child(
                        h_flex()
                            .id(SharedString::from(format!("tab-{}", tab.path)))
                            .h_full()
                            .px_3()
                            .gap_1()
                            .text_xs()
                            .border_r_1()
                            .border_color(t.border)
                            .cursor_pointer()
                            .when(active, |s| {
                                s.bg(t.tab_active).text_color(t.tab_active_foreground)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_tab = Some(i);
                                cx.notify();
                            }))
                            .child(name)
                            .when(dirty, |s| s.child(div().size_1p5().bg(t.warning)))
                            .child(
                                div()
                                    .id(SharedString::from(format!("tab-close-{i}")))
                                    .ml_1()
                                    .text_color(t.muted_foreground)
                                    .hover(|s| s.text_color(t.foreground))
                                    .active(|s| s.bg(t.foreground.opacity(0.10)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if i < this.tabs.len() {
                                            this.tabs.remove(i);
                                        }
                                        this.active_tab = if this.tabs.is_empty() {
                                            None
                                        } else {
                                            Some(i.min(this.tabs.len() - 1))
                                        };
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    );
                }
                let editor: AnyElement = match self.active_tab.and_then(|i| self.tabs.get(i)) {
                    Some(tab) => div()
                        .flex_1()
                        .min_h_0()
                        .child(Editor::new(&tab.editor).h_full().bordered(false))
                        .into_any_element(),
                    None => v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(t.muted_foreground)
                        .child("Select a file or press Ctrl+P")
                        .into_any_element(),
                };
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(tree)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(tabs)
                            .child(editor),
                    )
                    .into_any_element()
            }
        };
        motion::enter_x(
            v_flex()
                .w(px(if panel == Panel::Files { 640. } else { 560. }))
                .h_full()
                .flex_none()
                .border_l_1()
                .border_color(t.border)
                .bg(t.background)
                .child(header)
                .child(body),
            SharedString::from(format!("panel-in-{panel:?}")),
            260,
            24.,
            motion::ease_out(),
        )
    }

    pub(super) fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let items = self.model_items();
        let count = items.len();
        let sel = self.model_sel.min(count.saturating_sub(1));
        let (cp, cm) = self.current_model();
        let rail_btn = |id: &'static str, active: bool, icon: AnyElement, tip: &'static str| {
            div()
                .id(SharedString::from(format!("model-rail-{id}")))
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(if active {
                    t.foreground
                } else {
                    t.muted_foreground
                })
                .when(active, |s| s.bg(t.foreground.opacity(0.08)))
                .hover(|s| s.bg(t.foreground.opacity(0.05)).text_color(t.foreground))
                .active(|s| s.bg(t.foreground.opacity(0.10)))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .child(icon)
        };
        let mut rail = v_flex()
            .w(px(44.))
            .flex_shrink_0()
            .p(px(6.))
            .gap_1()
            .border_r_1()
            .border_color(t.foreground.opacity(0.08))
            .child(
                rail_btn(
                    "all",
                    self.model_rail.is_none(),
                    Icon::new(GIcon::LayoutDashboard)
                        .size(px(16.))
                        .into_any_element(),
                    "All providers",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model_rail = None;
                    this.model_sel = 0;
                    this.model_reveal.set(true);
                    cx.notify();
                })),
            );
        for p in PROVIDERS {
            let icon = match provider_brand(p.id) {
                Some(path) => svg()
                    .path(path.to_string())
                    .size(px(16.))
                    .text_color(t.muted_foreground)
                    .into_any_element(),
                None => Icon::new(GIcon::Bot).size(px(16.)).into_any_element(),
            };
            let id = p.id;
            rail = rail.child(
                rail_btn(id, self.model_rail == Some(id), icon, p.label).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.model_rail = Some(id);
                        this.model_sel = 0;
                        this.model_reveal.set(true);
                        cx.notify();
                    },
                )),
            );
        }
        let mut list = v_flex()
            .id("model-list")
            .track_scroll(&self.model_scroll)
            .flex_1()
            .max_h(px(380.))
            .overflow_y_scroll()
            .p_1();
        if items.is_empty() {
            list = list.child(
                div()
                    .px(px(10.))
                    .py_3()
                    .text_size(px(13.))
                    .text_color(t.muted_foreground)
                    .child("No models match"),
            );
        }
        let mut last = None;
        let (mut child_ix, mut sel_child) = (0, 0);
        for (i, (p, m)) in items.into_iter().enumerate() {
            if last != Some(p) {
                last = Some(p);
                child_ix += 1;
                list = list.child(
                    div()
                        .px(px(10.))
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .font_medium()
                        .text_color(t.muted_foreground.opacity(faint(&t, 0.55)))
                        .child(provider::provider_label(p).to_uppercase()),
                );
            }
            let selected = i == sel;
            if selected && i > 0 {
                sel_child = child_ix;
            }
            child_ix += 1;
            let current = p == cp && m == cm;
            list = list.child(
                h_flex()
                    .id(("model-item", i))
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
                        if this.model_sel != i {
                            this.model_sel = i;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick_model(i, window, cx)),
                    )
                    .child(div().flex_1().truncate().child(m))
                    .when(current, |s| {
                        s.child(
                            Icon::new(GIcon::Check)
                                .size(px(14.))
                                .text_color(if selected {
                                    t.primary_foreground
                                } else {
                                    t.foreground.opacity(0.55)
                                }),
                        )
                    }),
            );
        }
        if self.model_reveal.take() {
            self.model_scroll.scroll_to_item(sel_child);
        }
        let panel = h_flex()
            .id("model-picker")
            .occlude()
            .w(px(340.))
            .items_stretch()
            .relative()
            .border_1()
            .border_color(float_border(&t))
            .bg(glass_bg(&t))
            .shadow(hard_shadow(&t))
            .overflow_hidden()
            .capture_action(cx.listener(move |this, _: &MoveUp, _, cx| {
                if count > 0 {
                    this.model_sel = (this.model_sel.min(count - 1) + count - 1) % count;
                    this.model_reveal.set(true);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                if count > 0 {
                    this.model_sel = (this.model_sel.min(count - 1) + 1) % count;
                    this.model_reveal.set(true);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.model_closed_at = Some(Instant::now());
                this.close_overlays(window, cx);
            }))
            .child(rail)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .px_3()
                            .py(px(6.))
                            .text_size(px(13.))
                            .border_b_1()
                            .border_color(t.foreground.opacity(0.08))
                            .child(Input::new(&self.model_input).appearance(false).small()),
                    )
                    .child(list),
            )
            .child(glass_fx(&t));
        // Sits on the composer box's top edge, left-aligned with the chip;
        // deferred so it paints over the timeline.
        div()
            .absolute()
            .bottom_full()
            .left(px(8.))
            .pb(px(6.))
            .child(
                deferred(motion::enter(
                    panel,
                    "model-picker-in",
                    motion::POPOVER_MS,
                    -6.,
                    motion::ease_pop(),
                ))
                .with_priority(1),
            )
    }
}
