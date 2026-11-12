//! Editor quality of life, modelled on Zed: save prompts on close,
//! autosave, reopen closed tab, back/forward history, line edits
//! (move / duplicate / delete / comment) and go to line.
use super::*;

/// Idle time after the last keystroke before a dirty tab autosaves.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(1200);
/// Closed tabs remembered for Ctrl+Shift+T.
const CLOSED_CAP: usize = 30;

/// A modal question with buttons. `None` cancels.
pub(super) struct Confirm {
    pub title: String,
    pub detail: String,
    pub buttons: Vec<(String, Option<Cmd>, bool)>,
}

#[derive(Clone, Debug)]
pub(super) enum Closed {
    File(String),
    Thread(String),
}

impl GrayApp {
    fn tab_dirty(&self, i: usize, cx: &App) -> bool {
        self.tabs
            .get(i)
            .is_some_and(|t| t.editor.read(cx).value().as_ref() != t.saved.as_str())
    }

    /// Write tab `i` to disk. False (with a toast) on failure.
    pub(super) fn save_tab(&mut self, i: usize, quiet: bool, cx: &mut Context<Self>) -> bool {
        let Some(root) = self.work_root() else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(i) else {
            return false;
        };
        let value = tab.editor.read(cx).value().to_string();
        let path = tab.path.clone();
        let res = std::fs::write(root.join(&path), &value);
        if res.is_ok() {
            tab.saved = value;
            tab.edited_at = None;
        }
        match res {
            Ok(()) => {
                if !quiet {
                    self.toast(format!("Saved {path}"), false);
                }
                true
            }
            Err(e) => {
                self.toast(format!("Can't save {path}: {e}"), true);
                false
            }
        }
    }

    /// Close the file tab for `path`. `save`: `None` asks first when the
    /// tab has unsaved changes, `Some(true)` saves, `Some(false)` drops them.
    pub(super) fn close_file(&mut self, path: &str, save: Option<bool>, cx: &mut Context<Self>) {
        let Some(i) = self.tabs.iter().position(|t| t.path == path) else {
            return;
        };
        if self.tab_dirty(i, cx) {
            match save {
                None => {
                    let name = Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string());
                    self.confirm = Some(Confirm {
                        title: format!("Save changes to {name}?"),
                        detail: "Your changes will be lost if you don't save them.".into(),
                        buttons: vec![
                            (
                                "Save".into(),
                                Some(Cmd::CloseFile(path.into(), Some(true))),
                                true,
                            ),
                            (
                                "Don't Save".into(),
                                Some(Cmd::CloseFile(path.into(), Some(false))),
                                false,
                            ),
                            ("Cancel".into(), None, false),
                        ],
                    });
                    cx.notify();
                    return;
                }
                Some(true) => {
                    if !self.save_tab(i, true, cx) {
                        return;
                    }
                    self.refresh_workspace(cx);
                }
                Some(false) => {}
            }
        }
        self.tabs.remove(i);
        self.remember_closed(Closed::File(path.to_string()));
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.min(self.tabs.len() - 1)),
            other => other,
        };
        self.file_tabs_reveal = true;
        cx.notify();
    }

    pub(super) fn remember_closed(&mut self, c: Closed) {
        self.closed.push(c);
        if self.closed.len() > CLOSED_CAP {
            self.closed.remove(0);
        }
    }

    /// Ctrl+Shift+T: bring back the most recently closed tab.
    pub(super) fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(c) = self.closed.pop() {
            match c {
                Closed::File(p) => {
                    if self.work_root().is_some_and(|r| r.join(&p).is_file()) {
                        self.open_file(&p, window, cx);
                        cx.notify();
                        return;
                    }
                }
                Closed::Thread(id) => {
                    if self.store.state.thread(&id).is_some_and(|t| !t.archived) {
                        self.run(Cmd::Thread(id), window, cx);
                        return;
                    }
                }
            }
        }
        self.toast("No closed tabs to reopen", false);
        cx.notify();
    }

    /// Ctrl+W: the focused file editor's tab, else the thread tab.
    pub(super) fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel == Some(Panel::Files)
            && let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i))
            && tab.editor.read(cx).focus_handle(cx).is_focused(window)
        {
            let p = tab.path.clone();
            self.close_file(&p, None, cx);
            return;
        }
        match self.active_thread.clone() {
            Some(id) => self.close_tab(&id, window, cx),
            None if self.panes.len() > 1 => self.run(Cmd::ClosePane, window, cx),
            None => {}
        }
    }

    /// Autosave: dirty tabs idle for `AUTOSAVE_AFTER` are written, unless
    /// the file changed on disk underneath us (then we leave it alone).
    pub(super) fn autosave_tick(&mut self, cx: &mut Context<Self>) {
        if self.store.state.setting("autosave", "true") != "true" {
            return;
        }
        let Some(root) = self.work_root() else {
            return;
        };
        let due: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| {
                self.tabs[i]
                    .edited_at
                    .is_some_and(|t| t.elapsed() >= AUTOSAVE_AFTER)
            })
            .collect();
        let mut saved = false;
        for i in due {
            self.tabs[i].edited_at = None;
            if !self.tab_dirty(i, cx) {
                continue;
            }
            let disk = std::fs::read_to_string(root.join(&self.tabs[i].path)).ok();
            if disk.as_deref().is_some_and(|d| d != self.tabs[i].saved) {
                let p = self.tabs[i].path.clone();
                self.toast(
                    format!("{p} changed on disk — not autosaving (Ctrl+S to overwrite)"),
                    true,
                );
                continue;
            }
            saved |= self.save_tab(i, true, cx);
        }
        if saved {
            self.refresh_workspace(cx);
            cx.notify();
        }
    }

    // ---- navigation history (per pane) ----

    /// Note that the pane is leaving `from` for another thread.
    pub(super) fn push_history(&mut self, from: Option<String>) {
        if self.navigating {
            return;
        }
        if let Some(f) = from {
            if self.nav_back.last() != Some(&f) {
                self.nav_back.push(f);
            }
            if self.nav_back.len() > 100 {
                self.nav_back.remove(0);
            }
            self.nav_fwd.clear();
        }
    }

    pub(super) fn go_history(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let next = if back {
                self.nav_back.pop()
            } else {
                self.nav_fwd.pop()
            };
            let Some(id) = next else {
                return;
            };
            if Some(&id) == self.active_thread.as_ref()
                || self.store.state.thread(&id).is_none_or(|t| t.archived)
            {
                continue;
            }
            if let Some(cur) = self.active_thread.clone() {
                if back {
                    self.nav_fwd.push(cur);
                } else {
                    self.nav_back.push(cur);
                }
            }
            self.navigating = true;
            self.run(Cmd::Thread(id), window, cx);
            self.navigating = false;
            return;
        }
    }

    // ---- line edits ----

    fn active_editor(&self, window: &Window, cx: &App) -> Option<(Entity<EditorState>, String)> {
        let tab = self.active_tab.and_then(|i| self.tabs.get(i))?;
        if self.panel != Some(Panel::Files)
            || !tab.editor.read(cx).focus_handle(cx).is_focused(window)
        {
            return None;
        }
        Some((tab.editor.clone(), tab.path.clone()))
    }

    pub(super) fn line_edit(&mut self, op: LineOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, path)) = self.active_editor(window, cx) else {
            return;
        };
        let token = comment_token(&path);
        if op == LineOp::Comment && token.is_none() {
            self.toast("No line comment for this file type", true);
            cx.notify();
            return;
        }
        editor.update(cx, |s, cx| {
            let text = s.value().to_string();
            let sel = s.selected_range();
            let Some(edit) = line_op(&text, sel, op, token.unwrap_or("//")) else {
                return;
            };
            s.set_selected_range(edit.range, cx);
            s.replace(edit.text, window, cx);
            s.set_selected_range(edit.select, cx);
        });
    }

    pub(super) fn go_to_line(
        &mut self,
        line: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) else {
            self.toast("Open a file first", true);
            cx.notify();
            return;
        };
        let editor = tab.editor.clone();
        self.panel = Some(Panel::Files);
        editor.update(cx, |s, cx| {
            let text = s.value().to_string();
            let off = offset_of(&text, line, col);
            s.set_selected_range(off..off, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn render_confirm(&self, c: &Confirm, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut row = h_flex().gap_2().justify_end().pt_3();
        for (i, (label, cmd, primary)) in c.buttons.iter().enumerate() {
            let cmd = cmd.clone();
            let primary = *primary;
            row = row.child(
                div()
                    .id(("confirm-btn", i))
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(primary, |s| {
                        s.bg(t.primary).text_color(t.primary_foreground)
                    })
                    .when(!primary, |s| {
                        s.border_1()
                            .border_color(t.border)
                            .hover(|s| s.bg(t.foreground.opacity(0.06)))
                    })
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm = None;
                        if let Some(c) = cmd.clone() {
                            this.run(c, window, cx);
                        }
                        cx.notify();
                    })),
            );
        }
        div()
            .id("confirm-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::black().opacity(0.35))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm = None;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("confirm")
                    .w(px(400.))
                    .p_4()
                    .gap_1()
                    .bg(t.popover)
                    .border_1()
                    .border_color(t.border)
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_medium()
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.muted_foreground)
                            .child(c.detail.clone()),
                    )
                    .child(row),
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LineOp {
    MoveUp,
    MoveDown,
    Duplicate,
    Delete,
    Comment,
}

#[derive(Debug, PartialEq)]
pub(super) struct LineEdit {
    pub range: std::ops::Range<usize>,
    pub text: String,
    pub select: std::ops::Range<usize>,
}

fn line_start(text: &str, off: usize) -> usize {
    text[..off].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, off: usize) -> usize {
    text[off..].find('\n').map_or(text.len(), |i| off + i)
}

/// The edit for `op` over the lines touched by `sel` (byte offsets).
pub(super) fn line_op(
    text: &str,
    sel: std::ops::Range<usize>,
    op: LineOp,
    token: &str,
) -> Option<LineEdit> {
    let a = sel.start.min(text.len());
    let mut b = sel.end.min(text.len()).max(a);
    // A selection ending at column 0 doesn't include that line.
    if b > a && text[..b].ends_with('\n') {
        b -= 1;
    }
    let bs = line_start(text, a);
    let be = line_end(text, b);
    let block = &text[bs..be];
    let shift = |d: isize| {
        let f = |x: usize| (x as isize + d).max(0) as usize;
        f(sel.start)..f(sel.end)
    };
    match op {
        LineOp::MoveUp => {
            if bs == 0 {
                return None;
            }
            let ps = line_start(text, bs - 1);
            let prev = &text[ps..bs - 1];
            Some(LineEdit {
                range: ps..be,
                text: format!("{block}\n{prev}"),
                select: shift(-(prev.len() as isize + 1)),
            })
        }
        LineOp::MoveDown => {
            if be >= text.len() {
                return None;
            }
            let ne = line_end(text, be + 1);
            let next = &text[be + 1..ne];
            Some(LineEdit {
                range: bs..ne,
                text: format!("{next}\n{block}"),
                select: shift(next.len() as isize + 1),
            })
        }
        LineOp::Duplicate => Some(LineEdit {
            range: bs..be,
            text: format!("{block}\n{block}"),
            select: shift(block.len() as isize + 1),
        }),
        LineOp::Delete => {
            let range = if be < text.len() {
                bs..be + 1
            } else if bs > 0 {
                bs - 1..be
            } else {
                bs..be
            };
            let at = range.start.min(text.len() - range.len());
            Some(LineEdit {
                range,
                text: String::new(),
                select: at..at,
            })
        }
        LineOp::Comment => {
            let lines: Vec<&str> = block.split('\n').collect();
            let body: Vec<&str> = lines
                .iter()
                .copied()
                .filter(|l| !l.trim().is_empty())
                .collect();
            let commented =
                !body.is_empty() && body.iter().all(|l| l.trim_start().starts_with(token));
            let indent = body
                .iter()
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            let out: Vec<String> = lines
                .iter()
                .map(|l| {
                    if l.trim().is_empty() {
                        return l.to_string();
                    }
                    if commented {
                        let ws = l.len() - l.trim_start().len();
                        let rest = &l[ws + token.len()..];
                        let rest = rest.strip_prefix(' ').unwrap_or(rest);
                        format!("{}{rest}", &l[..ws])
                    } else {
                        format!("{}{token} {}", &l[..indent], &l[indent..])
                    }
                })
                .collect();
            let text = out.join("\n");
            let select = if sel.start == sel.end {
                let d = text.len() as isize - block.len() as isize;
                // Keep the caret on its character when it sits past the token.
                let c = if lines.len() == 1 && sel.start >= bs + indent {
                    (sel.start as isize + d).max(bs as isize) as usize
                } else {
                    sel.start
                };
                c..c
            } else {
                bs..bs + text.len()
            };
            Some(LineEdit {
                range: bs..be,
                text,
                select,
            })
        }
    }
}

/// Byte offset of 1-based `line`:`col`, clamped to the text.
pub(super) fn offset_of(text: &str, line: usize, col: usize) -> usize {
    let mut off = 0;
    for _ in 1..line.max(1) {
        match text[off..].find('\n') {
            Some(i) => off += i + 1,
            None => break,
        }
    }
    let end = line_end(text, off);
    let mut at = off;
    for (n, (i, _)) in text[off..end].char_indices().enumerate() {
        if n + 1 >= col.max(1) {
            at = off + i;
            break;
        }
        at = end;
    }
    if col <= 1 { off } else { at }
}

pub(super) fn comment_token(path: &str) -> Option<&'static str> {
    let name = Path::new(path)
        .file_name()?
        .to_string_lossy()
        .to_lowercase();
    if matches!(
        name.as_str(),
        "makefile" | "dockerfile" | "justfile" | ".gitignore" | ".env"
    ) {
        return Some("#");
    }
    let ext = Path::new(&name).extension()?.to_string_lossy().to_string();
    Some(match ext.as_str() {
        "rs" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp"
        | "go" | "java" | "kt" | "kts" | "swift" | "cs" | "scss" | "less" | "zig" | "dart"
        | "proto" | "jsonc" | "glsl" | "wgsl" | "scala" | "php" | "groovy" | "v" => "//",
        "py" | "sh" | "bash" | "zsh" | "fish" | "rb" | "toml" | "yaml" | "yml" | "nix" | "pl"
        | "r" | "ex" | "exs" | "conf" | "cfg" | "ini" | "mk" | "cmake" | "ps1" | "jl" | "tf"
        | "gd" | "nu" => "#",
        "lua" | "sql" | "hs" | "elm" | "ada" => "--",
        "lisp" | "clj" | "cljs" | "el" | "scm" | "rkt" | "asm" | "s" => ";",
        "vim" => "\"",
        "tex" | "erl" | "m" => "%",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{LineOp, line_op, offset_of};

    fn apply(
        text: &str,
        sel: std::ops::Range<usize>,
        op: LineOp,
    ) -> (String, std::ops::Range<usize>) {
        let e = line_op(text, sel, op, "//").unwrap();
        let mut s = text.to_string();
        s.replace_range(e.range, &e.text);
        (s, e.select)
    }

    #[test]
    fn move_duplicate_delete() {
        let t = "a\nbb\nccc";
        assert_eq!(apply(t, 3..3, LineOp::MoveUp), ("bb\na\nccc".into(), 1..1));
        assert_eq!(
            apply(t, 3..3, LineOp::MoveDown),
            ("a\nccc\nbb".into(), 7..7)
        );
        assert!(line_op(t, 0..0, LineOp::MoveUp, "//").is_none());
        assert!(line_op(t, 6..6, LineOp::MoveDown, "//").is_none());
        assert_eq!(
            apply(t, 3..3, LineOp::Duplicate),
            ("a\nbb\nbb\nccc".into(), 6..6)
        );
        assert_eq!(apply(t, 3..3, LineOp::Delete).0, "a\nccc");
        assert_eq!(apply(t, 7..7, LineOp::Delete).0, "a\nbb");
        // Selection over two lines moves both.
        assert_eq!(apply(t, 2..6, LineOp::MoveUp).0, "bb\nccc\na");
    }

    #[test]
    fn comment_toggles_at_min_indent() {
        let t = "fn x() {\n    a;\n\n      b;\n}";
        let (on, _) = apply(t, 9..25, LineOp::Comment);
        assert_eq!(on, "fn x() {\n    // a;\n\n    //   b;\n}");
        let (off, _) = apply(&on, 9..30, LineOp::Comment);
        assert_eq!(off, t);
    }

    #[test]
    fn offsets() {
        let t = "ab\ncde\nf";
        assert_eq!(offset_of(t, 1, 1), 0);
        assert_eq!(offset_of(t, 2, 1), 3);
        assert_eq!(offset_of(t, 2, 3), 5);
        assert_eq!(offset_of(t, 2, 99), 6);
        assert_eq!(offset_of(t, 99, 1), 7);
    }
}
