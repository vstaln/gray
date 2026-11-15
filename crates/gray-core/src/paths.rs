use std::path::PathBuf;

pub fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::home_dir()
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

pub fn tilde(path: &std::path::Path, home: &std::path::Path) -> Option<String> {
    let rest = path.strip_prefix(home).ok()?;
    Some(if rest.as_os_str().is_empty() {
        "~".to_string()
    } else {
        format!("~/{}", rest.display())
    })
}

pub fn expand_tilde(raw: &str) -> Option<PathBuf> {
    let home = user_home().filter(|h| !h.as_os_str().is_empty())?;
    if raw == "~" {
        return Some(home);
    }
    let rest = raw.strip_prefix("~/")?;
    Some(home.join(rest))
}

pub fn gray_home() -> Option<PathBuf> {
    std::env::var_os("GRAY_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|p| p.join(".gray")))
}

/// `gray_home`, or `$TMP/.gray` when no home resolves — for callers that
/// must keep working without one. Always absolute: never the cwd.
/// Publish the home Gray resolved, so the processes it spawns agree with it.
///
/// Plugins read `$GRAY_HOME`, and most fall back to `$HOME` — which does not
/// exist on Windows at all (only `USERPROFILE` does). A plugin that resolves
/// neither dies with `cannot resolve Gray home` and exits 1, which the host can
/// only render as a bare exit code: that is exactly how a widget slot turns
/// into `subagents widget unavailable (plugin exited exit code: 1)` on a
/// Windows install started from Explorer or a terminal. A no-op when the user
/// already set it.
pub fn export_gray_home() {
    let Some(home) = publish_home(std::env::var_os("GRAY_HOME"), gray_home()) else {
        return;
    };
    // Edition 2024 makes environment mutation unsafe: another thread could be
    // reading it. This runs before anything is spawned or started.
    unsafe { std::env::set_var("GRAY_HOME", home) };
}

/// The home worth publishing, or `None` when the caller already named one.
fn publish_home(current: Option<std::ffi::OsString>, resolved: Option<PathBuf>) -> Option<PathBuf> {
    let already_set = current.is_some_and(|value| !value.to_string_lossy().trim().is_empty());
    if already_set {
        return None;
    }
    resolved
}

pub fn gray_home_or_temp() -> PathBuf {
    gray_home().unwrap_or_else(|| std::env::temp_dir().join(".gray"))
}

pub fn gray_user_home() -> Option<PathBuf> {
    std::env::var_os("GRAY_USER_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|p| p.join(".gray")))
}

pub fn attachments_dir() -> Option<PathBuf> {
    gray_home().map(|h| h.join("attachments"))
}

pub fn materialize_media(media_type: &str, data: &str) -> Option<PathBuf> {
    use base64::Engine;
    use sha2::Digest;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let digest = sha2::Sha256::digest(&bytes);
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let dir = attachments_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{tag}.{}", media_extension(media_type)));
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt;
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut f) => {
                    if f.write_all(&bytes).is_err() {
                        let _ = std::fs::remove_file(&path);
                        return None;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return None,
            }
        }
        #[cfg(not(unix))]
        std::fs::write(&path, &bytes).ok()?;
    }
    Some(path)
}

fn media_extension(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or_default().trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "application/pdf" => "pdf",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::publish_home;
    use std::path::PathBuf;

    #[test]
    fn a_named_home_is_left_alone() {
        assert_eq!(
            publish_home(
                Some(PathBuf::from(r"D:\scratch\.gray").into_os_string()),
                Some(PathBuf::from(r"C:\Users\me\.gray"))
            ),
            None,
            "an explicit GRAY_HOME is the user's, not ours to overwrite"
        );
    }

    #[test]
    fn an_empty_home_is_replaced_by_the_resolved_one() {
        assert_eq!(
            publish_home(
                Some("   ".into()),
                Some(PathBuf::from(r"C:\Users\me\.gray"))
            ),
            Some(PathBuf::from(r"C:\Users\me\.gray"))
        );
        assert_eq!(
            publish_home(None, Some(PathBuf::from("/home/me/.gray"))),
            Some(PathBuf::from("/home/me/.gray"))
        );
    }

    #[test]
    fn an_unresolvable_home_publishes_nothing() {
        assert_eq!(publish_home(None, None), None);
    }
}
