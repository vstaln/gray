use super::*;

impl GrayApp {
    pub(super) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut list = v_flex()
            .id("palette-list")
            .track_scroll(&self.palette_scroll)
            .max_h(px(380.))
            .overflow_y_scroll()
            .p_1();
        let items = self.palette_items();
        let count = items.len().min(80);
        let search = self.palette_mode == PaletteMode::Search;
        let grouped = self.palette_mode != PaletteMode::Files;
        if search && items.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_3()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No threads or messages match"),
            );
        }
        let sel = self.palette_sel.min(items.len().saturating_sub(1));
        let mut last_group = None;
        let mut child_ix = 0;
        let mut sel_child = 0;
        for (i, (label, hint, cmd)) in items.into_iter().take(80).enumerate() {
            let (_, group) = cmd_group(&cmd);
            if grouped && last_group != Some(group) {
                last_group = Some(group);
                list = list.child(
                    div()
                        .px(px(10.))
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .font_medium()
                        .text_color(t.muted_foreground.opacity(faint(&t, 0.55)))
                        .child(group.to_uppercase()),
                );
                child_ix += 1;
            }
            if i == sel {
                // Reveal the group header too when the first row is selected.
                sel_child = if i == 0 { 0 } else { child_ix };
            }
            child_ix += 1;
            // Bare category hints repeat the group header; keep real detail
            // (project path, skill source, command description).
            let hint = if grouped && PALETTE_CATEGORY_HINTS.contains(&hint.as_str()) {
                String::new()
            } else {
                hint
            };
            let key = cmd_action(&cmd).and_then(|a| key_hint(a.as_ref(), cx));
            let icon = match &cmd {
                Cmd::OpenMessage(_, Some(_)) => Some(GIcon::Inbox),
                Cmd::OpenMessage(_, None) => Some(GIcon::SquareTerminal),
                _ => None,
            };
            let selected = i == sel;
            list = list.child(
                h_flex()
                    .id(("palette-item", i))
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
                        // Mouse movement (not hover) moves the highlight, so a
                        // list scrolling under a resting pointer doesn't steal it.
                        if this.palette_sel != i {
                            this.palette_sel = i;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx)),
                    )
                    .when_some(icon, |s, icon| {
                        s.child(Icon::new(icon).small().text_color(if selected {
                            t.primary_foreground
                        } else {
                            t.muted_foreground
                        }))
                    })
                    .child(div().flex_1().truncate().child(label))
                    .when(!hint.is_empty(), |s| {
                        s.child(
                            div()
                                .text_xs()
                                .text_color(if selected {
                                    t.primary_foreground
                                } else {
                                    t.muted_foreground
                                })
                                .max_w(px(220.))
                                .truncate()
                                .child(hint),
                        )
                    })
                    .when_some(key, |s, k| s.child(key_chip(&k, &t, selected))),
            );
        }
        if self.palette_reveal.take() {
            self.palette_scroll.scroll_to_item(sel_child);
        }
        let backdrop = div()
            .absolute()
            .inset_0()
            .bg(scrim(&t))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(96.))
            .capture_action(cx.listener(move |this, _: &MoveUp, _, cx| {
                if count > 0 {
                    this.palette_sel = (this.palette_sel.min(count - 1) + count - 1) % count;
                    this.palette_reveal.set(true);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                if count > 0 {
                    this.palette_sel = (this.palette_sel.min(count - 1) + 1) % count;
                    this.palette_reveal.set(true);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(motion::enter(
                v_flex()
                    .w(px(600.))
                    .h_auto()
                    .max_h(px(460.))
                    .border_1()
                    .border_color(float_border(&t))
                    .bg(glass_bg(&t))
                    .shadow(hard_shadow(&t))
                    .relative()
                    .child(
                        div().p_2().border_b_1().border_color(t.border).child(
                            Input::new(&self.palette_input)
                                .appearance(false)
                                .prefix(Icon::new(GIcon::Search).small())
                                .when(search, |s| {
                                    s.suffix(
                                        div()
                                            .text_xs()
                                            .text_color(t.muted_foreground)
                                            .child("Threads & messages"),
                                    )
                                }),
                        ),
                    )
                    .child(list)
                    .child(glass_fx(&t)),
                "palette-in",
                motion::POPOVER_MS,
                6.,
                motion::ease_pop(),
            ));
        motion::fade_in(backdrop, "palette-backdrop-in", motion::BACKDROP_MS)
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (dp, _, dmode) = self.defaults();
        let layout = self.store.state.setting("layout", "focus");
        let auto_accept = self.store.state.setting("auto_accept", "false") == "true";
        let notify = self.store.state.setting("notify", "false") == "true";
        let term_side = self.store.state.setting("terminal_placement", "bottom") == "right";
        let q = self.settings_query.to_lowercase();
        let vis = |label: &str| q.is_empty() || fuzzy(&label.to_lowercase(), &q);
        let nav = |_id: &'static str,
                   icon: GIcon,
                   label: &'static str,
                   section: &'static str,
                   cx: &mut Context<Self>| {
            let sel = self.settings_section == section;
            let sec = section.to_string();
            h_flex()
                .id(SharedString::from(format!("settings-nav-{section}")))
                .px_3()
                .py_1p5()
                .gap_2()
                .text_sm()
                .text_color(if sel {
                    t.primary_foreground
                } else {
                    t.muted_foreground
                })
                .cursor_pointer()
                .hover(|s| s.bg(t.sidebar_accent))
                .active(|s| s.bg(t.foreground.opacity(0.10)))
                .when(sel, |s| s.bg(t.primary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run(Cmd::SettingsSection(sec.clone()), window, cx)
                }))
                .child(Icon::new(icon).xsmall())
                .child(div().flex_1().child(label))
        };
        let mut nav_col = v_flex()
            .w(px(230.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .id("settings-back")
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.foreground))
                    .active(|s| s.bg(t.foreground.opacity(0.10)))
                    .on_click(cx.listener(|this, _, window, cx| this.close_overlays(window, cx)))
                    .child(Icon::new(GIcon::ArrowLeft).xsmall())
                    .child("Back to app"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(t.border)
                    .child(Input::new(&self.settings_search).small()),
            )
            .child(nav(
                "nav-preferences",
                GIcon::Settings,
                "Preferences",
                "preferences",
                cx,
            ))
            .child(nav(
                "nav-appearance",
                GIcon::Palette,
                "Appearance",
                "appearance",
                cx,
            ))
            .child(nav(
                "nav-shortcuts",
                GIcon::LayoutDashboard,
                "Shortcuts",
                "shortcuts",
                cx,
            ));
        nav_col = nav_col.child(
            div()
                .px_3()
                .pt_4()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(t.muted_foreground)
                .child("PROJECTS"),
        );
        for p in &self.store.state.projects {
            let pid = p.id.clone();
            let sel = Some(&p.id) == self.active_project.as_ref();
            nav_col = nav_col.child(
                h_flex()
                    .id(SharedString::from(format!("settings-project-{}", p.id)))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .text_color(if sel {
                        t.primary_foreground
                    } else {
                        t.muted_foreground
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(t.sidebar_accent))
                    .active(|s| s.bg(t.foreground.opacity(0.10)))
                    .when(sel, |s| s.bg(t.primary))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Project(pid.clone()), window, cx)
                    }))
                    .child(Icon::new(GIcon::Folder).xsmall())
                    .child(div().flex_1().truncate().child(p.name.clone())),
            );
        }
        nav_col = nav_col.child(div().flex_1()).child(nav(
            "nav-about",
            GIcon::Info,
            "About",
            "about",
            cx,
        ));

        let row = |label: &str, desc: &str, control: AnyElement| {
            h_flex()
                .py_3()
                .gap_4()
                .border_b_1()
                .border_color(t.border)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_medium().child(label.to_string()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.muted_foreground)
                                .child(desc.to_string()),
                        ),
                )
                .child(control)
        };
        let opt = |id: SharedString, label: &str, sel: bool| {
            Button::new(id)
                .small()
                .label(label.to_string())
                .when(sel, |b| b.primary())
        };

        let mut providers = h_flex().gap_1().flex_wrap();
        for prov in PROVIDERS {
            let id = prov.id.to_string();
            providers = providers.child(
                opt(
                    SharedString::from(format!("settings-provider-{}", prov.id)),
                    prov.label,
                    dp == prov.id,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.persist(Event::SettingChanged {
                        key: "default_provider".into(),
                        value: id.clone(),
                    });
                    cx.notify();
                })),
            );
        }
        let mut modes = h_flex().gap_1();
        for (id, label) in MODES {
            let id_s = id.to_string();
            modes = modes.child(
                opt(
                    SharedString::from(format!("settings-mode-{id}")),
                    label,
                    dmode == *id,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.persist(Event::SettingChanged {
                        key: "default_mode".into(),
                        value: id_s.clone(),
                    });
                    cx.notify();
                })),
            );
        }
        let dark = t.mode.is_dark();
        let mut content = v_flex()
            .id("settings-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_8();
        let mut page = v_flex().w(px(720.)).gap_1();
        let (title, subtitle) = match self.settings_section.as_str() {
            "appearance" => ("Appearance", "Theme and background"),
            "shortcuts" => ("Shortcuts", "Keyboard shortcuts"),
            "about" => ("About", "Version and data"),
            _ => ("Preferences", "Customize layout and default behavior"),
        };
        page = page
            .child(div().text_xl().font_semibold().child(title))
            .child(
                div()
                    .pb_4()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child(subtitle),
            );

        match self.settings_section.as_str() {
            "appearance" => {
                page = page
                    .when(vis("Theme"), |s| {
                        s.child(row(
                            "Theme",
                            "Light or dark appearance",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-light")
                                        .small()
                                        .icon(GIcon::Sun)
                                        .label("Light")
                                        .when(!dark, |b| b.primary())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if dark {
                                                this.run(Cmd::ToggleTheme, window, cx);
                                            }
                                        })),
                                )
                                .child(
                                    Button::new("settings-dark")
                                        .small()
                                        .icon(GIcon::Moon)
                                        .label("Dark")
                                        .when(dark, |b| b.primary())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if !dark {
                                                this.run(Cmd::ToggleTheme, window, cx);
                                            }
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Background"), |s| {
                        s.child(row(
                            "Background",
                            "Dithered wallpaper (custom: /background <file>)",
                            h_flex()
                                .gap_1()
                                .children(bg_rows(cx, self))
                                .into_any_element(),
                        ))
                    });
            }
            "shortcuts" => {
                let bound: [(&dyn Action, &str); 16] = [
                    (&TogglePalette, "Command palette"),
                    (&FilePicker, "File picker"),
                    (&SearchThreads, "Search threads"),
                    (&NewThread, "New thread"),
                    (&ToggleSidebar, "Toggle sidebar"),
                    (&ToggleFiles, "Files panel"),
                    (&ToggleDiff, "Diff panel"),
                    (&ToggleTerminal, "Terminal drawer"),
                    (&SaveFile, "Save editor file"),
                    (&OpenSettings, "Settings"),
                    (&TogglePluginsPanel, "Plugins"),
                    (&ToggleViewer, "Viewer"),
                    (&ToggleTheme, "Toggle theme"),
                    (&EffortPicker, "Effort"),
                    (&StopTurn, "Stop turn"),
                    (&Dismiss, "Close panel or overlay"),
                ];
                let mut rows: Vec<(String, &str)> = bound
                    .iter()
                    .filter_map(|(a, d)| key_hint(*a, cx).map(|k| (k, *d)))
                    .collect();
                rows.push(("Enter".into(), "Send message"));
                rows.push(("Shift+Enter".into(), "New line in composer"));
                for (keys, desc) in rows {
                    page = page.child(row(desc, "", key_chip(&keys, &t, false).into_any_element()));
                }
            }
            "about" => {
                page = page
                    .child(row(
                        "Version",
                        "gray-ide",
                        div()
                            .text_sm()
                            .text_color(t.muted_foreground)
                            .child(env!("CARGO_PKG_VERSION"))
                            .into_any_element(),
                    ))
                    .child(row(
                        "Data directory",
                        "Event log and settings",
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(crate::data_dir().display().to_string())
                            .into_any_element(),
                    ))
                    .child(row(
                        "Providers",
                        "Supported agent CLIs",
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child(
                                PROVIDERS
                                    .iter()
                                    .map(|p| p.label)
                                    .collect::<Vec<_>>()
                                    .join(" \u{00b7} "),
                            )
                            .into_any_element(),
                    ));
            }
            _ => {
                let bar = |w: f32, h: f32, c: Hsla| div().w(px(w)).h(px(h)).bg(c);
                let mut cards = h_flex().gap_4().pb_4();
                for (id, name, desc, sketch) in [
                    (
                        "focus",
                        "Focus",
                        "Roomy layout with the full sidebar. Made for prompting.",
                        h_flex()
                            .h(px(86.))
                            .w_full()
                            .gap_1()
                            .p_1p5()
                            .bg(t.muted.opacity(0.25))
                            .child(div().w(px(54.)).h_full().bg(t.sidebar_accent))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(bar(f32::MAX, 9., t.sidebar_accent))
                                    .child(bar(110., 6., t.muted.opacity(0.6)))
                                    .child(bar(80., 6., t.muted.opacity(0.6)))
                                    .child(bar(140., 6., t.muted.opacity(0.6)))
                                    .child(div().flex_1().bg(t.sidebar_accent)),
                            )
                            .into_any_element(),
                    ),
                    (
                        "pro",
                        "Pro",
                        "Compact layout with an icon-first rail and minimal chrome.",
                        h_flex()
                            .h(px(86.))
                            .w_full()
                            .gap_1()
                            .p_1p5()
                            .bg(t.muted.opacity(0.25))
                            .child(div().w(px(10.)).h_full().bg(t.sidebar_accent))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(bar(f32::MAX, 7., t.sidebar_accent))
                                    .child(bar(120., 6., t.muted.opacity(0.6)))
                                    .child(bar(90., 6., t.muted.opacity(0.6)))
                                    .child(bar(150., 6., t.muted.opacity(0.6)))
                                    .child(div().flex_1().bg(t.sidebar_accent)),
                            )
                            .into_any_element(),
                    ),
                ] {
                    let sel = layout == id;
                    let id_s = id.to_string();
                    cards = cards.child(
                        v_flex()
                            .id(SharedString::from(format!("layout-card-{id}")))
                            .w(px(320.))
                            .p_3()
                            .gap_2()
                            .border_1()
                            .border_color(if sel { t.info } else { t.border })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::SetLayout(id_s.clone()), window, cx)
                            }))
                            .child(sketch)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(14.))
                                            .h(px(14.))
                                            .border_2()
                                            .border_color(if sel {
                                                t.info
                                            } else {
                                                t.muted_foreground
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(sel, |s| {
                                                s.child(div().w(px(7.)).h(px(7.)).bg(t.info))
                                            }),
                                    )
                                    .child(div().text_sm().font_semibold().child(name)),
                            )
                            .child(div().text_xs().text_color(t.muted_foreground).child(desc)),
                    );
                }
                page = page
                    .when(vis("Layout"), |s| {
                        s.child(div().pb_1().text_sm().font_semibold().child("Layout"))
                            .child(cards)
                    })
                    .when(vis("Default provider"), |s| {
                        s.child(row(
                            "Default provider",
                            "Used for new threads",
                            providers.into_any_element(),
                        ))
                    })
                    .when(vis("Default access"), |s| {
                        s.child(row(
                            "Default access",
                            "Runtime permission mode for new threads",
                            modes.into_any_element(),
                        ))
                    })
                    .when(vis("Auto-accept permissions"), |s| {
                        s.child(row(
                            "Auto-accept permissions",
                            "Approve permission requests without asking",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-autoaccept-on")
                                        .small()
                                        .label("On")
                                        .when(auto_accept, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetAutoAccept(true), window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("settings-autoaccept-off")
                                        .small()
                                        .label("Off")
                                        .when(!auto_accept, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetAutoAccept(false), window, cx)
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Notifications"), |s| {
                        s.child(row(
                            "Notifications",
                            "Notify when a turn finishes or needs approval while you're in another thread or window",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-notify-on")
                                        .small()
                                        .label("On")
                                        .when(notify, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetNotify(true), window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("settings-notify-off")
                                        .small()
                                        .label("Off")
                                        .when(!notify, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(Cmd::SetNotify(false), window, cx)
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Terminal placement"), |s| {
                        s.child(row(
                            "Terminal placement",
                            "Where the terminal drawer lives",
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("settings-term-bottom")
                                        .small()
                                        .icon(GIcon::PanelBottom)
                                        .label("Bottom")
                                        .when(!term_side, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(
                                                Cmd::SetTerminalPlacement("bottom".into()),
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new("settings-term-right")
                                        .small()
                                        .icon(GIcon::PanelRight)
                                        .label("Right")
                                        .when(term_side, |b| b.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(
                                                Cmd::SetTerminalPlacement("right".into()),
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .into_any_element(),
                        ))
                    })
                    .when(vis("Language"), |s| {
                        s.child(row(
                            "Language",
                            "Display language",
                            div()
                                .text_sm()
                                .text_color(t.muted_foreground)
                                .child("English")
                                .into_any_element(),
                        ))
                    });
            }
        }
        content = content.child(page);
        motion::fade_in(
            div()
                .id("settings-page")
                .absolute()
                .inset_0()
                .bg(t.background)
                .child(glass_fx(&t))
                .child(motion::enter(
                    h_flex().size_full().child(nav_col).child(content),
                    "settings-panel-in",
                    motion::MODAL_MS,
                    -8.,
                    motion::ease_pop(),
                )),
            "settings-fade-in",
            motion::BACKDROP_MS,
        )
    }
}
