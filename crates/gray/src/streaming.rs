//! Minimal streaming collector — verbatim Codex `markdown_stream.rs` logic
//! trimmed to production only ( ponytail: newline-gated source buffer ).
//! Original: `codex-rs/tui/src/markdown_stream.rs` — `MarkdownStreamCollector`
//! buffers token deltas and only commits at newline boundaries so incomplete
//! markdown blocks don't re-render mid-token. Tidy: removed tracing/test
//! helpers + `width`/`cwd`/`crate::markdown` deps, kept pure `String` buffer.

use std::path::Path;

/// Newline-gated accumulator — buffers raw markdown deltas, commits only
/// completed lines (last `\n`). Lets the controller re-render whole source
/// while appending only new content. No markdown parsing here.
pub struct MarkdownStreamCollector {
    buffer: String,
    committed_source_len: usize,
}

impl MarkdownStreamCollector {
    pub fn new(_width: Option<usize>, _cwd: &Path) -> Self {
        Self { buffer: String::new(), committed_source_len: 0 }
    }
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.committed_source_len = 0;
    }
    pub fn push_delta(&mut self, delta: &str) {
        self.buffer.push_str(delta);
    }
    /// Commit newly completed source up to last `\n`. Returns range of
    /// newly-committed bytes, or `None` if no newline yet (prevents live
    /// rendering of incomplete blocks).
    pub fn commit_complete_source(&mut self) -> Option<std::ops::Range<usize>> {
        let commit_end = self.buffer.rfind('\n').map(|i| i + 1)?;
        let start = self.committed_source_len;
        if commit_end <= start { return None; }
        self.committed_source_len = commit_end;
        Some(start..commit_end)
    }
    pub fn committed_source(&self) -> &str {
        &self.buffer[..self.committed_source_len]
    }
    /// Finalize: take entire source (including final line without `\n`).
    pub fn finalize_and_take_source(&mut self) -> String {
        let s = self.buffer.clone();
        self.clear();
        s
    }
}
