use std::path::Path;

pub(crate) const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) fn rotation_guard(path: &Path) -> Option<std::fs::File> {
    lock_rotation(path)
}

fn lock_rotation(path: &Path) -> Option<std::fs::File> {
    let lock_path = path.with_extension("log.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok()?;
    let _ = f.try_lock();
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

pub(crate) fn rotate_if_needed(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= LOG_MAX_BYTES {
        return;
    }

    let _guard = lock_rotation(path);
    let _ = std::fs::remove_file(path.with_extension("log.2"));
    let _ = std::fs::rename(path.with_extension("log.1"), path.with_extension("log.2"));
    let _ = std::fs::rename(path, path.with_extension("log.1"));
    let _ = std::fs::File::create(path);
}
