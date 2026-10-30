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
                .border_1()
                .border_color(t.primary.opacity(0.4))
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
        col = col.child(label(&format!("Installed · {}", self.installed.len())));
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
                    .border_1()
                    .border_color(t.border)
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
        col = col.child(
            div()
                .mt_2()
                .child(label(&format!("Hub · {}", self.hub.len()))),
        );
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
                    .border_1()
                    .border_color(t.border)
                    .bg(hsla(0., 0., 0., 1.))
                    .hover(|s| s.border_color(t.primary))
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
                    .text_size(px(12.))
                    .font_medium()
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
                    .child(
                        Icon::new(if active_project {
                            GIcon::FolderOpen
                        } else {
                            GIcon::Folder
                        })
                        .xsmall(),
                    )
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
                        .pl(px(30.))
                        .py(px(4.))
                        .text_size(px(12.))
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
                    && self
                        .pending_approval
                        .as_ref()
                        .is_some_and(|(tid, _)| *tid == th.id);
                // 16px state slot: spinner while working, check when it
                // finished off screen, x on error, empty otherwise.
                let slot = div()
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center();
                let slot = if running {
                    slot.child(motion::braille_spinner(
                        SharedString::from(format!("thread-spin-{}", th.id)),
                        &t,
                    ))
                } else if th.last_status == "error" {
                    slot.child(Icon::new(GIcon::CircleX).xsmall().text_color(t.danger))
                } else if !active && self.done_unseen.contains(&th.id) {
                    slot.child(Icon::new(GIcon::Check).xsmall().text_color(t.success))
                } else {
                    slot
                };
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("thread-{}", th.id)))
                        .pl(px(5.))
                        .pr(px(10.))
                        .py(px(5.))
                        .gap(px(8.))
                        .border_1()
                        .border_color(transparent_black())
                        .text_size(px(13.))
                        .text_color(t.sidebar_foreground.opacity(0.8))
                        .cursor_pointer()
                        .hover(|s| {
                            if active {
                                s.bg(t.primary_hover)
                            } else {
                                s.bg(t.list_hover).text_color(t.sidebar_foreground)
                            }
                        })
                        .when(active, |s| s.bg(t.primary).text_color(t.primary_foreground))
                        .when(approval, |s| {
                            s.bg(t.foreground.opacity(0.10))
                                .border_dashed()
                                .border_color(t.foreground.opacity(0.30))
                        })
                        .active(|s| s.bg(t.foreground.opacity(0.10)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run(Cmd::Thread(id.clone()), window, cx)
                        }))
                        .child(slot)
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
        v_flex()
            .w_full()
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .child(
                h_flex()
                    .h(px(40.))
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(t.sidebar_border)
                    .child(
                        div()
                            .size(px(22.))
                            .bg(t.foreground)
                            .text_color(t.background)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(
                                svg()
                                    .path("icons/gray-logo.svg")
                                    .size_4()
                                    .text_color(t.background),
                            ),
                    )
                    .child(div().text_size(px(14.)).font_semibold().child("Gray"))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family(t.mono_font_family.clone())
                            .px(px(5.))
                            .bg(t.foreground.opacity(0.07))
                            .text_color(t.muted_foreground)
                            .child("IDE"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("new-thread")
                            .ghost()
                            .small()
                            .icon(GIcon::Plus)
                            .tooltip("New thread (Ctrl+N)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::NewThread, window, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .id("sidebar-search")
                    .mx_2()
                    .mt_2()
                    .px(px(10.))
                    .h(px(30.))
                    .gap_2()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.foreground.opacity(0.03))
                    .text_size(px(12.))
                    .text_color(t.muted_foreground.opacity(0.8))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.list_hover).border_color(t.foreground.opacity(0.12)))
                    .active(|s| s.bg(t.foreground.opacity(0.10)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_palette(PaletteMode::Commands, "", window, cx)
                    }))
                    .child(Icon::new(GIcon::Search).xsmall())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child("Search or run a command"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .bg(t.foreground.opacity(0.06))
                            .font_family(t.mono_font_family.clone())
                            .text_size(px(10.))
                            .child("Ctrl K"),
                    ),
            )
            .child(list)
            .child(
                h_flex()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(t.sidebar_border)
                    .child(
                        Button::new("open-settings")
                            .ghost()
                            .small()
                            .icon(GIcon::Settings)
                            .label("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::Settings, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("open-plugins")
                            .ghost()
                            .small()
                            .icon(GIcon::Star)
                            .tooltip("Plugins (Ctrl+Shift+P)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::TogglePlugins, window, cx)
                            })),
                    )
                    .child(
                        Button::new("toggle-archived")
                            .ghost()
                            .small()
                            .icon(GIcon::Inbox)
                            .tooltip("Show archived")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ShowArchived, window, cx)
                            })),
                    )
                    .child(
                        Button::new("toggle-theme")
                            .ghost()
                            .small()
                            .icon(if t.mode.is_dark() {
                                GIcon::Sun
                            } else {
                                GIcon::Moon
                            })
                            .tooltip("Toggle theme")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(Cmd::ToggleTheme, window, cx)
                            })),
                    ),
            )
    }

    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let project = self
            .active_project
            .as_ref()
            .and_then(|id| self.store.state.project(id));
        let title = thread
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "New thread".into());
        h_flex()
            .h(px(40.))
            .px_3()
            .gap_2()
            .flex_none()
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.title_bar_border)
            .when(!self.sidebar_open, |s| {
                s.child(
                    Button::new("show-sidebar")
                        .ghost()
                        .small()
                        .icon(GIcon::PanelLeft)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleSidebar, window, cx)
                        })),
                )
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .truncate()
                    .child(title),
            )
            .when_some(thread.and_then(|t| t.worktree.clone()), |s, w| {
                let name = Path::new(&w)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                s.child(
                    h_flex()
                        .h(px(20.))
                        .px_2()
                        .gap_1()
                        .border_1()
                        .border_color(t.primary.opacity(0.25))
                        .bg(t.primary.opacity(0.08))
                        .text_size(px(11.))
                        .text_color(t.primary)
                        .child(Icon::new(GIcon::Github).xsmall())
                        .child(name),
                )
            })
            .when_some(project, |s, p| {
                s.child(
                    h_flex()
                        .h(px(20.))
                        .px_2()
                        .bg(t.foreground.opacity(0.07))
                        .text_size(px(11.))
                        .text_color(t.muted_foreground)
                        .child(p.name.clone()),
                )
            })
            .when_some(self.branch.clone(), |s, b| {
                s.child(
                    chip("header-branch", GIcon::Github, b, cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_palette(PaletteMode::Commands, "Branch:", window, cx)
                        },
                    )),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("toggle-files")
                    .ghost()
                    .small()
                    .icon(GIcon::FileText)
                    .tooltip("Files (Ctrl+E)")
                    .when(self.panel == Some(Panel::Files), |b| b.primary())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(Cmd::ToggleFiles, window, cx)),
                    ),
            )
            .child(
                Button::new("toggle-diff")
                    .ghost()
                    .small()
                    .icon(GIcon::Copy)
                    .label(if self.diff_files.is_empty() {
                        "Diff".to_string()
                    } else {
                        format!("Diff {}", self.diff_files.len())
                    })
                    .tooltip("Diff (Ctrl+D)")
                    .when(self.panel == Some(Panel::Diff), |b| b.primary())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(Cmd::ToggleDiff, window, cx)),
                    ),
            )
            .child(
                Button::new("toggle-terminal")
                    .ghost()
                    .small()
                    .icon(GIcon::SquareTerminal)
                    .tooltip("Terminal (Ctrl+J)")
                    .when(self.terminal_open, |b| b.primary())
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleTerminal, window, cx)
                        }),
                    ),
            )
            .when(self.sidebar_open, |s| {
                s.child(
                    Button::new("hide-sidebar")
                        .ghost()
                        .small()
                        .icon(GIcon::PanelLeft)
                        .tooltip("Sidebar (Ctrl+B)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(Cmd::ToggleSidebar, window, cx)
                        })),
                )
            })
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let thread = self
            .active_thread
            .as_ref()
            .and_then(|id| self.store.state.thread(id));
        let (dp, dm, _) = self.defaults();
        let provider = thread.map(|t| t.provider.clone()).unwrap_or(dp);
        let model = thread.map(|t| t.model.clone()).unwrap_or(dm);
        let turn = thread.and_then(|th| self.turns.get(&th.id));
        let (dot, status) = if self.pending_approval.is_some() {
            (t.primary, "Awaiting approval".to_string())
        } else if let Some(a) = turn {
            (
                t.info,
                format!(
                    "Working · {}",
                    fmt_elapsed(a.started.elapsed().as_secs() as i64)
                ),
            )
        } else if thread.is_some_and(|th| th.last_status == "error") {
            (t.danger, "Error".to_string())
        } else {
            (t.success.opacity(0.8), "Ready".to_string())
        };
        let (status_word, status_num) = status
            .split_once(" · ")
            .map_or_else(|| (status.as_str(), ""), |(w, n)| (w, n));
        let running = self.turns.len();
        let sep = || div().w(px(1.)).h(px(10.)).bg(t.foreground.opacity(0.12));
        let item = |s: String| div().child(s);
        h_flex()
            .id("status-bar")
            .h(px(26.))
            .flex_none()
            .px_3()
            .gap(px(10.))
            .bg(t.status_bar)
            .border_t_1()
            .border_color(t.status_bar_border)
            .text_size(px(11.))
            .text_color(t.foreground.opacity(0.5))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().size(px(6.)).bg(dot))
                    .child(motion::decode(
                        SharedString::from(format!("status-{status_word}")),
                        status_word,
                        motion::CARD_MS,
                    ))
                    .when(!status_num.is_empty(), |s| {
                        s.child(motion::flap("status-num", &format!("· {status_num}"), &t))
                    }),
            )
            .child(sep())
            .child(
                h_flex()
                    .gap(px(5.))
                    .child(match provider_brand(&provider) {
                        Some(path) => svg()
                            .path(path.to_string())
                            .size(px(11.))
                            .text_color(t.foreground.opacity(0.5))
                            .into_any_element(),
                        None => Icon::new(GIcon::Bot).size(px(11.)).into_any_element(),
                    })
                    .child(item(format!(
                        "{} · {}",
                        provider::provider_label(&provider),
                        model
                    ))),
            )
            .when(running > 1, |s| {
                s.child(sep())
                    .child(item(format!("{running} threads running")))
            })
            .child(div().flex_1())
            .when(!self.diff_files.is_empty(), |s| {
                s.child(item(format!(
                    "{} changed file{}",
                    self.diff_files.len(),
                    if self.diff_files.len() == 1 { "" } else { "s" }
                )))
                .child(sep())
            })
            .when_some(self.branch.clone(), |s, b| {
                s.child(
                    h_flex()
                        .gap(px(4.))
                        .child(Icon::new(GIcon::Github).size(px(11.)))
                        .child(b),
                )
            })
            .when_some(
                self.active_project
                    .as_ref()
                    .and_then(|id| self.store.state.project(id)),
                |s, p| {
                    s.child(sep()).child(
                        h_flex()
                            .gap(px(4.))
                            .child(Icon::new(GIcon::Folder).size(px(11.)))
                            .child(p.name.clone()),
                    )
                },
            )
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
                    .border_1()
                    .border_color(t.warning)
                    .bg(t.warning.opacity(0.08))
                    .shadow(hard_shadow(&t))
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

    /// In-app toast card: slides in with a small overshoot, auto-dismissed by
    /// poll after `motion::TOAST_MS`.
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
                    .shadow(hard_shadow(&t))
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
