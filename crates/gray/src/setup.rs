//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot), persisting
//! to ~/.gray/config.json. Flow: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config::Config, tui::print_wrapped};

/// Provider entry from the vendored catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogProvider {
    pub name: String,
    pub base_url: String,
    /// models.dev emits either a string or a list of env var names.
    #[serde(default)]
    pub env_key: serde_json::Value,
    pub featured: bool,
    /// True when the upstream serves a keyless/free tier (9router noAuth).
    #[serde(default)]
    pub no_auth: bool,
    #[serde(default)]
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The full catalog, keyed by provider id (`openrouter`, `deepseek`, ...).
pub type Catalog = BTreeMap<String, CatalogProvider>;

/// Bundled models.dev snapshot.
pub const PROVIDERS_JSON: &str = include_str!("../assets/providers.json");

/// Parses the embedded catalog. Infinitely unlikely to fail (compiled in),
/// but returns a Result so callers can degrade gracefully.
pub fn load_catalog() -> anyhow::Result<Catalog> {
    Ok(serde_json::from_str(PROVIDERS_JSON)?)
}

/// First env var name from the catalog entry, for hints during key input.
fn env_hint(p: &CatalogProvider) -> String {
    match &p.env_key {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str())
            .next()
            .unwrap_or("API_KEY")
            .to_string(),
        _ => "API_KEY".to_string(),
    }
}

/// On-disk configuration, kept deliberately tiny.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How the provider authenticates: "api_key" | "oauth" | "none".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    /// Thinking / reasoning effort: "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,
}

/// Resolves `$GRAY_HOME` (or `$HOME/.gray`) — shared root for gray's files.
pub fn gray_home() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray")))
        .map_err(|_| anyhow::anyhow!("cannot resolve home: set HOME or GRAY_HOME"))?;
    Ok(PathBuf::from(base))
}

/// Path to the persisted config file.
pub fn saved_config_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("config.json"))
}

/// Loads the saved config; a missing file yields an all-None struct.
pub fn load_saved_config_at(path: &Path) -> SavedConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the config pretty-printed so users can hand-edit it too.
/// Mode 0600: the file stores the plaintext api_key.
pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(serde_json::to_string_pretty(cfg)?.as_bytes())?;
    Ok(())
}


/// Reads a secret with masked input (echoes `*` per char, like opencode's
/// password prompt). Enter confirms, Esc/Ctrl-C returns an error.
pub(crate) fn read_secret(prompt: &str) -> anyhow::Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::io::Write as _;
    print!("{prompt}");
    std::io::stdout().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let result = (|| -> anyhow::Result<String> {
        let mut buf = String::new();
        loop {
            if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = read()? {
                match code {
                    KeyCode::Enter => break,
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        anyhow::bail!("cancelled");
                    }
                    KeyCode::Esc => anyhow::bail!("cancelled"),
                    KeyCode::Backspace => {
                        if buf.pop().is_some() {
                            print!("\x08 \x08");
                            std::io::stdout().flush()?;
                        }
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        print!("*");
                        std::io::stdout().flush()?;
                    }
                    _ => {}
                }
            }
        }
        Ok(buf)
    })();
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    result
}

/// Per-provider API-key store (`~/.gray/auth.json`, mode 0600), mirroring
/// opencode's credential file: `{ "<provider-id>": "<key>", ... }`.
fn auth_store_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("auth.json"))
}

/// All stored keys keyed by provider id; missing file yields an empty map.
pub(crate) fn load_auth_keys() -> BTreeMap<String, String> {
    std::fs::read_to_string(auth_store_path().unwrap_or_else(|_| PathBuf::from("/dev/null")))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Upserts `key` under provider id `pid` (read-modify-write, 0600).
pub(crate) fn save_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let path = auth_store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut keys = load_auth_keys();
    keys.insert(pid.to_string(), key.to_string());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(serde_json::to_string_pretty(&keys)?.as_bytes())?;
    Ok(())
}

/// `/key [provider-id]`: masked key entry for a catalog provider. Stores the
/// key per-provider in `~/.gray/auth.json` (opencode-style) and activates it
/// (base_url + api_key) without touching the chosen model. Returns Ok(true)
/// if a key was configured.
pub fn run_key_setup(config: &mut Config, pid: Option<String>) -> anyhow::Result<bool> {
    let catalog = load_catalog()?;
    let pid = match pid {
        Some(p) if catalog.contains_key(&p) => p,
        Some(p) => {
            println!("unknown provider '{p}' — use /key with no argument to pick from the list");
            return Ok(false);
        }
        None => return run_connect_modal(config),
    };
    let provider = &catalog[&pid];
    let existing = load_auth_keys()
        .get(&pid)
        .cloned()
        .or_else(|| config.api_key.clone())
        .unwrap_or_default();
    let hint = env_hint(provider);
    let status = if existing.is_empty() {
        if hint == "API_KEY" { "input hidden" } else { &hint }
    } else {
        "stored — Enter keeps it"
    };
    let key_in = match read_secret(&format!("{} API key ({}): ", provider.name, status)) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };
    let key = if key_in.is_empty() { existing } else { key_in };
    if key.is_empty() {
        println!("no key entered");
        return Ok(false);
    }
    save_auth_key(&pid, &key)?;
    config.base_url = provider.base_url.clone();
    config.api_key = Some(key);
    let path = saved_config_path()?;
    let mut saved = load_saved_config_at(&path);
    saved.base_url = Some(config.base_url.clone());
    saved.api_key = config.api_key.clone();
    saved.auth_mode = Some("api_key".into());
    save_saved_config_at(&path, &saved)?;
    println!(
        "{} ready — key stored in {}",
        provider.name,
        auth_store_path()?.display()
    );
    Ok(true)
}

/// Provider item displayed in the "Connect a provider" modal.
#[derive(Debug, Clone)]
pub struct ConnectItem {
    pub id: String,
    pub name: String,
    pub sublabel: String,
    pub category: &'static str,
    pub base_url: String,
    pub default_model: String,
    pub env_key: String,
    pub no_auth: bool,
}

/// Builds the full list of providers for the connect modal:
/// Popular section on top, followed by all catalog providers under Providers.
pub fn build_connect_items(catalog: &Catalog) -> Vec<ConnectItem> {
    let popular_defs = [
        ("openai", "OpenAI", "(ChatGPT Plus/Pro or API key)", "https://api.openai.com/v1", "gpt-4o", "OPENAI_API_KEY", false),
        ("anthropic", "Anthropic", "(API key)", "https://api.anthropic.com/v1", "claude-3-7-sonnet-20250219", "ANTHROPIC_API_KEY", false),
        ("google", "Google", "(Gemini API key)", "https://generativelanguage.googleapis.com/v1beta/openai", "gemini-2.5-flash", "GEMINI_API_KEY", false),
        ("openrouter", "OpenRouter", "(Access 300+ models)", "https://openrouter.ai/api/v1", "anthropic/claude-3.7-sonnet", "OPENROUTER_API_KEY", false),
        ("deepseek", "DeepSeek", "", "https://api.deepseek.com", "deepseek-chat", "DEEPSEEK_API_KEY", false),
        ("groq", "Groq", "(Fast inference)", "https://api.groq.com/openai/v1", "llama-3.3-70b-versatile", "GROQ_API_KEY", false),
        ("ollama", "Ollama", "(Local http://localhost:11434)", "http://localhost:11434/v1", "llama3", "", true),
        ("github-copilot", "GitHub Copilot", "", "https://api.githubcopilot.com", "gpt-4o", "COPILOT_API_KEY", false),
        ("xai", "xAI (Grok)", "(Grok API key)", "https://api.x.ai/v1", "grok-2-latest", "XAI_API_KEY", false),
        ("mistral", "Mistral", "(API key)", "https://api.mistral.ai/v1", "mistral-large-latest", "MISTRAL_API_KEY", false),
    ];

    let mut items = Vec::new();
    let mut popular_ids = std::collections::HashSet::new();

    for (id, name, sublabel, base_url, def_model, env_k, no_auth) in popular_defs {
        popular_ids.insert(id.to_string());
        let (url, env) = if let Some(p) = catalog.get(id) {
            let e = env_hint(p);
            (p.base_url.as_str(), e)
        } else {
            (base_url, env_k.to_string())
        };
        items.push(ConnectItem {
            id: id.to_string(),
            name: name.to_string(),
            sublabel: sublabel.to_string(),
            category: "Popular",
            base_url: url.to_string(),
            default_model: def_model.to_string(),
            env_key: env,
            no_auth,
        });
    }

    // All catalog providers in alphabetical order
    let mut catalog_entries: Vec<_> = catalog.iter().collect();
    catalog_entries.sort_by_key(|(_, p)| p.name.to_lowercase());

    for (id, p) in catalog_entries {
        if popular_ids.contains(id) {
            continue;
        }
        let model = p.models.iter()
            .find(|m| m.id.contains("claude") || m.id.contains("gpt-4") || m.id.contains("gemini") || m.id.contains("deepseek"))
            .or_else(|| p.models.first())
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "default".to_string());

        items.push(ConnectItem {
            id: id.clone(),
            name: p.name.clone(),
            sublabel: String::new(),
            category: "Providers",
            base_url: p.base_url.clone(),
            default_model: model,
            env_key: env_hint(p),
            no_auth: p.no_auth,
        });
    }

    items
}

/// Converts a raw model ID to a friendly human-readable display name.
pub fn friendly_model_name(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    if let Ok(catalog) = load_catalog() {
        for provider in catalog.values() {
            for m in &provider.models {
                if m.id == model_id {
                    return m.name.clone();
                }
            }
        }
    }
    let name = model_id.split('/').last().unwrap_or(model_id);
    let words: Vec<String> = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(c).collect(),
            }
        })
        .collect();
    words.join(" ")
}

/// Returns the models list for a provider from the catalog.
pub fn get_provider_models(provider_id: &str, catalog: &Catalog) -> Vec<(String, String)> {
    let mut list = Vec::new();
    if let Some(p) = catalog.get(provider_id) {
        for m in &p.models {
            list.push((m.id.clone(), m.name.clone()));
        }
    }
    list
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
                        .timeout(std::time::Duration::from_millis(2500))
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return Vec::new(),
                    };

                    let trimmed_base = base.trim_end_matches('/');
                    let endpoints = if trimmed_base.contains("openrouter.ai") {
                        vec!["https://openrouter.ai/api/v1/models".to_string()]
                    } else if trimmed_base.ends_with("/v1") {
                        vec![format!("{trimmed_base}/models")]
                    } else {
                        vec![
                            format!("{trimmed_base}/models"),
                            format!("{trimmed_base}/v1/models"),
                            format!("{trimmed_base}/api/tags"),
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
                                    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                        for item in data {
                                            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                                                let name = item.get("name")
                                                    .and_then(|n| n.as_str())
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| friendly_model_name(id));
                                                models.push((id.to_string(), name));
                                            }
                                        }
                                    }
                                    if models.is_empty() {
                                        if let Some(items) = json.get("models").and_then(|m| m.as_array()) {
                                            for item in items {
                                                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                                                    models.push((name.to_string(), friendly_model_name(name)));
                                                } else if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                                                    models.push((id.to_string(), friendly_model_name(id)));
                                                }
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

/// Returns the models list for a provider (checking live endpoint first, falling back to catalog).
pub fn get_provider_models_with_live(
    provider_id: &str,
    base_url: &str,
    api_key: Option<&str>,
    catalog: &Catalog,
) -> Vec<(String, String)> {
    let live = fetch_live_provider_models(base_url, api_key);
    if !live.is_empty() {
        live
    } else {
        get_provider_models(provider_id, catalog)
    }
}

/// Interactive "Connect a provider" GUI modal with clean colored box styling.
/// Floating container block matching the composer prompt text box, live search filter,
/// peach selection highlight, and in-modal API key entry.
pub fn run_connect_modal(config: &mut Config) -> anyhow::Result<bool> {
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

    enable_raw_mode()?;
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let input_bg = Color::Rgb(32, 32, 32);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let auth_keys = load_auth_keys();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

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
                            if existing_key.is_some() {
                                Line::from(Span::styled(" (stored key exists \u{2014} press Enter to keep)", Style::default().fg(Color::Rgb(140, 140, 140)).bg(input_bg)))
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

                        // Footer buttons (enter submit - no brackets)
                        let footer = Line::from(vec![
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("submit", Style::default().fg(text_dim).bg(box_bg)),
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
                                } else if !item.default_model.is_empty() {
                                    item.default_model.clone()
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
    disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    let _ = std::io::stdout().flush();

    result
}

pub fn run_model_modal(config: &mut Config) -> anyhow::Result<bool> {
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

    let (item_id, item_name, default_model) = if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == config.base_url) {
        let dm = p.models.first().map(|m| m.id.clone()).unwrap_or_default();
        (pid.clone(), p.name.clone(), dm)
    } else {
        ("custom".to_string(), "Custom".to_string(), String::new())
    };

    let item = ConnectItem {
        id: item_id.clone(),
        name: item_name.clone(),
        sublabel: String::new(),
        base_url: config.base_url.clone(),
        default_model,
        category: "Providers",
        env_key: String::new(),
        no_auth: false,
    };

    let models = get_provider_models_with_live(&item_id, &config.base_url, config.api_key.as_deref(), &catalog);

    enable_raw_mode()?;
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

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
                        } else if !item.default_model.is_empty() {
                            item.default_model.clone()
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
    disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
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
pub fn run_effort_modal(config: &mut Config) -> anyhow::Result<bool> {
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

    enable_raw_mode()?;
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let current_level = config.thinking_effort.clone().unwrap_or_else(|| "high".to_string());
    let mut sel = THINKING_LEVELS.iter().position(|(l, _)| *l == current_level).unwrap_or(4);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                let modal_w = 58.min(area.width.saturating_sub(4)).max(36).min(area.width);
                let modal_h = (THINKING_LEVELS.len() as u16 + 5).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = area.x + (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = area.y + area.height.saturating_sub(modal_h + 1);
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
    disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    let _ = std::io::stdout().flush();

    result
}

pub async fn run_effort_menu(config: &mut Config) -> anyhow::Result<bool> {
    run_effort_modal(config)
}

pub async fn run_model_menu(config: &mut Config) -> anyhow::Result<bool> {
    run_model_modal(config)
}

pub async fn run_provider_menu(config: &mut Config) -> anyhow::Result<bool> {
    run_connect_modal(config)
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
    run_provider_menu(config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_is_sane() {
        let cat = load_catalog().expect("embedded catalog should parse");
        assert!(cat.len() > 50, "expected a large catalog, got {}", cat.len());
        let or = cat.get("openrouter").expect("openrouter present");
        assert!(or.base_url.starts_with("https://"));
        assert!(!or.models.is_empty(), "openrouter should suggest models");
    }

    #[test]
    fn auth_store_upserts_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("gray-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: test-only GRAY_HOME, set once before any auth-store call here.
        unsafe { std::env::set_var("GRAY_HOME", &dir) };

        assert!(load_auth_keys().is_empty());
        save_auth_key("openrouter", "sk-or-1").unwrap();
        save_auth_key("deepseek", "sk-ds").unwrap();
        save_auth_key("openrouter", "sk-or-2").unwrap(); // upsert, not duplicate

        let keys = load_auth_keys();
        assert_eq!(keys.get("openrouter").map(String::as_str), Some("sk-or-2"));
        assert_eq!(keys.get("deepseek").map(String::as_str), Some("sk-ds"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("auth.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auth_mode_serializes() {
        let cfg = SavedConfig {
            base_url: None,
            api_key: Some("k".into()),
            model: Some("m".into()),
            auth_mode: Some("none".into()),
        };
        let j = serde_json::to_string(&cfg).unwrap();
        assert!(j.contains("\"auth_mode\":\"none\""));
        let back: SavedConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back.auth_mode.as_deref(), Some("none"));
    }

    #[test]
    fn saved_config_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("gray-setup2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");

        assert!(load_saved_config_at(&path).model.is_none());

        let cfg = SavedConfig {
            base_url: Some("https://api.deepseek.com/v1".into()),
            api_key: Some("sk-test".into()),
            model: Some("deepseek-chat".into()),
            auth_mode: Some("api_key".into()),
        };
        save_saved_config_at(&path, &cfg).unwrap();
        let loaded = load_saved_config_at(&path);
        assert_eq!(loaded.api_key.as_deref(), Some("sk-test"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
