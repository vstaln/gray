use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    Valid,
    Rejected(u16),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthStyle {
    Bearer,
    Anthropic,
}

pub(crate) fn key_check_request(base_url: &str) -> (String, AuthStyle, bool) {
    let trimmed = base_url.trim().trim_end_matches('/');
    let meta = provider_meta();
    for (frag, url) in &meta.key_check.url_hosts {
        if trimmed.contains(frag.as_str()) {
            return (url.clone(), AuthStyle::Bearer, false);
        }
    }
    let host = reqwest::Url::parse(trimmed)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let style = if meta.key_check.anthropic_hosts.iter().any(|h| h == &host) {
        AuthStyle::Anthropic
    } else {
        AuthStyle::Bearer
    };
    (
        format!("{trimmed}/models"),
        style,
        meta.key_check.reject_400_hosts.iter().any(|h| h == &host),
    )
}

pub(crate) fn classify_key_status(status: u16, reject_400: bool) -> KeyCheck {
    match status {
        200..=299 => KeyCheck::Valid,
        401 | 403 => KeyCheck::Rejected(status),
        400 if reject_400 => KeyCheck::Rejected(status),
        _ => KeyCheck::Unknown,
    }
}

pub(super) async fn key_check_async(
    url: String,
    style: AuthStyle,
    reject_400: bool,
    key: String,
) -> KeyCheck {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return KeyCheck::Unknown;
    };
    let req = match style {
        AuthStyle::Bearer => client.get(&url).bearer_auth(key),
        AuthStyle::Anthropic => client
            .get(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
    };

    match req.send().await {
        Ok(resp) => classify_key_status(resp.status().as_u16(), reject_400),
        Err(_) => KeyCheck::Unknown,
    }
}

pub fn verify_api_key(base_url: &str, key: &str) -> KeyCheck {
    let (url, style, reject_400) = key_check_request(base_url);
    let key = key.to_string();
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(key_check_async(url, style, reject_400, key)),
                Err(_) => KeyCheck::Unknown,
            }
        })
        .join()
        .unwrap_or(KeyCheck::Unknown)
    })
}

pub(super) static OLLAMA_SEEN: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

pub(crate) fn ollama_last_seen() -> Option<bool> {
    *OLLAMA_SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn ollama_running() -> bool {
    let up = probe_ollama();
    *OLLAMA_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(up);
    up
}

pub(super) fn probe_ollama() -> bool {
    std::thread::scope(|s| {
        s.spawn(|| {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(300))
                        .build()
                    else {
                        return false;
                    };
                    client
                        .get("http://localhost:11434/api/tags")
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false)
                }),
                Err(_) => false,
            }
        })
        .join()
        .unwrap_or(false)
    })
}
