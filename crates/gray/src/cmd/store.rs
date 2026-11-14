use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::CmdError;
use crate::setup::variants::RowShape;

pub(crate) const PENDING_TTL_SECS: i64 = 3600;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingDoc {
    #[serde(default)]
    pub(crate) kind: String,
    pub(crate) created_at: i64,

    #[serde(default)]
    pub(crate) user: Option<String>,

    #[serde(default)]
    pub(crate) draft: ModelDraft,

    #[serde(default)]
    pub(crate) models: Vec<(String, String)>,

    #[serde(default)]
    pub(crate) shapes: BTreeMap<String, RowShape>,
    #[serde(default)]
    pub(crate) page: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModelDraft {
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    #[serde(default)]
    pub(crate) fast: bool,

    #[serde(default)]
    pub(crate) parts: BTreeMap<String, String>,
}

fn dir(home: &Path) -> PathBuf {
    home.join("cmd_ui")
}

fn doc_path(home: &Path, token: &str) -> PathBuf {
    dir(home).join(format!("{token}.json"))
}

fn write_doc(path: &Path, doc: &PendingDoc) -> std::io::Result<()> {
    let body = serde_json::to_vec(doc).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub(crate) fn put(home: &Path, doc: &PendingDoc) -> std::io::Result<String> {
    let dir = dir(home);
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(&dir)?;
    sweep_expired(&dir);
    let token = uuid::Uuid::new_v4().simple().to_string();
    write_doc(&doc_path(home, &token), doc)?;
    Ok(token)
}

fn sweep_expired(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ttl = std::time::Duration::from_secs(PENDING_TTL_SECS as u64);
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > ttl);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(crate) fn get(home: &Path, token: &str, now: i64) -> Result<PendingDoc, CmdError> {
    if token.is_empty()
        || token
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'-' && b != b'_')
    {
        return Err(CmdError::expired());
    }
    let path = doc_path(home, token);
    let doc: PendingDoc = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(CmdError::expired)?;
    if now.saturating_sub(doc.created_at) > PENDING_TTL_SECS {
        let _ = std::fs::remove_file(&path);
        return Err(CmdError::expired());
    }
    Ok(doc)
}

pub(crate) fn save(home: &Path, token: &str, doc: &PendingDoc) -> std::io::Result<()> {
    write_doc(&doc_path(home, token), doc)
}

pub(crate) fn remove(home: &Path, token: &str) {
    let _ = std::fs::remove_file(doc_path(home, token));
}
