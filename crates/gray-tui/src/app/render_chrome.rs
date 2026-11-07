use super::*;

impl GrayApp {
    pub(super) fn render_plugins(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let label = |s: &str| {
            div()
                .text_xs()
                .font_family(t.mono_font_family.clone())
                .text_color(t.muted_foreground)
                .child(s.to_uppercase())
        };
        let badge = |s: &str| {
            div()
                .px_1p5()
                .py_0p5()
                .bg(t.primary.opacity(0.1))
                .text_color(t.primary)
                .text_xs()
                .font_family(t.mono_font_family.clone())
                .child(s.to_uppercase())
        };
        let mut col = v_flex()
            .id("plugins-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3();
        // Marketplace first: the sidebar's "Marketplace" entry opens here.
        col = col.child(label(&format!("Marketplace · {}", self.hub.len())));
        if let Some(e) = &self.hub_error {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(t.danger)
                    .child(format!("Could not load {}: {e}", plugins::hub_url())),
            );
        } else if self.hub.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("Loading catalog"),
            );
        }
        for h in &self.hub {
            let installed = self.installed.iter().any(|p| p.name == h.name);
            let busy = self.plugin_busy.as_deref() == Some(h.name.as_str());
            let name = h.name.clone();
            col = col.child(
                v_flex()
                    .gap_2()
                    .p_4()
                    .bg(hsla(0., 0., 0., 1.))
                    .hover(|s| s.bg(t.foreground.opacity(0.08)))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(t.mono_font_family.clone())
                                    .font_semibold()
                                    .child(h.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(format!("v{}", h.version)),
                            )
                            .when(h.official, |s| s.child(badge("official")))
                            .when(h.verified, |s| s.child(badge("verified")))
                            .when(h.kind != "plugin", |s| s.child(badge(&h.kind)))
                            .child(div().flex_1())
                            .child(if installed {
                                Button::new(SharedString::from(format!("hub-installed-{}", h.name)))
                                    .xsmall()
                                    .ghost()
                                    .label("Installed")
                                    .icon(GIcon::Check)
                            } else {
                                Button::new(SharedString::from(format!("hub-install-{}", h.name)))
                                    .xsmall()
                                    .primary()
                                    .label(if busy { "Installing" } else { "Install" })
                                    .loading(busy)
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.run(Cmd::PluginInstall(name.clone()), w, cx)
                                    }))
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.muted_foreground)
                            .child(h.description.clone()),
                    )
                    .when(!h.author.is_empty(), |s| {
                        s.child(label(&format!("by {}", h.author)))
                    }),
            );
        }
        col = col.child(
            div()
                .mt_2()
                .child(label(&format!("Installed · {}", self.installed.len()))),
        );
        if self.installed.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No plugins installed"),
            );
        }
        for p in &self.installed {
            let (n1, n2, on) = (p.name.clone(), p.name.clone(), p.enabled);
            col = col.child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(t.popover)
                    .child(
                        div()
                            .font_family(t.mono_font_family.clone())
                            .text_sm()
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(p.version.clone()),
                    )
                    .when(!on, |s| s.child(badge("disabled")))
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("plugin-toggle-{}", p.name)))
                            .xsmall()
                            .ghost()
                            .label(if on { "Disable" } else { "Enable" })
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.run(Cmd::PluginEnable(n1.clone(), !on), w, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("plugin-remove-{}", p.name)))
                            .xsmall()
                            .ghost()
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.run(Cmd::PluginRemove(n2.clone()), w, cx)
                            })),
                    ),
            );
        }
        col
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut list = v_flex()
            .id("sidebar-scroll")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .gap_0p5();
        list = list.child(
            h_flex()
                .id("projects-header")
                .mt(px(10.))
                .mx(px(4.))
                .px(px(6.))
                .h(px(24.))
                .gap_1()
                .items_center()
                .text_size(px(11.5))
                .text_color(t.muted_foreground)
                .cursor_pointer()
                .hover(|s| s.bg(t.foreground.opacity(0.05)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.projects_collapsed = !this.projects_collapsed;
                    cx.notify();
                }))
                .child(
                    Icon::new(if self.projects_collapsed {
                        GIcon::ChevronRight
                    } else {
                        GIcon::ChevronDown
                    })
                    .size(px(12.))
                    .text_color(t.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .child(if self.store.state.projects.is_empty() {
                            "No project folder"
                        } else {
                            "Projects"
                        }),
                )
                .child(
                    div()
                        .id("sidebar-find")
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.foreground.opacity(0.07)).text_color(t.foreground))
                        .tooltip(|w, cx| Tooltip::new("Search threads (Ctrl+Shift+F)").build(w, cx))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.run(Cmd::SearchThreads, window, cx)
                        }))
                        .child(Icon::new(GIcon::Search).size(px(12.))),
                ),
        );
        if !self.projects_collapsed {
            for p in &self.store.state.projects {
                let pid = p.id.clone();
                let active_project = Some(&p.id) == self.active_project.as_ref();
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("project-{}", p.id)))
                        .mt_2()
                        .px(px(10.))
                        .py(px(5.))
                        .gap_2()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::LIGHT)
                        .text_color(if active_project {
                            t.sidebar_foreground.opacity(0.9)
                        } else {
                            t.muted_foreground
                                .opacity(if t.mode.is_dark() { 0.7 } else { 1.0 })
                        })
                        .cursor_pointer()
                        .hover(|s| s.bg(t.list_hover).text_color(t.sidebar_foreground))
                        .active(|s| s.bg(t.foreground.opacity(0.10)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run(Cmd::Project(pid.clone()), window, cx)
                        }))
                        .child(div().flex_1().truncate().child(p.name.clone())),
                );
                if !active_project {
                    continue;
                }
                let threads: Vec<_> = self
                    .store
                    .state
                    .threads_for(&p.id, self.show_archived)
                    .into_iter()
                    .filter(|t| self.show_archived || t.snoozed_until <= now())
                    .collect();
                if threads.is_empty() {
                    list = list.child(
                        div()
                            .pl(px(10.))
                            .py(px(4.))
                            .text_size(px(11.5))
                            .text_color(t.muted_foreground.opacity(if t.mode.is_dark() {
                                0.6
                            } else {
                                1.0
                            }))
                            .child("No threads yet"),
                    );
                }
                for th in threads {
                    let id = th.id.clone();
                    let active = Some(&th.id) == self.active_thread.as_ref();
                    let running = self.turns.contains_key(&th.id);
                    let approval = running
                        && (self
                            .pending_approval
                            .as_ref()
                            .is_some_and(|(tid, _)| *tid == th.id)
                            || self.pending_ask.as_ref().is_some_and(|a| a.tid == th.id));
                    // State slot, only while a state is live: spinner while
                    // working, x on error, check when it finished off screen.
                    let slot = if running {
                        Some(
                            motion::braille_spinner(
                                SharedString::from(format!("thread-spin-{}", th.id)),
                                &t,
                            )
                            .into_any_element(),
                        )
                    } else if th.last_status == "error" {
                        Some(
                            Icon::new(GIcon::CircleX)
                                .xsmall()
                                .text_color(t.danger)
                                .into_any_element(),
                        )
                    } else if !active && self.done_unseen.contains(&th.id) {
                        Some(
                            Icon::new(GIcon::Check)
                                .xsmall()
                                .text_color(t.success)
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("thread-{}", th.id)))
                            .pl(px(10.))
                            .pr(px(10.))
                            .py(px(5.))
                            .gap(px(8.))
                            .text_size(px(12.))
                            .text_color(t.sidebar_foreground.opacity(0.8))
                            .cursor_pointer()
                            .hover(|s| {
                                s.bg(t.foreground.opacity(0.06))
                                    .text_color(t.sidebar_foreground)
                            })
                            .when(active, |s| {
                                s.bg(t.foreground.opacity(0.09)).text_color(t.foreground)
                            })
                            .when(approval, |s| s.bg(t.foreground.opacity(0.10)))
                            .active(|s| s.bg(t.foreground.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::Thread(id.clone()), window, cx)
                            }))
                            .when_some(slot, |s, icon| {
                                s.child(
                                    div()
                                        .size(px(16.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon),
                                )
                            })
                            .child(div().flex_1().truncate().child(th.title.clone()))
                            .when(th.pinned, |s| {
                                s.child(Icon::new(GIcon::Star).xsmall().text_color(t.warning))
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .flex_none()
                                    .when(approval, |s| s.text_color(t.warning))
                                    .when(!approval, |s| {
                                        s.text_color(t.sidebar_foreground.opacity(faint(&t, 0.4)))
                                    })
                                    .child(if approval {
                                        "Needs approval".to_string()
                                    } else {
                                        relative(th.updated_at)
                                    }),
                            ),
                    );
                }
            }
        }
        list = list.child(
            h_flex()
                .id("open-project")
                .mt(px(6.))
                .mb_2()
                .px(px(8.))
                .h(px(28.))
                .gap(px(6.))
                .flex_none()
                .self_start()
                .bg(t.foreground.opacity(0.06))
                .text_size(px(12.))
                .text_color(t.sidebar_foreground.opacity(0.9))
                .cursor_pointer()
                .hover(|s| s.bg(t.foreground.opacity(0.10)))
                .on_click(cx.listener(|this, _, window, cx| this.pick_project_folder(window, cx)))
                .child(Icon::new(GIcon::Plus).size(px(12.)))
                .child("Open project"),
        );
        let nav = |id: &'static str,
                   icon: GIcon,
                   label: &'static str,
                   cmd: Cmd,
                   cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .mx_2()
                .px(px(8.))
                .h(px(32.))
                .gap(px(10.))
                .text_size(px(12.5))
                .text_color(t.sidebar_foreground.opacity(0.85))
                .cursor_pointer()
                .hover(|s| {
                    s.bg(t.foreground.opacity(0.06))
                        .text_color(t.sidebar_foreground)
                })
                .active(|s| s.bg(t.foreground.opacity(0.10)))
                .on_click(cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)))
                .child(Icon::new(icon).size(px(14.)).text_color(t.muted_foreground))
                .child(label)
        };
        let icon_btn = |id: &'static str, icon: GIcon| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.muted_foreground)
                .cursor_pointer()
                .hover(|s| s.bg(t.foreground.opacity(0.07)).text_color(t.foreground))
                .child(Icon::new(icon).size(px(14.)))
        };
        v_flex()
            .w_full()
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .font_weight(FontWeight::LIGHT)
            .child(
                h_flex()
                    .h(px(40.))
                    .flex_none()
                    .px(px(10.))
                    .gap_1()
                    .child(
                        icon_btn("sidebar-toggle", GIcon::PanelLeft)
                            .tooltip(|w, cx| Tooltip::new("Hide sidebar (Ctrl+B)").build(w, cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ToggleSidebar, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_btn("tab-prev", GIcon::ChevronLeft)
                            .tooltip(|w, cx| {
                                Tooltip::new("Previous tab (Ctrl+Shift+Tab)").build(w, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cycle_tab(-1, window, cx)),
                            ),
                    )
                    .child(
                        icon_btn("tab-next", GIcon::ChevronRight)
                            .tooltip(|w, cx| Tooltip::new("Next tab (Ctrl+Tab)").build(w, cx))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cycle_tab(1, window, cx)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(16.))
                    .pr(px(10.))
                    .pt(px(6.))
                    .pb(px(10.))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child("Workspace"),
                    ),
            )
            .child(
                h_flex()
                    .id("sidebar-search")
                    .flex_none()
                    .mx_2()
                    .mb(px(6.))
                    .px(px(10.))
                    .h(px(34.))
                    .gap(px(8.))
                    .bg(t.foreground.opacity(0.05))
                    .text_size(px(12.5))
                    .text_color(t.muted_foreground)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.foreground.opacity(0.05)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_palette(PaletteMode::Commands, "", window, cx)
                    }))
                    .child(Icon::new(GIcon::Search).size(px(14.)))
                    .child(div().flex_1().min_w_0().truncate().child("Search"))
                    .child(div().flex_none().text_size(px(10.5)).child("Ctrl+K")),
            )
            .child(nav(
                "nav-new",
                GIcon::Plus,
                "New thread",
                Cmd::NewThread,
                cx,
            ))
            .child(nav(
                "nav-sessions",
                GIcon::Inbox,
                "Sessions",
                Cmd::ToggleSessions,
                cx,
            ))
            .child(nav(
                "nav-marketplace",
                GIcon::LayoutDashboard,
                "Marketplace",
                Cmd::TogglePlugins,
                cx,
            ))
            .child(list)
            .child(
                h_flex()
                    .id("sidebar-settings")
                    .flex_none()
                    .m_2()
                    .px(px(8.))
                    .h(px(34.))
                    .gap(px(10.))
                    .text_size(px(12.5))
                    .text_color(t.sidebar_foreground.opacity(0.85))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.foreground.opacity(0.06)))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(Cmd::Settings, window, cx)),
                    )
                    .child(
                        Icon::new(GIcon::Settings)
                            .size(px(14.))
                            .text_color(t.muted_foreground),
                    )
                    .child(div().flex_1().child("Settings"))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(t.muted_foreground)
                            .child("Ctrl+,"),
                    ),
            )
    }

    /// Native folder picker; falls back to typing a path in the palette.
    pub(super) fn pick_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = rx.await;
            let _ = this.update_in(cx, |app, window, cx| match picked {
                Ok(Ok(Some(paths))) => {
                    if let Some(p) = paths.into_iter().next() {
                        app.run(Cmd::AddProject(p.to_string_lossy().into()), window, cx);
                    }
                }
                Ok(Ok(None)) => {}
                _ => app.open_palette(PaletteMode::Commands, "~/", window, cx),
            });
        })
        .detach();
    }

    pub(super) fn render_approval(&self, label: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div()
            .w_full()
            .max_w(px(896.))
            .mx_auto()
            .px_4()
            .pb_2()
            .child(motion::enter(
                h_flex()
                    .gap_2()
                    .items_center()
                    .bg(t.warning.opacity(0.10))
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(t.warning)
                            .child(format!("Approve? {label}")),
                    )
                    .child(
                        Button::new("approve")
                            .primary()
                            .small()
                            .label("Approve")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Approve(true), window, cx)
                            })),
                    )
                    .child(
                        Button::new("reject")
                            .ghost()
                            .small()
                            .label("Reject")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Approve(false), window, cx)
                            })),
                    ),
                SharedString::from(format!("approval-in-{label}")),
                motion::CARD_MS,
                8.,
                motion::ease_out(),
            ))
    }

    /// Engine `ask` card: one section per question; a single question answers
    /// on click, several collect picks behind a Submit button.
    pub(super) fn render_ask(
        &self,
        ask: &engine::PendingAsk,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let multi = ask.questions.len() > 1;
        let mut card = v_flex().gap_2().bg(t.warning.opacity(0.10)).px_3().py_2();
        for q in &ask.questions {
            let mut block = v_flex().gap_1();
            let head = if q.header.is_empty() {
                q.question.clone()
            } else if q.question.is_empty() {
                q.header.clone()
            } else {
                format!("{} — {}", q.header, q.question)
            };
            block = block.child(div().text_sm().text_color(t.warning).child(head));
            if !q.detail.is_empty() {
                block = block.child(
                    div()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child(q.detail.clone()),
                );
            }
            let mut opts = h_flex().gap_1().flex_wrap();
            for (label, _) in &q.options {
                let picked = ask.picks.get(&q.id) == Some(label);
                let qid = q.id.clone();
                let lbl = label.clone();
                let btn = Button::new(SharedString::from(format!("ask-{}-{}", q.id, label)))
                    .small()
                    .label(label.clone());
                opts = opts.child(if picked { btn.primary() } else { btn.ghost() }.on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::AskPick(qid.clone(), lbl.clone()), window, cx)
                    }),
                ));
            }
            block = block.child(opts);
            card = card.child(block);
        }
        if multi {
            let answered = ask.questions.iter().all(|q| ask.picks.contains_key(&q.id));
            card = card.child(
                h_flex().justify_end().child(
                    Button::new("ask-submit")
                        .primary()
                        .small()
                        .label(if answered {
                            "Submit"
                        } else {
                            "Accept defaults"
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run(Cmd::AskSubmit, window, cx)),
                        ),
                ),
            );
        }
        div()
            .w_full()
            .max_w(px(896.))
            .mx_auto()
            .px_4()
            .pb_2()
            .child(motion::enter(
                card,
                SharedString::from(format!("ask-in-{}", ask.ask_id)),
                motion::CARD_MS,
                8.,
                motion::ease_out(),
            ))
    }

    /// In-app toast card: slides in with a small overshoot, auto-dismissed by
    /// poll after `motion::TOAST_MS`.
    /// Zed-style status bar: sidebar toggle and branch on the left, the
    /// workspace panel toggles on the right.
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let btn = |id: &'static str, icon: GIcon, on: bool| {
            h_flex()
                .id(id)
                .h(px(22.))
                .min_w(px(24.))
                .px(px(5.))
                .gap(px(4.))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(if on { t.foreground } else { t.muted_foreground })
                .when(on, |s| s.bg(t.foreground.opacity(0.08)))
                .cursor_pointer()
                .hover(|s| s.bg(t.foreground.opacity(0.08)).text_color(t.foreground))
                .child(Icon::new(icon).size(px(14.)))
        };
        let tip =
            |s: &'static str| move |w: &mut Window, cx: &mut App| Tooltip::new(s).build(w, cx);
        let worktree = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id))
            .and_then(|t| t.worktree.clone())
            .and_then(|w| {
                Path::new(&w)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            });
        let branch = worktree.or_else(|| self.branch.clone());
        h_flex()
            .h(px(28.))
            .flex_none()
            .px(px(6.))
            .gap(px(2.))
            .items_center()
            .bg(t.sidebar)
            .border_t_1()
            .border_color(t.border)
            .child(
                btn("sb-sidebar", GIcon::PanelLeft, self.sidebar_open)
                    .tooltip(tip("Toggle sidebar (Ctrl+B)"))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::ToggleSidebar, w, cx))),
            )
            .when_some(branch, |s, b| {
                s.child(
                    btn("sb-branch", GIcon::Github, false)
                        .tooltip(tip("Switch branch (Ctrl+Shift+G)"))
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.open_palette(PaletteMode::Commands, "Branch:", w, cx)
                        }))
                        .child(div().max_w(px(220.)).truncate().child(b)),
                )
            })
            .child(div().flex_1())
            .child(
                btn(
                    "sb-files",
                    GIcon::FileText,
                    self.panel == Some(Panel::Files),
                )
                .tooltip(tip("Files (Ctrl+E)"))
                .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::ToggleFiles, w, cx))),
            )
            .child(
                btn("sb-diff", GIcon::Copy, self.panel == Some(Panel::Diff))
                    .tooltip(tip("Diff (Ctrl+D)"))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::ToggleDiff, w, cx)))
                    .when(!self.diff_files.is_empty(), |s| {
                        s.child(self.diff_files.len().to_string())
                    }),
            )
            .child(
                btn("sb-terminal", GIcon::SquareTerminal, self.terminal_open)
                    .tooltip(tip("Terminal (Ctrl+J)"))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::ToggleTerminal, w, cx))),
            )
    }

    pub(super) fn render_toast(
        &self,
        msg: String,
        err: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        div()
            .absolute()
            .right(px(20.))
            .bottom(px(40.))
            .child(motion::enter(
                h_flex()
                    .id(SharedString::from(format!("toast-{}", self.toast_seq)))
                    .max_w(px(360.))
                    .gap_2()
                    .items_center()
                    .border_1()
                    .border_color(if err {
                        t.danger.opacity(0.7)
                    } else {
                        float_border(&t)
                    })
                    .bg(glass_bg(&t))
                    .relative()
                    .px_3()
                    .py(px(8.))
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toast = None;
                        this.toast_at = None;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if err { GIcon::CircleX } else { GIcon::Check })
                            .small()
                            .text_color(if err { t.danger } else { t.primary }),
                    )
                    .child(div().flex_1().child(msg))
                    .child(glass_fx(&t)),
                SharedString::from(format!("toast-in-{}", self.toast_seq)),
                motion::CARD_MS,
                8.,
                motion::ease_pop(),
            ))
    }
}
