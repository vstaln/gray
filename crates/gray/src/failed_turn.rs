use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use gray_core::message::Message;

use crate::print::now_millis;
use crate::session_store::{JsonlSessionStore, SessionId, valid_session_id};

pub(crate) const FRESH_MS: u64 = 60 * 60 * 1000;

const FILE_PREFIX: &str = "gray-failed-turn-";
const KEY_CHARS: usize = 24;
const PROMPT_CAP: usize = 4000;
const KEEP: usize = 8;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct FailedTurn {
    pub session: String,
    pub cwd: PathBuf,
    pub prompt_hash: String,
    pub prompt: String,
    pub failed_at_ms: u64,
    pub exit: i32,
    pub code: String,
}

fn disabled() -> bool {
    std::env::var("GRAY_NO_RESUME").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn prompt_text(prompt: Option<&str>, message: &Message) -> String {
    match prompt {
        Some(p) => p.to_string(),
        None => message.text_content(),
    }
}

pub(crate) fn prompt_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn key(cwd: &Path, prompt_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(canonical(cwd).to_string_lossy().as_bytes());
    h.update([0]);
    h.update(prompt_hash.as_bytes());
    let hexed = format!("{:x}", h.finalize());
    hexed[..KEY_CHARS].to_string()
}

fn checkpoint_path(dir: &Path, cwd: &Path, prompt_hash: &str) -> PathBuf {
    dir.join(format!("{FILE_PREFIX}{}.json", key(cwd, prompt_hash)))
}

fn checkpoints_dir() -> Option<PathBuf> {
    Some(
        crate::setup::catalog::gray_home()
            .ok()?
            .join("continuation-checkpoints"),
    )
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

fn write_file(dir: &Path, name: &str, body: &str) -> std::io::Result<()> {
    private_dir(dir)?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&tmp)?, body.as_bytes())?;
    std::fs::rename(&tmp, dir.join(name))
}

fn sweep(dir: &Path) {
    let now = now_millis();
    let mut fresh_files = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().starts_with(FILE_PREFIX) {
            continue;
        }
        let path = e.path();
        let expired = std::fs::read_to_string(&path)
            .ok()
            .and_then(|b| serde_json::from_str::<FailedTurn>(&b).ok())
            .map(|cp| now.saturating_sub(cp.failed_at_ms) > FRESH_MS)
            .unwrap_or(false);
        if expired {
            let _ = std::fs::remove_file(&path);
        } else {
            let mtime = e.metadata().and_then(|m| m.modified()).ok();
            fresh_files.push((mtime, path));
        }
    }
    fresh_files.sort_by_key(|(m, _)| *m);
    fresh_files.reverse();
    for (_, path) in fresh_files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) fn record_failure(
    session_id: &SessionId,
    cwd: &Path,
    prompt_text: &str,
    prompt_hash: &str,
    code: &str,
    exit: i32,
) {
    if disabled() {
        return;
    }
    let Some(dir) = checkpoints_dir() else {
        return;
    };
    record_failure_in(&dir, session_id, cwd, prompt_text, prompt_hash, code, exit);
}

pub(crate) fn record_failure_in(
    dir: &Path,
    session_id: &SessionId,
    cwd: &Path,
    prompt_text: &str,
    prompt_hash: &str,
    code: &str,
    exit: i32,
) {
    let cp = FailedTurn {
        session: session_id.as_str().to_string(),
        cwd: canonical(cwd),
        prompt_hash: prompt_hash.to_string(),
        prompt: prompt_text.chars().take(PROMPT_CAP).collect(),
        failed_at_ms: now_millis(),
        exit,
        code: code.to_string(),
    };
    let Ok(body) = serde_json::to_string(&cp) else {
        return;
    };
    let path = checkpoint_path(dir, cwd, prompt_hash);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    if write_file(dir, &name, &body).is_err() {
        return;
    }
    sweep(dir);
}

pub(crate) async fn resume_session(
    store: &JsonlSessionStore,
    cwd: &Path,
    prompt_hash: &str,
) -> Option<SessionId> {
    if disabled() {
        return None;
    }
    let dir = checkpoints_dir()?;
    resume_session_in(&dir, store, cwd, prompt_hash).await
}

pub(crate) async fn resume_session_in(
    dir: &Path,
    store: &JsonlSessionStore,
    cwd: &Path,
    prompt_hash: &str,
) -> Option<SessionId> {
    let path = checkpoint_path(dir, cwd, prompt_hash);
    let cp: FailedTurn = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    if now_millis().saturating_sub(cp.failed_at_ms) > FRESH_MS {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    if cp.prompt_hash != prompt_hash
        || canonical(&cp.cwd) != canonical(cwd)
        || !valid_session_id(&cp.session)
    {
        return None;
    }
    let id = SessionId::new(cp.session);
    store.load(&id).await.ok()?;
    Some(id)
}

pub(crate) fn clear_checkpoint(session_id: &SessionId, cwd: &Path, prompt_hash: &str) {
    let Some(dir) = checkpoints_dir() else {
        return;
    };
    clear_checkpoint_in(&dir, session_id, cwd, prompt_hash);
}

pub(crate) fn clear_checkpoint_in(
    dir: &Path,
    session_id: &SessionId,
    cwd: &Path,
    prompt_hash: &str,
) {
    let path = checkpoint_path(dir, cwd, prompt_hash);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(cp) = serde_json::from_str::<FailedTurn>(&body) else {
        return;
    };
    if cp.session == session_id.as_str() {
        let _ = std::fs::remove_file(&path);
    }
}
