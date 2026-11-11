use super::*;

impl GrayApp {
    pub(super) fn render_skills(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut rows = v_flex().gap_0p5();
        for sk in &self.skills {
            let name = sk.name.clone();
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("skill-{}", sk.name)))
                    .px_3()
                    .py_2()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .font_family(t.mono_font_family.clone())
                                            .text_xs()
                                            .child(sk.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(t.mono_font_family.clone())
                                            .text_color(t.info)
                                            .child(sk.source.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .child(sk.description.clone()),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("use-skill-{}", sk.name)))
                            .ghost()
                            .xsmall()
                            .label("Use")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::UseSkill(name.clone()), window, cx)
                            })),
                    ),
            );
        }
        if self.skills.is_empty() {
            rows = rows.child(
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("No skills found. Add SKILL.md files under .gray/skills/ in this project or ~/.gray/skills/."),
            );
        }
        rows
    }

    pub(super) fn render_sessions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut rows = v_flex().gap_0p5();
        for gs in &self.gray_sessions {
            let id = gs.id.clone();
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("gsession-{}", gs.id)))
                    .px_3()
                    .py_2()
                    .gap_3()
                    // The prompt is the title; id, model, age and cwd are
                    // the muted detail line.
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .when(gs.live, |s| {
                                        s.child(
                                            div()
                                                .flex_none()
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(t.success),
                                        )
                                    })
                                    .child(div().flex_1().min_w_0().text_sm().truncate().child(
                                        if gs.preview.is_empty() {
                                            gs.id.clone()
                                        } else {
                                            gs.preview.clone()
                                        },
                                    ))
                                    .when(gs.subagent, |s| {
                                        s.child(
                                            div()
                                                .flex_none()
                                                .px_1()
                                                .text_size(px(10.))
                                                .text_color(t.muted_foreground)
                                                .bg(t.foreground.opacity(0.06))
                                                .child("subagent"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .truncate()
                                    .child(
                                        [
                                            gs.id.as_str(),
                                            gs.model.as_str(),
                                            &relative(gs.timestamp),
                                            gs.cwd.as_str(),
                                        ]
                                        .iter()
                                        .filter(|p| !p.is_empty())
                                        .copied()
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                    ),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("resume-{}", gs.id)))
                            .small()
                            .label("Resume")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(Cmd::ResumeSession(id.clone()), window, cx);
                                this.panel = None;
                            })),
                    ),
            );
        }
        v_flex()
            .flex_1()
            .h_full()
            .child(rows)
            .when(self.gray_sessions.is_empty(), |s| {
                s.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .text_color(t.muted_foreground)
                        .child("No Gray sessions found")
                        .child(
                            div()
                                .text_xs()
                                .child("Run gray in a terminal, or send a turn here"),
                        ),
                )
            })
    }

    pub(super) fn render_viewer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        match &self.viewer {
            Some((rel, st)) => {
                let lang = workspace::language_for(rel);
                let mut crumbs = h_flex().gap_1().items_center().text_xs();
                crumbs = crumbs.child(Icon::new(GIcon::File).xsmall());
                let parts: Vec<&str> = rel.split('/').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        crumbs = crumbs.child(
                            Icon::new(GIcon::ChevronRight)
                                .xsmall()
                                .text_color(t.muted_foreground),
                        );
                    }
                    let part = part.to_string();
                    crumbs = crumbs.child(
                        div()
                            .when(i < parts.len() - 1, |s| s.text_color(t.muted_foreground))
                            .child(part),
                    );
                }
                let body = v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(st).h_full().bordered(false).readonly(true));
                v_flex()
                    .flex_1()
                    .h_full()
                    .child(
                        h_flex()
                            .h(px(32.))
                            .flex_none()
                            .px_3()
                            .items_center()
                            .child(crumbs)
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(format!("{lang} · read-only")),
                            )
                            .child(
                                Button::new("viewer-copy")
                                    .ghost()
                                    .small()
                                    .icon(GIcon::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some((_, st)) = &this.viewer {
                                            let v = st.read(cx).value().to_string();
                                            cx.write_to_clipboard(ClipboardItem::new_string(v));
                                            this.toast("Copied", false);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("viewer-open-editor")
                                    .ghost()
                                    .small()
                                    .icon(GIcon::ExternalLink)
                                    .on_click(cx.listener({
                                        let rel = rel.clone();
                                        move |this, _, window, cx| {
                                            this.open_file(&rel, window, cx);
                                        }
                                    })),
                            ),
                    )
                    .child(body)
                    .into_any_element()
            }
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.muted_foreground)
                .child("Click a file in the tree to preview it")
                .into_any_element(),
        }
    }
}
