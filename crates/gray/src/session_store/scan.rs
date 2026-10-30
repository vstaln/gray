use super::*;

pub(crate) enum SessionScanEvent {
    Running(std::collections::HashSet<String>),

    Summary(SessionSummary),

    Done,
}

pub(super) enum SessionFileRead {
    Summary(SessionSummary),

    Corrupt(PathBuf),
}

pub(super) fn user_summary_text(message: &Message) -> String {
    let text = message.text_content();
    if !text.is_empty() {
        return text;
    }
    let has = |f: fn(&ContentBlock) -> bool| message.content.iter().any(f);
    if has(|b| matches!(b, ContentBlock::Image { .. })) {
        "[Image]".to_string()
    } else if has(|b| matches!(b, ContentBlock::Media { .. })) {
        "[File]".to_string()
    } else {
        String::new()
    }
}

pub(super) fn summarize_session_file(
    root: &Path,
    path: &Path,
    cwd: Option<&Path>,
) -> Option<SessionFileRead> {
    let slices = read_session_ends(path, cwd)?;
    let stem = path.file_stem().and_then(|s| s.to_str())?;

    let complete_first_line = slices.head.contains('\n');

    let header_str = match slices.head.lines().find(|l| !l.trim().is_empty()) {
        Some(h) => h,
        None => {
            log::warn!("skipping empty session file: {}", path.display());
            return None;
        }
    };

    let header: Header = match serde_json::from_str(header_str) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("skipping corrupt header in {}: {}", path.display(), e);

            if complete_first_line {
                return Some(SessionFileRead::Corrupt(path.to_path_buf()));
            }
            return None;
        }
    };
    if header.id.as_str() != stem {
        log::warn!(
            "skipping session whose header id does not match its filename: {}",
            path.display()
        );
        return None;
    }

    let mut last_message_at = header.timestamp;
    let mut first_user_text = None;
    let mut last_user_text = None;
    let mut last_end = SessionEnd::Done;

    for (in_tail, chunk) in [(false, &slices.head), (true, &slices.tail)] {
        for line in chunk.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(entry) = serde_json::from_str::<SessionEntry>(line) else {
                continue;
            };
            last_message_at = last_message_at.max(entry.timestamp);
            if entry.compaction_boundary {
                first_user_text = None;
                last_user_text = None;
                continue;
            }
            let blocks = &entry.message.content;
            let has_block = |f: fn(&ContentBlock) -> bool| blocks.iter().any(f);
            last_end = match entry.message.role {
                Role::Assistant if has_block(|b| matches!(b, ContentBlock::ToolUse { .. })) => {
                    SessionEnd::ToolUse
                }
                Role::Assistant => SessionEnd::Done,
                Role::User if has_block(|b| matches!(b, ContentBlock::ToolResult { .. })) => {
                    SessionEnd::ToolResult
                }
                Role::User => SessionEnd::UserUnanswered,
                Role::System => last_end,
            };
            if entry.message.role == Role::User && !entry.message.injected {
                let text = user_summary_text(&entry.message);
                if !text.is_empty() {
                    if first_user_text.is_none() && !in_tail {
                        first_user_text = Some(text.clone());
                    }
                    last_user_text = Some(text);
                }
            }
        }
    }

    Some(SessionFileRead::Summary(SessionSummary {
        id: header.id,
        started_at: header.timestamp,
        last_message_at,
        cwd: header.cwd,
        first_user_text,
        last_user_text,
        last_end,
        dismissed_at: if last_end == SessionEnd::Done {
            None
        } else {
            read_dismissal(root, stem)
        },
        title: read_title(root, stem),
        origin: header.origin,
    }))
}

pub(super) async fn scan_session_dir(
    root: PathBuf,
    cwd: Option<PathBuf>,
    probe_locks: bool,
    tx: tokio::sync::mpsc::UnboundedSender<SessionScanEvent>,
) {
    let mut read_dir = match tokio::fs::read_dir(&root).await {
        Ok(rd) => rd,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("failed to read session directory {}: {}", root.display(), e);
            }
            let _ = tx.send(SessionScanEvent::Done);
            return;
        }
    };

    let mut paths = Vec::new();
    let mut open_paths = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        let ext = path.extension().and_then(|ext| ext.to_str());
        if ext == Some("open") {
            if probe_locks
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(valid_session_id)
            {
                open_paths.push(path);
            }
            continue;
        }
        if ext != Some("jsonl") {
            continue;
        }

        if !entry
            .file_type()
            .await
            .map(|t| t.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_session_id(stem) {
            log::warn!("skipping session with invalid id: {}", path.display());
            continue;
        }
        paths.push(path);
    }

    if probe_locks {
        let probe_tx = tx.clone();
        tokio::spawn(async move {
            use futures::StreamExt as _;
            let mut probes = futures::stream::iter(open_paths)
                .map(|path| {
                    tokio::task::spawn_blocking(move || {
                        if !open_lock_held_at(&path) {
                            return None;
                        }
                        path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
                    })
                })
                .buffer_unordered(32);
            let mut held = std::collections::HashSet::new();
            while let Some(joined) = probes.next().await {
                if let Ok(Some(stem)) = joined {
                    held.insert(stem);
                }
            }
            let _ = probe_tx.send(SessionScanEvent::Running(held));
        });
    }

    use futures::StreamExt as _;
    let mut reads = futures::stream::iter(paths)
        .map(|path| {
            let root = root.clone();
            let cwd = cwd.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    summarize_session_file(&root, &path, cwd.as_deref())
                })
                .await
            }
        })
        .buffer_unordered(64);
    while let Some(joined) = reads.next().await {
        let Ok(Some(read)) = joined else {
            continue;
        };
        match read {
            SessionFileRead::Summary(s) => {
                if tx.send(SessionScanEvent::Summary(s)).is_err() {
                    return;
                }
            }
            SessionFileRead::Corrupt(path) => {
                JsonlSessionStore::quarantine_corrupt_file(&path).await;
            }
        }
    }
    let _ = tx.send(SessionScanEvent::Done);
}

pub(super) const SUMMARY_END_BYTES: u64 = 16 * 1024;

pub(super) struct SessionSlices {
    pub(super) head: String,
    pub(super) tail: String,
}

pub(super) fn read_session_ends(path: &Path, cwd: Option<&Path>) -> Option<SessionSlices> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok().or_else(|| {
        log::warn!("failed to open session file {}", path.display());
        None
    })?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    let head_len = SUMMARY_END_BYTES.min(len);
    let mut head_bytes = vec![0u8; head_len as usize];
    file.read_exact(&mut head_bytes).ok()?;
    if let Some(cwd) = cwd
        && let Some(end) = head_bytes.iter().position(|b| *b == b'\n')
        && let Ok(header) = serde_json::from_slice::<Header>(&head_bytes[..end])
        && header.cwd != cwd
    {
        return None;
    }
    if head_len < len && !head_bytes.last().is_some_and(|b| *b == b'\n') {
        std::io::BufRead::read_until(
            &mut std::io::BufReader::new(&mut file),
            b'\n',
            &mut head_bytes,
        )
        .ok()?;
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();

    let head_end = head_bytes.len() as u64;
    let mut back = SUMMARY_END_BYTES.min(len.saturating_sub(head_end));
    let tail = loop {
        let start = len - back;
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail_bytes = Vec::with_capacity(back as usize);
        file.read_to_end(&mut tail_bytes).ok()?;
        if start <= head_end {
            break String::from_utf8_lossy(&tail_bytes).into_owned();
        }
        let body = &tail_bytes[..tail_bytes.len().saturating_sub(1)];
        match body.iter().position(|b| *b == b'\n') {
            Some(at) => break String::from_utf8_lossy(&tail_bytes[at + 1..]).into_owned(),
            None => back = back.saturating_mul(4).min(len - head_end),
        }
    };
    Some(SessionSlices { head, tail })
}
