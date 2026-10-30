use super::*;

pub(super) const CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

pub(super) async fn load_catalog<T: serde::de::DeserializeOwned>(
    name: &str,
    url: &str,
    apply: fn(T) -> usize,
) -> usize {
    let path = crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("cache").join(name));
    let from_disk = |max_age: std::time::Duration| -> Option<T> {
        let p = path.as_ref()?;
        let age = std::fs::metadata(p)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .unwrap_or_default();
        (age <= max_age).then_some(())?;
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    };
    let catalog = match from_disk(CATALOG_TTL) {
        Some(t) => Some(t),
        None => match fetch_body(url)
            .await
            .and_then(|body| Some((serde_json::from_slice::<T>(body.as_ref()).ok()?, body)))
        {
            Some((t, body)) => {
                if let Some(p) = &path
                    && let Some(dir) = p.parent()
                    && std::fs::create_dir_all(dir).is_ok()
                {
                    let tmp = p.with_extension("json.tmp");
                    if std::fs::write(&tmp, body.as_ref()).is_ok() {
                        let _ = std::fs::rename(&tmp, p);
                    }
                }
                Some(t)
            }
            None => from_disk(std::time::Duration::MAX),
        },
    };
    catalog.map_or(0, apply)
}

pub(super) async fn fetch_body(url: &str) -> Option<impl AsRef<[u8]>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok()
}

pub(super) fn models_cache_path() -> Option<std::path::PathBuf> {
    crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("models.json"))
}

pub fn load_models_cache_to_memory() -> usize {
    let Some(path) = models_cache_path() else {
        return 0;
    };
    let s = std::fs::read_to_string(path).ok().unwrap_or_default();
    if s.is_empty() {
        return 0;
    }
    let map: std::collections::HashMap<String, usize> =
        serde_json::from_str(&s).unwrap_or_default();
    let mut n = 0;
    for (k, v) in map {
        if v == 0 || get_cached_model_context(&k).is_some() {
            continue;
        }
        cache_model_context_with_source(&k, v, "disk", false);
        n += 1;
    }
    n
}

pub(super) fn merged_models_cache(
    mut disk: std::collections::HashMap<String, usize>,
    mem: impl IntoIterator<Item = (String, usize)>,
) -> Option<std::collections::HashMap<String, usize>> {
    let mut dirty = false;
    for (k, v) in mem {
        if disk.get(&k) != Some(&v) {
            disk.insert(k, v);
            dirty = true;
        }
    }
    if dirty { Some(disk) } else { None }
}

pub fn save_models_cache_to_disk() {
    // Unit tests share the real $GRAY_HOME — a mocked fetch succeeding must
    // not persist the test models they seed the in-memory cache with.
    if cfg!(test) {
        return;
    }
    let Some(path) = models_cache_path() else {
        return;
    };
    let mem: Vec<(String, usize)> = model_context_cache()
        .read()
        .map(|g| g.iter().map(|(k, v)| (k.clone(), *v)).collect())
        .unwrap_or_default();
    if mem.is_empty() {
        return;
    }
    let disk: std::collections::HashMap<String, usize> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let Some(map) = merged_models_cache(disk, mem) else {
        return;
    };
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }

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

pub(crate) fn ensure_disk_loaded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = load_models_cache_to_memory();
    });
}

pub(crate) fn warm_rates_from_disk_cache() {
    if cfg!(test) {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(dir) = crate::setup::catalog::gray_home()
            .ok()
            .map(|h| h.join("cache"))
        else {
            return;
        };
        for (name, apply) in [
            (
                "models.dev.json",
                parse_models_dev_json as fn(&serde_json::Value) -> usize,
            ),
            ("litellm.json", parse_litellm_context_json as _),
            ("openrouter.json", parse_openrouter_models_json as _),
        ] {
            if let Ok(s) = std::fs::read(dir.join(name))
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&s)
            {
                apply(&v);
            }
        }
    });
}
