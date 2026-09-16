//! Context compaction and summarization for Gray conversations.
//!
//! Thin wrappers over `gray_core`'s codex-v2 pipeline ([`Agent::compact_v2`]):
//! manual `/compact` and the REPL auto paths share the same in-band
//! trigger → retention walk → summary-last logic as the in-turn recovery path,
//! plus the gray-specific lifecycle (file-ledger disarm, continuation
//! checkpoint).

use gray_core::agent::Agent;
use gray_core::error::CoreError;
use gray_core::event::Usage;
use gray_core::message::Message;

pub mod policy;

pub use policy::{
    CompactionSettings, compaction_settings_for, estimate_context_tokens, estimate_tokens,
    init_auto_compact_from_env, is_auto_compact_enabled, is_context_overflow_error,
    set_auto_compact_enabled, should_compact,
};

pub async fn auto_compact_if_needed(agent: &mut Agent) -> Result<bool, CoreError> {
    if !is_auto_compact_enabled() {
        return Ok(false);
    }
    let keep = crate::setup::user_keep_recent_tokens();
    Ok(compact_with_keep(agent, None, keep).await?.is_some())
}

/// Manual `/compact` (`custom_instructions` from `/compact <text>`) and REPL
/// auto compaction: one codex-v2 call via [`Agent::compact_v2`] with
/// `keep_tokens` as the retained-history budget (`0` → summary-only). Returns
/// the summary when history was replaced; `None` when nothing was gained
/// (history untouched). Keeps the gray-specific lifecycle: file-ledger dedup
/// entries are disarmed, and the summary is checkpointed for human recovery.
pub async fn compact_with_keep(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
    keep_tokens: usize,
) -> Result<Option<String>, CoreError> {
    let replaced = agent.messages().len();
    if replaced == 0 {
        return Ok(None);
    }
    let Some(summary) = agent
        .compact_v2(custom_instructions, Some(keep_tokens))
        .await?
    else {
        return Ok(None);
    };
    // T3.4 lifecycle: entries stay for the write guard, but no dedup stub
    // may reference a compacted-away result.
    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
        ledger.disarm_all_dedup();
    }
    // Reversible checkpoint: summary on disk, so nothing is truly lost.
    // Best-effort; compaction succeeds even if it fails. Logged, never
    // `eprintln!`: raw stderr writes land on the live composer viewport and
    // collide with the next draw (ghost input) while `Compacting context`
    // owns the status dock.
    let path = write_continuation_checkpoint(&summary, replaced);
    if let Some(p) = path {
        log::info!(target: "gray_compact", "continuation checkpoint: {}", p.display());
    }
    Ok(Some(summary))
}

/// Best-effort snapshot of the compact summary for human recovery. Carries
/// only the summary: the full pre-compact transcript stays durable in the
/// session JSONL (pre-boundary entries) and nothing ever reads this file back
/// into the model, so copying the transcript here only widened secret
/// exposure. Lives under the gray home dir (never /tmp or the workspace) with
/// an unpredictable uuid name, owner-only (0600) on unix; newest 5 kept.
pub fn write_continuation_checkpoint(
    summary: &str,
    replaced_messages: usize,
) -> Option<std::path::PathBuf> {
    let dir = crate::setup::catalog::gray_home()
        .ok()?
        .join("continuation-checkpoints");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "gray-continuation-{}.md",
        uuid::Uuid::new_v4().as_simple()
    ));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let doc = format!(
        "# Gray continuation checkpoint ({ts})\n\nCompacted {replaced_messages} messages.\n\n## Summary\n\n{summary}\n"
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).ok()?;
    use std::io::Write as _;
    file.write_all(doc.as_bytes()).ok()?;
    let _ = file.sync_all();
    rotate_continuation_checkpoints(&dir);
    Some(path)
}

/// Keeps the newest `KEEP` checkpoints; best-effort, failures ignored.
fn rotate_continuation_checkpoints(dir: &std::path::Path) {
    const KEEP: usize = 5;
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)
        .ok()
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("gray-continuation-")
                })
                .filter_map(|e| {
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((mtime, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(mtime, _)| *mtime);
    files.reverse();
    for (_, path) in files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
