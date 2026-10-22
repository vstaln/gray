use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
pub struct SearchArgs {
    pub pattern: String,

    pub path: Option<PathBuf>,
    pub limit: Option<usize>,

    pub glob: Option<String>,
    pub ignore_case: bool,
    pub literal: bool,
    pub context: Option<usize>,
}


pub enum IndexBackend {
    #[cfg(unix)]
    Fs(std::sync::Arc<crate::search_index_fs::FsearchPool>),
    Off,
}

pub(crate) fn global_index() -> IndexBackend {
    static BACKEND: std::sync::OnceLock<IndexBackend> = std::sync::OnceLock::new();
    BACKEND
        .get_or_init(
            || match backend_kind(std::env::var("GRAY_SEARCH_INDEX").ok().as_deref()) {
                BackendKind::Off => IndexBackend::Off,
                #[cfg(unix)]
                BackendKind::Fs => {
                    IndexBackend::Fs(crate::search_index_fs::global_fs_pool().clone())
                }
            },
        )
        .clone()
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BackendKind {
    #[cfg(unix)]
    Fs,
    Off,
}

pub(crate) fn backend_kind(v: Option<&str>) -> BackendKind {
    match v {
        Some("off") | Some("none") => BackendKind::Off,
        #[cfg(unix)]
        _ => BackendKind::Fs,
        #[cfg(not(unix))]
        _ => BackendKind::Off,
    }
}

impl Clone for IndexBackend {
    fn clone(&self) -> Self {
        match self {
            #[cfg(unix)]
            Self::Fs(p) => Self::Fs(p.clone()),
            Self::Off => Self::Off,
        }
    }
}

#[cfg(unix)]
pub(crate) async fn spawn_index<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.ok().flatten()
}
