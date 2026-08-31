pub mod catalog;
pub use catalog::{
    build_connect_items, gray_home, load_catalog, load_saved_config_at, mask_key_pretty,
    save_saved_config_at, saved_config_path, Catalog, CatalogModel, CatalogProvider,
    ConnectItem, SavedConfig, PROVIDERS_JSON,
};
pub(crate) use catalog::{load_auth_keys, save_auth_key};

use crate::{config::Config, tui::print_wrapped};

/// Converts a raw model ID to a friendly human-readable display name.
pub fn friendly_model_name(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    let name = model_id.split('/').last().unwrap_or(model_id);
    let words: Vec<String> = name
        .split(['-', '_', ':'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            if lower == "gpt" || lower == "glm" || lower == "ai" || lower == "api" {
                w.to_uppercase()
            } else if lower.starts_with('v') && lower.len() > 1 && lower[1..].chars().all(|c| c.is_ascii_digit() || c == '.') {
                format!("v{}", &lower[1..])
            } else {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            }
        })
        .collect();
    words.join(" ")
}

/// Returns the models list for a provider from the catalog.
pub fn get_provider_models(_provider_id: &str, _catalog: &Catalog) -> Vec<(String, String)> {
    Vec::new()
}

/// Dynamically queries the provider's live /models endpoint (e.g. OpenAI, OpenRouter, Ollama, vLLM, LMStudio, etc.).
pub fn fetch_live_provider_models(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let base = base_url.to_string();
        let key = api_key.map(|k| k.to_string());
        std::thread::scope(|s| {
            s.spawn(move || {
                handle.block_on(async move {
                    let client = match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(3000))
                        .user_agent("gray/0.1.0")
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return Vec::new(),
                    };

                    let trimmed_base = base.trim_end_matches('/');
                    let endpoints = if trimmed_base.contains("openrouter.ai") {
                        vec!["https://openrouter.ai/api/v1/models".to_string()]
                    } else if trimmed_base.ends_with("/v1") {
                        vec![
                            format!("{trimmed_base}/models"),
                            format!("{trimmed_base}/tags"),
                        ]
                    } else {
                        vec![
                            format!("{trimmed_base}/models"),
                            format!("{trimmed_base}/v1/models"),
                            format!("{trimmed_base}/api/tags"),
                            format!("{trimmed_base}/api/v1/models"),
                        ]
                    };

                    for url in endpoints {
                        let mut req = client.get(&url);
                        if let Some(k) = &key {
                            if !k.is_empty() {
                                req = req.header("Authorization", format!("Bearer {k}"));
                            }
                        }
                        if url.contains("openrouter") {
                            req = req.header("HTTP-Referer", "https://github.com/vstaln/gray");
                            req = req.header("X-Title", "Gray");
                        }

                        if let Ok(resp) = req.send().await {
                            if resp.status().is_success() {
                                if let Ok(json) = resp.json::<serde_json::Value>().await {
                                    let mut models = Vec::new();
                                    let items_opt = if let Some(arr) = json.as_array() {
                                        Some(arr)
                                    } else if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                                        Some(arr)
                                    } else if let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                                        Some(arr)
                                    } else {
                                        None
                                    };

                                    if let Some(items) = items_opt {
                                        for item in items {
                                            let id = item.get("id")
                                                .or_else(|| item.get("name"))
                                                .or_else(|| item.get("model"))
                                                .and_then(|v| v.as_str());
                                            if let Some(id_str) = id {
                                                let name = item.get("name")
                                                    .or_else(|| item.get("display_name"))
                                                    .and_then(|n| n.as_str())
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| friendly_model_name(id_str));
                                                if let Some(len) = extract_context_length_from_json(item) {
                                                    cache_model_context(id_str, len);
                                                }
                                                models.push((id_str.to_string(), name));
                                            }
                                        }
                                    }
                                    if !models.is_empty() {
                                        return models;
                                    }
                                }
                            }
                        }
                    }

                    Vec::new()
                })
            }).join().unwrap_or_default()
        })
    } else {
        Vec::new()
    }
}

/// Returns the models list for a provider dynamically from live endpoint.
pub fn get_provider_models_with_live(
    _provider_id: &str,
    base_url: &str,
    api_key: Option<&str>,
    _catalog: &Catalog,
) -> Vec<(String, String)> {
    fetch_live_provider_models(base_url, api_key)
}

static MODEL_CONTEXT_CACHE: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, usize>>> = std::sync::OnceLock::new();

fn model_context_cache() -> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
    MODEL_CONTEXT_CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_context(model_id: &str, length: usize) {
    if length == 0 { return; }
    if let Ok(mut g) = model_context_cache().write() {
        g.insert(model_id.to_string(), length);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, length);
        }
    }
}

pub fn get_cached_model_context(model_id: &str) -> Option<usize> {
    if let Ok(g) = model_context_cache().read() {
        if let Some(v) = g.get(model_id).copied() {
            return Some(v);
        }
        let lower = model_id.to_lowercase();
        if let Some(v) = g.get(&lower).copied() {
            return Some(v);
        }
    }
    None
}

pub fn extract_context_length_from_json(val: &serde_json::Value) -> Option<usize> {
    const KEYS: &[&str] = &[
        "context_length",
        "context_window",
        "max_context_length",
        "max_context_window",
        "max_position_embeddings",
        "max_model_len",
        "max_input_tokens",
        "max_sequence_length",
        "max_seq_len",
        "n_ctx_train",
        "n_ctx",
        "ctx_size",
    ];
    for key in KEYS {
        if let Some(v) = val.get(*key) {
            if let Some(n) = v.as_u64() {
                if n > 0 {
                    return Some(n as usize);
                }
            } else if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    if n > 0 {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

pub fn format_context_length(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        let val = tokens as f64 / 1_000_000.0;
        let rounded = val.round();
        if (val - rounded).abs() < 0.05 {
            format!("{:.0}M", rounded)
        } else {
            format!("{:.1}M", val)
        }
    } else if tokens >= 1_000 {
        let val = tokens as f64 / 1000.0;
        let rounded = val.round();
        if (val - rounded).abs() < 0.05 {
            format!("{:.0}k", rounded)
        } else {
            let val_kibi = tokens as f64 / 1024.0;
            let rounded_kibi = val_kibi.round();
            if (val_kibi - rounded_kibi).abs() < 0.05 {
                format!("{:.0}k", rounded_kibi)
            } else {
                format!("{:.1}k", val)
            }
        }
    } else {
        tokens.to_string()
    }
}

pub fn resolve_model_context_length(model_name: &str) -> usize {
    if let Some(cached) = get_cached_model_context(model_name) {
        return cached;
    }
    let lower = model_name.to_lowercase();
    if let Some(cached) = get_cached_model_context(&lower) {
        return cached;
    }

    // Advertised model context capacities
    if lower.contains("gemini-1.5-pro") || lower.contains("gemini-2.0") || lower.contains("gemini-2.5") || lower.contains("gemini-1.5-flash") || lower.contains("gemini") {
        1_048_576
    } else if lower.contains("claude-opus-4") || lower.contains("claude-sonnet-4") || lower.contains("claude-4") || lower.contains("claude-5") {
        1_000_000
    } else if lower.contains("claude-3") || lower.contains("claude") {
        200_000
    } else if lower.contains("gpt-5") || lower.contains("gpt-4.5") || lower.contains("gpt-4.1") {
        1_048_576
    } else if lower.contains("gpt-4o") || lower.contains("o1") || lower.contains("o3") || lower.contains("gpt-4-turbo") {
        128_000
    } else if lower.contains("gpt-4-32k") {
        32_768
    } else if lower.contains("gpt-4") {
        8_192
    } else if lower.contains("gpt-3.5-turbo-16k") {
        16_384
    } else if lower.contains("gpt-3.5") {
        4_096
    } else if lower.contains("deepseek-v4") {
        1_000_000
    } else if lower.contains("deepseek-chat") || lower.contains("deepseek-reasoner") || lower.contains("deepseek-v3") || lower.contains("deepseek-r1") || lower.contains("deepseek") {
        131_072
    } else if lower.contains("qwen3") {
        1_000_000
    } else if lower.contains("qwen2.5") || lower.contains("qwen") {
        131_072
    } else if lower.contains("grok-4") {
        2_000_000
    } else if lower.contains("grok-3") || lower.contains("grok-2") || lower.contains("grok") {
        131_072
    } else if lower.contains("llama-3.3") || lower.contains("llama-3.2") || lower.contains("llama-3.1") {
        131_072
    } else if lower.contains("llama-3") {
        8_192
    } else if lower.contains("mistral-large") || lower.contains("codestral") {
        128_000
    } else if lower.contains("kimi-k3") {
        1_048_576
    } else if lower.contains("kimi") {
        262_144
    } else if lower.contains("glm-5") {
        1_048_576
    } else if lower.contains("glm") {
        128_000
    } else if lower.contains("1m") {
        1_000_000
    } else if lower.contains("2m") {
        2_000_000
    } else if lower.contains("128k") {
        128_000
    } else if lower.contains("256k") {
        256_000
    } else {
        256_000
    }
}

/// Returns the model context limit in tokens and a human-friendly label (e.g. 256_000, "256k").
pub fn model_context_info(model_name: &str) -> (usize, String) {
    let tokens = resolve_model_context_length(model_name);
    let label = format_context_length(tokens);
    (tokens, label)
}

/// Snapshot of background UI to render dimmed underneath popups.
#[derive(Debug, Clone, Default)]
pub struct BackgroundSnapshot {
    pub transcript: Vec<ratatui::text::Line<'static>>,
    pub cwd: String,
    pub model_name: String,
    pub thinking_effort: String,
    pub prompt_text: String,
    pub used_tokens: usize,
    pub cache_hit_rate: f64,
}

impl BackgroundSnapshot {
    pub fn default_initial() -> Self {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};

        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let w = cols as usize;
        let logo_raw = crate::tui::logo_lines();
        let l_rows = logo_raw.len().max(1) as f32;
        let max_logo_w = logo_raw.iter().map(|l| l.trim_end().chars().count()).max().unwrap_or(0);
        let l_cols = (max_logo_w as f32).max(1.0);
        let logo_pad = w.saturating_sub(max_logo_w) / 2;

        let base = Color::Rgb(110, 110, 110);
        let hilite = Color::Rgb(240, 240, 240);

        let mut welcome_lines: Vec<Line<'static>> = Vec::new();
        welcome_lines.push(Line::from(""));
        for (row, line) in logo_raw.iter().enumerate() {
            let trimmed = line.trim_end();
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::raw(" ".repeat(logo_pad)));
            for (col, ch) in trimmed.chars().enumerate() {
                let diag = (col as f32 + (l_rows - 1.0 - row as f32)) / (l_cols + l_rows);
                let t = (0.15 + 0.85 * diag).clamp(0.0, 1.0);
                let color = crate::tui::blend_color(base, hilite, t);
                spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            }
            welcome_lines.push(Line::from(spans));
        }
        welcome_lines.push(Line::from(""));
        let banner_raw = format!("gray {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION"));
        let banner_len = banner_raw.chars().count();
        let pad = w.saturating_sub(banner_len) / 2;
        welcome_lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled("gray", Style::default().add_modifier(Modifier::BOLD).fg(Color::Rgb(225, 225, 225))),
            Span::styled(format!(" {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::Rgb(140, 140, 140))),
        ]));
        welcome_lines.push(Line::from(""));

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        Self {
            transcript: welcome_lines,
            cwd,
            model_name: String::new(),
            thinking_effort: "high".to_string(),
            prompt_text: String::new(),
            used_tokens: 0,
            cache_hit_rate: 0.0,
        }
    }
}

pub fn dim_color(c: ratatui::style::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match c {
        Color::Rgb(r, g, b) => {
            let k = 0.38f32;
            let r2 = ((r as f32) * k).round() as u8;
            let g2 = ((g as f32) * k).round() as u8;
            let b2 = ((b as f32) * k).round() as u8;
            Color::Rgb(r2, g2, b2)
        }
        Color::White => Color::Rgb(85, 85, 85),
        Color::Gray => Color::Rgb(60, 60, 60),
        Color::DarkGray => Color::Rgb(40, 40, 40),
        Color::Black => Color::Rgb(8, 8, 8),
        Color::Green => Color::Rgb(30, 90, 50),
        Color::Yellow => Color::Rgb(95, 80, 30),
        Color::Blue => Color::Rgb(35, 50, 90),
        Color::Magenta => Color::Rgb(70, 35, 70),
        Color::Cyan => Color::Rgb(35, 70, 70),
        Color::Red => Color::Rgb(90, 35, 35),
        Color::Reset => Color::Rgb(60, 60, 60),
        other => other,
    }
}

pub fn dim_style(style: ratatui::style::Style) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style};
    let mut s = Style::default().add_modifier(Modifier::DIM);
    if let Some(fg) = style.fg {
        s = s.fg(dim_color(fg));
    } else {
        s = s.fg(Color::Rgb(70, 70, 70));
    }
    if let Some(bg) = style.bg {
        s = s.bg(dim_color(bg));
    }
    s
}

pub fn dim_line(line: &ratatui::text::Line<'_>) -> ratatui::text::Line<'static> {
    use ratatui::text::{Line, Span};
    let spans: Vec<Span<'static>> = line.spans.iter().map(|span| {
        Span::styled(span.content.to_string(), dim_style(span.style))
    }).collect();
    let mut new_line = Line::from(spans);
    new_line.style = dim_style(line.style);
    new_line
}

pub fn render_dimmed_background(frame: &mut ratatui::Frame, bg: &BackgroundSnapshot) {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let area = frame.area();
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let box_bg = Color::Rgb(10, 10, 10);
    let prompt_arrow_color = Color::Rgb(70, 70, 70);
    let text_dimmed_color = Color::Rgb(85, 85, 85);
    let footer_cwd_color = Color::Rgb(48, 48, 48);
    let footer_model_color = Color::Rgb(58, 58, 58);

    let arrow_span = Span::styled("❯ ", Style::default().fg(prompt_arrow_color).add_modifier(Modifier::DIM).bg(box_bg));
    let prompt_line = if bg.prompt_text.is_empty() {
        Line::from(vec![arrow_span]).style(Style::default().bg(box_bg))
    } else {
        Line::from(vec![
            arrow_span,
            Span::styled(bg.prompt_text.clone(), Style::default().fg(text_dimmed_color).add_modifier(Modifier::DIM).bg(box_bg)),
        ]).style(Style::default().bg(box_bg))
    };

    let bottom_box_lines = vec![
        Line::from("").style(Style::default().bg(box_bg)),
        prompt_line,
        Line::from("").style(Style::default().bg(box_bg)),
    ];

    let (_, max_label) = model_context_info(&bg.model_name);
    let ctx_display = format!("{}/{}", format_context_length(bg.used_tokens), max_label);
    let cache_display = format!("{:.1}% cache", bg.cache_hit_rate * 100.0);

    let model_display = friendly_model_name(&bg.model_name);
    let effort_display = if bg.thinking_effort.is_empty() { "high" } else { &bg.thinking_effort };
    let right_text = if model_display.is_empty() {
        effort_display.to_string()
    } else {
        format!("{model_display} · {effort_display}")
    };
    let left_len = 2 + ctx_display.chars().count() + 3 + cache_display.chars().count();
    let pad_len = w.saturating_sub(left_len + right_text.chars().count());

    let footer_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(ctx_display, Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM)),
        Span::styled(" · ", Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM)),
        Span::styled(cache_display, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM)),
        Span::raw(" ".repeat(pad_len)),
        Span::styled(right_text, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM)),
    ]);

    let composer_h = 4usize;
    let transcript_avail_h = h.saturating_sub(composer_h);

    let mut full_screen_lines: Vec<Line<'static>> = Vec::with_capacity(h);

    let transcript = &bg.transcript;
    if transcript.len() <= transcript_avail_h {
        for l in transcript {
            full_screen_lines.push(dim_line(l));
        }
    } else {
        let skip = transcript.len() - transcript_avail_h;
        for l in &transcript[skip..] {
            full_screen_lines.push(dim_line(l));
        }
    }

    for l in bottom_box_lines {
        full_screen_lines.push(l);
    }

    full_screen_lines.push(footer_line);

    full_screen_lines.truncate(h);
    while full_screen_lines.len() < h {
        full_screen_lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(full_screen_lines),
        Rect::new(area.x, area.y, area.width, area.height),
    );
}

/// Interactive "Connect a provider" GUI modal with clean colored box styling.
/// Floating container block matching the composer prompt text box, live search filter,
/// peach selection highlight, and in-modal API key entry.
pub fn run_connect_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let catalog = load_catalog()?;
    let all_items = build_connect_items(&catalog);
    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    enum ModalState {
        Selecting,
        EnteringKey {
            item: ConnectItem,
            key_buf: String,
            existing_key: Option<String>,
            status_msg: Option<String>,
        },
        SelectingModel {
            item: ConnectItem,
            models: Vec<(String, String)>,
            filter: String,
            sel: usize,
            scroll_top: usize,
        },
    }

    let mut state = ModalState::Selecting;
    let mut connected_name: Option<(String, String)> = None;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let input_bg = Color::Rgb(32, 32, 32);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let auth_keys = load_auth_keys();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                let modal_h = 18.min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                match &mut state {
                    ModalState::Selecting => {
                        let filtered: Vec<&ConnectItem> = all_items
                            .iter()
                            .filter(|item| {
                                let f = filter.to_lowercase();
                                f.is_empty()
                                    || item.name.to_lowercase().contains(&f)
                                    || item.id.to_lowercase().contains(&f)
                                    || item.sublabel.to_lowercase().contains(&f)
                            })
                            .collect();

                        // Clear popup background
                        frame.render_widget(Clear, modal_rect);

                        // Container Box (pure colored block matching text box, no border characters)
                        let box_block = Block::default()
                            .style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, modal_rect);

                        let pad_x = 3u16;
                        let inner_w = modal_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                        // 1. Header Line (with esc at top right)
                        let title_str = "Connect a provider";
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let header_line = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                        // 2. Search Bar
                        let search_line = if filter.is_empty() {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("Type to filter providers...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                            ])
                        } else {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled(&filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                            ])
                        };
                        frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // 3. Provider List
                        let list_y = inner.y + 3;
                        let list_h = inner.height.saturating_sub(4) as usize;

                        if filtered.is_empty() {
                            let empty_msg = Paragraph::new(Line::from(vec![
                                Span::styled("  No matching providers found", Style::default().fg(text_dim).bg(box_bg)),
                            ]));
                            frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                        } else {
                            let safe_sel = sel.min(filtered.len().saturating_sub(1));
                            if safe_sel < scroll_top {
                                scroll_top = safe_sel;
                            } else if safe_sel >= scroll_top + list_h {
                                scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                            }

                            for r in 0..list_h {
                                let idx = scroll_top + r;
                                if idx >= filtered.len() {
                                    break;
                                }

                                let item = filtered[idx];
                                let is_selected = idx == safe_sel;

                                let is_connected = auth_keys.contains_key(&item.id)
                                    || (config.base_url == item.base_url && config.api_key.is_some());

                                let check_glyph = if is_connected { "✓ " } else { "  " };

                                let sub = if item.sublabel.is_empty() {
                                    String::new()
                                } else {
                                    format!(" {}", item.sublabel)
                                };

                                let raw_content = format!(" {check_glyph}{}{sub}", item.name);
                                let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                                let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                                let row_line = if is_selected {
                                    Line::from(Span::styled(
                                        full_row_str,
                                        Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                                    ))
                                } else {
                                    let check_span = if is_connected {
                                        Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                                    } else {
                                        Span::styled("   ", Style::default().bg(box_bg))
                                    };
                                    let name_span = Span::styled(&item.name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                                    let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                                    let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                                    Line::from(vec![check_span, name_span, sub_span, pad_span])
                                };

                                frame.render_widget(
                                    Paragraph::new(row_line),
                                    Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                                );
                            }
                        }

                        // 4. Footer Help Line (no brackets)
                        let footer_line = Line::from(vec![
                            Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(
                            Paragraph::new(footer_line),
                            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                        );
                    }
                    ModalState::EnteringKey {
                        item,
                        key_buf,
                        existing_key,
                        status_msg,
                    } => {
                        let dialog_w = 64.min(area.width.saturating_sub(4)).max(40).min(area.width);
                        let dialog_h = 10.min(area.height.saturating_sub(2)).max(8).min(area.height);
                        let dialog_x = (area.width.saturating_sub(dialog_w)) / 2;
                        let dialog_y = (area.height.saturating_sub(dialog_h)) / 3;
                        let dialog_rect = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

                        frame.render_widget(Clear, dialog_rect);

                        let box_block = Block::default()
                            .style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, dialog_rect);

                        let pad_x = 3u16;
                        let inner_w = dialog_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(dialog_x + pad_x, dialog_y + 1, inner_w, dialog_h.saturating_sub(2));

                        // Header (with esc at top right)
                        let title_str = "API Key Configuration";
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let line0 = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        let line1 = Line::from(vec![
                            Span::styled(format!("Provider: {}", item.name), Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(line0), Rect::new(inner.x, inner.y, inner.width, 1));
                        frame.render_widget(Paragraph::new(line1), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // Input Box (inset colored block)
                        let input_content = if key_buf.is_empty() {
                            if let Some(existing) = existing_key.as_ref() {
                                let masked = mask_key_pretty(existing);
                                Line::from(vec![
                                    Span::styled(
                                        format!(" {masked}"),
                                        Style::default().fg(Color::Rgb(210, 210, 210)).bg(input_bg),
                                    ),
                                    Span::styled(
                                        "  \u{00b7} Enter to keep, paste to replace",
                                        Style::default().fg(Color::Rgb(110, 110, 110)).bg(input_bg),
                                    ),
                                ])
                            } else {
                                Line::from(vec![
                                    Span::styled(" Paste or type API key...", Style::default().fg(Color::Rgb(110, 110, 110)).bg(input_bg)),
                                ])
                            }
                        } else {
                            let masked = "•".repeat(key_buf.chars().count());
                            Line::from(vec![
                                Span::styled(format!(" {masked}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(input_bg)),
                            ])
                        };

                        let input_rect = Rect::new(inner.x, inner.y + 3, inner.width, 1);
                        frame.render_widget(Clear, input_rect);
                        frame.render_widget(Paragraph::new(input_content).style(Style::default().bg(input_bg)), input_rect);

                        // Status or note
                        let note_line = if let Some(msg) = status_msg {
                            Line::from(Span::styled(format!(" \u{2022} {msg}"), Style::default().fg(Color::Rgb(239, 68, 68)).bg(box_bg)))
                        } else {
                            Line::from(Span::styled(" (Key stored securely in ~/.gray/auth.json)", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)))
                        };
                        frame.render_widget(Paragraph::new(note_line), Rect::new(inner.x, inner.y + 5, inner.width, 1));

                        // Footer buttons (enter update / submit - no brackets)
                        let action_label = if existing_key.is_some() { "update" } else { "submit" };
                        let footer = Line::from(vec![
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(action_label, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(footer), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
                    }
                    ModalState::SelectingModel {
                        item,
                        models,
                        filter: m_filter,
                        sel: m_sel,
                        scroll_top: m_scroll_top,
                    } => {
                        let filtered_models: Vec<&(String, String)> = models
                            .iter()
                            .filter(|(m_id, m_name)| {
                                let f = m_filter.to_lowercase();
                                f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                            })
                            .collect();

                        let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                        let modal_h = 20.min(area.height.saturating_sub(2)).max(12).min(area.height);
                        let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                        let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                        let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                        frame.render_widget(Clear, modal_rect);

                        let box_block = Block::default().style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, modal_rect);

                        let pad_x = 3u16;
                        let inner_w = modal_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                        // Header: Select Model — Provider ... esc
                        let title_str = format!("Select model \u{2014} {}", item.name);
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let header_line = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                        // Search Bar
                        let search_line = if m_filter.is_empty() {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("Type to filter models...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                            ])
                        } else {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled(&*m_filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                            ])
                        };
                        frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // Model list
                        let list_y = inner.y + 3;
                        let list_h = inner.height.saturating_sub(4) as usize;

                        if filtered_models.is_empty() {
                            let empty_msg = if m_filter.is_empty() {
                                Paragraph::new(Line::from(vec![
                                    Span::styled("  No models listed — press Enter to continue", Style::default().fg(text_dim).bg(box_bg)),
                                ]))
                            } else {
                                Paragraph::new(Line::from(vec![
                                    Span::styled("  Use custom model: ", Style::default().fg(text_dim).bg(box_bg)),
                                    Span::styled(&*m_filter, Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                ]))
                            };
                            frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                        } else {
                            let safe_sel = (*m_sel).min(filtered_models.len().saturating_sub(1));
                            if safe_sel < *m_scroll_top {
                                *m_scroll_top = safe_sel;
                            } else if safe_sel >= *m_scroll_top + list_h {
                                *m_scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                            }

                            for r in 0..list_h {
                                let idx = *m_scroll_top + r;
                                if idx >= filtered_models.len() {
                                    break;
                                }

                                let (m_id, m_name) = filtered_models[idx];
                                let is_selected = idx == safe_sel;
                                let is_current = config.model.as_deref() == Some(m_id.as_str());

                                let check_glyph = if is_current { "✓ " } else { "  " };

                                let display_name = if m_name.is_empty() { m_id.as_str() } else { m_name.as_str() };
                                let sub = if m_name.is_empty() || m_name == m_id {
                                    String::new()
                                } else {
                                    format!(" {}", m_id)
                                };

                                let raw_content = format!(" {check_glyph}{}{sub}", display_name);
                                let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                                let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                                let row_line = if is_selected {
                                    Line::from(Span::styled(
                                        full_row_str,
                                        Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                                    ))
                                } else {
                                    let check_span = if is_current {
                                        Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                                    } else {
                                        Span::styled("   ", Style::default().bg(box_bg))
                                    };
                                    let name_span = Span::styled(display_name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                                    let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                                    let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                                    Line::from(vec![check_span, name_span, sub_span, pad_span])
                                };

                                frame.render_widget(
                                    Paragraph::new(row_line),
                                    Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                                );
                            }
                        }

                        // Footer
                        let footer_line = Line::from(vec![
                            Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(
                            Paragraph::new(footer_line),
                            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                        );
                    }
                }
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            match &mut state {
                ModalState::Selecting => {
                    let filtered: Vec<&ConnectItem> = all_items
                        .iter()
                        .filter(|item| {
                            let f = filter.to_lowercase();
                            f.is_empty()
                                || item.name.to_lowercase().contains(&f)
                                || item.id.to_lowercase().contains(&f)
                                || item.sublabel.to_lowercase().contains(&f)
                        })
                        .collect();

                    if filtered.is_empty() {
                        sel = 0;
                    } else if sel >= filtered.len() {
                        sel = filtered.len().saturating_sub(1);
                    }

                    match read()? {
                        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                            if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                        Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            match code {
                                KeyCode::Char('p') => sel = sel.saturating_sub(1),
                                KeyCode::Char('n') => {
                                    if !filtered.is_empty() {
                                        sel = (sel + 1).min(filtered.len() - 1);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                            KeyCode::Up => sel = sel.saturating_sub(1),
                            KeyCode::Down => {
                                if !filtered.is_empty() {
                                    sel = (sel + 1).min(filtered.len() - 1);
                                }
                            }
                            KeyCode::PageUp => sel = sel.saturating_sub(8),
                            KeyCode::PageDown => {
                                if !filtered.is_empty() {
                                    sel = (sel + 8).min(filtered.len() - 1);
                                }
                            }
                            KeyCode::Char(ch) => {
                                filter.push(ch);
                                sel = 0;
                            }
                            KeyCode::Backspace => {
                                filter.pop();
                                sel = 0;
                            }
                            KeyCode::Esc => return Ok(false),
                            KeyCode::Enter => {
                                if let Some(&item) = filtered.get(sel) {
                                    if item.no_auth {
                                        config.base_url = item.base_url.clone();
                                        config.api_key = None;
                                        let models = get_provider_models_with_live(&item.id, &item.base_url, None, &catalog);
                                        state = ModalState::SelectingModel {
                                            item: item.clone(),
                                            models,
                                            filter: String::new(),
                                            sel: 0,
                                            scroll_top: 0,
                                        };
                                    } else {
                                        let existing = load_auth_keys()
                                            .get(&item.id)
                                            .cloned()
                                            .or_else(|| if config.base_url == item.base_url { config.api_key.clone() } else { None });
                                        state = ModalState::EnteringKey {
                                            item: item.clone(),
                                            key_buf: String::new(),
                                            existing_key: existing,
                                            status_msg: None,
                                        };
                                    }
                                }
                            }
                            _ => {}
                        },
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                ModalState::EnteringKey {
                    item,
                    key_buf,
                    existing_key,
                    status_msg,
                } => match read()? {
                    Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                        if modifiers.contains(KeyModifiers::CONTROL) => {
                        state = ModalState::Selecting;
                    }
                    Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                        KeyCode::Esc => {
                            state = ModalState::Selecting;
                        }
                        KeyCode::Char(ch) => {
                            key_buf.push(ch);
                            *status_msg = None;
                        }
                        KeyCode::Backspace => {
                            key_buf.pop();
                            *status_msg = None;
                        }
                        KeyCode::Enter => {
                            let final_key = if key_buf.is_empty() {
                                existing_key.clone().unwrap_or_default()
                            } else {
                                key_buf.clone()
                            };

                            if final_key.is_empty() {
                                *status_msg = Some("No API key entered — please enter a valid key".into());
                            } else if existing_key.is_some() {
                                // ponytail: update skips picker; preserve model if same provider else use default
                                save_auth_key(&item.id, &final_key)?;
                                let path = saved_config_path()?;
                                let mut saved = load_saved_config_at(&path);
                                let is_switching = saved.base_url.as_deref() != Some(item.base_url.as_str());
                                config.base_url = item.base_url.clone();
                                config.api_key = Some(final_key.clone());
                                saved.base_url = Some(config.base_url.clone());
                                saved.api_key = config.api_key.clone();
                                saved.auth_mode = Some("api_key".into());
                                if is_switching {
                                    let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                    saved.model = models.first().map(|(id, _)| id.clone());
                                    config.model = saved.model.clone();
                                } else if saved.model.is_none() {
                                    if let Some(m) = &config.model {
                                        saved.model = Some(m.clone());
                                    } else {
                                        let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                        saved.model = models.first().map(|(id, _)| id.clone());
                                        config.model = saved.model.clone();
                                    }
                                } else {
                                    config.model = saved.model.clone();
                                }
                                save_saved_config_at(&path, &saved)?;
                                return Ok(true);
                            } else {
                                save_auth_key(&item.id, &final_key)?;
                                config.base_url = item.base_url.clone();
                                config.api_key = Some(final_key.clone());
                                let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                state = ModalState::SelectingModel {
                                    item: item.clone(),
                                    models,
                                    filter: String::new(),
                                    sel: 0,
                                    scroll_top: 0,
                                };
                            }
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => {}
                    _ => {}
                },
                ModalState::SelectingModel {
                    item,
                    models,
                    filter: m_filter,
                    sel: m_sel,
                    scroll_top: _,
                } => {
                    let filtered_models: Vec<&(String, String)> = models
                        .iter()
                        .filter(|(m_id, m_name)| {
                            let f = m_filter.to_lowercase();
                            f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                        })
                        .collect();

                    if filtered_models.is_empty() {
                        *m_sel = 0;
                    } else if *m_sel >= filtered_models.len() {
                        *m_sel = filtered_models.len().saturating_sub(1);
                    }

                    match read()? {
                        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                            if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                        Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            match code {
                                KeyCode::Char('p') => *m_sel = m_sel.saturating_sub(1),
                                KeyCode::Char('n') => {
                                    if !filtered_models.is_empty() {
                                        *m_sel = (*m_sel + 1).min(filtered_models.len() - 1);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                            KeyCode::Up => *m_sel = m_sel.saturating_sub(1),
                            KeyCode::Down => {
                                if !filtered_models.is_empty() {
                                    *m_sel = (*m_sel + 1).min(filtered_models.len() - 1);
                                }
                            }
                            KeyCode::PageUp => *m_sel = m_sel.saturating_sub(8),
                            KeyCode::PageDown => {
                                if !filtered_models.is_empty() {
                                    *m_sel = (*m_sel + 8).min(filtered_models.len() - 1);
                                }
                            }
                            KeyCode::Char(ch) => {
                                m_filter.push(ch);
                                *m_sel = 0;
                            }
                            KeyCode::Backspace => {
                                m_filter.pop();
                                *m_sel = 0;
                            }
                            KeyCode::Esc => {
                                state = ModalState::Selecting;
                            }
                            KeyCode::Enter => {
                                let chosen_model = if let Some(&(m_id, _)) = filtered_models.get(*m_sel) {
                                    m_id.clone()
                                } else if !m_filter.is_empty() {
                                    m_filter.trim().to_string()
                                } else {
                                    "default".to_string()
                                };

                                config.model = Some(chosen_model.clone());

                                let path = saved_config_path()?;
                                let mut saved = load_saved_config_at(&path);
                                saved.base_url = Some(config.base_url.clone());
                                saved.api_key = config.api_key.clone();
                                saved.model = config.model.clone();
                                saved.auth_mode = Some(if item.no_auth { "none".into() } else { "api_key".into() });
                                save_saved_config_at(&path, &saved)?;

                                connected_name = Some((item.name.clone(), chosen_model));
                                return Ok(true);
                            }
                            _ => {}
                        },
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
            }
        }
    })();

    let _ = terminal.clear();
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();

    result
}

pub fn run_model_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::time::Duration;

    let catalog = match load_catalog() {
        Ok(c) => c,
        Err(_) => BTreeMap::new(),
    };

    let (item_id, item_name) = if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == config.base_url) {
        (pid.clone(), p.name.clone())
    } else {
        ("custom".to_string(), "Custom".to_string())
    };

    let item = ConnectItem {
        id: item_id.clone(),
        name: item_name.clone(),
        sublabel: String::new(),
        base_url: config.base_url.clone(),
        category: "Providers",
        env_key: String::new(),
        no_auth: false,
    };

    let models = get_provider_models_with_live(&item_id, &config.base_url, config.api_key.as_deref(), &catalog);

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let filtered_models: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    let f = filter.to_lowercase();
                    f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                })
                .collect();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                let modal_h = 16.min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                // Header
                let title_str = format!("Select model \u{2014} {}", item.name);
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                // Search Bar
                let search_line = if filter.is_empty() {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("Type to filter models...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(&filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                };
                frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                // List
                let list_y = inner.y + 3;
                let list_h = inner.height.saturating_sub(4) as usize;

                if filtered_models.is_empty() {
                    let empty_msg = if filter.is_empty() {
                        Paragraph::new(Line::from(vec![
                            Span::styled("  No models listed — press Enter to continue", Style::default().fg(text_dim).bg(box_bg)),
                        ]))
                    } else {
                        Paragraph::new(Line::from(vec![
                            Span::styled("  Use custom model: ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled(&filter, Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        ]))
                    };
                    frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                } else {
                    let safe_sel = sel.min(filtered_models.len().saturating_sub(1));
                    if safe_sel < scroll_top {
                        scroll_top = safe_sel;
                    } else if safe_sel >= scroll_top + list_h {
                        scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                    }

                    for r in 0..list_h {
                        let idx = scroll_top + r;
                        if idx >= filtered_models.len() {
                            break;
                        }

                        let (m_id, m_name) = filtered_models[idx];
                        let is_selected = idx == safe_sel;
                        let is_current = config.model.as_deref() == Some(m_id.as_str());

                        let check_glyph = if is_current { "✓ " } else { "  " };

                        let display_name = if m_name.is_empty() { m_id.as_str() } else { m_name.as_str() };
                        let sub = if m_name.is_empty() || m_name == m_id {
                            String::new()
                        } else {
                            format!(" {}", m_id)
                        };

                        let raw_content = format!(" {check_glyph}{}{sub}", display_name);
                        let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                        let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                full_row_str,
                                Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let check_span = if is_current {
                                Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                            } else {
                                Span::styled("   ", Style::default().bg(box_bg))
                            };
                            let name_span = Span::styled(display_name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                            let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                            let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                            Line::from(vec![check_span, name_span, sub_span, pad_span])
                        };

                        frame.render_widget(
                            Paragraph::new(row_line),
                            Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                        );
                    }
                }

                // Footer
                let footer_line = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(footer_line),
                    Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                );
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            if filtered_models.is_empty() {
                sel = 0;
            } else if sel >= filtered_models.len() {
                sel = filtered_models.len().saturating_sub(1);
            }

            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => {
                            if !filtered_models.is_empty() {
                                sel = (sel + 1).min(filtered_models.len() - 1);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        if !filtered_models.is_empty() {
                            sel = (sel + 1).min(filtered_models.len() - 1);
                        }
                    }
                    KeyCode::PageUp => sel = sel.saturating_sub(8),
                    KeyCode::PageDown => {
                        if !filtered_models.is_empty() {
                            sel = (sel + 8).min(filtered_models.len() - 1);
                        }
                    }
                    KeyCode::Char(ch) => {
                        filter.push(ch);
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        sel = 0;
                    }
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        let chosen_model = if let Some(&(m_id, _)) = filtered_models.get(sel) {
                            m_id.clone()
                        } else if !filter.is_empty() {
                            filter.trim().to_string()
                        } else {
                            "default".to_string()
                        };

                        config.model = Some(chosen_model.clone());

                        let path = saved_config_path()?;
                        let mut saved = load_saved_config_at(&path);
                        saved.model = config.model.clone();
                        save_saved_config_at(&path, &saved)?;

                        return Ok(true);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();

    let _ = terminal.clear();
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();

    result
}

/// Thinking levels and descriptions matching Pi / Prime-Agent.
pub const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning"),
    ("low", "Light reasoning"),
    ("medium", "Moderate reasoning"),
    ("high", "Deep reasoning"),
    ("xhigh", "Very deep reasoning"),
    ("max", "Maximum reasoning"),
];

/// Interactive "Thinking effort" GUI modal matching Pi / Prime-Agent.
pub fn run_effort_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let current_level = config.thinking_effort.clone().unwrap_or_else(|| "high".to_string());
    let mut sel = THINKING_LEVELS.iter().position(|(l, _)| *l == current_level).unwrap_or(4);

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 58.min(area.width.saturating_sub(4)).max(36).min(area.width);
                let modal_h = (THINKING_LEVELS.len() as u16 + 5).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                // Header
                let title_str = "Thinking effort";
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                // List of levels
                let list_y = inner.y + 2;
                for (idx, (level, desc)) in THINKING_LEVELS.iter().enumerate() {
                    let is_selected = idx == sel;
                    let is_current = current_level == *level;

                    let check_glyph = if is_current { "✓ " } else { "  " };
                    let raw_content = format!(" {check_glyph}{level:<8}  {desc}");
                    let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                    let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                    let row_line = if is_selected {
                        Line::from(Span::styled(
                            full_row_str,
                            Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        let check_span = if is_current {
                            Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                        } else {
                            Span::styled("   ", Style::default().bg(box_bg))
                        };
                        let name_span = Span::styled(format!("{level:<8}  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                        let desc_span = Span::styled(*desc, Style::default().fg(Color::Rgb(140, 140, 140)).bg(box_bg));
                        let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                        Line::from(vec![check_span, name_span, desc_span, pad_span])
                    };

                    frame.render_widget(
                        Paragraph::new(row_line),
                        Rect::new(inner.x, list_y + idx as u16, inner.width, 1),
                    );
                }

                // Footer
                let footer_line = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(footer_line),
                    Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                );
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => sel = (sel + 1).min(THINKING_LEVELS.len().saturating_sub(1)),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(THINKING_LEVELS.len().saturating_sub(1)),
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        let (chosen, _) = THINKING_LEVELS[sel];
                        config.thinking_effort = Some(chosen.to_string());

                        let path = saved_config_path()?;
                        let mut saved = load_saved_config_at(&path);
                        saved.thinking_effort = config.thinking_effort.clone();
                        save_saved_config_at(&path, &saved)?;

                        return Ok(true);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();

    let _ = terminal.clear();
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();

    result
}

pub async fn run_effort_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_effort_modal(config, bg)
}

pub async fn run_model_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_model_modal(config, bg)
}

pub async fn run_provider_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_connect_modal(config, bg)
}

pub async fn run_onboarding(config: &mut Config) -> anyhow::Result<bool> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    print!("\r\n");
    crate::tui::print_logo();
    print!("\r\n");
    print_wrapped("\x1b[2mWelcome to gray by alignment\x1b[0m", 2);
    print_wrapped(
        "\x1b[2mgray is a minimal agent that runs tools, edits code, and works with any model provider.\x1b[0m",
        2,
    );
    print!("\r\n");
    run_provider_menu(config, None).await
}

