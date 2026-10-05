//! Env-only configuration, resolved per call so respawns pick up changes.

pub struct Config {
    /// Base URL without trailing slash or `/v1` suffix.
    pub base: String,
    pub model: String,
    /// Never printed, logged, or embedded in any reply.
    pub key: Option<String>,
}

const DEFAULT_BASE: &str = "http://localhost:11435";

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Normalize a base URL: trim trailing `/`, then strip a `/v1` suffix.
pub fn normalize_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

/// Host portion of a URL, lowercased, without port or brackets.
pub fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.to_ascii_lowercase()
}

pub fn is_local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

impl Config {
    pub fn resolve() -> Self {
        let raw = env_non_empty("SYSTEMONE_BASE_URL")
            .or_else(|| env_non_empty("TYPESAFE_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE.to_string());
        let base = normalize_base(&raw);
        let local = is_local_host(&host_of(&base));
        let model = env_non_empty("SYSTEMONE_MODEL")
            .unwrap_or_else(|| if local { "laya" } else { "jev-latest" }.to_string());
        let key = env_non_empty("SYSTEMONE_API_KEY").or_else(|| env_non_empty("TYPESAFE_API_KEY"));
        Config { base, model, key }
    }

    pub fn systemone_url(&self) -> String {
        format!("{}/v1/systemone", self.base)
    }

    pub fn models_url(&self) -> String {
        format!("{}/v1/models", self.base)
    }
}

#[path = "config_tests.rs"]
#[cfg(test)]
mod tests;
