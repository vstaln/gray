//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot), persisting
//! to ~/.gray/config.json. Flow: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config::Config, rule, tui::print_wrapped};

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

pub(crate) fn read_line(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
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

/// Case-insensitive substring match over id and name; empty filter matches all.
/// Pure so the picker's core logic is testable without a tty.
fn matches_filter(filter: &str, primary: &str, secondary: &str) -> bool {
    let f = filter.to_lowercase();
    f.is_empty() || primary.to_lowercase().contains(&f) || secondary.to_lowercase().contains(&f)
}

/// Indices of `items` matching `filter`, in original order.
fn filtered_indices(items: &[(String, String)], filter: &str) -> Vec<usize> {
    (0..items.len())
        .filter(|&i| matches_filter(filter, &items[i].0, &items[i].1))
        .collect()
}

/// First visible row index for a window of `max_rows` with `sel` kept in view.
pub(crate) fn scroll_start(sel: usize, max_rows: usize) -> usize {
    sel.saturating_sub(max_rows - 1)
}

/// Clips to at most `max` chars.
/// ponytail: char count, not unicode display width — wide glyphs may overflow; swap in unicode-width if that matters.
pub(crate) fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Interactive live-filter list picker. Renders inline (no alternate screen),
/// redraws on every keystroke. Returns the selected index, or `None` on Esc /
/// Ctrl+C. Restores cooked mode before returning on every path.
pub(crate) fn select_from_list(
    title: &str,
    items: &[(String, String)],
    filterable: bool,
) -> anyhow::Result<Option<usize>> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    const ROWS: usize = 12;
    // Keep the whole frame inside short panes: banner/welcome occupy ~8 rows
    // above us and one rule row sits below; never let printing scroll the pane.
    let term_rows = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
    let rows = ROWS.min(term_rows.saturating_sub(11)).max(3);
    let mut stdout = std::io::stdout();
    let mut filter = String::new();
    let mut sel = 0usize;
    use crate::tui::{Border, Container, InlineFrame, Text};
    let mut frame = InlineFrame::default();

    crossterm::terminal::enable_raw_mode()?;
    // Hide the cursor: its block glyph parked at column 0 reads as a stray
    // box-drawing artifact on the bottom border (TUI convention: hidden cursor).
    let _ = write!(stdout, "\x1b[?25l");
    stdout.flush()?;
    let result = (|| -> anyhow::Result<Option<usize>> {
        loop {
            let filtered = filtered_indices(items, &filter);
            if sel >= filtered.len() {
                sel = filtered.len().saturating_sub(1);
            }
            let selected = filtered.get(sel).copied();

            // Fresh width at EVERY paint; rebuild the whole container fresh
            // (FirstTimeSetupComponent.update()), so a resize mid-menu redraws
            // borders and wrapping at the new size.
            let tw = crate::term_width();
            let header = if filter.is_empty() {
                title.to_string()
            } else {
                let counter = format!(
                    "({}/{})",
                    if filtered.is_empty() { 0 } else { sel + 1 },
                    filtered.len()
                );
                // Budget leaves room for padding plus `<title> `, one space, counter.
                let filter_budget = tw
                    .saturating_sub(6 + title.chars().count() + counter.chars().count());
                format!("{title}> {} \x1b[2m{counter}\x1b[0m", clip(&filter, filter_budget))
            };
            let mut c = Container::new();
            c.push(Box::new(Border));
            c.push(Box::new(Text::new(header, 1)));
            if filtered.is_empty() {
                c.push(Box::new(Text::new("no matches", 3)));
            } else {
                let start = scroll_start(sel, rows);
                for &i in &filtered[start..(start + rows).min(filtered.len())] {
                    let body = format!(
                        "{}  {}",
                        clip(&items[i].0, 32),
                        clip(&items[i].1, tw.saturating_sub(40))
                    );
                    c.push(Box::new(Text::new(
                        if Some(i) == selected {
                            format!("\x1b[7m{body}\x1b[0m")
                        } else {
                            body
                        },
                        3,
                    )));
                }
            }
            c.push(Box::new(Border));
            frame.draw(&mut stdout, &c, tw)?;

            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => sel = (sel + 1).min(filtered.len().saturating_sub(1)),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Char(c) if filterable => {
                        filter.push(c);
                        sel = 0;
                    }
                    KeyCode::Backspace if filterable => {
                        filter.pop();
                        sel = 0;
                    }
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(filtered.len().saturating_sub(1)),
                    KeyCode::Enter => return Ok(selected),
                    KeyCode::Esc => return Ok(None),
                    _ => {}
                },
                Event::Resize(_, _) => {} // width re-queried at top of loop; frame repaints
                _ => {}
            }
        }
    })();
    crossterm::terminal::disable_raw_mode()?;
    let _ = write!(stdout, "\x1b[?25h");
    stdout.flush()?;

    // Erase the picker UI so the transcript shows only the outcome.
    frame.erase(&mut stdout)?;
    result
}

/// Interactive provider picker over the bundled catalog. Returns the chosen
/// provider id, or None if the user aborts with Esc/Ctrl+C.
pub fn select_from_catalog(catalog: &Catalog) -> anyhow::Result<Option<String>> {
    let items: Vec<(String, String)> = catalog
        .iter()
        .map(|(id, p)| (id.clone(), p.name.clone()))
        .collect();
    match select_from_list("provider", &items, true)? {
        Some(i) => Ok(Some(items[i].0.clone())),
        None => Ok(None),
    }
}

/// What the user picked on the onboarding/provider screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingChoice {
    ApiKey,
    OAuth,
    Free,
    Local,
    Skip,
}

/// Routes an onboarding menu selection index (pure, unit-tested).
pub fn route_onboarding(i: usize) -> OnboardingChoice {
    match i {
        0 => OnboardingChoice::Free,
        1 => OnboardingChoice::ApiKey,
        2 => OnboardingChoice::OAuth,
        3 => OnboardingChoice::Local,
        _ => OnboardingChoice::Skip,
    }
}

/// Interactive API-key flow: pick provider from catalog -> enter key -> pick model -> save.
/// Returns Ok(true) on success, Ok(false) if cancelled.
pub fn run_api_key_setup(config: &mut Config) -> anyhow::Result<bool> {
    let catalog = load_catalog()?;
    let pid = match select_from_catalog(&catalog)? {
        Some(id) => id,
        None => return Ok(false),
    };
    let provider = &catalog[&pid];
    println!("provider → {}", provider.name);

    println!("{}", rule("credentials"));
    let hint = env_hint(provider);
    let env_key = config.api_key.clone().unwrap_or_default();
    let key_in = match read_secret(&format!(
        "{} API key ({}): ",
        provider.name,
        if hint == "API_KEY" { "input hidden" } else { &hint }
    )) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };
    let api_key = if key_in.is_empty() { env_key } else { key_in };
    let _ = save_auth_key(&pid, &api_key);

    println!("{}", rule("model"));
    let model = if provider.models.is_empty() {
        let m = match read_line("model id: ") {
            Ok(m) => m,
            Err(_) => return Ok(false),
        };
        if m.is_empty() {
            return Ok(false);
        }
        m
    } else {
        let items: Vec<(String, String)> = provider
            .models
            .iter()
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect();
        match select_from_list("model", &items, true)? {
            Some(i) => items[i].0.clone(),
            None => items[0].0.clone(),
        }
    };

    let saved = SavedConfig {
        base_url: Some(provider.base_url.clone()),
        api_key: Some(api_key.clone()),
        model: Some(model),
        auth_mode: Some("api_key".into()),
    };
    let path = saved_config_path()?;
    save_saved_config_at(&path, &saved)?;

    config.base_url = saved.base_url.unwrap();
    config.api_key = saved.api_key;
    config.model = saved.model;

    println!();
    println!("saved — edit {} anytime, or /sys for the system prompt.", path.display());
    Ok(true)
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
        None => match select_from_catalog(&catalog)? {
            Some(id) => id,
            None => return Ok(false),
        },
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
        ("opencode", "OpenCode Zen", "(Recommended)", "https://opencode.ai/zen/v1", "glm-5.2", "OPENCODE_API_KEY", false),
        ("opencode-go", "OpenCode Go", "Low cost subscription for everyone", "https://opencode.ai/zen/go/v1", "glm-5.2", "OPENCODE_API_KEY", false),
        ("openai", "OpenAI", "(ChatGPT Plus/Pro or API key)", "https://api.openai.com/v1", "gpt-4o", "OPENAI_API_KEY", false),
        ("github-copilot", "GitHub Copilot", "", "https://api.githubcopilot.com", "gpt-4o", "COPILOT_API_KEY", false),
        ("anthropic", "Anthropic", "(API key)", "https://api.anthropic.com/v1", "claude-3-7-sonnet-20250219", "ANTHROPIC_API_KEY", false),
        ("google", "Google", "(Gemini API key)", "https://generativelanguage.googleapis.com/v1beta/openai", "gemini-2.5-flash", "GEMINI_API_KEY", false),
        ("deepseek", "DeepSeek", "", "https://api.deepseek.com", "deepseek-chat", "DEEPSEEK_API_KEY", false),
        ("openrouter", "OpenRouter", "(Access 300+ models)", "https://openrouter.ai/api/v1", "anthropic/claude-3.7-sonnet", "OPENROUTER_API_KEY", false),
        ("groq", "Groq", "(Fast inference)", "https://api.groq.com/openai/v1", "llama-3.3-70b-versatile", "GROQ_API_KEY", false),
        ("ollama", "Ollama", "(Local http://localhost:11434)", "http://localhost:11434/v1", "llama3", "", true),
    ];

    let mut items = Vec::new();
    let mut popular_ids = std::collections::HashSet::new();

    for (id, name, sublabel, base_url, def_model, env_k, no_auth) in popular_defs {
        popular_ids.insert(id.to_string());
        let (url, model, env) = if let Some(p) = catalog.get(id) {
            let m = p.models.first().map(|m| m.id.as_str()).unwrap_or(def_model);
            let e = env_hint(p);
            (p.base_url.as_str(), m, e)
        } else {
            (base_url, def_model, env_k.to_string())
        };
        items.push(ConnectItem {
            id: id.to_string(),
            name: name.to_string(),
            sublabel: sublabel.to_string(),
            category: "Popular",
            base_url: url.to_string(),
            default_model: model.to_string(),
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
        let model = p.models.first().map(|m| m.id.clone()).unwrap_or_default();
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

/// Interactive "Connect a provider" GUI modal, matching OpenCode's visual design.
/// Live search filter, categorized into Popular and Providers, peach selection highlight,
/// instant API key entry, and auto-configured default model.
pub fn run_connect_modal(config: &mut Config) -> anyhow::Result<bool> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::io::Write as _;

    let catalog = load_catalog()?;
    let all_items = build_connect_items(&catalog);
    let mut filter = String::new();
    let mut sel = 0usize;
    let mut frame = crate::tui::InlineFrame::default();
    let mut stdout = std::io::stdout();

    crossterm::terminal::enable_raw_mode()?;
    let _ = write!(stdout, "\x1b[?25l"); // Hide cursor during selection
    stdout.flush()?;

    let selected_item = (|| -> anyhow::Result<Option<ConnectItem>> {
        loop {
            let auth_keys = load_auth_keys();
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

            let tw = crate::term_width();
            let th = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
            let modal_w = 54.min(tw.saturating_sub(4)).max(36);
            let modal_pad = tw.saturating_sub(modal_w) / 2;
            let pad_str = " ".repeat(modal_pad);

            let max_visible = 12.min(th.saturating_sub(8)).max(4);
            let start = scroll_start(sel, max_visible);
            let visible_slice = if filtered.is_empty() {
                &[]
            } else {
                &filtered[start..(start + max_visible).min(filtered.len())]
            };

            let mut c = crate::tui::Container::new();

            // 1. Header: Connect a provider                 esc
            let title_left = "\x1b[1;37mConnect a provider\x1b[0m";
            let esc_right = "\x1b[2;37mesc\x1b[0m";
            let title_pad = modal_w.saturating_sub(18 + 3);
            c.push(Box::new(crate::tui::Text::new(
                format!("{pad_str}{}{}{}", title_left, " ".repeat(title_pad), esc_right),
                0,
            )));
            c.push(Box::new(crate::tui::Text::new(String::new(), 0)));

            // 2. Search input
            let search_line = if filter.is_empty() {
                format!("{pad_str}\x1b[48;2;246;173;126m\x1b[38;2;0;0;0mS\x1b[0m\x1b[2;37mearch\x1b[0m")
            } else {
                format!("{pad_str}\x1b[1;37mSearch:\x1b[0m {filter}\x1b[7m \x1b[0m")
            };
            c.push(Box::new(crate::tui::Text::new(search_line, 0)));
            c.push(Box::new(crate::tui::Text::new(String::new(), 0)));

            // 3. Render items with category headers when appropriate
            if filtered.is_empty() {
                c.push(Box::new(crate::tui::Text::new(format!("{pad_str}  \x1b[2mNo matching providers\x1b[0m"), 0)));
            } else {
                let mut last_category: Option<&'static str> = None;
                for (rel_idx, item) in visible_slice.iter().enumerate() {
                    let abs_idx = start + rel_idx;
                    let is_selected = abs_idx == sel;

                    // Show category header if category changes (and filter is empty)
                    if filter.is_empty() && last_category != Some(item.category) {
                        last_category = Some(item.category);
                        c.push(Box::new(crate::tui::Text::new(
                            format!("{pad_str}\x1b[1;38;2;167;139;250m{}\x1b[0m", item.category),
                            0,
                        )));
                    }

                    let is_connected = auth_keys.contains_key(&item.id)
                        || (config.base_url == item.base_url && config.api_key.is_some());

                    let check_glyph = if is_connected { "✓ " } else { "  " };

                    if is_selected {
                        // OpenCode peach highlight bar across the full row
                        let sub = if item.sublabel.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", item.sublabel)
                        };
                        let raw_content = format!(" {check_glyph}{}{sub}", item.name);
                        let fill = modal_w.saturating_sub(raw_content.chars().count());
                        let full_bar = format!("{}{}", raw_content, " ".repeat(fill));
                        let row_styled = format!("{pad_str}\x1b[48;2;246;173;126m\x1b[38;2;0;0;0m{full_bar}\x1b[0m");
                        c.push(Box::new(crate::tui::Text::new(row_styled, 0)));
                    } else {
                        let check_styled = if is_connected {
                            "\x1b[38;2;74;222;128m✓\x1b[0m "
                        } else {
                            "  "
                        };
                        let name_styled = format!("\x1b[1;37m{}\x1b[0m", item.name);
                        let sub_styled = if item.sublabel.is_empty() {
                            String::new()
                        } else {
                            format!(" \x1b[2;38;2;130;130;130m{}\x1b[0m", item.sublabel)
                        };
                        c.push(Box::new(crate::tui::Text::new(
                            format!("{pad_str} {check_styled}{name_styled}{sub_styled}"),
                            0,
                        )));
                    }
                }
            }

            frame.draw(&mut stdout, &c, tw)?;

            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
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
                    KeyCode::PageUp => sel = sel.saturating_sub(10),
                    KeyCode::PageDown => {
                        if !filtered.is_empty() {
                            sel = (sel + 10).min(filtered.len() - 1);
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
                    KeyCode::Enter => {
                        if let Some(&item) = filtered.get(sel) {
                            return Ok(Some(item.clone()));
                        }
                    }
                    KeyCode::Esc => return Ok(None),
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();

    crossterm::terminal::disable_raw_mode()?;
    let _ = write!(stdout, "\x1b[?25h"); // Restore cursor
    stdout.flush()?;

    frame.erase(&mut stdout)?;

    let Some(item) = selected_item? else {
        return Ok(false);
    };

    // Connection flow
    if item.no_auth {
        config.base_url = item.base_url.clone();
        config.api_key = None;
        config.model = Some(item.default_model.clone());
        let path = saved_config_path()?;
        save_saved_config_at(&path, &SavedConfig {
            base_url: Some(item.base_url.clone()),
            api_key: None,
            model: Some(item.default_model.clone()),
            auth_mode: Some("none".into()),
        })?;
        println!("\r\x1b[38;2;74;222;128m✓\x1b[0m Connected to \x1b[1m{}\x1b[0m! Active model: \x1b[1m{}\x1b[0m\r\n", item.name, item.default_model);
        return Ok(true);
    }

    let existing = load_auth_keys()
        .get(&item.id)
        .cloned()
        .or_else(|| if config.base_url == item.base_url { config.api_key.clone() } else { None });

    let hint = if item.env_key.is_empty() { "API_KEY" } else { &item.env_key };
    let status_hint = if existing.is_some() {
        "stored \u{2014} Enter keeps it"
    } else {
        hint
    };

    println!("\r\x1b[1;37mConnect to {}\x1b[0m", item.name);
    let prompt = format!("Enter API key ({}): ", status_hint);
    let key_in = match read_secret(&prompt) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };
    let key = if key_in.is_empty() {
        existing.unwrap_or_default()
    } else {
        key_in
    };

    if key.is_empty() {
        println!("no API key entered");
        return Ok(false);
    }

    save_auth_key(&item.id, &key)?;
    config.base_url = item.base_url.clone();
    config.api_key = Some(key.clone());

    let model = if !item.default_model.is_empty() {
        item.default_model.clone()
    } else {
        "default".to_string()
    };
    config.model = Some(model.clone());

    let path = saved_config_path()?;
    let mut saved = load_saved_config_at(&path);
    saved.base_url = Some(config.base_url.clone());
    saved.api_key = config.api_key.clone();
    saved.model = config.model.clone();
    saved.auth_mode = Some("api_key".into());
    save_saved_config_at(&path, &saved)?;

    println!(
        "\r\x1b[38;2;74;222;128m✓\x1b[0m Connected to \x1b[1m{}\x1b[0m! Active model: \x1b[1m{}\x1b[0m\r\n",
        item.name, model
    );
    Ok(true)
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
    fn route_onboarding_maps_indices() {
        use super::route_onboarding as r;
        assert_eq!(r(0), super::OnboardingChoice::Free);
        assert_eq!(r(1), super::OnboardingChoice::ApiKey);
        assert_eq!(r(2), super::OnboardingChoice::OAuth);
        assert_eq!(r(3), super::OnboardingChoice::Local);
        assert_eq!(r(4), super::OnboardingChoice::Skip);
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

    #[test]
    fn filter_matches_id_or_name_case_insensitively() {
        assert!(matches_filter("", "openrouter", "OpenRouter"));
        assert!(matches_filter("open", "openrouter", "Whatever"));
        assert!(matches_filter("ROUTER", "openrouter", "whatever"));
        assert!(matches_filter("deep", "x", "DeepSeek"));
        assert!(!matches_filter("zzz", "openrouter", "OpenRouter"));
        // empty filter matches everything; non-empty must hit id or name
        let items = vec![
            ("anthropic".into(), "Anthropic".into()),
            ("openrouter".into(), "OpenRouter".into()),
        ];
        assert_eq!(filtered_indices(&items, ""), vec![0, 1]);
        assert_eq!(filtered_indices(&items, "OPEN"), vec![1]);
        assert!(filtered_indices(&items, "nomatch").is_empty());
    }

    #[test]
    fn scroll_window_keeps_selection_visible() {
        assert_eq!(scroll_start(0, 12), 0);
        assert_eq!(scroll_start(11, 12), 0); // first page holds sel 0..=11
        assert_eq!(scroll_start(12, 12), 1);
        assert_eq!(scroll_start(200, 12), 189);
        assert_eq!(scroll_start(4, 5), 0); // slash-panel window size
        assert_eq!(scroll_start(6, 5), 2);
    }

    #[test]
    fn catalog_items_are_built_for_picker() {
        let cat = load_catalog().unwrap();
        let items: Vec<(String, String)> = cat.iter().map(|(id, p)| (id.clone(), p.name.clone())).collect();
        assert_eq!(items.len(), cat.len());
        // filtering the built items finds a known provider by both id and name
        assert!(items.iter().any(|(id, name)| matches_filter("deep", id, name)));
    }
}
