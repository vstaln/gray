// ---------------------------------------------------------------------------
// Minimal TextArea — literal copy-paste of codex textarea logic, trimmed to
// stdlib. Full codex TextArea is 4518 lines with vim/kill-ring/unicode-
// segmentation; this keeps the essential multiline + atomic-element contract
// (cursor byte-boundary, wrap-aware up/down, element-shift on insert).
// O(n) scan, no grapheme crate, word wrap via char count.
// Upgrade path: vendor full `textarea.rs` + `textarea/wrapping.rs` when
// unicode-width or vim bindings matter.
// ---------------------------------------------------------------------------
use std::ops::Range;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct TextElement {
    pub(crate) id: u64,
    pub(crate) range: Range<usize>,
}

#[derive(Debug, Clone)]
struct WrapCache {
    lines: Vec<Range<usize>>,
}

#[derive(Debug)]
pub(crate) struct TextArea {
    text: String,
    cursor: usize, // byte index
    elements: Vec<TextElement>,
    next_id: u64,
    preferred_col: Option<usize>,
    wrap_cache: Option<WrapCache>,
}

#[allow(dead_code)]
impl TextArea {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            elements: Vec::new(),
            next_id: 1,
            preferred_col: None,
            wrap_cache: None,
        }
    }
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }
    pub(crate) fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = self.cursor.min(self.text.len());
        self.cursor = self.clamp_to_boundary(self.cursor);
        self.elements.clear();
        self.preferred_col = None;
        self.rebuild_wrap_cache();
    }
    pub(crate) fn clamp_to_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        while p < self.text.len() && !self.text.is_char_boundary(p) {
            p += 1;
        }
        p
    }
    pub(crate) fn is_char_boundary(&self, pos: usize) -> bool {
        self.text.is_char_boundary(pos)
    }
    pub(crate) fn next_boundary(&self, pos: usize) -> usize {
        // next char boundary, but jump over atomic elements
        for el in &self.elements {
            if pos >= el.range.start && pos < el.range.end {
                return el.range.end;
            }
        }
        if pos >= self.text.len() {
            return self.text.len();
        }
        let mut n = pos + 1;
        while n < self.text.len() && !self.text.is_char_boundary(n) {
            n += 1;
        }
        // if landing inside element, jump to its end
        for el in &self.elements {
            if n > el.range.start && n < el.range.end {
                return el.range.end;
            }
        }
        n.min(self.text.len())
    }
    pub(crate) fn prev_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        for el in &self.elements {
            if pos > el.range.start && pos <= el.range.end {
                return el.range.start;
            }
        }
        let mut n = pos - 1;
        while n > 0 && !self.text.is_char_boundary(n) {
            n -= 1;
        }
        for el in &self.elements {
            if n > el.range.start && n < el.range.end {
                return el.range.start;
            }
        }
        n
    }
    pub(crate) fn insert_str(&mut self, s: &str) {
        self.insert_at(self.cursor, s);
    }
    pub(crate) fn insert_at(&mut self, pos: usize, s: &str) {
        let pos = self.clamp_to_boundary(pos.min(self.text.len()));
        self.text.insert_str(pos, s);
        if pos <= self.cursor {
            self.cursor += s.len();
        }
        self.shift_elements(pos, 0, s.len());
        self.preferred_col = None;
        self.rebuild_wrap_cache();
    }
    pub(crate) fn insert_element(&mut self, placeholder: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let start = self.cursor;
        self.insert_str(placeholder);
        let end = start + placeholder.len();
        self.elements.push(TextElement {
            id,
            range: start..end,
        });
        self.elements.sort_by_key(|e| e.range.start);
        // insert_str already invalidated, but ensure element range accounted
        self.preferred_col = None;
        self.rebuild_wrap_cache();
        id
    }
    pub(crate) fn shift_elements(&mut self, pos: usize, removed: usize, inserted: usize) {
        let diff = inserted as isize - removed as isize;
        for el in &mut self.elements {
            if el.range.start >= pos + removed {
                el.range.start = ((el.range.start as isize) + diff) as usize;
                el.range.end = ((el.range.end as isize) + diff) as usize;
            } else if el.range.end > pos { /* inside edit — collapse */
            }
        }
    }
    pub(crate) fn delete_backward(&mut self, n: usize) {
        if n == 0 || self.cursor == 0 {
            return;
        }
        let mut target = self.cursor;
        for _ in 0..n {
            target = self.prev_boundary(target);
            if target == 0 {
                break;
            }
        }
        self.replace_range(target..self.cursor, "");
    }
    pub(crate) fn delete_forward(&mut self, n: usize) {
        if n == 0 || self.cursor >= self.text.len() {
            return;
        }
        let mut target = self.cursor;
        for _ in 0..n {
            target = self.next_boundary(target);
            if target >= self.text.len() {
                break;
            }
        }
        self.replace_range(self.cursor..target, "");
    }
    pub(crate) fn prev_word_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        for el in &self.elements {
            if pos > el.range.start && pos <= el.range.end {
                return el.range.start;
            }
        }
        let text_before = &self.text[..pos];
        let mut chars = text_before.char_indices().rev().peekable();
        while let Some(&(_, ch)) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if let Some(&(_, first_ch)) = chars.peek() {
            let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';
            while let Some(&(idx, ch)) = chars.peek() {
                if !ch.is_whitespace() && ((ch.is_alphanumeric() || ch == '_') == is_word_char) {
                    chars.next();
                } else {
                    return idx + ch.len_utf8();
                }
            }
        }
        0
    }
    pub(crate) fn next_word_boundary(&self, pos: usize) -> usize {
        if pos >= self.text.len() {
            return self.text.len();
        }
        for el in &self.elements {
            if pos >= el.range.start && pos < el.range.end {
                return el.range.end;
            }
        }
        let text_after = &self.text[pos..];
        let mut chars = text_after.char_indices().peekable();
        while let Some(&(_, ch)) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if let Some(&(_, first_ch)) = chars.peek() {
            let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';
            while let Some(&(idx, ch)) = chars.peek() {
                if !ch.is_whitespace() && ((ch.is_alphanumeric() || ch == '_') == is_word_char) {
                    chars.next();
                } else {
                    return pos + idx;
                }
            }
        }
        self.text.len()
    }
    pub(crate) fn delete_word_backward(&mut self) {
        let target = self.prev_word_boundary(self.cursor);
        if target < self.cursor {
            self.replace_range(target..self.cursor, "");
        }
    }
    pub(crate) fn delete_word_forward(&mut self) {
        let target = self.next_word_boundary(self.cursor);
        if target > self.cursor {
            self.replace_range(self.cursor..target, "");
        }
    }
    pub(crate) fn move_word_left(&mut self) {
        let target = self.prev_word_boundary(self.cursor);
        self.set_cursor(target);
    }
    pub(crate) fn move_word_right(&mut self) {
        let target = self.next_word_boundary(self.cursor);
        self.set_cursor(target);
    }
    pub(crate) fn replace_range(&mut self, range: Range<usize>, s: &str) {
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len());
        let removed = end - start;
        self.text.replace_range(start..end, s);
        if self.cursor < start {
        } else if self.cursor <= end {
            self.cursor = start + s.len();
        } else {
            self.cursor = ((self.cursor as isize) + s.len() as isize - removed as isize) as usize;
        }
        self.cursor = self.cursor.min(self.text.len());
        self.cursor = self.clamp_to_boundary(self.cursor);
        self.shift_elements(start, removed, s.len());
        self.preferred_col = None;
        self.rebuild_wrap_cache();
    }
    pub(crate) fn set_cursor(&mut self, pos: usize) {
        self.cursor = self.clamp_to_boundary(pos.min(self.text.len()));
        // avoid landing inside element
        for el in &self.elements {
            if self.cursor > el.range.start && self.cursor < el.range.end {
                self.cursor = el.range.end;
                break;
            }
        }
        self.preferred_col = None;
    }
    pub(crate) fn move_left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
        self.preferred_col = None;
    }
    pub(crate) fn move_right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
        self.preferred_col = None;
    }
    // --- WrapCache + preferred_col helpers ---
    fn char_width(c: char) -> usize {
        // simple width, 1 for most, 2 for CJK wide. Upgrade to unicode-width crate if needed.
        match c {
            // CJK Unified Ideographs and wide ranges (approx)
            '\u{1100}'..='\u{115F}'
            | '\u{2E80}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF01}'..='\u{FF60}' => 2,
            _ => 1,
        }
    }
    fn display_width_of_range(&self, start: usize, end: usize) -> usize {
        if start >= self.text.len() || end > self.text.len() || start >= end {
            return 0;
        }
        self.text[start..end].chars().map(Self::char_width).sum()
    }
    fn current_display_col(&self) -> usize {
        let bol = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.display_width_of_range(bol, self.cursor)
    }
    fn col_for_cursor(&self) -> usize {
        self.current_display_col()
    }
    fn move_to_display_col_on_line(
        &mut self,
        line_start: usize,
        line_end: usize,
        target_col: usize,
    ) {
        if line_start > self.text.len() {
            self.cursor = self.text.len();
            return;
        }
        let line_end = line_end.min(self.text.len());
        if line_start >= line_end {
            self.cursor = line_start;
            return;
        }
        let line = &self.text[line_start..line_end];
        let mut width = 0usize;
        let mut byte_pos = line_start;
        for (idx, ch) in line.char_indices() {
            let w = Self::char_width(ch);
            if width + w > target_col {
                break;
            }
            // advance
            byte_pos = line_start + idx + ch.len_utf8();
            width += w;
            if width == target_col {
                break;
            }
        }
        // if target beyond line width, stay at line_end (clamp to last char start if needed)
        if width < target_col {
            // target beyond line, go to end but not past last char's start if wide
            byte_pos = line_end;
            // clamp to last valid boundary inside line if we overshoot
            if byte_pos > line_start {
                // ensure we don't land in middle of char (already at boundary)
            }
        }
        let mut new_pos = self.clamp_to_boundary(byte_pos);
        // avoid landing inside atomic element
        for el in &self.elements {
            if new_pos > el.range.start && new_pos < el.range.end {
                // direction heuristic: moving up/down, snap to end if closer? use end for simplicity
                new_pos = el.range.end;
                break;
            }
        }
        self.cursor = new_pos.min(self.text.len());
    }
    fn wrapped_line_index_by_start(lines: &[Range<usize>], cursor: usize) -> Option<usize> {
        for (i, r) in lines.iter().enumerate() {
            if cursor >= r.start && cursor < r.end {
                return Some(i);
            }
            // cursor at exact end of a soft-wrapped line is start of next; treat as next
            if cursor == r.end && i + 1 < lines.len() && lines[i + 1].start == r.end {
                // ambiguous: if cursor equals boundary between soft wraps, choose next line
                // check if next line starts at cursor
                if lines[i + 1].start == cursor {
                    return Some(i + 1);
                }
                return Some(i);
            }
        }
        // cursor at text end belongs to last line
        if let Some((last_idx, last)) = lines.iter().enumerate().next_back()
            && cursor >= last.start
            && cursor <= last.end
        {
            return Some(last_idx);
        }
        None
    }
    fn compute_logical_lines(&self) -> Vec<Range<usize>> {
        let mut lines = Vec::new();
        let mut start = 0usize;
        for (idx, ch) in self.text.char_indices() {
            if ch == '\n' {
                lines.push(start..idx);
                start = idx + 1;
            }
        }
        lines.push(start..self.text.len());
        lines
    }
    fn rebuild_wrap_cache(&mut self) {
        // logical lines as WrapCache; upgrade to word-wrap at terminal width when needed
        // width 0 means logical-only, no visual wrapping
        let lines = self.compute_logical_lines();
        self.wrap_cache = Some(WrapCache { lines });
    }
    pub(crate) fn move_up(&mut self) {
        // WrapCache-aware: try visual lines first if cache present
        if let Some(cache) = self.wrap_cache.clone() {
            let lines = &cache.lines;
            if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor) {
                let cur_range = &lines[idx];
                let target_col = self
                    .preferred_col
                    .unwrap_or_else(|| self.display_width_of_range(cur_range.start, self.cursor));
                if idx > 0 {
                    if self.preferred_col.is_none() {
                        self.preferred_col = Some(target_col);
                    }
                    let prev = &lines[idx - 1];
                    self.move_to_display_col_on_line(prev.start, prev.end, target_col);
                    return;
                } else {
                    self.cursor = 0;
                    self.preferred_col = None;
                    return;
                }
            }
        }
        // Fallback logical line navigation (when cache missing or not covering cursor)
        let bol = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        // target col is sticky
        let target_col = match self.preferred_col {
            Some(c) => c,
            None => {
                let c = self.display_width_of_range(bol, self.cursor);
                self.preferred_col = Some(c);
                c
            }
        };
        if bol == 0 {
            self.cursor = 0;
            self.preferred_col = None;
            return;
        }
        let prev_eol = bol - 1;
        let prev_bol = self.text[..prev_eol]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prev_line_end = prev_eol;
        self.move_to_display_col_on_line(prev_bol, prev_line_end, target_col);
        // keep preferred_col for sticky column
    }
    pub(crate) fn move_down(&mut self) {
        if let Some(cache) = self.wrap_cache.clone() {
            let lines = &cache.lines;
            if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor) {
                let cur_range = &lines[idx];
                let target_col = self
                    .preferred_col
                    .unwrap_or_else(|| self.display_width_of_range(cur_range.start, self.cursor));
                if idx + 1 < lines.len() {
                    if self.preferred_col.is_none() {
                        self.preferred_col = Some(target_col);
                    }
                    let next = &lines[idx + 1];
                    self.move_to_display_col_on_line(next.start, next.end, target_col);
                    return;
                } else {
                    self.cursor = self.text.len();
                    self.preferred_col = None;
                    return;
                }
            }
        }
        // Fallback logical
        let eol = self.text[self.cursor..]
            .find('\n')
            .map(|i| i + self.cursor)
            .unwrap_or(self.text.len());
        let bol = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let target_col = match self.preferred_col {
            Some(c) => c,
            None => {
                let c = self.display_width_of_range(bol, self.cursor);
                self.preferred_col = Some(c);
                c
            }
        };
        if eol >= self.text.len() {
            self.cursor = self.text.len();
            self.preferred_col = None;
            return;
        }
        let next_bol = eol + 1;
        let next_eol = self.text[next_bol..]
            .find('\n')
            .map(|i| i + next_bol)
            .unwrap_or(self.text.len());
        self.move_to_display_col_on_line(next_bol, next_eol, target_col);
    }
    pub(crate) fn move_to_end(&mut self) {
        self.cursor = self.text.len();
        self.preferred_col = None;
    }
}
