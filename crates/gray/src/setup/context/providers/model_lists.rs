#[derive(Default)]
pub(super) struct ProviderModels {
    pub(super) active: String,
    pub(super) ids: std::collections::HashMap<String, Vec<String>>,
}

pub(super) static PROVIDER_MODELS: std::sync::OnceLock<std::sync::RwLock<ProviderModels>> =
    std::sync::OnceLock::new();

pub(super) fn provider_models_cell() -> &'static std::sync::RwLock<ProviderModels> {
    PROVIDER_MODELS.get_or_init(|| std::sync::RwLock::new(ProviderModels::default()))
}

pub fn set_active_model_provider(base_url: &str) {
    if let Ok(mut cache) = provider_models_cell().write() {
        cache.active = base_url.trim_end_matches('/').to_string();
    }
}

pub(super) fn active_provider_base_url() -> String {
    provider_models_cell()
        .read()
        .ok()
        .map(|cache| cache.active.clone())
        .unwrap_or_default()
}

pub(crate) fn cache_provider_model_ids(base_url: &str, models: &[(String, String)]) {
    if let Ok(mut cache) = provider_models_cell().write() {
        let mut ids: Vec<String> = models.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids.dedup();
        cache
            .ids
            .insert(base_url.trim_end_matches('/').to_string(), ids);
    }
}

pub fn cached_model_ids() -> Vec<String> {
    provider_models_cell()
        .read()
        .ok()
        .and_then(|cache| cache.ids.get(&cache.active).cloned())
        .unwrap_or_default()
}

pub(super) fn is_loopback_base(base: &str) -> bool {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|host| is_loopback_host(&host))
}

pub(super) fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub(crate) fn save_provider_model_list_at(
    home: &std::path::Path,
    base_url: &str,
    models: &[(String, String)],
) {
    let rows = models
        .iter()
        .map(|(id, name)| crate::setup::variants::CachedRow::Pair(id.clone(), name.clone()))
        .collect();
    save_provider_model_rows_at(home, base_url, rows);
}

pub(crate) fn save_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    use crate::setup::variants::CachedRow;
    if rows.is_empty() {
        return;
    }
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    if base.is_empty() || is_loopback_base(&base) {
        return;
    }
    let path = home.join("provider_models.json");
    let mut map = read_model_entries(&path);
    let old = map.remove(&base).map(CacheEntry::rows).unwrap_or_default();
    let rows = rows
        .into_iter()
        .map(|row| match row {
            CachedRow::Pair(id, name) => match old.iter().find(|o| o.id() == id) {
                Some(CachedRow::Full(m)) => {
                    CachedRow::Full(crate::setup::variants::CachedModel { name, ..m.clone() })
                }
                _ => CachedRow::Pair(id, name),
            },
            full => full,
        })
        .collect();
    map.insert(
        base,
        CacheEntry::Stamped {
            fetched_at: gray_core::time::now_secs(),
            rows,
        },
    );
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };

    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if std::fs::write(&tmp, s).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

pub(super) const PROVIDER_MODELS_TTL_SECS: u64 = 30 * 24 * 3600;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum CacheEntry {
    Stamped {
        fetched_at: u64,
        rows: Vec<crate::setup::variants::CachedRow>,
    },
    Rows(Vec<crate::setup::variants::CachedRow>),
}

impl CacheEntry {
    pub(super) fn rows(self) -> Vec<crate::setup::variants::CachedRow> {
        match self {
            CacheEntry::Stamped { rows, .. } | CacheEntry::Rows(rows) => rows,
        }
    }

    pub(super) fn fresh(self) -> Option<Self> {
        match &self {
            CacheEntry::Stamped { fetched_at, .. }
                if gray_core::time::now_secs().saturating_sub(*fetched_at)
                    > PROVIDER_MODELS_TTL_SECS =>
            {
                None
            }
            _ => Some(self),
        }
    }
}

pub(super) fn read_model_entries(
    path: &std::path::Path,
) -> std::collections::BTreeMap<String, CacheEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| {
            serde_json::from_str::<std::collections::BTreeMap<String, CacheEntry>>(&s).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, e)| e.fresh().map(|e| (k, e)))
        .collect()
}

pub(super) fn read_model_list_map(
    path: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>> {
    read_model_entries(path)
        .into_iter()
        .map(|(k, e)| (k, e.rows()))
        .collect()
}

pub(crate) fn provider_model_lists(
    home: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>> {
    read_model_list_map(&home.join("provider_models.json"))
}

pub(crate) fn load_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
) -> Vec<crate::setup::variants::CachedRow> {
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    read_model_list_map(&home.join("provider_models.json"))
        .remove(&base)
        .unwrap_or_default()
}

pub(crate) fn save_provider_model_list(base_url: &str, models: &[(String, String)]) {
    if cfg!(test) {
        return;
    }
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_list_at(&home, base_url, models);
    }
}

pub(crate) fn save_provider_model_rows(
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    if cfg!(test) {
        return;
    }
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_rows_at(&home, base_url, rows);
    }
}

pub(crate) fn load_provider_model_list(base_url: &str) -> Vec<(String, String)> {
    let Ok(home) = crate::setup::catalog::gray_home() else {
        return Vec::new();
    };
    let mut rows = load_provider_model_rows_at(&home, base_url);
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    crate::setup::composite_rows::extend_with_composites(&home, &base, &mut rows);
    for row in &rows {
        row.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}

/// [`load_provider_model_list`] against an already-parsed
/// [`provider_model_lists`] map, for callers that resolve many providers in
/// one go (the /models rail) and shouldn't re-read the file for each.
pub(crate) fn provider_model_list_in(
    home: &std::path::Path,
    lists: &std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>>,
    base_url: &str,
) -> Vec<(String, String)> {
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    let mut rows = lists.get(&base).cloned().unwrap_or_default();
    crate::setup::composite_rows::extend_with_composites_in(home, lists, &base, &mut rows);
    for row in &rows {
        row.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}
