use super::*;

impl Tui {
    pub(crate) fn upsert_live_tool(&mut self, id: &str, header: Vec<Line<'static>>, running: bool) {
        if let Some(slot) = self.live_tools.iter_mut().find(|t| t.id == id) {
            slot.header = header;
            slot.running = running;
        } else {
            self.live_tools.push(LiveTool {
                id: id.to_string(),
                header,
                running,
                tail: LiveToolTail::default(),
            });
        }
        let _ = self.draw();
    }

    pub(crate) fn push_live_tool_output(&mut self, id: &str, bytes: &[u8]) {
        if let Some(slot) = self.live_tools.iter_mut().find(|t| t.id == id) {
            slot.tail.feed(bytes);
        }
    }

    pub(crate) fn remove_live_tool(&mut self, id: &str) {
        if let Some(pos) = self.live_tools.iter().position(|t| t.id == id) {
            self.live_tools.remove(pos);
            let _ = self.draw();
        }
    }

    pub(crate) fn live_tool_rows(&self, w: usize) -> Vec<Line<'static>> {
        let elapsed = self
            .turn_started
            .or_else(|| self.status.as_ref().map(|(started, _)| *started))
            .map(|started| started.elapsed())
            .unwrap_or_default();
        live_tool_rows(&self.live_tools, elapsed, w)
    }

    pub(crate) fn stream_tail_rows(&self, w: usize) -> Vec<Line<'static>> {
        let view = self.markdown_renderer.view();
        let start = self.committed_markdown_lines.min(view.lines.len());
        let lines = &view.lines[start..];
        let max_w = transcript::prose_width(w);
        let mut out = Vec::new();
        for line in lines {
            for mut row in transcript::wrap_styled_line(line.clone(), max_w) {
                if !row.spans.is_empty() {
                    row.spans.insert(0, transcript::left_pad());
                }
                out.push(row);
            }
        }
        out
    }

    pub(crate) fn live_tool_overflow(&self) -> usize {
        live_tool_overflow(self.live_tools.len())
    }

    pub(crate) fn clear_live_tools(&mut self) {
        if !self.live_tools.is_empty() {
            self.live_tools.clear();
            let _ = self.draw();
        }
    }
}
