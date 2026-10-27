use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub(crate) struct ExeWatch {
    path: PathBuf,
    dev: u64,
    ino: u64,
    mtime: Option<SystemTime>,
}

#[cfg(unix)]
fn dev_ino(m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (m.dev(), m.ino())
}

#[cfg(not(unix))]
fn dev_ino(_m: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

pub(crate) fn on_disk_exe() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if let Some(live) = exe
        .to_str()
        .and_then(|s| s.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Ok(live);
    }
    Ok(exe)
}

impl ExeWatch {
    pub(crate) fn capture() -> Option<Self> {
        let path = std::env::current_exe().ok()?.canonicalize().ok()?;
        let m = std::fs::metadata(&path).ok()?;
        let (dev, ino) = dev_ino(&m);
        Some(Self {
            path,
            dev,
            ino,
            mtime: m.modified().ok(),
        })
    }

    pub(crate) fn disk_id(&self) -> Option<(u64, u64, Option<SystemTime>)> {
        let m = std::fs::metadata(&self.path).ok()?;
        let (dev, ino) = dev_ino(&m);
        Some((dev, ino, m.modified().ok()))
    }

    pub(crate) fn replaced(&self) -> bool {
        #[cfg(unix)]
        if let Ok(link) = std::fs::read_link("/proc/self/exe")
            && link.to_string_lossy().ends_with(" (deleted)")
        {
            return true;
        }
        match std::fs::metadata(&self.path) {
            Err(_) => true,
            Ok(m) => {
                let (dev, ino) = dev_ino(&m);
                dev != self.dev || ino != self.ino || m.modified().ok() != self.mtime
            }
        }
    }
}
