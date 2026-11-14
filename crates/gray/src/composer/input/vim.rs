use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::super::Tui;
use super::keys::{self, KeyOutcome};
use crate::keymap::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Insert,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
}

#[derive(Debug, Default)]
pub(crate) struct Vim {
    mode: Mode,
    op: Option<Op>,
    op_count: usize,
    count: Option<usize>,
    register: String,
    register_linewise: bool,
}

impl Vim {
    pub(crate) fn is_normal(&self) -> bool {
        self.mode == Mode::Normal
    }

    fn clear_pending(&mut self) {
        self.op = None;
        self.op_count = 1;
        self.count = None;
    }

    fn pending(&self) -> bool {
        self.op.is_some() || self.count.is_some()
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    fn push_digit(&mut self, c: char) {
        let d = c.to_digit(10).unwrap_or(0) as usize;
        self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
    }
}

pub(crate) fn dispatch(tui: &mut Tui, ev: &KeyEvent) -> Option<KeyOutcome> {
    if !crate::settings::vim_mode() {
        tui.vim.mode = Mode::Insert;
        tui.vim.clear_pending();
        return None;
    }
    if ev.kind == KeyEventKind::Release
        || ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        tui.vim.clear_pending();
        return None;
    }
    match tui.vim.mode {
        Mode::Insert => insert_key(tui, ev),
        Mode::Normal => normal_key(tui, ev),
    }
}

fn insert_key(tui: &mut Tui, ev: &KeyEvent) -> Option<KeyOutcome> {
    if ev.code != KeyCode::Esc || !tui.matches.is_empty() {
        return None;
    }
    tui.vim.mode = Mode::Normal;
    tui.vim.clear_pending();
    let bol = keys::line_start(tui.textarea.text(), tui.textarea.cursor());
    if tui.textarea.cursor() > bol {
        tui.textarea.move_left();
    }
    Some(KeyOutcome::Edited)
}

fn normal_key(tui: &mut Tui, ev: &KeyEvent) -> Option<KeyOutcome> {
    let KeyCode::Char(c) = ev.code else {
        if matches!(ev.code, KeyCode::Esc) && tui.vim.pending() {
            tui.vim.clear_pending();
            return Some(KeyOutcome::Edited);
        }
        tui.vim.clear_pending();
        return match ev.code {
            KeyCode::Enter => keys::apply(tui, Action::Submit),
            KeyCode::Tab => keys::apply(tui, Action::Tab),
            _ => None,
        };
    };

    if let Some(op) = tui.vim.op {
        tui.vim.op = None;
        let n = tui.vim.take_count().saturating_mul(tui.vim.op_count.max(1));
        tui.vim.op_count = 1;
        return Some(op_key(tui, op, c, n));
    }

    if c.is_ascii_digit() && (c != '0' || tui.vim.count.is_some()) {
        tui.vim.push_digit(c);
        return Some(KeyOutcome::Ignored);
    }

    if matches!(c, 'd' | 'c' | 'y') {
        let op = match c {
            'd' => Op::Delete,
            'c' => Op::Change,
            _ => Op::Yank,
        };
        tui.vim.op_count = tui.vim.take_count();
        tui.vim.op = Some(op);
        tui.vim.count = None;
        return Some(KeyOutcome::Ignored);
    }

    let n = tui.vim.take_count();
    Some(match c {
        'h' => {
            move_left(tui, n);
            KeyOutcome::Edited
        }
        'l' => {
            move_right(tui, n);
            KeyOutcome::Edited
        }
        'j' => return vertical(tui, n, false),
        'k' => return vertical(tui, n, true),
        'w' => {
            move_word_start(tui, n);
            KeyOutcome::Edited
        }
        'b' => {
            move_word_back(tui, n);
            KeyOutcome::Edited
        }
        'e' => {
            move_word_end(tui, n);
            KeyOutcome::Edited
        }
        '0' => {
            let p = keys::line_start(tui.textarea.text(), tui.textarea.cursor());
            tui.textarea.set_cursor(p);
            KeyOutcome::Edited
        }
        '^' => {
            set_first_nonblank(tui);
            KeyOutcome::Edited
        }
        '$' => {
            let p = keys::line_end(tui.textarea.text(), tui.textarea.cursor());
            tui.textarea.set_cursor(p);
            KeyOutcome::Edited
        }
        'x' => {
            delete_chars(tui, n, false);
            KeyOutcome::Edited
        }
        'X' => {
            delete_chars(tui, n, true);
            KeyOutcome::Edited
        }
        'D' => {
            let cur = tui.textarea.cursor();
            let end = line_end_pos(tui);
            apply_range(tui, Op::Delete, cur, end)
        }
        'C' => {
            let cur = tui.textarea.cursor();
            let end = line_end_pos(tui);
            apply_range(tui, Op::Change, cur, end)
        }
        'p' => {
            paste(tui, true);
            KeyOutcome::Edited
        }
        'P' => {
            paste(tui, false);
            KeyOutcome::Edited
        }
        'i' => {
            enter_insert(tui);
            KeyOutcome::Edited
        }
        'a' => {
            append_insert(tui);
            KeyOutcome::Edited
        }
        'I' => {
            set_first_nonblank(tui);
            enter_insert(tui);
            KeyOutcome::Edited
        }
        'A' => {
            let p = line_end_pos(tui);
            tui.textarea.set_cursor(p);
            enter_insert(tui);
            KeyOutcome::Edited
        }
        'o' => {
            open_line(tui, true);
            KeyOutcome::Edited
        }
        'O' => {
            open_line(tui, false);
            KeyOutcome::Edited
        }
        _ => KeyOutcome::Ignored,
    })
}

fn op_key(tui: &mut Tui, op: Op, c: char, n: usize) -> KeyOutcome {
    let doubled = matches!(
        (op, c),
        (Op::Delete, 'd') | (Op::Change, 'c') | (Op::Yank, 'y')
    );
    if doubled {
        return line_op(tui, op, n);
    }
    let cur = tui.textarea.cursor();
    match c {
        'w' => {
            let mut end = cur;
            for _ in 0..n {
                end = next_start(tui.textarea.text(), end);
            }
            let eol = line_end_pos(tui);
            let end = end.min(eol);
            apply_range(tui, op, cur, end)
        }
        'e' => {
            let mut end = cur;
            for _ in 0..n {
                end = word_end_pos(tui.textarea.text(), end);
            }
            let eol = line_end_pos(tui);
            let end = end.min(eol);
            apply_range(tui, op, cur, end)
        }
        'b' => {
            let mut start = cur;
            for _ in 0..n {
                start = prev_start(tui.textarea.text(), start);
            }
            apply_range(tui, op, start, cur)
        }
        '0' | '^' => {
            let start = keys::line_start(tui.textarea.text(), cur);
            apply_range(tui, op, start, cur)
        }
        '$' => apply_range(tui, op, cur, line_end_pos(tui)),
        'h' => {
            let mut start = cur;
            for _ in 0..n {
                start = prev_char_boundary(tui.textarea.text(), start);
            }
            apply_range(tui, op, start, cur)
        }
        'l' => {
            let mut end = cur;
            for _ in 0..n {
                end = next_char_boundary(tui.textarea.text(), end);
            }
            let eol = line_end_pos(tui);
            apply_range(tui, op, cur, end.min(eol))
        }
        _ => KeyOutcome::Ignored,
    }
}

fn apply_range(tui: &mut Tui, op: Op, start: usize, end: usize) -> KeyOutcome {
    if start >= end {
        return KeyOutcome::Ignored;
    }
    let slice = tui.textarea.text()[start..end].to_string();
    tui.vim.register = slice;
    tui.vim.register_linewise = false;
    match op {
        Op::Yank => {
            tui.textarea.set_cursor(start);
        }
        Op::Delete | Op::Change => {
            tui.textarea.replace_range(start..end, "");
            tui.textarea.set_cursor(start);
            after_edit(tui);
            if op == Op::Change {
                enter_insert(tui);
            }
        }
    }
    KeyOutcome::Edited
}

fn line_op(tui: &mut Tui, op: Op, n: usize) -> KeyOutcome {
    let text = tui.textarea.text().to_string();
    let cur = tui.textarea.cursor();
    let mut start = keys::line_start(&text, cur);
    let mut end = keys::line_end(&text, cur);
    let mut lines = 1;
    while lines < n.max(1) && end < text.len() {
        end = keys::line_end(&text, end + 1);
        lines += 1;
    }
    while lines < n.max(1) && start > 0 {
        start = keys::line_start(&text, start - 1);
        lines += 1;
    }
    if start >= end {
        return KeyOutcome::Ignored;
    }
    tui.vim.register = text[start..end].to_string();
    tui.vim.register_linewise = true;
    match op {
        Op::Yank => {
            tui.textarea.set_cursor(start);
        }
        Op::Delete => {
            let (range, cursor) = if end < text.len() {
                (start..end + 1, start)
            } else if start > 0 {
                (start - 1..end, start - 1)
            } else {
                (0..end, 0)
            };
            tui.textarea.replace_range(range, "");
            tui.textarea.set_cursor(cursor);
            after_edit(tui);
        }
        Op::Change => {
            tui.textarea.replace_range(start..end, "");
            tui.textarea.set_cursor(start);
            after_edit(tui);
            enter_insert(tui);
        }
    }
    KeyOutcome::Edited
}

fn delete_chars(tui: &mut Tui, n: usize, backward: bool) {
    let mut cur = tui.textarea.cursor();
    let mut removed = String::new();
    for _ in 0..n {
        if backward {
            let bol = keys::line_start(tui.textarea.text(), cur);
            if cur <= bol {
                break;
            }
        } else {
            let eol = keys::line_end(tui.textarea.text(), cur);
            if cur >= eol {
                break;
            }
        }
        let (a, b) = if backward {
            (prev_char_boundary(tui.textarea.text(), cur), cur)
        } else {
            (cur, next_char_boundary(tui.textarea.text(), cur))
        };
        if let Some(s) = tui.textarea.text().get(a..b) {
            removed.insert_str(0, s);
        }
        tui.textarea.replace_range(a..b, "");
        cur = a;
        tui.textarea.set_cursor(cur);
    }
    if !removed.is_empty() {
        tui.vim.register = removed;
        tui.vim.register_linewise = false;
        after_edit(tui);
    }
}

fn paste(tui: &mut Tui, after: bool) {
    let reg = tui.vim.register.clone();
    let linewise = tui.vim.register_linewise;
    if reg.is_empty() {
        return;
    }
    let text = tui.textarea.text().to_string();
    let cur = tui.textarea.cursor();
    if linewise {
        if after {
            let eol = keys::line_end(&text, cur);
            tui.textarea.set_cursor(eol);
            tui.textarea.insert_str(&format!("\n{reg}"));
            tui.textarea.set_cursor(eol + 1);
        } else {
            let bol = keys::line_start(&text, cur);
            tui.textarea.set_cursor(bol);
            tui.textarea.insert_str(&format!("{reg}\n"));
            tui.textarea.set_cursor(bol);
        }
    } else {
        let pos = if after {
            next_char_boundary(&text, cur).min(keys::line_end(&text, cur))
        } else {
            cur
        };
        tui.textarea.set_cursor(pos);
        tui.textarea.insert_str(&reg);
        tui.textarea.set_cursor(pos);
    }
    after_edit(tui);
}

fn enter_insert(tui: &mut Tui) {
    tui.vim.mode = Mode::Insert;
    tui.vim.clear_pending();
}

fn append_insert(tui: &mut Tui) {
    let text = tui.textarea.text().to_string();
    let cur = tui.textarea.cursor();
    if cur < keys::line_end(&text, cur) {
        tui.textarea.set_cursor(next_char_boundary(&text, cur));
    }
    enter_insert(tui);
}

fn open_line(tui: &mut Tui, below: bool) {
    let text = tui.textarea.text().to_string();
    let cur = tui.textarea.cursor();
    let at = if below {
        keys::line_end(&text, cur)
    } else {
        keys::line_start(&text, cur)
    };
    tui.textarea.set_cursor(at);
    tui.textarea.insert_str("\n");
    if !below {
        tui.textarea.set_cursor(at);
    }
    after_edit(tui);
    enter_insert(tui);
}

fn set_first_nonblank(tui: &mut Tui) {
    let text = tui.textarea.text().to_string();
    let cur = tui.textarea.cursor();
    let bol = keys::line_start(&text, cur);
    let eol = keys::line_end(&text, cur);
    let mut p = bol;
    while p < eol {
        let Some(ch) = text[p..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        p += ch.len_utf8();
    }
    tui.textarea.set_cursor(p);
}

fn line_end_pos(tui: &Tui) -> usize {
    keys::line_end(tui.textarea.text(), tui.textarea.cursor())
}

fn move_left(tui: &mut Tui, n: usize) {
    for _ in 0..n {
        let bol = keys::line_start(tui.textarea.text(), tui.textarea.cursor());
        if tui.textarea.cursor() <= bol {
            break;
        }
        tui.textarea.move_left();
    }
}

fn move_right(tui: &mut Tui, n: usize) {
    for _ in 0..n {
        let eol = line_end_pos(tui);
        if tui.textarea.cursor() >= eol {
            break;
        }
        tui.textarea.move_right();
    }
}

fn move_word_start(tui: &mut Tui, n: usize) {
    let mut p = tui.textarea.cursor();
    for _ in 0..n {
        p = next_start(tui.textarea.text(), p);
    }
    tui.textarea.set_cursor(p);
}

fn move_word_back(tui: &mut Tui, n: usize) {
    let mut p = tui.textarea.cursor();
    for _ in 0..n {
        p = prev_start(tui.textarea.text(), p);
    }
    tui.textarea.set_cursor(p);
}

fn move_word_end(tui: &mut Tui, n: usize) {
    let mut p = tui.textarea.cursor();
    for _ in 0..n {
        p = word_end_pos(tui.textarea.text(), p);
    }
    tui.textarea.set_cursor(p);
}

fn vertical(tui: &mut Tui, n: usize, up: bool) -> Option<KeyOutcome> {
    let cur = tui.textarea.cursor();
    if up {
        if keys::line_start(tui.textarea.text(), cur) == 0 {
            return keys::apply(tui, Action::HistoryPrevious);
        }
        for _ in 0..n {
            if tui.textarea.cursor() == 0 {
                break;
            }
            tui.textarea.move_up();
        }
    } else {
        if keys::line_end(tui.textarea.text(), cur) >= tui.textarea.text().len() {
            return keys::apply(tui, Action::HistoryNext);
        }
        for _ in 0..n {
            tui.textarea.move_down();
        }
    }
    Some(KeyOutcome::Edited)
}

fn after_edit(tui: &mut Tui) {
    super::sync_attachment_markers(tui);
    tui.history_idx = None;
    tui.sel = 0;
}

fn next_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut n = pos + 1;
    while n < text.len() && !text.is_char_boundary(n) {
        n += 1;
    }
    n.min(text.len())
}

fn prev_char_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut n = pos - 1;
    while n > 0 && !text.is_char_boundary(n) {
        n -= 1;
    }
    n
}

fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn char_at(text: &str, pos: usize) -> Option<char> {
    text.get(pos..).and_then(|s| s.chars().next())
}

fn next_start(text: &str, pos: usize) -> usize {
    let len = text.len();
    let mut p = pos;
    if let Some(ch) = char_at(text, p) {
        let cls = class(ch);
        if cls != 0 {
            while p < len && char_at(text, p).is_some_and(|c| class(c) == cls) {
                p = next_char_boundary(text, p);
            }
        }
    }
    while p < len && char_at(text, p).is_some_and(|c| class(c) == 0) {
        p = next_char_boundary(text, p);
    }
    p
}

fn prev_start(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = pos;
    while p > 0 {
        p = prev_char_boundary(text, p);
        if char_at(text, p).is_some_and(|c| class(c) != 0) {
            break;
        }
    }
    let Some(ch) = char_at(text, p) else {
        return p;
    };
    let cls = class(ch);
    if cls == 0 {
        return p;
    }
    while p > 0 {
        let q = prev_char_boundary(text, p);
        if char_at(text, q).is_some_and(|c| class(c) == cls) {
            p = q;
        } else {
            break;
        }
    }
    p
}

fn word_end_pos(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    let mut p = next_char_boundary(text, pos);
    while p < len && char_at(text, p).is_some_and(|c| class(c) == 0) {
        p = next_char_boundary(text, p);
    }
    if p >= len {
        return len;
    }
    let cls = char_at(text, p).map(class).unwrap_or(1);
    while p < len && char_at(text, p).is_some_and(|c| class(c) == cls) {
        p = next_char_boundary(text, p);
    }
    prev_char_boundary(text, p)
}
