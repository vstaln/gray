use super::*;

impl JsonlSessionStore {
    pub async fn create(&self, meta: SessionMeta) -> Result<SessionId> {
        let _guard = self.lock.lock().await;
        let id = meta.id.clone();
        let path = self.session_path(&id)?;

        ensure_private_dir(&self.root_dir)?;

        let header = Header {
            version: SUPPORTED_SESSION_VERSION,
            id: meta.id,
            timestamp: meta.timestamp,
            cwd: meta.cwd,
            model: meta.model,
            origin: meta.origin,
        };

        let json = serde_json::to_string(&header)?;
        let line = format!("{json}\n");

        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        match options.open(&path).await {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(line.as_bytes()).await?;
                file.flush().await?;
                file.sync_all().await?;
                tighten_file_mode(&path);

                if header.origin.is_none() {
                    self.remember(&id, &header.cwd).await;
                }
                Ok(id)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(SessionError::AlreadyExists(id))
            }
            Err(e) => Err(SessionError::Io(e)),
        }
    }

    pub(super) fn scan_entries(
        id: &SessionId,
        content: &str,
        path: &Path,
    ) -> Result<(u64, Option<u64>)> {
        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        if lines.next().is_none() {
            return Err(SessionError::NotFound(id.clone()));
        }
        if !content.ends_with('\n') {
            return Err(SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session has an incomplete tail; repair before appending",
            )));
        }
        let mut max_id: Option<u64> = None;
        let mut last_id: Option<u64> = None;
        for (n, line) in lines.enumerate() {
            let entry = serde_json::from_str::<SessionEntry>(line).map_err(|source| {
                SessionError::Corrupt {
                    path: path.to_path_buf(),

                    line: n + 2,
                    source,
                }
            })?;
            max_id = Some(max_id.map_or(entry.entry_id, |m| m.max(entry.entry_id)));
            last_id = Some(entry.entry_id);
        }
        Ok((max_id.map_or(0, |m| m + 1), last_id))
    }

    pub async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId> {
        self.append_with_usage_and_duration(id, msg, None, None)
            .await
    }

    pub async fn append_with_usage_and_duration(
        &self,
        id: &SessionId,
        msg: &Message,
        usage: Option<gray_core::event::Usage>,
        duration_ms: Option<u64>,
    ) -> Result<SessionEntryId> {
        let _lock_file = self.lock_session_file(id).await?;
        let mut tails = self.lock.lock().await;
        let path = self.session_path(id)?;
        let stamp = |p: &Path| {
            std::fs::metadata(p)
                .ok()
                .map(|m| (m.len(), m.modified().ok()))
        };

        let (next_id, parent_id) = match (tails.get(id.as_str()), stamp(&path)) {
            (Some(&(len, mtime, next, last)), Some(now)) if (len, mtime) == now => (next, last),
            _ => Self::next_ids_for_append(id, &path).await?,
        };

        let entry = SessionEntry {
            compaction_boundary: false,
            entry_id: next_id,
            parent_id,
            timestamp: now_millis(),
            message: msg.clone(),
            usage,
            duration_ms,
        };

        let json = serde_json::to_string(&entry)?;
        let line = format!("{}\n", json);

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SessionError::NotFound(id.clone())
                } else {
                    SessionError::Io(e)
                }
            })?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        tighten_file_mode(&path);
        if let Some((len, mtime)) = stamp(&path) {
            tails.insert(
                id.as_str().to_string(),
                (len, mtime, next_id + 1, Some(next_id)),
            );
        }

        Ok(next_id)
    }

    pub async fn append_compaction_replacement(
        &self,
        id: &SessionId,
        replacement: &[Message],
    ) -> Result<()> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;

        let (mut next_id, mut parent_id) = Self::next_ids_for_append(id, &path).await?;
        let mut carried = match Self::read_session_text(&path).await? {
            Some((text, _)) => carried_meta(&text),
            None => Default::default(),
        };

        let mut out = String::new();

        let mut msgs = Vec::with_capacity(replacement.len() + 1);
        msgs.push((
            true,
            Message::system(
                "compaction boundary: entries before this point are superseded; replay starts after it",
            ),
        ));
        for msg in replacement {
            msgs.push((false, msg.clone()));
        }
        for (boundary, msg) in msgs {
            let (timestamp, usage, duration_ms) = carry_key(&msg)
                .and_then(|k| carried.get_mut(&k)?.pop_front())
                .filter(|_| !boundary)
                .unwrap_or((now_millis(), None, None));
            let entry = SessionEntry {
                compaction_boundary: boundary,
                entry_id: next_id,
                parent_id,
                timestamp,
                message: msg,
                usage,
                duration_ms,
            };
            let json = serde_json::to_string(&entry)?;
            out.push_str(&json);
            out.push('\n');
            parent_id = Some(next_id);
            next_id += 1;
        }

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SessionError::NotFound(id.clone())
                } else {
                    SessionError::Io(e)
                }
            })?;

        file.write_all(out.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        tighten_file_mode(&path);

        Ok(())
    }

    pub(super) fn validate_loaded_graph(
        path: &Path,
        expected_id: &SessionId,
        header_line_num: usize,
        header: &Header,
        entries: &[SessionEntry],
        entry_line_nums: &[usize],
    ) -> Result<()> {
        use serde::de::Error as _;
        if header.version != SUPPORTED_SESSION_VERSION {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "unsupported session version {} (expected {})",
                    header.version, SUPPORTED_SESSION_VERSION
                )),
            });
        }
        if header.id != *expected_id {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "session id mismatch: filename '{}' != header id '{}'",
                    expected_id.as_str(),
                    header.id.as_str()
                )),
            });
        }
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(prev) = seen.insert(entry.entry_id, i) {
                let _ = prev;
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line: entry_line_nums.get(i).copied().unwrap_or(0),
                    source: serde_json::Error::custom(format!(
                        "duplicate entry id {}",
                        entry.entry_id
                    )),
                });
            }
        }
        let index_of = |eid: u64| seen.get(&eid).copied();
        for (i, entry) in entries.iter().enumerate() {
            let Some(parent) = entry.parent_id else {
                continue;
            };
            let line = entry_line_nums.get(i).copied().unwrap_or(0);
            if parent == entry.entry_id {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent cycle: entry {} is its own parent",
                        entry.entry_id
                    )),
                });
            }
            let Some(pi) = index_of(parent) else {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "missing parent {} for entry {}",
                        parent, entry.entry_id
                    )),
                });
            };
            if pi >= i {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent {} of entry {} must precede it (forward reference)",
                        parent, entry.entry_id
                    )),
                });
            }
        }
        Ok(())
    }

    pub async fn maintain(&self, id: &SessionId) -> Result<bool> {
        use std::io::{BufRead, Seek, SeekFrom, Write};
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() < 8 * 1024 * 1024 {
            return Ok(false);
        }
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let header: Header = match serde_json::from_str(&line) {
            Ok(header) => header,
            Err(_) => return Ok(false),
        };
        if header.version != SUPPORTED_SESSION_VERSION || header.id != *id {
            return Ok(false);
        }
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        serde_json::to_writer(&mut replacement, &header)?;
        writeln!(replacement)?;
        let header_end = replacement.stream_position()?;
        let mut previous = None;
        let mut next = 0u64;
        let mut boundary = false;
        loop {
            line.clear();

            let n = std::io::Read::take(&mut reader, 16 * 1024 * 1024 + 1).read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if !line.ends_with('\n') || n > 16 * 1024 * 1024 {
                return Ok(false);
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut entry: SessionEntry = match serde_json::from_str(&line) {
                Ok(entry) => entry,
                Err(_) => return Ok(false),
            };
            if entry.parent_id != previous || previous.is_some_and(|p| entry.entry_id <= p) {
                return Ok(false);
            }
            previous = Some(entry.entry_id);
            if entry.compaction_boundary {
                replacement.as_file().set_len(header_end)?;
                replacement.seek(SeekFrom::Start(header_end))?;
                next = 0;
                boundary = true;
                continue;
            }

            if !boundary {
                continue;
            }
            entry.entry_id = next;
            entry.parent_id = next.checked_sub(1);
            next += 1;
            serde_json::to_writer(&mut replacement, &entry)?;
            writeln!(replacement)?;
        }
        if !boundary {
            return Ok(false);
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(true)
    }

    pub(super) fn archive_original(&self, id: &SessionId, path: &Path) -> Result<()> {
        let archive_dir = self.root_dir.join("archive");
        ensure_private_dir(&archive_dir)?;
        let mut archive = tempfile::Builder::new()
            .prefix(&format!("{}-", id.as_str()))
            .suffix(".jsonl")
            .tempfile_in(&archive_dir)?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut archive)?;
        archive.as_file().sync_all()?;
        let _ = archive.keep().map_err(|e| SessionError::Io(e.error))?;
        Ok(())
    }

    pub async fn rewind(&self, id: &SessionId, keep: usize) -> Result<Vec<SessionEntry>> {
        use std::io::Write;

        let (_meta, active) = self.load(id).await?;
        if keep >= active.len() {
            return Ok(Vec::new());
        }
        let dropped = active[keep..].to_vec();
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let content = std::fs::read_to_string(&path)?;
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let header_line = lines
            .next()
            .ok_or_else(|| SessionError::Io(std::io::Error::other("session file has no header")))?;
        let records: Vec<&str> = lines.collect();
        let superseded = records
            .iter()
            .rposition(|line| {
                serde_json::from_str::<SessionEntry>(line)
                    .map(|entry| entry.compaction_boundary)
                    .unwrap_or(false)
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let keep_records = superseded + keep;
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        writeln!(replacement, "{header_line}")?;
        for record in &records[..keep_records] {
            writeln!(replacement, "{record}")?;
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(dropped)
    }

    pub(super) async fn read_session_text(
        path: &Path,
    ) -> Result<Option<(String, Option<Vec<u8>>)>> {
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SessionError::Io(e)),
        };
        match String::from_utf8(bytes) {
            Ok(text) => Ok(Some((text, None))),
            Err(error) => {
                let valid = error.utf8_error().valid_up_to();
                let rest = &error.as_bytes()[valid..];
                if rest.contains(&b'\n') {
                    return Err(SessionError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "session file contains invalid UTF-8 before the final record",
                    )));
                }
                let bytes = error.into_bytes();

                let prefix = std::str::from_utf8(&bytes[..valid])
                    .expect("prefix bounded by valid_up_to is valid UTF-8")
                    .to_owned();
                Ok(Some((prefix, Some(bytes))))
            }
        }
    }

    pub async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let (content, raw) = match Self::read_session_text(&path).await? {
            Some(both) => both,
            None => return Err(SessionError::NotFound(id.clone())),
        };

        let all_lines: Vec<(usize, &str)> = content
            .lines()
            .enumerate()
            .map(|(idx, line)| (idx + 1, line))
            .filter(|(_, line)| !line.trim().is_empty())
            .collect();

        let (header_line_num, header_str) =
            all_lines.first().map(|&(n, s)| (n, s)).unwrap_or((1, ""));
        let header: Header = match serde_json::from_str(header_str) {
            Ok(h) => h,
            Err(e) => {
                Self::quarantine_corrupt_file(&path).await;
                return Err(SessionError::Corrupt {
                    path: path.clone(),
                    line: header_line_num,
                    source: e,
                });
            }
        };

        let mut damaged_tail = false;
        let entry_lines = &all_lines[1..];
        let parsed: Vec<std::result::Result<SessionEntry, serde_json::Error>> =
            if content.len() > (1 << 21) {
                use rayon::prelude::*;
                entry_lines
                    .par_iter()
                    .map(|(_, line_str)| serde_json::from_str::<SessionEntry>(line_str))
                    .collect()
            } else {
                entry_lines
                    .iter()
                    .map(|(_, line_str)| serde_json::from_str::<SessionEntry>(line_str))
                    .collect()
            };
        let mut entries = Vec::with_capacity(entry_lines.len());
        let mut entry_line_nums = Vec::with_capacity(entry_lines.len());

        for (idx, ((line_num, _), result)) in entry_lines.iter().zip(parsed).enumerate() {
            let is_final_line = idx == entry_lines.len() - 1;
            match result {
                Ok(entry) => {
                    entries.push(entry);
                    entry_line_nums.push(*line_num);
                }
                Err(e) => {
                    if is_final_line {
                        damaged_tail = true;
                        log::warn!(
                            "ignoring corrupt or torn final line in {}:{}: {}",
                            path.display(),
                            line_num,
                            e
                        );
                    } else {
                        return Err(SessionError::Corrupt {
                            path: path.clone(),
                            line: *line_num,
                            source: e,
                        });
                    }
                }
            }
        }

        Self::validate_loaded_graph(
            &path,
            id,
            header_line_num,
            &header,
            &entries,
            &entry_line_nums,
        )?;

        if damaged_tail || !content.ends_with('\n') {
            use std::io::Write;
            let mut backup = tempfile::Builder::new()
                .prefix("session-tail-backup-")
                .tempfile_in(&self.root_dir)?;
            backup.write_all(raw.as_deref().unwrap_or(content.as_bytes()))?;
            let (_, backup_path) = backup.keep().map_err(|e| SessionError::Io(e.error))?;
            let mut repaired = String::new();
            let keep = if damaged_tail {
                all_lines.len() - 1
            } else {
                all_lines.len()
            };
            for (_, line) in all_lines.iter().take(keep) {
                repaired.push_str(line);
                repaired.push('\n');
            }
            let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
            replacement.write_all(repaired.as_bytes())?;
            replacement.as_file().sync_all()?;
            replacement
                .persist(&path)
                .map_err(|e| SessionError::Io(e.error))?;
            log::warn!(
                "repaired session tail; original saved at {}",
                backup_path.display()
            );
        }

        let meta = SessionMeta {
            id: header.id,
            timestamp: header.timestamp,
            cwd: header.cwd.clone(),
            model: header.model,
            origin: header.origin,
        };
        if meta.origin.is_none() {
            self.remember(id, &meta.cwd).await;
        }

        let mut active = entries;
        if let Some(last) = active.iter().rposition(|e| e.compaction_boundary) {
            active.drain(..=last);
        }

        Ok((meta, active))
    }

    pub async fn list(&self) -> Vec<SessionSummary> {
        self.list_matching(None).await
    }

    pub async fn list_in(&self, cwd: &Path) -> Vec<SessionSummary> {
        self.list_matching(Some(cwd.to_path_buf())).await
    }

    pub(super) async fn list_matching(&self, cwd: Option<PathBuf>) -> Vec<SessionSummary> {
        let mut rx = self.scan(cwd, false);
        let mut summaries = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                SessionScanEvent::Summary(s) => summaries.push(s),
                SessionScanEvent::Done => break,
                SessionScanEvent::Running(_) => {}
            }
        }

        summaries.sort_by(|a, b| {
            b.last_message_at
                .cmp(&a.last_message_at)
                .then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        summaries
    }

    pub(crate) fn scan(
        &self,
        cwd: Option<PathBuf>,
        probe_locks: bool,
    ) -> tokio::sync::mpsc::UnboundedReceiver<SessionScanEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let root = self.root_dir.clone();
        tokio::spawn(scan_session_dir(root, cwd, probe_locks, tx));
        rx
    }

    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        let _lock = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;

        if self.open_lock_held(id).await {
            return Err(SessionError::Locked {
                id: id.clone(),
                pid: self.open_holder_pid(id),
            });
        }
        let path = self.session_path(id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(SessionError::Io(e)),
        }

        for ext in ["dismissed", "title", "autotitle", "prefix"] {
            let sidecar = self.root_dir.join(format!("{}.{ext}", id.as_str()));
            let _ = tokio::fs::remove_file(&sidecar).await;
        }
        Ok(())
    }

    pub async fn prune_before(&self, cutoff_ms: u64) -> Result<Vec<SessionId>> {
        let mut removed = Vec::new();
        for summary in self.list().await {
            if summary.started_at < cutoff_ms {
                match self.delete(&summary.id).await {
                    Ok(()) => removed.push(summary.id),

                    Err(SessionError::Locked { .. }) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(removed)
    }
}
