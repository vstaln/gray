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

const SYNTHETIC_USER_PREFIXES: &[&str] = &[
    crate::composer::transcript::CONTEXT_UPDATE_MARKER,
    "<system-reminder>",
    crate::composer::transcript::SUBAGENT_DONE_MARKER,
    "[Harness note]",
    "The conversation history before this point was compacted",
];

pub(crate) fn real_user_text(message: &Message) -> Option<String> {
    if message.role != Role::User || message.injected {
        return None;
    }
    if message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
    {
        return None;
    }
    let text = user_summary_text(message);
    let head = text.trim_start();
    if head.is_empty() || SYNTHETIC_USER_PREFIXES.iter().any(|p| head.starts_with(p)) {
        return None;
    }
    Some(text)
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
            if let Some(text) = real_user_text(&entry.message) {
                if first_user_text.is_none() && !in_tail {
                    first_user_text = Some(text.clone());
                }
                last_user_text = Some(text);
            }
        }
    }

    if first_user_text.is_none() {
        first_user_text = slices.head_cut_text;
    }
    if slices.tail_cut {
        last_message_at = last_message_at.max(slices.modified_ms);
    }

    if last_end != SessionEnd::Done
        && read_turn_end(root, stem).is_some_and(|at| at >= last_message_at)
    {
        last_end = SessionEnd::Done;
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
    since: Option<std::time::SystemTime>,
    probe_locks: bool,
    tx: tokio::sync::mpsc::UnboundedSender<SessionScanEvent>,
) {
    let listing = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || list_session_files(&root, since, probe_locks)).await
    };
    let (paths, open_paths) = match listing {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("failed to read session directory {}: {}", root.display(), e);
            }
            let _ = tx.send(SessionScanEvent::Done);
            return;
        }
        Err(_) => {
            let _ = tx.send(SessionScanEvent::Done);
            return;
        }
    };
    let cwd = cwd.map(|c| c.canonicalize().unwrap_or(c));

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
        .map(|(_, path)| {
            let root = root.clone();
            let cwd = cwd.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    summarize_session_file(&root, &path, cwd.as_deref())
                })
                .await
            }
        })
        .buffered(64);
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

const SUMMARY_LINE_CAP: u64 = 1 << 20;

type SessionFiles = (Vec<(std::time::SystemTime, PathBuf)>, Vec<PathBuf>);

pub(super) fn list_session_files(
    root: &Path,
    since: Option<std::time::SystemTime>,
    probe_locks: bool,
) -> std::io::Result<SessionFiles> {
    let mut files = Vec::new();
    let mut open_paths = Vec::new();
    for entry in std::fs::read_dir(root)?.flatten() {
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
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_session_id(stem) {
            log::warn!("skipping session with invalid id: {}", path.display());
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if since.is_some_and(|cutoff| modified < cutoff) {
            continue;
        }
        files.push((modified, path));
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    if since.is_some() {
        let listed: std::collections::HashSet<&std::ffi::OsStr> =
            files.iter().filter_map(|(_, p)| p.file_stem()).collect();
        open_paths.retain(|p| p.file_stem().is_some_and(|s| listed.contains(s)));
    }
    Ok((files, open_paths))
}

pub(super) struct SessionSlices {
    pub(super) head: String,
    pub(super) tail: String,
    pub(super) tail_cut: bool,
    pub(super) head_cut_text: Option<String>,
    pub(super) modified_ms: u64,
}

fn cut_line_user_text(line: &[u8]) -> Option<String> {
    const PATTERN: &str = "\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"";
    const KEEP: usize = 2000;
    let prefix = String::from_utf8_lossy(&line[..line.len().min(64 * 1024)]);
    let body = &prefix[prefix.find(PATTERN)? + PATTERN.len() - 1..];
    let mut escaped = false;
    let mut end = body.len();
    for (i, c) in body.char_indices().skip(1) {
        if i > KEEP {
            end = i;
            break;
        }
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = i;
            break;
        }
    }
    let literal = &body[..end];
    let text = (0..6).find_map(|k| {
        let cut = literal.len().checked_sub(k)?;
        let piece = literal.get(..cut)?;
        serde_json::from_str::<String>(&format!("{piece}\"")).ok()
    })?;
    let head = text.trim_start();
    if head.is_empty() || SYNTHETIC_USER_PREFIXES.iter().any(|p| head.starts_with(p)) {
        return None;
    }
    Some(text)
}

pub(super) fn read_session_ends(path: &Path, cwd: Option<&Path>) -> Option<SessionSlices> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok().or_else(|| {
        log::warn!("failed to open session file {}", path.display());
        None
    })?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    if len == 0 {
        return None;
    }

    let mut head_cut_text = None;
    let head_len = SUMMARY_END_BYTES.min(len);
    let mut head_bytes = vec![0u8; head_len as usize];
    file.read_exact(&mut head_bytes).ok()?;
    if let Some(cwd) = cwd
        && let Some(end) = head_bytes.iter().position(|b| *b == b'\n')
        && let Ok(header) = serde_json::from_slice::<Header>(&head_bytes[..end])
        && header.cwd != cwd
        && header.cwd.canonicalize().ok().as_deref() != Some(cwd)
    {
        return None;
    }
    if head_len < len && !head_bytes.last().is_some_and(|b| *b == b'\n') {
        std::io::BufRead::read_until(
            &mut std::io::BufReader::new((&mut file).take(SUMMARY_LINE_CAP)),
            b'\n',
            &mut head_bytes,
        )
        .ok()?;
        if head_bytes.last() != Some(&b'\n')
            && let Some(at) = head_bytes.iter().rposition(|b| *b == b'\n')
        {
            head_cut_text = cut_line_user_text(&head_bytes[at + 1..]);
            head_bytes.truncate(at + 1);
        }
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();

    let head_end = head_bytes.len() as u64;
    let mut back = SUMMARY_END_BYTES.min(len.saturating_sub(head_end));
    let mut tail_cut = false;
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
            None if back >= SUMMARY_LINE_CAP => {
                tail_cut = true;
                break String::new();
            }
            None => back = back.saturating_mul(4).min(len - head_end),
        }
    };
    Some(SessionSlices {
        head,
        tail,
        tail_cut,
        head_cut_text,
        modified_ms,
    })
}
