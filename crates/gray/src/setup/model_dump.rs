//! Machine-readable view of the `/models` provider rail and model lists, for
//! embedding surfaces (gray-ide) that must mirror the picker's contents without
//! reimplementing provider/plugin resolution. `--dump-models` prints
//! [`model_catalog_json`]; `--provider <id>` switches this process's provider
//! in memory via [`select_cli_provider`] — the same resolution the panes apply
//! on pick, minus the saved-config write.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::Config;

use super::catalog::{
    ConnectAuth, load_saved_config_at, normalize_custom_base_url, saved_config_path,
};
use super::model_modal::{
    RECOMMENDED_HEAD, composites_first, fetch_models_live, head_first, is_composite_id,
    recent_head_for, recent_prefix_len, row_levels, saved_models_for,
};
use super::model_panes::{
    PaneProvider, Place, SavedLists, build_rail, place, provider_is_active, scoped_config,
};
use super::variants::Parts;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DumpModels {
    /// Cached model lists; providers with an empty cache are fetched live.
    Cached,
    /// Refresh every usable provider's list live before printing.
    Live,
}

const LIVE_BUDGET: Duration = Duration::from_secs(15);

/// Total wall-clock budget for catalog work. Provider metadata, model-list
/// fetches and the saved-config lock can all block far longer than an
/// embedding surface can wait, so the public entry points always answer
/// within this window — partial or empty results beat a hang.
const DUMP_BUDGET: Duration = Duration::from_secs(20);

fn bounded<T: Send + 'static>(d: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(d).ok()
}

/// Providers found without a saved credential (env API key, running ollama).
type Detected = std::collections::BTreeMap<String, super::catalog::Detected>;

fn rail_ctx(
    config: &Config,
) -> (
    Vec<PaneProvider>,
    std::collections::BTreeMap<String, String>,
    Detected,
) {
    let ollama = super::context::ollama_last_seen().unwrap_or(false);
    let (rail, keys) = build_rail(config, ollama, None);
    let detected = super::presets::detect_connect_candidates(ollama, |k| std::env::var(k).ok());
    (rail, keys, detected)
}

/// Can this provider run right now — saved credential, no-auth, or detected?
fn usable(
    p: &PaneProvider,
    keys: &std::collections::BTreeMap<String, String>,
    detected: &Detected,
) -> bool {
    p.install.is_none()
        && p.item.id != "custom"
        && (p.adoptable(keys) || detected.contains_key(&p.item.id))
}

fn env_key(
    p: &PaneProvider,
    keys: &std::collections::BTreeMap<String, String>,
    config: &Config,
    detected: &Detected,
) -> Option<String> {
    p.key(keys, config).or_else(|| {
        detected
            .get(&p.item.id)
            .and_then(|d| d.env.clone().map(|(_, v)| v))
    })
}

fn load_cached(
    lists: Option<&SavedLists>,
    saved: &super::catalog::SavedConfig,
    p: &PaneProvider,
) -> Vec<(String, String)> {
    let load = |key: &str| match lists {
        Some(l) => l.models(saved, key, &p.item.base_url),
        None => saved_models_for(key, &p.item.base_url),
    };
    let mut models = load(&p.list_key);
    if models.is_empty() {
        let alt = normalize_custom_base_url(&p.item.base_url);
        if alt != p.list_key {
            models = load(&alt);
        }
    }
    models
}

/// The picker's row order for one provider, exactly as `model_picker_loop`
/// composes it: composites first, then the recent/current prefix, then a
/// recommended head (when no recents lead), then everything else. Each row is
/// tagged so callers can draw the same dividers the modal does.
fn ordered_rows(p: &PaneProvider, config: &Config, models: &[(String, String)]) -> Vec<Value> {
    let (mut current_id, recent_ids) = recent_head_for(&p.item.base_url);
    if !provider_is_active(p, config) {
        current_id = None;
    }
    let matched: Vec<&(String, String)> = models.iter().collect();
    let lead = matched.iter().filter(|(id, _)| is_composite_id(id)).count();
    let mut ordered = composites_first(&matched);
    let mut plain = ordered.split_off(lead);
    let sep = recent_prefix_len(
        current_id.as_deref(),
        &recent_ids,
        plain.iter().map(|(id, _)| id.as_str()),
    );
    let mut all_head = false;
    let mut div_at = sep;
    if sep == 0 {
        let curated = super::context::curated_recommended(&p.item.id);
        let head: Vec<String> = super::recommend::recommended_ids(
            models,
            curated.as_deref(),
            super::context::model_meta,
            RECOMMENDED_HEAD,
        )
        .into_iter()
        .filter(|id| !is_composite_id(id))
        .collect();
        plain = head_first(&plain, &head);
        if !head.is_empty() && head.len() < plain.len() {
            div_at = head.len();
            all_head = true;
        }
    }
    ordered.extend(plain);
    ordered
        .iter()
        .enumerate()
        .map(|(i, (id, name))| {
            let group = if i < lead {
                "composite"
            } else if i < lead + sep {
                "recent"
            } else if all_head && i < lead + div_at {
                "recommended"
            } else {
                "other"
            };
            json!({
                "id": id,
                "name": name,
                "efforts": row_levels(id, false, &Parts::new()),
                "group": group,
                "current": Some(id.as_str()) == current_id.as_deref(),
            })
        })
        .collect()
}

/// `gray --dump-models[cached|live]`: the /models rail, in rail order, each
/// usable provider with its model rows in picker order and effort levels. No
/// credentials are emitted.
pub fn dump_model_catalog_json(config: &Config, mode: DumpModels) -> Value {
    let config = config.clone();
    bounded(DUMP_BUDGET, move || build_model_catalog(&config, mode)).unwrap_or_else(|| {
        json!({"version": 1, "active": null, "providers": [], "error": "model catalog timed out"})
    })
}

fn build_model_catalog(config: &Config, mode: DumpModels) -> Value {
    let (rail, keys, detected) = rail_ctx(config);
    let path = saved_config_path().ok();
    let saved = path
        .as_deref()
        .map(load_saved_config_at)
        .unwrap_or_default();
    let lists = SavedLists::load();

    let usable: Vec<&PaneProvider> = rail
        .iter()
        .filter(|p| usable(p, &keys, &detected))
        .collect();

    let mut models_of: HashMap<String, Vec<(String, String)>> = usable
        .iter()
        .map(|p| (p.item.id.clone(), load_cached(lists.as_ref(), &saved, p)))
        .collect();

    let fetching: Vec<&&PaneProvider> = usable
        .iter()
        .filter(|p| mode == DumpModels::Live || models_of[&p.item.id].is_empty())
        .collect();
    if !fetching.is_empty() {
        let (tx, rx) = mpsc::channel();
        let mut pending = 0usize;
        for p in fetching {
            pending += 1;
            let id = p.item.id.clone();
            let list_key = p.list_key.clone();
            let installed = p.installed.clone();
            let base = p.item.base_url.clone();
            let api_key = env_key(p, &keys, config, &detected);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let live = fetch_models_live(&list_key, installed, &base, api_key.as_deref());
                let _ = tx.send((id, live));
            });
        }
        drop(tx);
        let deadline = Instant::now() + LIVE_BUDGET;
        let sort_base: HashMap<String, String> = usable
            .iter()
            .map(|p| (p.item.id.clone(), p.item.base_url.clone()))
            .collect();
        for _ in 0..pending {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok((id, mut merged)) => {
                    if !merged.is_empty() {
                        let base = sort_base.get(&id).map(String::as_str).unwrap_or("");
                        saved.sort_models(base, &mut merged);
                        models_of.insert(id, merged);
                    }
                }
                Err(_) => break,
            }
        }
    }

    let providers: Vec<Value> = usable
        .iter()
        .map(|p| {
            let (kind, group) = match &p.item.auth {
                ConnectAuth::Plugin { .. } => (
                    "plugin",
                    match place(p) {
                        Place::Group(g) => match g.label() {
                            "OAuth" => "oauth",
                            _ => "logins",
                        },
                        _ => "flat",
                    },
                ),
                ConnectAuth::ApiKey => (
                    "key",
                    match place(p) {
                        Place::Group(_) => "apikey",
                        _ => "flat",
                    },
                ),
                ConnectAuth::None => ("free", "flat"),
            };
            json!({
                "id": p.item.id,
                "name": p.item.name,
                "sublabel": p.item.sublabel,
                "kind": kind,
                "group": group,
                "active": provider_is_active(p, config),
                "connected": p.connected,
                "detected": p.detected,
                "models": ordered_rows(
                    p,
                    config,
                    models_of.get(&p.item.id).map(Vec::as_slice).unwrap_or(&[]),
                ),
            })
        })
        .collect();

    json!({
        "version": 1,
        "active": rail
            .iter()
            .find(|p| provider_is_active(p, config))
            .map(|p| p.item.id.clone()),
        "providers": providers,
    })
}

/// `gray --provider <id>`: point this process's config at a provider from the
/// /models rail — same credential/model resolution the panes use on pick, but
/// in memory only (the saved selection is left alone). `cli_model`/`GRAY_MODEL`
/// keep their override; otherwise the provider's saved or first model is used.
pub fn select_cli_provider(
    config: &mut Config,
    want: &str,
    cli_model: Option<&str>,
    explicit_parts: bool,
) -> anyhow::Result<()> {
    let cfg = config.clone();
    let Some((rail, keys, detected)) = bounded(DUMP_BUDGET, move || rail_ctx(&cfg)) else {
        anyhow::bail!("provider lookup timed out — the model catalog is busy, try again");
    };
    let want_l = want.to_lowercase();
    let Some(p) = rail
        .iter()
        .find(|p| p.item.id == want)
        .or_else(|| rail.iter().find(|p| p.item.id.to_lowercase() == want_l))
        .or_else(|| rail.iter().find(|p| p.item.name.to_lowercase() == want_l))
    else {
        let mut ids: Vec<&str> = rail
            .iter()
            .filter(|p| p.install.is_none() && p.item.id != "custom")
            .map(|p| p.item.id.as_str())
            .collect();
        ids.sort();
        anyhow::bail!("unknown provider '{want}' — known ids: {}", ids.join(", "));
    };
    if p.install.is_some() || p.item.id == "custom" {
        anyhow::bail!(
            "provider '{}' isn't installed — run `gray` and use /models to set it up",
            p.item.id
        );
    }
    let detected_env = detected.get(&p.item.id).and_then(|d| d.env.clone());
    if !p.adoptable(&keys) && detected_env.is_none() {
        anyhow::bail!(
            "provider '{}' has no credentials — run `gray` and use /models to connect or sign in",
            p.item.id
        );
    }

    let kept_model = config.model.clone();
    let explicit_model = cli_model.is_some() || std::env::var_os("GRAY_MODEL").is_some();
    *config = scoped_config(config, p, &keys);
    if config.api_key.is_none()
        && let Some((_, v)) = detected_env
    {
        config.api_key = Some(v);
    }
    if explicit_model {
        config.model = kept_model;
    }

    let path = saved_config_path().ok();
    let mut saved = path
        .as_deref()
        .map(load_saved_config_at)
        .unwrap_or_default();
    let lists = SavedLists::load();
    let models = load_cached(lists.as_ref(), &saved, p);

    if config.model.is_none() {
        config.model = models
            .iter()
            .find(|(id, _)| !is_composite_id(id))
            .map(|(id, _)| id.clone());
    }
    // Canonicalize and split variant suffixes (`id-high`, `id-fast`, …) the way
    // `/models <id>` does, so the wire model and effort land correctly.
    if let Some(raw) = config.model.clone()
        && let Ok(canonical) = super::model_modal::validate_direct_model_id(&raw, &models)
    {
        match super::model_modal::decompose_model_variant(&canonical, &models) {
            Some((base, tier, fast)) => {
                config.model = Some(base);
                if let Some(tier) = tier
                    && std::env::var_os("GRAY_THINKING_EFFORT").is_none()
                {
                    config.thinking_effort = Some(tier);
                }
                if fast && std::env::var_os("GRAY_FAST").is_none() {
                    config.fast_mode = Some(true);
                }
            }
            None => config.model = Some(canonical),
        }
    }

    if std::env::var_os("GRAY_THINKING_EFFORT").is_none() {
        super::provider_auth::apply_connection_effort(config, &mut saved);
    }
    if !explicit_parts && let Some(m) = config.model.clone() {
        config.model_parts = saved
            .parts_memory
            .get(&super::catalog::effort_memory_key(
                &config.provider_id,
                &config.base_url,
                &m,
            ))
            .cloned()
            .unwrap_or_default();
    }
    crate::config::export_provider_env(config);
    Ok(())
}
