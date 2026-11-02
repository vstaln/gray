//! Thread tabs and side-by-side split panes.
//!
//! The focused pane's view state lives in the `GrayApp` fields the rest of
//! the app already reads (`active_thread`, `composer`, `scroll`, ...). Every
//! other pane parks its state in `panes[j]`; `panes[focused_pane]` holds a
//! hollow placeholder. `swap_view` exchanges a slot with the live fields, so
//! the existing render and poll code runs unchanged against any pane.
use super::*;

pub(super) const MAX_PANES: usize = 6;
/// Smallest a pane may be dragged (px): Zed's HORIZONTAL_MIN_SIZE /
/// VERTICAL_MIN_SIZE.
/// Stored title of a thread nothing has been sent in yet.
const UNTITLED: &str = "New thread";
const MIN_PANE_W: f32 = 80.;
const MIN_PANE_H: f32 = 100.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SplitDir {
    Right,
    Left,
    Up,
    Down,
}

impl SplitDir {
    pub(super) const ALL: [SplitDir; 4] = [Self::Right, Self::Left, Self::Up, Self::Down];
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Right => "Split Right",
            Self::Left => "Split Left",
            Self::Up => "Split Up",
            Self::Down => "Split Down",
        }
    }
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// A thread tab being dragged; dropping it on a pane edge splits there.
#[derive(Clone)]
pub(super) struct DraggedTab {
    tid: String,
    from: usize,
    title: SharedString,
}

/// What follows the pointer during a tab drag.
pub(super) struct TabGhost(SharedString);

impl Render for TabGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .h(px(30.))
            .max_w(px(220.))
            .px(px(14.))
            .flex()
            .items_center()
            .bg(t.background)
            .border_1()
            .border_color(t.border)
            .text_size(px(13.))
            .text_color(t.foreground)
            .child(div().truncate().child(self.0.clone()))
    }
}

/// Zed-style pane layout: a tree of splits whose leaves index `panes`.
/// `vertical` splits stack children top-to-bottom; `flexes` are the
/// children's relative sizes (all 1.0 = equal).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PaneNode {
    Leaf(usize),
    Split {
        vertical: bool,
        children: Vec<PaneNode>,
        flexes: Vec<f32>,
    },
}

impl PaneNode {
    pub(super) fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            Self::Leaf(j) => out.push(*j),
            Self::Split { children, .. } => children.iter().for_each(|c| c.leaves(out)),
        }
    }

    /// Put pane `new` beside pane `target`. A split along the same axis
    /// gains a sibling and its sizes reset to equal (like Zed); otherwise
    /// the target leaf becomes a new two-way split.
    fn split(&mut self, target: usize, new: usize, vertical: bool, before: bool) -> bool {
        let pair = |a: usize, b: usize| {
            let (x, y) = if before { (b, a) } else { (a, b) };
            Self::Split {
                vertical,
                children: vec![Self::Leaf(x), Self::Leaf(y)],
                flexes: vec![1., 1.],
            }
        };
        match self {
            Self::Leaf(j) if *j == target => {
                *self = pair(target, new);
                true
            }
            Self::Leaf(_) => false,
            Self::Split {
                vertical: v,
                children,
                flexes,
            } => {
                if let Some(i) = children.iter().position(|c| *c == Self::Leaf(target)) {
                    if *v == vertical {
                        let at = if before { i } else { i + 1 };
                        children.insert(at, Self::Leaf(new));
                        *flexes = vec![1.; children.len()];
                    } else {
                        children[i] = pair(target, new);
                    }
                    return true;
                }
                children
                    .iter_mut()
                    .any(|c| c.split(target, new, vertical, before))
            }
        }
    }

    /// Drop pane `target`, reset that split to equal sizes (like Zed),
    /// collapse one-child splits, and shift higher pane indices down by one.
    fn remove(&mut self, target: usize) {
        if let Self::Split {
            children, flexes, ..
        } = self
        {
            if let Some(i) = children.iter().position(|c| *c == Self::Leaf(target)) {
                children.remove(i);
                *flexes = vec![1.; children.len()];
            } else {
                children.iter_mut().for_each(|c| c.remove(target));
            }
            if children.len() == 1 {
                *self = children.remove(0);
            }
        }
        self.reindex(target);
    }

    /// Exchange the places of panes `a` and `b`.
    pub(super) fn swap(&mut self, a: usize, b: usize) {
        match self {
            Self::Leaf(j) if *j == a => *j = b,
            Self::Leaf(j) if *j == b => *j = a,
            Self::Leaf(_) => {}
            Self::Split { children, .. } => children.iter_mut().for_each(|c| c.swap(a, b)),
        }
    }

    fn reindex(&mut self, removed: usize) {
        match self {
            Self::Leaf(j) if *j > removed => *j -= 1,
            Self::Leaf(_) => {}
            Self::Split { children, .. } => children.iter_mut().for_each(|c| c.reindex(removed)),
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Self> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Self::Split { children, .. } => children.get_mut(*i)?.at_mut(rest),
                Self::Leaf(_) => None,
            },
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Leaf(j) => serde_json::json!(j),
            Self::Split {
                vertical,
                children,
                flexes,
            } => serde_json::json!({
                "v": vertical,
                "c": children.iter().map(Self::to_json).collect::<Vec<_>>(),
                "f": flexes,
            }),
        }
    }

    fn from_json(v: &serde_json::Value) -> Option<Self> {
        if let Some(j) = v.as_u64() {
            return Some(Self::Leaf(j as usize));
        }
        let children: Vec<Self> = v["c"]
            .as_array()?
            .iter()
            .map(Self::from_json)
            .collect::<Option<_>>()?;
        let flexes: Vec<f32> = v["f"]
            .as_array()?
            .iter()
            .map(|x| x.as_f64().map(|f| f as f32))
            .collect::<Option<_>>()?;
        (children.len() >= 2 && flexes.len() == children.len() && flexes.iter().all(|f| *f > 0.))
            .then(|| Self::Split {
                vertical: v["v"].as_bool().unwrap_or(false),
                children,
                flexes,
            })
    }

    /// A plain left-to-right row of `n` panes.
    fn row(n: usize) -> Self {
        if n <= 1 {
            return Self::Leaf(0);
        }
        Self::Split {
            vertical: false,
            children: (0..n).map(Self::Leaf).collect(),
            flexes: vec![1.; n],
        }
    }
}

/// An in-progress divider drag: the split's path in the tree and the index
/// of the child left of / above the divider.
#[derive(Clone, Debug)]
pub(super) struct PaneDrag {
    path: Vec<usize>,
    ix: usize,
}

pub(super) struct PaneView {
    thread: Option<String>,
    thread_tabs: Vec<String>,
    composer: Entity<TextareaState>,
    composer_pop: Option<PopKind>,
    pop_sel: usize,
    pop_scroll: ScrollHandle,
    model_open: bool,
    attachments: Vec<PathBuf>,
    scroll: ScrollHandle,
    follow: bool,
    last_offset: Pixels,
    last_max: Pixels,
    follow_thread: Option<String>,
    rise_base: std::cell::RefCell<Option<(String, usize)>>,
    items_scroll: ScrollHandle,
    scroll_target: Option<(String, usize, u8)>,
    dock_slide_at: Option<Instant>,
    nav_back: Vec<String>,
    nav_fwd: Vec<String>,
}

impl PaneView {
    pub(super) fn new(composer: Entity<TextareaState>) -> Self {
        Self {
            thread: None,
            thread_tabs: Vec::new(),
            composer,
            composer_pop: None,
            pop_sel: 0,
            pop_scroll: ScrollHandle::new(),
            model_open: false,
            attachments: Vec::new(),
            scroll: ScrollHandle::new(),
            follow: true,
            last_offset: px(0.),
            last_max: px(0.),
            follow_thread: None,
            rise_base: std::cell::RefCell::new(None),
            items_scroll: ScrollHandle::new(),
            scroll_target: None,
            dock_slide_at: None,
            nav_back: Vec::new(),
            nav_fwd: Vec::new(),
        }
    }
}

impl GrayApp {
    /// What a thread tab shows. Untitled threads (no message sent yet) are
    /// named after their project folder, numbered when the pane holds several
    /// of them: `vstaln`, `vstaln 2`, ...
    pub(super) fn tab_title(&self, id: &str) -> String {
        let Some(th) = self.store.state.thread(id) else {
            return String::new();
        };
        if th.title != UNTITLED {
            return th.title.clone();
        }
        let folder = self
            .store
            .state
            .project(&th.project_id)
            .map(|p| {
                std::path::Path::new(&p.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.name.clone())
            })
            .unwrap_or_else(|| UNTITLED.to_string());
        let same = |t: &&String| {
            self.store
                .state
                .thread(t)
                .is_some_and(|o| o.title == UNTITLED && o.project_id == th.project_id)
        };
        let n = self
            .thread_tabs
            .iter()
            .filter(same)
            .position(|t| t == id)
            .unwrap_or(0);
        if n == 0 {
            folder
        } else {
            format!("{folder} {}", n + 1)
        }
    }

    /// A composer wired to the shared handler. Events from a composer that
    /// isn't the live one first focus its pane, so `send` & co. act on it.
    pub(super) fn new_composer(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TextareaState>, Subscription) {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .submit_on_enter(true)
                .placeholder("Ask Gray anything, @ to mention files, / for commands")
        });
        let sub = cx.subscribe_in(
            &composer,
            window,
            |this, state, ev: &InputEvent, window, cx| {
                if *state != this.composer {
                    if let InputEvent::Blur = ev {
                        if let Some(p) = this.panes.iter_mut().find(|p| p.composer == *state) {
                            p.composer_pop = None;
                        }
                        return;
                    }
                    let Some(j) = this.panes.iter().position(|p| p.composer == *state) else {
                        return;
                    };
                    this.focus_pane(j, false, window, cx);
                }
                match ev {
                    InputEvent::PressEnter { shift: false, .. } => {
                        if !this.accept_pop(window, cx) {
                            this.send(window, cx);
                        }
                    }
                    InputEvent::Change => this.update_composer_pop(cx),
                    InputEvent::Blur => {
                        this.composer_pop = None;
                        cx.notify();
                    }
                    _ => {}
                }
            },
        );
        (composer, sub)
    }

    fn swap_view(&mut self, j: usize) {
        use std::mem::swap;
        let p = &mut self.panes[j];
        swap(&mut self.active_thread, &mut p.thread);
        swap(&mut self.thread_tabs, &mut p.thread_tabs);
        swap(&mut self.composer, &mut p.composer);
        swap(&mut self.composer_pop, &mut p.composer_pop);
        swap(&mut self.pop_sel, &mut p.pop_sel);
        swap(&mut self.pop_scroll, &mut p.pop_scroll);
        swap(&mut self.model_open, &mut p.model_open);
        swap(&mut self.attachments, &mut p.attachments);
        swap(&mut self.scroll, &mut p.scroll);
        swap(&mut self.follow, &mut p.follow);
        swap(&mut self.last_offset, &mut p.last_offset);
        swap(&mut self.last_max, &mut p.last_max);
        swap(&mut self.follow_thread, &mut p.follow_thread);
        swap(&mut self.rise_base, &mut p.rise_base);
        swap(&mut self.items_scroll, &mut p.items_scroll);
        swap(&mut self.scroll_target, &mut p.scroll_target);
        swap(&mut self.dock_slide_at, &mut p.dock_slide_at);
        swap(&mut self.nav_back, &mut p.nav_back);
        swap(&mut self.nav_fwd, &mut p.nav_fwd);
    }

    /// Run `f` with pane `j`'s state live, then restore the focused pane.
    pub(super) fn with_pane<R>(&mut self, j: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let k = self.focused_pane;
        if j == k {
            return f(self);
        }
        self.swap_view(k);
        self.swap_view(j);
        let r = f(self);
        self.swap_view(j);
        self.swap_view(k);
        r
    }

    /// Thread shown by pane `j`.
    fn pane_thread(&self, j: usize) -> Option<&String> {
        if j == self.focused_pane {
            self.active_thread.as_ref()
        } else {
            self.panes[j].thread.as_ref()
        }
    }

    pub(super) fn thread_visible(&self, tid: &str) -> bool {
        (0..self.panes.len()).any(|j| self.pane_thread(j).is_some_and(|t| t == tid))
    }

    /// Another pane already showing `tid`, if any.
    pub(super) fn pane_showing(&self, tid: &str) -> Option<usize> {
        (0..self.panes.len())
            .find(|&j| j != self.focused_pane && self.pane_thread(j).is_some_and(|t| t == tid))
    }

    pub(super) fn focus_pane(
        &mut self,
        j: usize,
        focus_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if j >= self.panes.len() {
            return;
        }
        if j != self.focused_pane {
            let k = self.focused_pane;
            self.swap_view(k);
            self.swap_view(j);
            self.focused_pane = j;
            self.model_open = false;
            // Panes can show threads from different projects: the files,
            // diff and branch views follow the focused pane.
            let pid = self
                .active_thread
                .as_ref()
                .and_then(|t| self.store.state.thread(t))
                .map(|t| t.project_id.clone());
            if pid.is_some() && pid != self.active_project && self.dirty_tabs(cx) == 0 {
                self.active_project = pid;
                self.tabs.clear();
                self.active_tab = None;
                self.refresh_workspace(cx);
            }
            if let Some(id) = &self.active_thread {
                self.done_unseen.remove(id);
            }
            cx.notify();
        }
        if focus_input {
            self.composer.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    pub(super) fn split_pane(
        &mut self,
        dir: SplitDir,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_menu = None;
        if self.panes.len() >= MAX_PANES {
            self.toast(format!("At most {MAX_PANES} panes"), true);
            return;
        }
        let (composer, sub) = Self::new_composer(window, cx);
        self._subs.push(sub);
        self.zoomed = false;
        // New panes go at the end of `panes` so no existing index (or the
        // focused pane's hollow slot) shifts; the tree decides placement.
        let at = self.panes.len();
        self.panes.push(PaneView::new(composer));
        let (vertical, before) = match dir {
            SplitDir::Right => (false, false),
            SplitDir::Left => (false, true),
            SplitDir::Down => (true, false),
            SplitDir::Up => (true, true),
        };
        let target = self.focused_pane;
        self.pane_tree.split(target, at, vertical, before);
        self.focus_pane(at, true, window, cx);
    }

    pub(super) fn close_pane(&mut self, j: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() <= 1 || j >= self.panes.len() {
            return;
        }
        self.zoomed = false;
        self.split_menu = None;
        self.pane_drag = None;
        if j == self.focused_pane {
            // Focus the neighbour in reading order.
            let order = self.pane_order();
            let pos = order.iter().position(|&x| x == j).unwrap_or(0);
            let next = if pos + 1 < order.len() {
                order[pos + 1]
            } else {
                order[pos - 1]
            };
            self.focus_pane(next, true, window, cx);
        }
        self.panes.remove(j);
        if j < self.focused_pane {
            self.focused_pane -= 1;
        }
        self.pane_tree.remove(j);
        cx.notify();
    }

    /// Pane indices in reading order (left→right, top→bottom).
    pub(super) fn pane_order(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.pane_tree.leaves(&mut out);
        out
    }

    /// The pane next to the focused one toward `dir`: whichever pane holds
    /// a point just past that edge, on the focused pane's center line (how
    /// Zed's `find_pane_in_direction` probes).
    pub(super) fn pane_toward(&self, dir: SplitDir) -> Option<usize> {
        if self.zoomed {
            return None;
        }
        let map = self.pane_bounds.borrow();
        let c = map.get(&self.focused_pane)?;
        let mid = c.center();
        let gap = px(8.);
        let probe = match dir {
            SplitDir::Left => point(c.left() - gap, mid.y),
            SplitDir::Right => point(c.right() + gap, mid.y),
            SplitDir::Up => point(mid.x, c.top() - gap),
            SplitDir::Down => point(mid.x, c.bottom() + gap),
        };
        map.iter()
            .find(|(k, b)| **k != self.focused_pane && b.contains(&probe))
            .map(|(k, _)| *k)
    }

    /// Finish a tab drag onto pane `to`: split it toward `dir` and show the
    /// thread there, or (no `dir`) move the tab into `to`. A pane left with
    /// no tabs closes, like Zed's.
    pub(super) fn drop_tab(
        &mut self,
        d: DraggedTab,
        to: usize,
        dir: Option<SplitDir>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = d.from;
        if from >= self.panes.len() || to >= self.panes.len() {
            return;
        }
        let src_tabs = self.with_pane(from, |this| this.thread_tabs.len());
        if from == to && (dir.is_none() || src_tabs <= 1) {
            return;
        }
        if dir.is_some() && self.panes.len() >= MAX_PANES {
            self.toast(format!("At most {MAX_PANES} panes"), true);
            return;
        }
        if self.dirty_tabs(cx) > 0 {
            self.toast("Unsaved files — save (Ctrl+S) or /discard first", true);
            return;
        }
        // Take the tab out of its pane first, so opening it below doesn't
        // just refocus the pane that still shows it.
        let tid = d.tid.clone();
        let emptied = self.with_pane(from, |this| {
            if let Some(ix) = this.thread_tabs.iter().position(|t| *t == tid) {
                this.thread_tabs.remove(ix);
                if this.active_thread.as_deref() == Some(tid.as_str()) {
                    this.active_thread = this
                        .thread_tabs
                        .get(ix)
                        .or_else(|| this.thread_tabs.get(ix.wrapping_sub(1)))
                        .cloned();
                }
            }
            this.thread_tabs.is_empty()
        });
        self.focus_pane(to, false, window, cx);
        if let Some(dir) = dir {
            self.split_pane(dir, window, cx);
        }
        self.run(Cmd::Thread(tid), window, cx);
        if emptied && from != to && self.panes.len() > 1 {
            self.close_pane(from, window, cx);
        }
        cx.notify();
    }

    pub(super) fn cycle_tab(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.thread_tabs.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self
            .active_thread
            .as_ref()
            .and_then(|a| self.thread_tabs.iter().position(|t| t == a))
            .map_or(-1, |i| i as isize);
        let next = self.thread_tabs[(cur + d).rem_euclid(n) as usize].clone();
        self.run(Cmd::Thread(next), window, cx);
    }

    /// Close a tab in the focused pane. Its turn keeps running; the thread
    /// stays in the sidebar.
    pub(super) fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.thread_tabs.iter().position(|t| t == id) else {
            return;
        };
        self.thread_tabs.remove(ix);
        self.remember_closed(editing::Closed::Thread(id.to_string()));
        if self.active_thread.as_deref() == Some(id) {
            let next = self
                .thread_tabs
                .get(ix)
                .or_else(|| self.thread_tabs.get(ix.wrapping_sub(1)))
                .cloned();
            self.active_thread = next;
        }
        if self.thread_tabs.is_empty() && self.panes.len() > 1 {
            let j = self.focused_pane;
            self.close_pane(j, window, cx);
        }
        cx.notify();
    }

    /// Keep each pane's tab list in step with its shown thread: the open
    /// thread always has a tab, and deleted/archived threads lose theirs.
    pub(super) fn sync_tabs(&mut self) {
        for j in 0..self.panes.len() {
            self.with_pane(j, |this| {
                let state = &this.store.state;
                this.thread_tabs
                    .retain(|id| state.thread(id).is_some_and(|t| !t.archived));
                if let Some(id) = this.active_thread.clone()
                    && !this.thread_tabs.contains(&id)
                {
                    this.thread_tabs.push(id);
                }
            });
        }
    }

    fn layout_json(&self) -> String {
        let panes: Vec<serde_json::Value> = (0..self.panes.len())
            .map(|j| {
                let (tabs, active) = if j == self.focused_pane {
                    (&self.thread_tabs, &self.active_thread)
                } else {
                    (&self.panes[j].thread_tabs, &self.panes[j].thread)
                };
                serde_json::json!({ "tabs": tabs, "active": active })
            })
            .collect();
        serde_json::json!({
            "focused": self.focused_pane,
            "panes": panes,
            "tree": self.pane_tree.to_json(),
        })
        .to_string()
    }

    /// Persist open tabs and splits when they change.
    pub(super) fn save_layout(&mut self) {
        let json = self.layout_json();
        if json != self.last_layout {
            self.last_layout = json.clone();
            if self.store.state.setting("pane_layout", "") != json {
                self.persist(Event::SettingChanged {
                    key: "pane_layout".into(),
                    value: json,
                });
            }
        }
    }

    pub(super) fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.store.state.setting("pane_layout", "");
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return;
        };
        let Some(saved) = v["panes"].as_array().filter(|p| !p.is_empty()) else {
            return;
        };
        let live = |id: &str| self.store.state.thread(id).is_some_and(|t| !t.archived);
        let parsed: Vec<(Vec<String>, Option<String>)> = saved
            .iter()
            .take(MAX_PANES)
            .map(|p| {
                let tabs: Vec<String> = p["tabs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str())
                    .filter(|t| live(t))
                    .map(String::from)
                    .collect();
                let active = p["active"].as_str().filter(|t| live(t)).map(String::from);
                (tabs, active)
            })
            .collect();
        let focused = (v["focused"].as_u64().unwrap_or(0) as usize).min(parsed.len() - 1);
        for (j, (tabs, active)) in parsed.into_iter().enumerate() {
            if j == 0 {
                self.thread_tabs = tabs;
                // The launch project wins: keep its thread if one is open.
                let in_project = |id: &String| {
                    self.store.state.thread(id).map(|t| &t.project_id)
                        == self.active_project.as_ref()
                };
                if let Some(a) = active.filter(|a| in_project(a)) {
                    self.active_thread = Some(a);
                }
            } else {
                let (composer, sub) = Self::new_composer(window, cx);
                self._subs.push(sub);
                let mut p = PaneView::new(composer);
                p.thread_tabs = tabs;
                p.thread = active;
                self.panes.push(p);
            }
        }
        // The saved tree must name exactly the restored panes; anything else
        // (older layouts, edited files) falls back to an even row.
        let n = self.panes.len();
        self.pane_tree = PaneNode::from_json(&v["tree"])
            .filter(|t| {
                let mut l = Vec::new();
                t.leaves(&mut l);
                l.sort_unstable();
                l == (0..n).collect::<Vec<_>>()
            })
            .unwrap_or_else(|| PaneNode::row(n));
        if focused != 0 {
            self.focus_pane(focused, false, window, cx);
        }
        self.sync_tabs();
        self.last_layout = self.layout_json();
    }

    pub(super) fn render_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_tabs();
        self.save_layout();
        self.pane_bounds.borrow_mut().clear();
        if !cx.has_active_drag() {
            self.tab_drop = None;
        }
        if self.panes.len() == 1 || self.zoomed {
            let j = self.focused_pane;
            return self.render_pane(j, window, cx);
        }
        let tree = self.pane_tree.clone();
        let body = self.render_node(&tree, Vec::new(), window, cx);
        // Divider drags are tracked here so the pointer can leave the thin
        // handle (and cross into panes) without dropping the drag.
        let dragging = self.pane_drag.clone();
        div()
            .id("pane-group")
            .size_full()
            .min_w_0()
            .min_h_0()
            .when_some(dragging.as_ref(), |s, d| {
                let vertical = matches!(
                    self.pane_tree.clone().at_mut(&d.path),
                    Some(PaneNode::Split { vertical: true, .. })
                );
                s.cursor(if vertical {
                    CursorStyle::ResizeRow
                } else {
                    CursorStyle::ResizeColumn
                })
            })
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let Some(d) = this.pane_drag.clone() else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.pane_drag = None;
                    cx.notify();
                    return;
                }
                let Some(b) = this.split_bounds.borrow().get(&d.path).copied() else {
                    return;
                };
                if let Some(PaneNode::Split {
                    vertical, flexes, ..
                }) = this.pane_tree.at_mut(&d.path)
                {
                    let (pos, origin, len): (f32, f32, f32) = if *vertical {
                        (
                            ev.position.y.into(),
                            b.origin.y.into(),
                            b.size.height.into(),
                        )
                    } else {
                        (ev.position.x.into(), b.origin.x.into(), b.size.width.into())
                    };
                    if len <= 0. {
                        return;
                    }
                    let total: f32 = flexes.iter().sum();
                    let before: f32 = flexes[..d.ix].iter().sum();
                    let pair = flexes[d.ix] + flexes[d.ix + 1];
                    let floor = if *vertical { MIN_PANE_H } else { MIN_PANE_W };
                    let min = (floor / len * total).min(pair / 2.);
                    let at =
                        ((pos - origin) / len * total).clamp(before + min, before + pair - min);
                    flexes[d.ix] = at - before;
                    flexes[d.ix + 1] = pair - flexes[d.ix];
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.pane_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(body)
            .into_any_element()
    }

    fn render_node(
        &mut self,
        node: &PaneNode,
        path: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (vertical, children, flexes) = match node {
            PaneNode::Leaf(j) => {
                let j = *j;
                return self.with_pane(j, |this| this.render_pane(j, window, cx));
            }
            PaneNode::Split {
                vertical,
                children,
                flexes,
            } => (*vertical, children, flexes),
        };
        let t = cx.theme().clone();
        let total: f32 = flexes.iter().sum();
        let mut group = if vertical { v_flex() } else { h_flex() }
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0();
        for (i, child) in children.iter().enumerate() {
            let mut sub = path.clone();
            sub.push(i);
            let el = self.render_node(child, sub, window, cx);
            let mut cell = div()
                .relative()
                .flex_basis(px(0.))
                .min_w_0()
                .min_h_0()
                .overflow_hidden();
            cell.style().flex_grow = Some(flexes[i]);
            cell.style().flex_shrink = Some(1.);
            cell = if vertical {
                cell.w_full()
            } else {
                cell.h_full()
            };
            group = group.child(cell.child(el));
        }
        // Record this split's bounds for drag math.
        let bounds = self.split_bounds.clone();
        let key = path.clone();
        group = group.child(
            canvas(
                move |b, _, _| {
                    bounds.borrow_mut().insert(key, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        // Dividers sit in an overlay painted after the panes, so their
        // hit area can straddle the seam (Zed: 1px line, wider hitbox).
        let mut acc = 0.;
        for i in 0..children.len() - 1 {
            acc += flexes[i];
            let frac = acc / total;
            let line = t.border;
            let (p1, p2) = (path.clone(), path.clone());
            let handle = div()
                .id(SharedString::from(format!("pane-divider-{path:?}-{i}")))
                .absolute()
                .cursor(if vertical {
                    CursorStyle::ResizeRow
                } else {
                    CursorStyle::ResizeColumn
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if ev.click_count >= 2 {
                            // Double-click: back to equal sizes, like Zed.
                            if let Some(PaneNode::Split { flexes, .. }) = this.pane_tree.at_mut(&p1)
                            {
                                flexes.iter_mut().for_each(|f| *f = 1.);
                            }
                            this.pane_drag = None;
                        } else {
                            this.pane_drag = Some(PaneDrag {
                                path: p2.clone(),
                                ix: i,
                            });
                        }
                        cx.notify();
                    }),
                )
                .child(div().absolute().bg(line).map(|d| {
                    if vertical {
                        d.left_0().right_0().top(px(1.5)).h(px(1.))
                    } else {
                        d.top_0().bottom_0().left(px(1.5)).w(px(1.))
                    }
                }));
            let handle = if vertical {
                handle
                    .left_0()
                    .right_0()
                    .h(px(4.))
                    .top(gpui_kit::relative(frac))
                    .mt(px(-2.))
            } else {
                handle
                    .top_0()
                    .bottom_0()
                    .w(px(4.))
                    .left(gpui_kit::relative(frac))
                    .ml(px(-2.))
            };
            group = group.child(handle);
        }
        group.into_any_element()
    }

    fn render_pane(&mut self, j: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let n = self.panes.len();
        let focused = j == self.focused_pane;
        // The approval card shows in the pane that has the asking thread,
        // or the focused pane when no pane does.
        let approval = self
            .pending_approval
            .as_ref()
            .filter(|(tid, _)| {
                self.active_thread.as_deref() == Some(tid.as_str())
                    || (focused && !self.thread_visible(tid))
            })
            .map(|(_, label)| label.clone());
        let ask = self.pending_ask.as_ref().filter(|a| {
            self.active_thread.as_deref() == Some(a.tid.as_str())
                || (focused && !self.thread_visible(&a.tid))
        });
        let t = cx.theme().clone();
        let bounds = self.pane_bounds.clone();
        // Zed's drop target: the half of the pane on the nearest edge, or
        // the whole pane when the pointer is in the middle.
        let drop_zone = self.tab_drop.filter(|(p, _)| *p == j).map(|(_, dir)| {
            let half = gpui_kit::relative(0.5);
            let zone = div()
                .absolute()
                .bg(t.primary.opacity(0.12))
                .border_1()
                .border_color(t.primary.opacity(0.5));
            match dir {
                None => zone.top_0().left_0().right_0().bottom_0(),
                Some(SplitDir::Up) => zone.top_0().left_0().right_0().h(half),
                Some(SplitDir::Down) => zone.bottom_0().left_0().right_0().h(half),
                Some(SplitDir::Left) => zone.top_0().left_0().bottom_0().w(half),
                Some(SplitDir::Right) => zone.top_0().right_0().bottom_0().w(half),
            }
        });
        v_flex()
            .id(SharedString::from(format!("pane-{j}")))
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .capture_any_mouse_down(
                cx.listener(move |this, _, window, cx| this.focus_pane(j, false, window, cx)),
            )
            .on_drag_move(
                cx.listener(move |this, ev: &DragMoveEvent<DraggedTab>, _, cx| {
                    let b = ev.bounds;
                    let pos = ev.event.position;
                    if !b.contains(&pos) {
                        if this.tab_drop.is_some_and(|(p, _)| p == j) {
                            this.tab_drop = None;
                            cx.notify();
                        }
                        return;
                    }
                    let next = Some((j, drop_edge(b, pos)));
                    if this.tab_drop != next {
                        this.tab_drop = next;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                let dir = this
                    .tab_drop
                    .take()
                    .filter(|(p, _)| *p == j)
                    .and_then(|(_, dir)| dir);
                this.drop_tab(d.clone(), j, dir, window, cx);
            }))
            .child(
                canvas(
                    move |b, _, _| {
                        bounds.borrow_mut().insert(j, b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.render_header(j, n, cx))
            .child(self.render_timeline(window, cx))
            .children(approval.map(|l| self.render_approval(&l, cx).into_any_element()))
            .children(ask.map(|a| self.render_ask(a, cx).into_any_element()))
            .child(self.render_composer(window, cx))
            .children(drop_zone)
            .into_any_element()
    }

    /// Zed-style per-pane tab bar: history arrows, tabs, pane controls.
    /// Workspace toggles (files, diff, terminal) live in the status bar.
    pub(super) fn render_header(
        &self,
        j: usize,
        n: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let focused = j == self.focused_pane;
        let bar_bg = t.sidebar;
        let btn = |id: SharedString, icon: GIcon, on: bool| {
            div()
                .id(id)
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_color(if on { t.foreground } else { t.muted_foreground })
                .when(on, |s| s.bg(t.foreground.opacity(0.08)))
                .cursor_pointer()
                .hover(|s| s.bg(t.foreground.opacity(0.08)).text_color(t.foreground))
                .child(Icon::new(icon).size(px(14.)))
        };
        let tip =
            |s: &'static str| move |w: &mut Window, cx: &mut App| Tooltip::new(s).build(w, cx);
        let key = |s: &str| SharedString::from(format!("{s}-{j}"));

        let mut tabs = h_flex()
            .id(key("tabs"))
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll();
        for id in &self.thread_tabs {
            if self.store.state.thread(id).is_none() {
                continue;
            }
            let title = self.tab_title(id);
            let active = self.active_thread.as_deref() == Some(id.as_str());
            let running = self.turns.contains_key(id);
            let unseen = !active && self.done_unseen.contains(id);
            let (open, close, mid) = (id.clone(), id.clone(), id.clone());
            let drag = DraggedTab {
                tid: id.clone(),
                from: j,
                title: title.clone().into(),
            };
            tabs = tabs.child(
                h_flex()
                    .id(SharedString::from(format!("ttab-{j}-{id}")))
                    .group("ttab")
                    .relative()
                    .flex_none()
                    .h_full()
                    .max_w(px(220.))
                    .pl(px(14.))
                    .pr(px(6.))
                    .gap(px(6.))
                    .items_center()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |s| {
                        s.bg(t.background).text_color(if focused {
                            t.foreground
                        } else {
                            t.foreground.opacity(0.7)
                        })
                    })
                    .when(!active, |s| {
                        s.text_color(t.muted_foreground)
                            .hover(|s| s.bg(t.foreground.opacity(0.05)).text_color(t.foreground))
                    })
                    .when(running, |s| {
                        s.child(motion::braille_spinner(
                            SharedString::from(format!("tab-spin-{j}-{id}")),
                            &t,
                        ))
                    })
                    .when(!running && unseen, |s| {
                        s.child(div().size(px(6.)).rounded_full().bg(t.success))
                    })
                    .child(div().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(SharedString::from(format!("ttab-x-{j}-{id}")))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .text_color(t.muted_foreground)
                            .when(!active, |s| {
                                s.invisible().group_hover("ttab", |s| s.visible())
                            })
                            .hover(|s| s.bg(t.foreground.opacity(0.10)).text_color(t.foreground))
                            .child(Icon::new(GIcon::Close).size(px(10.)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(&close, window, cx);
                            })),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| this.close_tab(&mid, window, cx)),
                    )
                    .on_drag(drag, |d, _, _, cx| cx.new(|_| TabGhost(d.title.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run(Cmd::Thread(open.clone()), window, cx)
                    })),
            );
        }
        if self.active_thread.is_none() {
            tabs = tabs.child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .px(px(14.))
                    .items_center()
                    .relative()
                    .bg(t.background)
                    .text_size(px(13.))
                    .text_color(if focused {
                        t.foreground
                    } else {
                        t.foreground.opacity(0.7)
                    })
                    .child(
                        self.project_path()
                            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                            .unwrap_or_else(|| "New thread".into()),
                    ),
            );
        }

        h_flex()
            .h(px(36.))
            .flex_none()
            .bg(bar_bg)
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .px(px(6.))
                    .gap(px(2.))
                    .items_center()
                    .child(
                        btn(key("tab-back"), GIcon::ChevronLeft, false)
                            .tooltip(tip("Go back (Ctrl+Alt+-)"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.focus_pane(j, false, window, cx);
                                this.run(Cmd::GoBack, window, cx)
                            })),
                    )
                    .child(
                        btn(key("tab-fwd"), GIcon::ChevronRight, false)
                            .tooltip(tip("Go forward (Ctrl+Alt+Shift+-)"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.focus_pane(j, false, window, cx);
                                this.run(Cmd::GoForward, window, cx)
                            })),
                    ),
            )
            .child(tabs)
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .px(px(6.))
                    .gap(px(2.))
                    .items_center()
                    // Like Zed, pane controls show on the focused pane only.
                    .when(focused, |s| {
                        s.child(
                            btn(key("tab-new"), GIcon::Plus, false)
                                .tooltip(tip("New thread (Ctrl+N)"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.focus_pane(j, false, window, cx);
                                    this.new_tab(window, cx)
                                })),
                        )
                    })
                    .when(focused, |s| {
                        s.child({
                            // Zed-style split menu: Right / Left / Up / Down.
                            let open = self.split_menu == Some(j);
                            let can_split = n < MAX_PANES;
                            let mut menu_el = None;
                            if open {
                                let mut menu = v_flex()
                                    .id(key("split-menu"))
                                    .occlude()
                                    .w(px(176.))
                                    .p_1()
                                    .border_1()
                                    .border_color(float_border(&t))
                                    .bg(t.popover)
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.split_menu = None;
                                        cx.notify();
                                    }));
                                for dir in SplitDir::ALL {
                                    menu = menu.child(
                                        h_flex()
                                            .id(SharedString::from(format!("split-{j}-{dir:?}")))
                                            .h(px(28.))
                                            .px_2()
                                            .gap_2()
                                            .text_size(px(12.5))
                                            .text_color(t.foreground)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(t.foreground.opacity(0.08)))
                                            .child(div().flex_1().child(dir.label()))
                                            .when(dir == SplitDir::Right, |s| {
                                                s.child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(t.muted_foreground)
                                                        .child("Ctrl+\\"),
                                                )
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.focus_pane(j, false, window, cx);
                                                this.run(Cmd::SplitTo(dir), window, cx)
                                            })),
                                    );
                                }
                                menu_el = Some(
                                    div()
                                        .absolute()
                                        .top_full()
                                        .right_0()
                                        .mt(px(4.))
                                        .child(deferred(menu).with_priority(2)),
                                );
                            }
                            div()
                                .relative()
                                .flex_none()
                                .child(
                                    btn(key("split"), GIcon::Columns, open)
                                        .when(!open && can_split, |s| s.tooltip(tip("Split pane")))
                                        .when(!can_split, |s| {
                                            s.opacity(0.4)
                                                .cursor_default()
                                                .tooltip(tip("At most 6 panes"))
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.panes.len() >= MAX_PANES {
                                                return;
                                            }
                                            this.focus_pane(j, false, window, cx);
                                            this.split_menu = if this.split_menu == Some(j) {
                                                None
                                            } else {
                                                Some(j)
                                            };
                                            cx.notify();
                                        })),
                                )
                                .children(menu_el)
                        })
                    })
                    .when(focused && n > 1, |s| {
                        s.child(
                            btn(
                                key("zoom"),
                                if self.zoomed {
                                    GIcon::Minimize
                                } else {
                                    GIcon::Maximize
                                },
                                false,
                            )
                            .tooltip(tip(if self.zoomed {
                                "Zoom out (Shift+Esc)"
                            } else {
                                "Zoom in (Shift+Esc)"
                            }))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.focus_pane(j, false, window, cx);
                                    this.zoomed = !this.zoomed;
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            btn(key("close-pane"), GIcon::Close, false)
                                .tooltip(tip("Close pane"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_pane(j, window, cx)
                                })),
                        )
                    }),
            )
    }

    /// Empty tab: the welcome view; the thread is created on first send.
    pub(super) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_thread = None;
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
}

/// Which edge a tab dropped at `pos` would split on: the nearest one when
/// the pointer is within Zed's default `drop_target_size` (0.2 of the
/// pane's smaller side) of it, else `None` (drop into the pane).
fn drop_edge(b: Bounds<Pixels>, pos: Point<Pixels>) -> Option<SplitDir> {
    let size = b.size.width.min(b.size.height) * 0.2;
    let (x, y) = (pos.x - b.left(), pos.y - b.top());
    let (w, h) = (b.size.width, b.size.height);
    if x >= size && x <= w - size && y >= size && y <= h - size {
        return None;
    }
    [
        (SplitDir::Up, y),
        (SplitDir::Right, w - x),
        (SplitDir::Down, h - y),
        (SplitDir::Left, x),
    ]
    .into_iter()
    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    .map(|(d, _)| d)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that glob brings gpui's `test` attribute macro,
    // which shadows the builtin `#[test]` and recurses forever.
    use super::{PaneNode, SplitDir, drop_edge};
    use gpui_kit::{Bounds, point, px, size};

    fn flexes(n: &PaneNode) -> Vec<f32> {
        match n {
            PaneNode::Split { flexes, .. } => flexes.clone(),
            PaneNode::Leaf(_) => vec![],
        }
    }

    #[test]
    fn split_and_close_reset_the_axis_to_equal_sizes() {
        let mut t = PaneNode::Leaf(0);
        assert!(t.split(0, 1, false, false));
        if let PaneNode::Split { flexes, .. } = &mut t {
            *flexes = vec![3., 1.];
        }
        assert!(t.split(1, 2, false, false));
        assert_eq!(flexes(&t), vec![1., 1., 1.]);
        let mut order = vec![];
        t.leaves(&mut order);
        assert_eq!(order, vec![0, 1, 2]);
        t.remove(1);
        assert_eq!(flexes(&t), vec![1., 1.]);
        let mut order = vec![];
        t.leaves(&mut order);
        assert_eq!(order, vec![0, 1], "pane 2 reindexed to 1");
    }

    #[test]
    fn split_up_nests_and_swap_exchanges_places() {
        let mut t = PaneNode::Leaf(0);
        t.split(0, 1, false, false); // 0 | 1
        t.split(1, 2, true, true); // 0 | (2 / 1)
        let mut order = vec![];
        t.leaves(&mut order);
        assert_eq!(order, vec![0, 2, 1]);
        t.swap(0, 1);
        let mut order = vec![];
        t.leaves(&mut order);
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn drop_edge_picks_the_nearest_edge_or_the_middle() {
        let b = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(500.)));
        // 0.2 * min(1000, 500) = 100px band.
        assert_eq!(drop_edge(b, point(px(500.), px(250.))), None);
        assert_eq!(drop_edge(b, point(px(50.), px(250.))), Some(SplitDir::Left));
        assert_eq!(
            drop_edge(b, point(px(960.), px(250.))),
            Some(SplitDir::Right)
        );
        assert_eq!(drop_edge(b, point(px(500.), px(20.))), Some(SplitDir::Up));
        assert_eq!(
            drop_edge(b, point(px(500.), px(470.))),
            Some(SplitDir::Down)
        );
    }
}
