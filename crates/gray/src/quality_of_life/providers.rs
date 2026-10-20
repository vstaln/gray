use std::collections::BTreeMap;

use crate::setup::catalog::{
    Catalog, ConnectAuth, ConnectItem, Detected, normalize_custom_base_url,
};

const PROVIDERS_JSON: &str = include_str!("../../assets/providers.json");

pub(crate) fn preset_catalog() -> anyhow::Result<Catalog> {
    if !super::enabled() {
        return Ok(Catalog::new());
    }
    Ok(serde_json::from_str(PROVIDERS_JSON)?)
}

pub(crate) const POPULAR_DEFS: &[(&str, &str, &str, &str, bool)] = &[
    (
        "openai",
        "OpenAI",
        "(API key)",
        "https://api.openai.com/v1",
        false,
    ),
    (
        "anthropic",
        "Anthropic",
        "(API key)",
        "https://api.anthropic.com/v1",
        false,
    ),
    (
        "google",
        "Google",
        "(Gemini API key)",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        false,
    ),
    (
        "openrouter",
        "OpenRouter",
        "(Access 300+ models)",
        "https://openrouter.ai/api/v1",
        false,
    ),
    (
        "commandcode",
        "CommandCode",
        "(API key)",
        "https://api.commandcode.ai/provider/v1",
        false,
    ),
    (
        "deepseek",
        "DeepSeek",
        "",
        "https://api.deepseek.com",
        false,
    ),
    (
        "groq",
        "Groq",
        "(Fast inference)",
        "https://api.groq.com/openai/v1",
        false,
    ),
    (
        "ollama",
        "Ollama",
        "(Local http://localhost:11434)",
        "http://localhost:11434/v1",
        true,
    ),
    (
        "github-copilot",
        "GitHub Copilot",
        "",
        "https://api.githubcopilot.com",
        false,
    ),
    (
        "xai",
        "xAI (Grok)",
        "(Grok login or API key)",
        "https://api.x.ai/v1",
        false,
    ),
    (
        "mistral",
        "Mistral",
        "(API key)",
        "https://api.mistral.ai/v1",
        false,
    ),
];

const ENV_KEY_TABLE: &[(&str, &[&str], &str)] = &[
    (
        "openai",
        &["OPENAI_API_KEY"],
        "https://platform.openai.com/api-keys",
    ),
    (
        "anthropic",
        &["ANTHROPIC_API_KEY"],
        "https://console.anthropic.com/settings/keys",
    ),
    (
        "google",
        &[
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY",
        ],
        "https://aistudio.google.com/apikey",
    ),
    (
        "openrouter",
        &["OPENROUTER_API_KEY"],
        "https://openrouter.ai/keys",
    ),
    (
        "deepseek",
        &["DEEPSEEK_API_KEY"],
        "https://platform.deepseek.com/api_keys",
    ),
    ("groq", &["GROQ_API_KEY"], "https://console.groq.com/keys"),
    (
        "mistral",
        &["MISTRAL_API_KEY"],
        "https://console.mistral.ai/api-keys",
    ),
    ("xai", &["XAI_API_KEY"], "https://console.x.ai"),
];

pub(crate) fn is_popular_preset(id: &str) -> bool {
    POPULAR_DEFS.iter().any(|(pid, ..)| *pid == id)
}

pub(crate) fn is_known_preset_base(normalized_base: &str, catalog: &Catalog) -> bool {
    super::enabled()
        && (POPULAR_DEFS
            .iter()
            .any(|(.., url, _)| normalize_custom_base_url(url) == normalized_base)
            || catalog
                .values()
                .any(|p| normalize_custom_base_url(&p.base_url) == normalized_base))
}

pub(crate) fn append_connect_presets(items: &mut Vec<ConnectItem>, catalog: &Catalog) {
    if !super::enabled() {
        return;
    }

    let mut popular_ids = std::collections::HashSet::new();
    popular_ids.insert("custom".to_string());

    for &(id, name, sublabel, base_url, no_auth) in POPULAR_DEFS {
        popular_ids.insert(id.to_string());
        let url = catalog.get(id).map_or(base_url, |p| p.base_url.as_str());
        items.push(ConnectItem {
            id: id.to_string(),
            name: name.to_string(),
            sublabel: sublabel.to_string(),
            base_url: url.to_string(),
            no_auth,
            auth: if no_auth {
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
) -> Option<(&'static str, String)> {
    let (_, vars, _) = ENV_KEY_TABLE.iter().find(|(k, ..)| *k == id)?;
    vars.iter()
        .find_map(|var| env(var).filter(|v| !v.trim().is_empty()).map(|v| (*var, v)))
}

pub(crate) fn key_url(id: &str) -> Option<&'static str> {
    ENV_KEY_TABLE
        .iter()
        .find(|(k, ..)| *k == id)
        .map(|(.., url)| *url)
}

pub(crate) fn detect_connect_candidates(
    ollama_running: bool,
    env: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, Detected> {
    let mut found = BTreeMap::new();
    if !super::enabled() {
        return found;
    }
    for (id, ..) in ENV_KEY_TABLE {
        if let Some((var, value)) = detected_env_key(id, &env) {
            found.insert(
                id.to_string(),
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
