use super::pickers::FAV_TAB;
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
            .child(icons)
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .items_center()
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
        let mut outer = v_flex().flex_none().bg(t.sidebar);
        outer = if side {
            outer.w(px(300.)).h_full()
        } else {
            outer.h(px(240.))
        };
        outer
            .child(
                h_flex()
                    .h(px(32.))
                    .px_3()
                    .gap_2()
                    .text_xs()
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
                    .track_scroll(&self.terminal_scroll)
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

    pub(super) fn render_panel(
        &mut self,
        panel: Panel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut header = h_flex().h(px(40.)).px_2().gap_1().flex_none();
        for (id, label, p) in [
            ("panel-files", "Files", Panel::Files),
            ("panel-sessions", "Sessions", Panel::Sessions),
            ("panel-viewer", "Viewer", Panel::Viewer),
            ("panel-plugins", "Plugins", Panel::Plugins),
            ("panel-diff", "Diff", Panel::Diff),
        ] {
            header = header.child(
                Button::new(id)
                    .ghost()
                    .small()
                    .label(label)
                    .when(panel == p, |b| b.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.panel = Some(p);
                        match p {
                            Panel::Sessions => this.gray_sessions = sessions::list(),
                            Panel::Plugins => plugins::refresh(this.plugin_tx.clone()),
                            Panel::Diff => this.refresh_workspace(cx),
                            _ => {}
                        }
                        cx.notify();
                    })),
            );
        }
        let header = header
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
                    .bg(t.sidebar);
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
                    .id("file-tabs")
                    .h(px(32.))
                    .flex_none()
                    .track_scroll(&self.file_tabs_scroll)
                    .overflow_x_scroll();
                for (i, tab) in self.tabs.iter().enumerate() {
                    let dirty = tab.editor.read(cx).value().as_ref() != tab.saved.as_str();
                    let name = Path::new(&tab.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let path = tab.path.clone();
                    let active = Some(i) == self.active_tab;
                    tabs = tabs.child(
                        h_flex()
                            .id(SharedString::from(format!("tab-{}", tab.path)))
                            .h_full()
                            .flex_none()
                            .px_3()
                            .gap_1()
                            .text_xs()
                            .cursor_pointer()
                            .when(active, |s| {
                                s.bg(t.tab_active).text_color(t.tab_active_foreground)
                            })
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener({
                                    let path = path.clone();
                                    move |this, _, _, cx| {
                                        this.close_file(&path, None, cx);
                                        cx.stop_propagation();
                                    }
                                }),
                            )
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
                                        this.close_file(&path, None, cx);
                                        cx.stop_propagation();
                                    }))
                                    .child("×"),
                            ),
                    );
                }
                if std::mem::take(&mut self.file_tabs_reveal)
                    && let Some(i) = self.active_tab
                {
                    self.file_tabs_scroll.scroll_to_item(i);
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
                .bg(t.sidebar)
                .child(header)
                .child(body),
            SharedString::from(format!("panel-in-{panel:?}")),
            260,
            24.,
            motion::ease_out(),
        )
    }

    pub(super) fn render_model_picker(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let items = self.model_items();
        let count = items.len();
        let sel = self.model_sel.min(count.saturating_sub(1));
        let (cp, cm) = self.current_model();
        let tab = self.model_tab();
        let favs = self.model_favorites();
        let providers = self.providers();
        let label_of = |id: &str| {
            providers
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| id.to_string())
        };

        // ── Rail: favorites, then one tab per provider ──────────────────
        let rail_btn = |key: SharedString, active: bool, icon: AnyElement, tip: SharedString| {
            div()
                .id(SharedString::from(format!("model-rail-{key}")))
                .size(px(32.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(if active {
                    t.foreground
                } else {
                    t.muted_foreground
                })
                .when(active, |s| s.bg(t.foreground.opacity(0.10)))
                .hover(|s| s.bg(t.foreground.opacity(0.05)).text_color(t.foreground))
                .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                .child(icon)
        };
        let select_tab = |id: String| {
            cx.listener(
                move |this: &mut Self, _: &ClickEvent, _, cx: &mut Context<Self>| {
                    this.model_rail = Some(id.clone());
                    this.model_sel = 0;
                    this.model_reveal.set(true);
                    cx.notify();
                },
            )
        };
        let mut rail = v_flex().p(px(6.)).gap_1().child(
            rail_btn(
                "fav".into(),
                tab == FAV_TAB,
                Icon::new(GIcon::Star).size(px(16.)).into_any_element(),
                "Favorites".into(),
            )
            .on_click(select_tab(FAV_TAB.to_string())),
        );
        for p in &providers {
            let icon = match provider_brand(&p.id) {
                Some(path) => svg()
                    .path(path.to_string())
                    .size(px(16.))
                    .text_color(t.muted_foreground)
                    .into_any_element(),
                None => Icon::new(GIcon::Bot).size(px(16.)).into_any_element(),
            };
            rail = rail.child(
                rail_btn(
                    p.id.clone().into(),
                    tab == p.id,
                    icon,
                    p.label.clone().into(),
                )
                .on_click(select_tab(p.id.clone())),
            );
        }

        // ── Model rows: one flat list for the open tab ──────────────────
        const ROW_H: f32 = 32.;
        let mut list = v_flex()
            .id("model-list")
            .track_scroll(&self.model_scroll)
            .overflow_y_scroll()
            .p_1();
        if items.is_empty() {
            let msg = if tab == FAV_TAB && self.model_query.trim().is_empty() {
                "No favorites yet. Star a model to pin it here."
            } else {
                "No models match"
            };
            list = list.child(
                div()
                    .px(px(10.))
                    .py_3()
                    .text_size(px(13.))
                    .text_color(t.muted_foreground)
                    .child(msg),
            );
        }
        for (i, m) in items.iter().enumerate() {
            let selected = i == sel;
            let current = m.provider == cp && m.id == cm;
            let key = format!("{}:{}", m.provider, m.id);
            let fav = favs.contains(&key);
            // In favorites, rows come from many providers: name the source.
            let source = (tab == FAV_TAB).then(|| label_of(&m.provider));
            list = list.child(
                h_flex()
                    .id(("model-item", i))
                    .h(px(ROW_H))
                    .flex_shrink_0()
                    .px(px(10.))
                    .gap_2()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(selected, |s| {
                        s.bg(t.foreground.opacity(0.09)).text_color(t.foreground)
                    })
                    .when(!selected, |s| s.hover(|s| s.bg(t.foreground.opacity(0.05))))
                    .active(|s| s.bg(t.foreground.opacity(0.14)))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.model_sel != i {
                            this.model_sel = i;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick_model(i, window, cx)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
                    .when_some(source, |s, src| {
                        s.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.5))
                                .text_color(t.muted_foreground)
                                .child(src),
                        )
                    })
                    .when(current, |s| {
                        s.child(
                            Icon::new(GIcon::Check)
                                .size(px(14.))
                                .text_color(t.foreground.opacity(0.7)),
                        )
                    })
                    // Star: always shown when starred, on the hovered row otherwise.
                    .when(fav || selected, |s| {
                        s.child(
                            div()
                                .id(("model-fav", i))
                                .size(px(20.))
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .text_color(if fav {
                                    t.foreground
                                } else {
                                    t.muted_foreground.opacity(0.6)
                                })
                                .hover(|s| s.text_color(t.foreground))
                                .tooltip(move |w, cx| {
                                    Tooltip::new(if fav {
                                        "Remove from favorites"
                                    } else {
                                        "Add to favorites"
                                    })
                                    .build(w, cx)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_model_favorite(key.clone(), cx);
                                }))
                                .child(Icon::new(GIcon::Star).size(px(13.))),
                        )
                    }),
            );
        }
        // Cap the list so the panel stays inside the pane: the composer
        // card, tab strip and status bar take ~180px below it, the picker's
        // own chrome (search, label, effort) ~100px above the list.
        let pane = self.pane_bounds.borrow().get(&self.focused_pane).cloned();
        let viewport: gpui_kit::Size<Pixels> = window.viewport_size();
        let pane_h: f32 = pane
            .as_ref()
            .map(|b| b.size.height.into())
            .unwrap_or(viewport.height.into());
        let content_h = if items.is_empty() {
            44.
        } else {
            8. + count as f32 * ROW_H
        };
        let list_h = content_h.min(352.).min((pane_h - 284.).max(96.));
        let list = list.h(px(list_h));
        if self.model_reveal.take() {
            self.model_scroll.scroll_to_item(sel);
        }

        // ── Effort: stepped slider for the highlighted model ────────────
        let effort_row = items.get(sel).filter(|m| m.efforts.len() > 1).map(|m| {
            let row_current = m.provider == cp && m.id == cm;
            let cur = self.model_effort_for(m);
            let levels = m.efforts.clone();
            let n = levels.len();
            let cur_ix = levels.iter().position(|e| *e == cur).unwrap_or(0);
            let key = format!("{}:{}", m.provider, m.id);
            let frac = cur_ix as f32 / (n - 1) as f32;
            // Click or drag across equal-width zones; each zone is one level.
            let mut zones = h_flex().absolute().inset_0();
            for (j, e) in levels.iter().enumerate() {
                let (k1, e1) = (key.clone(), e.clone());
                let (k2, e2) = (key.clone(), e.clone());
                let set = move |this: &mut Self, k: &String, e: &String, cx: &mut Context<Self>| {
                    if this.model_efforts.get(k) == Some(e) {
                        return;
                    }
                    this.model_efforts.insert(k.clone(), e.clone());
                    // Live model: apply to the running engine right away.
                    if row_current && let Some(tid) = this.active_thread.clone() {
                        this.engine_set_model(&tid, None, None, Some(e));
                    }
                    cx.notify();
                };
                zones = zones.child(
                    div()
                        .id(SharedString::from(format!("effort-zone-{j}")))
                        .flex_1()
                        .h_full()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| set(this, &k1, &e1, cx)),
                        )
                        .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                            if ev.pressed_button == Some(MouseButton::Left) {
                                set(this, &k2, &e2, cx);
                            }
                        })),
                );
            }
            // Track spans zone centers: inset by half a zone on each side.
            let inset = 100. / (2. * n as f32);
            let track = div()
                .relative()
                .flex_1()
                .h(px(20.))
                .child(
                    div()
                        .absolute()
                        .top(px(9.))
                        .left(gpui_kit::relative(inset / 100.))
                        .right(gpui_kit::relative(inset / 100.))
                        .h(px(2.))
                        .bg(t.foreground.opacity(0.14))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .h_full()
                                .w(gpui_kit::relative(frac))
                                .bg(t.foreground.opacity(0.7)),
                        )
                        .children((0..n).map(|j| {
                            div()
                                .absolute()
                                .top(px(-2.))
                                .left(gpui_kit::relative(j as f32 / (n - 1) as f32))
                                .ml(px(-3.))
                                .size(px(6.))
                                .bg(if j <= cur_ix {
                                    t.foreground.opacity(0.7)
                                } else {
                                    t.foreground.opacity(0.22)
                                })
                        }))
                        .child(
                            div()
                                .absolute()
                                .top(px(-5.))
                                .left(gpui_kit::relative(frac))
                                .ml(px(-6.))
                                .size(px(12.))
                                .bg(t.foreground)
                                .border_2()
                                .border_color(t.popover),
                        ),
                )
                .child(zones);
            h_flex()
                .px(px(10.))
                .py(px(8.))
                .gap_3()
                .items_center()
                .border_t_1()
                .border_color(t.foreground.opacity(0.08))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.muted_foreground)
                        .child("Effort"),
                )
                .child(track)
                .child(
                    div()
                        .w(px(48.))
                        .text_size(px(11.))
                        .text_color(t.foreground)
                        .text_right()
                        .child(cur.clone()),
                )
        });

        let col = v_flex()
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
            .child(
                div()
                    .px(px(14.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .text_size(px(10.))
                    .font_medium()
                    .text_color(t.muted_foreground)
                    .child(if tab == FAV_TAB {
                        "FAVORITES".to_string()
                    } else {
                        label_of(&tab).to_uppercase()
                    }),
            )
            .child(list)
            .children(effort_row);

        // The rail sits in normal flow, so the panel is as tall as the taller
        // of rail and list (a one-model tab no longer clips the rail); past
        // the pane-derived cap the rail scrolls.
        let rail = rail
            .id("model-rail")
            .w(px(44.))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .bg(t.foreground.opacity(0.03));

        let panel = h_flex()
            .id("model-picker")
            .occlude()
            .w(px(380.))
            .max_h(px((pane_h - 200.).max(160.)))
            .items_stretch()
            .relative()
            .border_1()
            .border_color(float_border(&t))
            .bg(t.popover)
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
            .child(col);
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
