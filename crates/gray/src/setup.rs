//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot, see scripts/gen-providers.py), persisting
//! to ~/.gray/config.json. Flow mirrors pi: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Provider entry from the vendored catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogProvider {
    pub name: String,
    pub base_url: String,
    /// models.dev emits either a string or a list of env var names.
    #[serde(default)]
    pub env_key: serde_json::Value,
    pub featured: bool,
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

fn read_line(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Renders one page of the filtered provider list; returns the visible slice
/// and its starting offset within `filtered`.
fn render_page(filtered: &[&String], catalog: &Catalog, page: usize, filter: &str) -> (usize, usize) {
    const PER_PAGE: usize = 12;
    let start = page * PER_PAGE;
    let end = (start + PER_PAGE).min(filtered.len());
    println!();
    for (i, pid) in filtered[start..end].iter().enumerate() {
        let p = &catalog[*pid];
        let star = if p.featured { "*" } else { " " };
        println!("  {:>2}. {}{}", i + 1, star, p.name);
    }
    println!();
    println!(
        "  showing {}–{} of {} (filter: \"{}\") — number to choose, text to filter",
        start + 1,
        end,
        filtered.len(),
        if filter.is_empty() { "all" } else { filter }
    );
    (start, end)
}

/// Runs the interactive provider/key/model picker, mutating `config` in place
/// and persisting the result.
pub fn run_setup(config: &mut Config) -> anyhow::Result<()> {
    let catalog = load_catalog()?;
    println!("welcome to gray — pick a provider to get started");
    println!("(saved to {}; /sys edits the system prompt)", saved_config_path()?.display());

    // ---- provider: filter-as-you-type over the catalog -------------------
    let pid = 'provider: {
        let mut filter = String::new();
        loop {
            let filtered: Vec<&String> = catalog
                .keys()
                .filter(|k| {
                    filter.is_empty()
                        || k.contains(&filter.to_lowercase())
                        || catalog[*k].name.to_lowercase().contains(&filter.to_lowercase())
                })
                .collect();
            if filtered.is_empty() {
                println!("  no providers match \"{}\"", filter);
                filter.clear();
                continue;
            }
            let (start, end) = render_page(&filtered, &catalog, 0, &filter);
            let input = read_line("\nprovider: ")?;
            if input.is_empty() {
                continue;
            }
            if let Ok(n) = input.parse::<usize>() {
                if n >= 1 && n <= end - start {
                    break 'provider filtered[start + n - 1].clone();
                }
            }
            filter = input.to_lowercase(); // anything else becomes the new filter
        }
    };
    let provider = &catalog[&pid];
    println!("→ {}", provider.name);

    // ---- api key ----------------------------------------------------------
    let hint = env_hint(provider);
    let env_key = config.api_key.clone().unwrap_or_default();
    let key_in = read_line(&format!(
        "{} API key ({}): ",
        provider.name,
        if hint == "API_KEY" { "stored locally" } else { &hint }
    ))?;
    let api_key = if key_in.is_empty() { env_key } else { key_in };

    // ---- model ------------------------------------------------------------
    println!();
    for (i, m) in provider.models.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, m.id, m.name);
    }
    let model_in = read_line(&format!("model [{}]: ", provider.models[0].id))?;
    let model = if model_in.is_empty() {
        provider.models[0].id.clone()
    } else {
        match model_in.parse::<usize>() {
            Ok(n) if (1..=provider.models.len()).contains(&n) => provider.models[n - 1].id.clone(),
            _ => model_in, // free-text: any model id the endpoint accepts
        }
    };

    // ---- persist + apply --------------------------------------------------
    let saved = SavedConfig {
        base_url: Some(provider.base_url.clone()),
        api_key: Some(api_key.clone()),
        model: Some(model),
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
    fn saved_config_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("gray-setup2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");

        assert!(load_saved_config_at(&path).model.is_none());

        let cfg = SavedConfig {
            base_url: Some("https://api.deepseek.com/v1".into()),
            api_key: Some("sk-test".into()),
            model: Some("deepseek-chat".into()),
        };
        save_saved_config_at(&path, &cfg).unwrap();
        let loaded = load_saved_config_at(&path);
        assert_eq!(loaded.api_key.as_deref(), Some("sk-test"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
