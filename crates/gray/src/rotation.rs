use std::path::Path;

pub(crate) const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) fn rotation_guard(path: &Path) -> Option<std::fs::File> {
    let lock_path = path.with_extension("log.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok()?;
    f.try_lock().ok()?;
    Some(f)
}

#[cfg(unix)]
pub(crate) fn handle_drifted(path: &Path, handle: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(want) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(got) = handle.metadata() else {
        return false;
    };
    (got.dev(), got.ino()) != (want.dev(), want.ino())
}

#[cfg(not(unix))]
pub(crate) fn handle_drifted(_path: &Path, _handle: &std::fs::File) -> bool {
    false
}
