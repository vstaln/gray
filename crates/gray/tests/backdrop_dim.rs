//! Backdrop dim is carried by the color map, not just SGR faint:
//! terminals that ignore `Modifier::DIM` (web UIs, some GPU terms) must
//! still see the background drop behind modals.
use gray::composer::build_welcome_lines;
use gray::setup::{BackgroundSnapshot, dim_color, render_dimmed_background};

fn draw_bg(w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let bg = BackgroundSnapshot {
        transcript: build_welcome_lines(w as usize),
        history_entries: vec![],
        cwd: "/tmp".to_string(),
        model_name: "m".to_string(),
        thinking_effort: "xhigh".to_string(),
        prompt_text: "/thinking".to_string(),
        used_tokens: 0,
        cache_hit_rate: 0.0,
    };
    terminal
        .draw(|f| render_dimmed_background(f, &bg))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn cells_with(buf: &ratatui::buffer::Buffer, sym: &str) -> Vec<(u16, u16)> {
    let area = buf.area;
    let mut out = vec![];
    for y in 0..area.height {
        for x in 0..area.width {
            if buf[(x, y)].symbol() == sym {
                out.push((x, y));
            }
        }
    }
    out
}

#[test]
fn backdrop_input_arrow_fg_is_color_dimmed() {
    let theme = gray::theme::theme();
    let buf = draw_bg(150, 45);
    let pos = cells_with(&buf, "\u{276f}");
    assert!(!pos.is_empty(), "arrow cell must exist");
    for (x, y) in pos {
        assert_eq!(
            buf[(x, y)].fg,
            dim_color(theme.text_faint),
            "input arrow fg must go through dim_color, not full text_faint"
        );
    }
}

#[test]
fn backdrop_footer_fg_is_color_dimmed() {
    let theme = gray::theme::theme();
    let buf = draw_bg(150, 45);
    // footer row carries the cache readout; find its cells by content
    let area = buf.area;
    let mut found = 0;
    for y in 0..area.height {
        let row: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        if row.contains("cache") {
            for x in 0..area.width {
                let c = &buf[(x, y)];
                if !c.symbol().trim().is_empty() {
                    assert_eq!(
                        c.fg,
                        dim_color(theme.text_faint),
                        "footer fg must go through dim_color, not full text_faint"
                    );
                    found += 1;
                }
            }
        }
    }
    assert!(found > 0, "footer row must exist");
}
