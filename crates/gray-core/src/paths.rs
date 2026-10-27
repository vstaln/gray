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

pub fn gray_home() -> Option<PathBuf> {
    std::env::var_os("GRAY_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|p| p.join(".gray")))
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
        std::fs::write(&path, &bytes).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
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
