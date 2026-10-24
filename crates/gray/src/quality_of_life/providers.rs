use std::collections::BTreeMap;

use crate::setup::catalog::{
    Catalog, ConnectAuth, ConnectItem, Detected, normalize_custom_base_url,
};

pub(crate) fn preset_catalog() -> anyhow::Result<Catalog> {
    if !super::enabled() {
        return Ok(Catalog::new());
    }
    Ok(crate::setup::context::provider_meta().catalog.clone())
}

pub(crate) fn is_popular_preset(id: &str) -> bool {
    crate::setup::context::provider_meta()
        .popular
        .iter()
        .any(|p| p.id == id)
}

pub(crate) fn is_known_preset_base(normalized_base: &str, catalog: &Catalog) -> bool {
    super::enabled()
        && (crate::setup::context::provider_meta()
            .popular
            .iter()
            .any(|p| normalize_custom_base_url(&p.base_url) == normalized_base)
            || catalog
                .values()
                .any(|p| normalize_custom_base_url(&p.base_url) == normalized_base))
}

pub(crate) fn append_connect_presets(items: &mut Vec<ConnectItem>, catalog: &Catalog) {
    if !super::enabled() {
        return;
    }

    let meta = crate::setup::context::provider_meta();
    let mut popular_ids = std::collections::HashSet::new();
    popular_ids.insert("custom".to_string());

    for p in &meta.popular {
        popular_ids.insert(p.id.clone());
        let url = catalog.get(&p.id).map_or(p.base_url.as_str(), |e| e.base_url.as_str());
        items.push(ConnectItem {
            id: p.id.clone(),
            name: p.name.clone(),
            sublabel: p.sublabel.clone(),
            base_url: url.to_string(),
            no_auth: p.no_auth,
            auth: if p.no_auth {
                ConnectAuth::None
            } else {
                ConnectAuth::ApiKey
            },
        });
    }

    let mut catalog_entries: Vec<_> = catalog.iter().collect();
    catalog_entries.sort_by_key(|(_, p)| p.name.to_lowercase());

    for (id, p) in catalog_entries {
        if popular_ids.contains(id) {
            continue;
        }

        items.push(ConnectItem {
            id: id.clone(),
            name: p.name.clone(),
            sublabel: String::new(),
            base_url: p.base_url.clone(),
            no_auth: p.no_auth,
            auth: if p.no_auth {
                ConnectAuth::None
            } else {
                ConnectAuth::ApiKey
            },
        });
    }
}

pub(crate) fn detected_env_key(
    id: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    let meta = crate::setup::context::provider_meta();
    meta.env_keys.get(id)?.vars.iter().find_map(|var| {
        env(var)
            .filter(|v| !v.trim().is_empty())
            .map(|v| (var.clone(), v))
    })
}

pub(crate) fn key_url(id: &str) -> Option<String> {
    crate::setup::context::provider_meta()
        .env_keys
        .get(id)?
        .key_url
        .clone()
}

pub(crate) fn detect_connect_candidates(
    ollama_running: bool,
    env: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, Detected> {
    let mut found = BTreeMap::new();
    if !super::enabled() {
        return found;
    }
    for id in crate::setup::context::provider_meta().env_keys.keys() {
        if let Some((var, value)) = detected_env_key(id, &env) {
            found.insert(
                id.clone(),
                Detected {
                    note: format!("(found {var})"),
                    env: Some((var, value)),
                },
            );
        }
    }
    if ollama_running {
        found.insert(
            "ollama".to_string(),
            Detected {
                note: "(running locally)".to_string(),
                env: None,
            },
        );
    }
    found
}

#[path = "providers_tests.rs"]
#[cfg(test)]
mod tests;
