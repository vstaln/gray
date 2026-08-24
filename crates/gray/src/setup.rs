//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot, see scripts/gen-providers.py), persisting
//! to ~/.gray/config.json. Flow mirrors pi: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config::Config, rule};

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

/// Bundled snapshot — regenerated via scripts/gen-providers.py.
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
pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

pub(crate) fn read_line(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
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
fn select_from_list(title: &str, items: &[(String, String)]) -> anyhow::Result<Option<usize>> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyModifiers};

    const ROWS: usize = 12;
    let mut stdout = std::io::stdout();
    let mut filter = String::new();
    let mut sel = 0usize;
    let mut drawn = 0usize;

    crossterm::terminal::enable_raw_mode()?;
    let result = (|| -> anyhow::Result<Option<usize>> {
        loop {
            let filtered = filtered_indices(items, &filter);
            if sel >= filtered.len() {
                sel = filtered.len().saturating_sub(1);
            }
            let selected = filtered.get(sel).copied();

            // Build rows plain, clip parts to terminal width, then add ANSI.
            let width = crate::term_width().saturating_sub(6);
            let mut lines = vec![format!(
                "{title}> {} \x1b[2m({}/{})\x1b[0m",
                filter,
                if filtered.is_empty() { 0 } else { sel + 1 },
                filtered.len()
            )];
            if filtered.is_empty() {
                lines.push("  no matches".to_string());
            } else {
                let start = scroll_start(sel, ROWS);
                for &i in &filtered[start..(start + ROWS).min(filtered.len())] {
                    let body = format!(
                        "  {}  {}",
                        clip(&items[i].0, 32),
                        clip(&items[i].1, width.saturating_sub(34))
                    );
                    lines.push(if Some(i) == selected {
                        format!("\x1b[7m{body}\x1b[0m")
                    } else {
                        body
                    });
                }
            }

            // Redraw: jump back over the previous frame, clear each line as written.
            if drawn > 0 {
                write!(stdout, "\x1b[{drawn}A")?;
            }
            for l in &lines {
                write!(stdout, "\r\x1b[2K{l}\r\n")?;
            }
            write!(stdout, "\r")?;
            drawn = lines.len();
            stdout.flush()?;

            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => sel = (sel + 1).min(filtered.len().saturating_sub(1)),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, .. }) => match code {
                    KeyCode::Char(c) => {
                        filter.push(c);
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        sel = 0;
                    }
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(filtered.len().saturating_sub(1)),
                    KeyCode::Enter => return Ok(selected),
                    KeyCode::Esc => return Ok(None),
                    _ => {}
                },
                _ => {}
            }
        }
    })();
    crossterm::terminal::disable_raw_mode()?;

    // Erase the picker UI so the transcript shows only the outcome.
    if drawn > 0 {
        write!(stdout, "\x1b[{drawn}A\r\x1b[J")?;
        stdout.flush()?;
    }
    result
}

/// Interactive provider picker over the bundled catalog. Returns the chosen
/// provider id; errors if the user aborts with Esc/Ctrl+C.
pub fn select_from_catalog(catalog: &Catalog) -> anyhow::Result<String> {
    let items: Vec<(String, String)> = catalog
        .iter()
        .map(|(id, p)| (id.clone(), p.name.clone()))
        .collect();
    match select_from_list("provider", &items)? {
        Some(i) => Ok(items[i].0.clone()),
        None => anyhow::bail!("provider selection aborted"),
    }
}

/// What the user picked on the onboarding screen.
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

/// Default model suggestions for keyless/local setups.
pub const LOCAL_MODEL_SUGGESTIONS: [&str; 3] = ["llama3.2", "qwen2.5-coder", "deepseek-r1"];

/// fx-style first-run screen. Returns true when the user finished configuration.
pub fn run_onboarding(config: &mut Config) -> anyhow::Result<bool> {
    println!();
    println!("  Welcome to gray");
    println!("  \x1b[2mgray by alignment — a minimal agent harness that runs tools,");
    println!("  edits code, and works with any model provider.\x1b[0m");
    println!();
    println!("  Get started");

    let options = vec![
        ("Start free — no sign-up", "chat in seconds via OpenCode Zen".to_string()),
        ("Add an API key", "pick from 200+ providers".to_string()),
        ("Sign in with account", "(coming soon)".to_string()),
        ("Use a local model", "Ollama / llama.cpp / vLLM".to_string()),
        ("Skip for now", String::new()),
    ];
    let items: Vec<(String, String)> = options
        .into_iter()
        .map(|(a, b)| (a.to_string(), b))
        .collect();
    let choice = match select_from_list("Get started", &items)? {
        Some(i) => route_onboarding(i),
        None => OnboardingChoice::Skip,
    };

    match choice {
        OnboardingChoice::Free => {
            // Zero-friction path: keyless OpenCode Zen free tier.
            let path = saved_config_path()?;
            save_saved_config_at(&path, &SavedConfig {
                base_url: Some("https://opencode.ai/zen/v1".into()),
                api_key: Some("not-needed".into()),
                model: Some("deepseek-v4-flash-free".into()),
                auth_mode: Some("none".into()),
            })?;
            config.base_url = "https://opencode.ai/zen/v1".into();
            config.api_key = Some("not-needed".into());
            config.model = Some("deepseek-v4-flash-free".into());
            println!("saved — you're on the free tier. /model to switch anytime.");
            return Ok(true);
        }
        OnboardingChoice::ApiKey => run_setup(config)?,
        OnboardingChoice::OAuth => {
            println!("(OAuth sign-in lands in a future release — API keys work today)");
            return Ok(false);
        }
        OnboardingChoice::Local => {
            println!("{}", rule("local model"));
            let base = read_line("base url [http://localhost:11434/v1]: ")?;
            let base = if base.is_empty() { "http://localhost:11434/v1".to_string() } else { base };
            let suggested = LOCAL_MODEL_SUGGESTIONS[0];
            let model_in = read_line(&format!("model [{suggested}]: "))?;
            let model = if model_in.is_empty() { suggested.to_string() } else { model_in };
            let path = saved_config_path()?;
            save_saved_config_at(&path, &SavedConfig {
                base_url: Some(base.clone()),
                api_key: Some("not-needed".into()),
                model: Some(model.clone()),
                auth_mode: Some("none".into()),
            })?;
            config.base_url = base;
            config.api_key = Some("not-needed".into());
            config.model = Some(model);
            println!("saved — no auth needed for local endpoints.");
        }
        OnboardingChoice::Skip => return Ok(false),
    }
    Ok(true)
}

/// Runs the interactive provider/key/model setup, mutating `config` in place
pub fn run_setup(config: &mut Config) -> anyhow::Result<()> {
    let catalog = load_catalog()?;
    println!("welcome to gray — pick a provider to get started");
    println!("(saved to {}; /sys edits the system prompt)", saved_config_path()?.display());

    // ---- provider: live-filter picker over the catalog --------------------
    let pid = select_from_catalog(&catalog)?;
    let provider = &catalog[&pid];
    println!("provider → {}", provider.name);

    // ---- api key ----------------------------------------------------------
    println!("{}", rule("credentials"));
    let hint = env_hint(provider);
    let env_key = config.api_key.clone().unwrap_or_default();
    let key_in = read_line(&format!(
        "{} API key ({}): ",
        provider.name,
        if hint == "API_KEY" { "stored locally" } else { &hint }
    ))?;
    let api_key = if key_in.is_empty() { env_key } else { key_in };

    // ---- model: same live-filter picker over the provider's models --------
    println!("{}", rule("model"));
    let model = if provider.models.is_empty() {
        // Catalog entry with no models: fall back to free text.
        let m = read_line("model id: ")?;
        anyhow::ensure!(!m.is_empty(), "no model given");
        m
    } else {
        let items: Vec<(String, String)> = provider
            .models
            .iter()
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect();
        match select_from_list("model", &items)? {
            Some(i) => items[i].0.clone(),
            None => items[0].0.clone(), // Esc keeps the default model
        }
    };

    // ---- persist + apply --------------------------------------------------
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
    Ok(())
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
