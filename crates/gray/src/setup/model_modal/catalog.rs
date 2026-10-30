use super::*;

pub(crate) fn cached_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let cached = saved_models_for(base_url, base_url);
    if cached.is_empty() {
        return picker_models_for(base_url, api_key);
    }
    let (base, key) = (base_url.to_string(), api_key.map(str::to_string));

    std::thread::spawn(move || {
        super::super::context::fetch_live_provider_models(&base, key.as_deref())
    });
    cached
}

pub(super) const MODEL_REFRESH_TTL: std::time::Duration = std::time::Duration::from_secs(300);

pub(super) static MODEL_REFRESHED: std::sync::Mutex<Vec<(String, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

pub(crate) fn model_refresh_due(list_key: &str) -> bool {
    let seen = MODEL_REFRESHED.lock().unwrap_or_else(|e| e.into_inner());
    !seen
        .iter()
        .any(|(k, at)| k == list_key && at.elapsed() < MODEL_REFRESH_TTL)
}

pub(super) fn note_model_refreshed(list_key: &str) {
    let mut seen = MODEL_REFRESHED.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    match seen.iter_mut().find(|(k, _)| k == list_key) {
        Some(entry) => entry.1 = now,
        None => seen.push((list_key.to_string(), now)),
    }
}

pub(crate) fn fetch_models_live(
    list_key: &str,
    plugin: Option<crate::providers::InstalledProvider>,
    base_url: &str,
    api_key: Option<&str>,
) -> Vec<(String, String)> {
    let started = std::time::Instant::now();
    let live = match plugin {
        Some(installed) => super::super::context::fetch_plugin_provider_models(installed),
        None => super::super::context::fetch_live_provider_models(base_url, api_key),
    };
    if !live.is_empty() {
        note_model_refreshed(list_key);
    }
    log::debug!(target: "gray_timing", "model list refresh list_key={list_key} rows={} elapsed_ms={}", live.len(), started.elapsed().as_millis());
    live
}

pub(crate) fn merge_models(base_url: &str, live: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut models = live;
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

pub(crate) fn picker_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut models = fetch_live_provider_models(base_url, api_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}
