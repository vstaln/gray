//! Rendering bridge — routes TUI content through optional rich_output.
//!
//! 1:1 port of `tui_gateway/render.py` (49 LOC).
//! When `agent.rich_output` exists its functions are used; when it doesn't
//! everything returns `None` and the TUI falls back to its own markdown
//! renderer (`markdown.tsx` in the original, `tui.rs`/`print.rs` here).

/// Render a message via `agent.rich_output::format_response` if available.
///
/// Python:
///
/// ```python
/// def render_message(text: str, cols: int = 80) -> str | None:
///     try: from agent.rich_output import format_response
///     except ImportError: return None
///     try: return format_response(text, cols=cols)
///     except TypeError: return format_response(text)
///     except Exception: return None
/// ```
///
/// Rust: no `agent.rich_output` crate is wired in this build, so the
/// `ImportError` path is taken and `None` is returned. The `TypeError`
/// fallback (cols vs no-cols) and the blanket `Exception -> None` are
/// preserved as docs for when a backend is injected via the `RichOutput`
/// trait below.
pub fn render_message(text: &str, cols: usize) -> Option<String> {
    let _ = (text, cols);
    // ImportError path — no backend present, fall back to TUI markdown.
    None
}

/// Render a diff via `agent.rich_output::render_diff` if available.
///
/// Mirrors `render_diff(text, cols=80)` with the same
/// `ImportError -> None`, `TypeError(cols) -> retry without cols`,
/// `Exception -> None` chain as `render_message`.
pub fn render_diff(text: &str, cols: usize) -> Option<String> {
    let _ = (text, cols);
    None
}

/// Opaque handle mirroring `agent.rich_output::StreamingRenderer`.
///
/// Python keeps the instance alive and feeds it deltas; here it is a
/// plain data holder so call sites can thread it through even when the
/// backend is absent (they just get `None` from `make_stream_renderer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingRenderer {
    /// Terminal width the renderer was created for (default 80).
    pub cols: usize,
}

impl StreamingRenderer {
    pub fn new(cols: usize) -> Self {
        Self { cols }
    }
}

/// Create a `StreamingRenderer` if `agent.rich_output` is available.
///
/// Python:
///
/// ```python
/// def make_stream_renderer(cols: int = 80):
///     try: from agent.rich_output import StreamingRenderer
///     except ImportError: return None
///     try: return StreamingRenderer(cols=cols)
///     except TypeError: return StreamingRenderer()
///     except Exception: return None
/// ```
///
/// Rust again takes the `ImportError -> None` path by default. The
/// `TypeError` fallback (try `cols`, then no-arg) is documented for a
/// future `RichOutput::streaming_renderer(Option<usize>)` impl.
pub fn make_stream_renderer(cols: usize) -> Option<StreamingRenderer> {
    let _ = cols;
    None
}

// ponytail: optional trait for wiring a real backend without touching
// call sites above — YAGNI until a `rich_output` equivalent exists.
// When needed, implement this and thread it through a context struct
// rather than adding a global / feature-flag edge here.
#[allow(dead_code)]
trait RichOutput {
    fn format_response(&self, text: &str, cols: Option<usize>) -> Option<String>;
    fn render_diff(&self, text: &str, cols: Option<usize>) -> Option<String>;
    fn streaming_renderer(&self, cols: Option<usize>) -> Option<StreamingRenderer>;
}
