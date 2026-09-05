//! T2.1 — streaming, memory-capped line reader for the `read` tool.
//!
//! Replaces `tokio::fs::read` (whole file into RAM) with a 64 KiB
//! `BufReader` pull stream: lines before `offset` are counted, never decoded
//! or stored, and no single line ever occupies more than ~8 KiB — longer
//! lines are cut at [`LINE_BYTE_CAP`] and the rest discarded to the next
//! `\n` (counted as `overflow_bytes`). Worst-case resident buffers per
//! stream: ~8 KiB line scratch + 64 KiB reader = ~72 KiB, regardless of file
//! size (a 200 MiB single line streams, it never materializes).
//!
//! Hygiene runs on the stream, mirroring `hygiene::prepare`: BOM stripped
//! from the first line only, one trailing `\r` stripped per line (CRLF).
//! Known gap vs `prepare`: a lone interior `\r` (old-Mac endings) is kept,
//! not treated as a line break — no zoo fixture covers it, and splitting
//! mid-line would break `line_no` accounting. Integrator: revisit if a
//! lone-CR fixture lands.
//!
//! Sniff (T1.4) runs on the first chunk inside [`LineStream::open`] — a
//! `fill_buf` peek, so no byte is consumed — before any decoding (decoding
//! itself is lazy per line via [`RawLine::text`]). A binary hit sets
//! [`LineStream::binary_note`]; the driver returns it as-is
//! (`is_error=false`).
//!
//! [`LineStream::next_line`] is the whole API (pull, not `futures::Stream`:
//! same semantics, no `Pin`/boxing machinery; a `Stream` adapter is five
//! lines if anyone needs combinators).
//!
//! Line-boundary invariant: between `next_line` calls the reader always sits
//! on a line boundary — overflow is drained to the terminating `\n` before
//! yielding. The only exits that abandon position are cancel and the binary
//! hit, which both mark the stream done, clear the scratch, and leave the
//! reader to drop with the stream (never a half-read buffer handed on).
//!
//! Total counting: after the window is filled the driver calls
//! [`LineStream::count_rest_lines`] (exact remaining lines: newlines plus one
//! for a trailing unterminated line; `fill_buf`/`consume` pass, no per-line
//! buffering). Files over [`COUNT_SKIP_LIMIT_BYTES`] skip the count: report
//! [`count_skipped_total`] and still provide `next_offset`. The legacy
//! [`LineStream::count_rest`] (newline count only) stays for its unit tests.
//!
//! T2.2 deferred cut: the driver reads one line past a filled window before
//! claiming more remains (empty → complete; non-empty → the cut names that
//! line), and one line past the pre-offset skip before claiming past-EOF.
//! So a cut/resume offset always names an observed line.
//!
//! Raw-byte hashing: every byte flows through the inner [`HashRead`] exactly
//! once, so [`LineStream::content_hash`] is byte-identical to
//! `FileLedger::hash_bytes` over `fs::read` output once fully drained
//! (`None` past [`crate::ledger::MAX_HASH_BYTES`]).
//!
//! Cancellation: `cancel` is checked at open, on every `next_line` entry,
//! on every backlog chunk, and inside the drain. A hit marks the stream
//! done and yields `None`; the driver renders [`cancelled_note`].
//!
//! Notice strings live in `notices.rs` (moved verbatim at the wave gate);
//! [`cancelled_note`]/[`count_skipped_total`] below delegate there.
//! `MAX_LINE_CHARS` is re-exported from `window.rs` (canonical home, same
//! spec-fixed value 2000 — do not drift).
//!
//! Driver contract (`read/mod.rs::execute_streamed`, which replaced the
//! `tokio::fs::read` → `prepare` → `text.lines()` chain in T2.2):
//!
//! 1. `LineStream::open` (open error → the `read failed …` path).
//! 2. `binary_note()` → return as-is; `file_size() == 0` → the empty note.
//! 3. Skip to `offset` with `next_line` (count, don't store), plus the
//!    deferred skip peek; tail mode rings with `tail::drain_tail`.
//! 4. Collect the window, plus the deferred window peek; total via
//!    `count_rest_lines()` (exact) or the skipped fragment for huge files.
//! 5. `window::window` renders; a loop ending with `cancelled()` renders
//!    `cancelled_note(line_no())`.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

use super::hygiene;

/// Reader chunk size: one `fill_buf` window.
pub const READ_CHUNK_BYTES: usize = 64 * 1024;

/// Spec-fixed line ceiling in chars (canonical home is `window.rs`;
/// re-exported here so existing callers/tests keep working).
pub const MAX_LINE_CHARS: usize = super::window::MAX_LINE_CHARS;

/// No buffered line prefix ever exceeds this: `MAX_LINE_CHARS * 4`
/// (worst-case UTF-8 expansion). Past it, the stream stops buffering and
/// discards to the next `\n`, counting [`RawLine::overflow_bytes`].
pub const LINE_BYTE_CAP: usize = MAX_LINE_CHARS * 4;

/// Files at or under this size get an exact total via [`LineStream::count_rest`];
/// larger files skip the count ([`count_skipped_total`]) but keep `next_offset`.
pub const COUNT_SKIP_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// `[read: cancelled after <n> lines]` — delegates to `notices.rs`.
pub fn cancelled_note(lines_read: usize) -> String {
    super::notices::cancelled_note(lines_read)
}

/// `≥<min> lines (file is <size>, count skipped)` fragment for the total
/// when the file exceeds [`COUNT_SKIP_LIMIT_BYTES`] — delegates to `notices.rs`.
pub fn count_skipped_total(min_total: usize, file_size: u64) -> String {
    super::notices::count_skipped_total(min_total, file_size)
}

/// True when the file is small enough for an exact total count.
pub fn should_count_exact(file_size: u64) -> bool {
    file_size <= COUNT_SKIP_LIMIT_BYTES
}

/// Codepoints in `bytes` via the leading-byte count — exact for valid UTF-8
/// (every codepoint has exactly one byte with the top bits != `10`). Used
/// only for the clamp marker on over-long lines, where the full text never
/// materializes; splitting the count across chunks stays exact because a
/// split codepoint's leading byte is counted exactly once.
fn count_chars(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count() as u64
}

/// `AsyncRead` wrapper that feeds every raw byte through a hasher exactly
/// once (reads happen once per byte no matter how `fill_buf`/`consume`
/// slice the buffer). `hash == None` past [`crate::ledger::MAX_HASH_BYTES`].
struct HashRead<R> {
    inner: R,
    hash: Option<DefaultHasher>,
}

impl<R: AsyncRead + Unpin> AsyncRead for HashRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let n = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if let Some(h) = self.hash.as_mut() {
                    h.write(&buf.filled()[n..]);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// One hygiene-cleaned line. `bytes` never holds more than
/// [`LINE_BYTE_CAP`] + 1 (cap plus the kept `\n` before stripping… in
/// practice ≤ cap after terminator/CRLF/BOM strips); the remainder of an
/// over-long line is counted in `overflow_bytes`/`overflow_chars`, never stored.
pub struct RawLine {
    /// Absolute 1-based file line number.
    pub line_no: usize,
    /// Line content: terminator, trailing `\r`, and (first line) BOM removed.
    /// Lossy-decoded on demand via [`RawLine::text`].
    pub bytes: Vec<u8>,
    /// Discarded content bytes of this line past [`LINE_BYTE_CAP`]
    /// (terminating `\n` excluded).
    pub overflow_bytes: u64,
    /// Codepoints in the discarded bytes ([`count_chars`]).
    /// `text().chars().count() + overflow_chars` is the line's full char
    /// count (exact for valid UTF-8), so the clamp marker stays exact.
    pub overflow_chars: u64,
    /// False for a final line with no trailing newline (and for an
    /// overflow line cut by EOF rather than `\n`).
    pub had_newline: bool,
}

impl RawLine {
    /// Lossy-decode this line (mirrors `hygiene::prepare`'s decode step).
    /// Lines skipped before `offset` never call this: counted, not decoded.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }
}

/// Bounded-memory forward line reader over a regular file.
pub struct LineStream {
    reader: BufReader<HashRead<File>>,
    display: String,
    cancel: CancellationToken,
    line_no: usize,
    file_size: u64,
    scratch: Vec<u8>,
    binary_note: Option<String>,
    cancelled: bool,
    done: bool,
}

impl LineStream {
    /// Open + peek the first chunk for the T1.4 sniff before any decoding.
    /// The peek is `fill_buf` (buffered, unconsumed): no byte is lost.
    /// Open errors are plain `io::Error` for today's `read failed …` path.
    pub async fn open(
        path: &Path,
        display: &str,
        cancel: CancellationToken,
    ) -> std::io::Result<Self> {
        let file = File::open(path).await?;
        let file_size = file.metadata().await?.len();
        // Hash gate mirrors FileLedger::hash_bytes (whole file or None).
        let hash = (file_size <= crate::ledger::MAX_HASH_BYTES).then(DefaultHasher::new);
        let mut reader = BufReader::with_capacity(READ_CHUNK_BYTES, HashRead { inner: file, hash });
        let binary_note = {
            let chunk = reader.fill_buf().await?;
            let sample = &chunk[..chunk.len().min(hygiene::SNIFF_SAMPLE_BYTES)];
            hygiene::sniff(sample, display).err()
        };
        let done = binary_note.is_some();
        Ok(Self {
            reader,
            display: display.to_string(),
            cancel,
            line_no: 0,
            file_size,
            scratch: Vec::new(),
            binary_note,
            cancelled: false,
            done,
        })
    }

    /// Next line, or `None` at EOF / after cancel / on a binary hit.
    /// Check [`LineStream::cancelled`] and [`LineStream::binary_note`] when
    /// the stream ends early to tell the three apart.
    pub async fn next_line(&mut self) -> std::io::Result<Option<RawLine>> {
        if self.done {
            return Ok(None);
        }
        if self.cancel.is_cancelled() {
            self.abandon();
            return Ok(None);
        }
        self.scratch.clear();
        let mut overflow_bytes: u64 = 0;
        let mut overflow_chars: u64 = 0;
        let mut had_newline = false;
        loop {
            if self.cancel.is_cancelled() {
                self.abandon();
                return Ok(None);
            }
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                break; // EOF
            }
            let room = (LINE_BYTE_CAP + 1).saturating_sub(self.scratch.len());
            if room == 0 {
                // Past the cap: buffer nothing, scan for the terminator.
                match chunk.iter().position(|&b| b == b'\n') {
                    Some(p) => {
                        overflow_bytes += p as u64;
                        overflow_chars += count_chars(&chunk[..p]);
                        self.reader.consume(p + 1);
                        had_newline = true;
                        break;
                    }
                    None => {
                        overflow_bytes += chunk.len() as u64;
                        overflow_chars += count_chars(chunk);
                        let len = chunk.len();
                        self.reader.consume(len);
                    }
                }
            } else {
                let n = chunk.len().min(room);
                match chunk[..n].iter().position(|&b| b == b'\n') {
                    Some(p) => {
                        self.scratch.extend_from_slice(&chunk[..p + 1]);
                        self.reader.consume(p + 1);
                        had_newline = true;
                        break;
                    }
                    None => {
                        self.scratch.extend_from_slice(&chunk[..n]);
                        self.reader.consume(n);
                    }
                }
            }
        }
        if self.scratch.is_empty() && overflow_bytes == 0 && !had_newline {
            // EOF exactly on a line boundary: nothing left.
            self.done = true;
            return Ok(None);
        }
        // Drop a kept terminator (overflow lines never hold one in-scratch,
        // so gate on the byte itself, not had_newline — and never pop content).
        if self.scratch.ends_with(b"\n") {
            let len = self.scratch.len();
            self.scratch.truncate(len - 1);
        }
        if self.scratch.ends_with(b"\r") {
            let len = self.scratch.len();
            self.scratch.truncate(len - 1); // CRLF: one trailing \r per line
        }
        if self.line_no == 0 {
            // BOM: first line only (mirrors hygiene::strip_bom).
            let bom = self.scratch.len() - hygiene::strip_bom(&self.scratch).len();
            if bom > 0 {
                self.scratch.drain(..bom);
            }
        }
        self.line_no += 1;
        let bytes = std::mem::take(&mut self.scratch);
        Ok(Some(RawLine {
            line_no: self.line_no,
            bytes,
            overflow_bytes,
            overflow_chars,
            had_newline,
        }))
    }

    /// Drain of the rest of the file (no per-line buffering): newline count,
    /// whether any byte was seen, and whether the tail ends with `\n`.
    /// Bytes flow through the hasher, so the full-file hash stays complete.
    async fn drain_rest(&mut self) -> std::io::Result<(u64, bool, bool)> {
        let mut newlines: u64 = 0;
        let mut saw_any = false;
        let mut ended_newline = true;
        loop {
            if self.cancel.is_cancelled() {
                self.abandon();
                break;
            }
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                self.done = true;
                break;
            }
            saw_any = true;
            newlines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
            ended_newline = chunk.last() == Some(&b'\n');
            let len = chunk.len();
            self.reader.consume(len);
        }
        Ok((newlines, saw_any, ended_newline))
    }

    /// Newline-count-only drain of the rest of the file (no per-line
    /// buffering). Call at a line boundary (the invariant `next_line`
    /// maintains); the caller adds one if the last line was unterminated.
    pub async fn count_rest(&mut self) -> std::io::Result<u64> {
        Ok(self.drain_rest().await?.0)
    }

    /// Exact remaining line count after the last yielded line: newlines plus
    /// one for a trailing unterminated line. Call at a line boundary; the
    /// total is then `line_no() + count_rest_lines()`.
    pub async fn count_rest_lines(&mut self) -> std::io::Result<u64> {
        let (newlines, saw_any, ended_newline) = self.drain_rest().await?;
        Ok(newlines + u64::from(saw_any && !ended_newline))
    }

    /// Incremental raw-byte hash for the T3.2 write guard (rule 5): `Some`
    /// only when the driver consumed the whole file (EOF / `count_rest*`
    /// drained) and the file fit [`crate::ledger::MAX_HASH_BYTES`].
    /// Byte-identical to `FileLedger::hash_bytes` over `fs::read` output.
    pub fn content_hash(&mut self) -> Option<u64> {
        self.reader.get_mut().hash.take().map(|h| h.finish())
    }

    /// Mark done and drop any half-read state (cancel path): the reader is
    /// abandoned at whatever position, never handed on mid-line.
    fn abandon(&mut self) {
        self.cancelled = true;
        self.done = true;
        self.scratch.clear();
    }

    /// Lines yielded so far (== last yielded `line_no`; 0 before the first).
    pub fn line_no(&self) -> usize {
        self.line_no
    }

    /// `stat` size captured at open (drives [`should_count_exact`]).
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// `Some(note)` when the first-chunk sniff said binary — return as-is,
    /// `is_error=false`. Set at open; `next_line` then yields `None`.
    pub fn binary_note(&self) -> Option<&str> {
        self.binary_note.as_deref()
    }

    /// True once a cancel check has fired (stream is done).
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Display path used for sniff notes (kept for driver messages).
    pub fn display(&self) -> &str {
        &self.display
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> CancellationToken {
        CancellationToken::new()
    }

    #[tokio::test]
    async fn bom_stripped_first_line_only_and_cr_stripped_per_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("t.txt");
        std::fs::write(&path, b"\xEF\xBB\xBFalpha\r\nbeta\r\n\xEF\xBB\xBFgamma\n").unwrap();
        let mut s = LineStream::open(&path, "t.txt", token()).await.unwrap();
        let l1 = s.next_line().await.unwrap().unwrap();
        assert_eq!(l1.line_no, 1);
        assert_eq!(l1.text(), "alpha");
        let l2 = s.next_line().await.unwrap().unwrap();
        assert_eq!(l2.text(), "beta");
        // BOM bytes on line 3 survive (only the first line is stripped).
        let l3 = s.next_line().await.unwrap().unwrap();
        assert!(l3.text().starts_with('\u{FEFF}'), "{}", l3.text());
        assert!(s.next_line().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn overlong_line_buffers_cap_only_and_counts_overflow() {
        let dir = tempfile::TempDir::new().unwrap();
        // 20,000 bytes, no trailing newline: buffered prefix + exact discard count.
        let path = dir.path().join("big.txt");
        std::fs::write(&path, vec![b'x'; 20_000]).unwrap();
        let mut s = LineStream::open(&path, "big.txt", token()).await.unwrap();
        let line = s.next_line().await.unwrap().unwrap();
        assert_eq!(line.line_no, 1);
        assert_eq!(line.bytes.len(), LINE_BYTE_CAP + 1);
        assert_eq!(line.overflow_bytes, 20_000 - (LINE_BYTE_CAP + 1) as u64);
        assert!(!line.had_newline);
        assert!(s.next_line().await.unwrap().is_none());
        // Same with a trailing newline: terminator excluded from the overflow count.
        std::fs::write(&path, [vec![b'y'; 20_000], vec![b'\n']].concat()).unwrap();
        let mut s = LineStream::open(&path, "big.txt", token()).await.unwrap();
        let line = s.next_line().await.unwrap().unwrap();
        assert_eq!(line.bytes.len(), LINE_BYTE_CAP + 1);
        assert_eq!(line.overflow_bytes, 20_000 - (LINE_BYTE_CAP + 1) as u64);
        assert!(line.had_newline);
        assert!(s.next_line().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn zoo_parity_with_whole_file_read_on_every_small_fixture() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::testkit::write_fixtures(dir.path(), false).unwrap();
        for name in [
            "long.txt",
            "lockfile.txt",
            "minified.js",
            "wide.log",
            "empty.txt",
            "crlf.txt",
            "bom.txt",
            "emoji.txt",
            "fake.png",
            "real.png",
            "nul.bin",
            "Screenshot 3.04\u{202F}PM.png",
            "cafe\u{301}.txt",
            "caf\u{e9}.txt",
            "AGENTS.md",
        ] {
            let path = dir.path().join(name);
            let data = std::fs::read(&path).unwrap();
            let mut s = LineStream::open(&path, name, token()).await.unwrap();
            match super::super::hygiene::prepare(&data, name) {
                Ok(text) => {
                    assert_eq!(s.binary_note(), None, "{name}");
                    let expected: Vec<&str> = text.lines().collect();
                    let mut got = Vec::new();
                    let mut n = 0;
                    while let Some(line) = s.next_line().await.unwrap() {
                        n += 1;
                        assert_eq!(line.line_no, n, "{name} line_no");
                        assert_eq!(line.overflow_bytes, 0, "{name} no fixture overflows");
                        got.push(line.text().into_owned());
                    }
                    assert_eq!(got, expected, "{name} byte-for-byte parity");
                    if data.is_empty() {
                        assert_eq!(s.file_size(), 0, "{name}");
                    }
                }
                Err(note) => {
                    assert_eq!(s.binary_note(), Some(note.as_str()), "{name}");
                }
            }
        }
    }

    #[tokio::test]
    async fn deep_offset_counts_without_storing() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::testkit::write_fixtures(dir.path(), false).unwrap();
        let path = dir.path().join("lockfile.txt");
        let mut s = LineStream::open(&path, "lockfile.txt", token())
            .await
            .unwrap();
        // Skip 79,989 lines the way the driver will for offset=79990.
        for _ in 0..79_989 {
            s.next_line().await.unwrap().unwrap();
        }
        let mut rest = Vec::new();
        while let Some(line) = s.next_line().await.unwrap() {
            rest.push((line.line_no, line.text().into_owned()));
        }
        assert_eq!(rest.len(), 11);
        assert_eq!(rest[0].0, 79_990);
        assert_eq!(rest[0].1, "lock entry 079990 sha=abcdef");
        assert_eq!(rest[10].0, 80_000);
    }

    #[tokio::test]
    async fn count_rest_counts_newlines_only() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::testkit::write_fixtures(dir.path(), false).unwrap();
        let path = dir.path().join("crlf.txt");
        let mut s = LineStream::open(&path, "crlf.txt", token()).await.unwrap();
        s.next_line().await.unwrap().unwrap();
        assert_eq!(s.count_rest().await.unwrap(), 2);
        // Unterminated tail: newlines undercount lines by one; had_newline flags it.
        let p2 = dir.path().join("u.txt");
        std::fs::write(&p2, b"a\nb").unwrap();
        let mut s = LineStream::open(&p2, "u.txt", token()).await.unwrap();
        let l1 = s.next_line().await.unwrap().unwrap();
        assert!(l1.had_newline);
        let l2 = s.next_line().await.unwrap().unwrap();
        assert!(!l2.had_newline);
        assert_eq!(s.count_rest().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn big_lines_stream_with_tiny_buffers() {
        if !super::super::testkit::big_enabled() {
            return;
        }
        // The zoo sparse.txt is 200 MiB of NULs: post-T1.4 it sniffs as
        // binary at open (one 8 KiB peek, ~72 KiB resident) — the spec's
        // "clamp marker" premise predates the sniff. Document that here…
        let dir = tempfile::TempDir::new().unwrap();
        let sparse = dir.path().join("sparse.txt");
        super::super::testkit::make_sparse(&sparse).unwrap();
        let mut s = LineStream::open(&sparse, "sparse.txt", token())
            .await
            .unwrap();
        let note = s.binary_note().unwrap().to_string();
        assert!(note.contains("NUL"), "{note}");
        assert!(s.next_line().await.unwrap().is_none());
        // …and exercise the overflow path at scale with a 5 MiB text line.
        let path = dir.path().join("wide1.txt");
        std::fs::write(&path, vec![b'x'; 5 * 1024 * 1024]).unwrap();
        let mut s = LineStream::open(&path, "wide1.txt", token()).await.unwrap();
        assert_eq!(s.binary_note(), None);
        let line = s.next_line().await.unwrap().unwrap();
        assert_eq!(line.line_no, 1);
        assert_eq!(line.bytes.len(), LINE_BYTE_CAP + 1); // ~8 KiB, never 5 MiB
        assert_eq!(
            line.overflow_bytes,
            5 * 1024 * 1024 - (LINE_BYTE_CAP + 1) as u64
        );
        assert!(s.next_line().await.unwrap().is_none());
        #[cfg(target_os = "linux")]
        {
            // Peak RSS stays < 50 MiB: the long line never materializes.
            let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
            let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
            let rss = pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
            assert!(rss < 50 * 1024 * 1024, "RSS {rss} bytes");
        }
    }

    #[tokio::test]
    async fn pre_cancelled_token_yields_none_and_flag() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        let cancel = token();
        cancel.cancel();
        let mut s = LineStream::open(&path, "a.txt", cancel).await.unwrap();
        assert!(s.next_line().await.unwrap().is_none());
        assert!(s.cancelled());
        assert_eq!(s.line_no(), 0);
        assert_eq!(
            cancelled_note(s.line_no()),
            "[read: cancelled after 0 lines]"
        );
        // Mid-stream cancel reports lines already yielded.
        let cancel = token();
        let mut s = LineStream::open(&path, "a.txt", cancel.clone())
            .await
            .unwrap();
        s.next_line().await.unwrap().unwrap();
        s.next_line().await.unwrap().unwrap();
        cancel.cancel();
        assert!(s.next_line().await.unwrap().is_none());
        assert!(s.cancelled());
        assert_eq!(
            cancelled_note(s.line_no()),
            "[read: cancelled after 2 lines]"
        );
    }

    #[test]
    fn count_gate_and_skipped_fragment_are_contract_exact() {
        assert!(should_count_exact(COUNT_SKIP_LIMIT_BYTES));
        assert!(!should_count_exact(COUNT_SKIP_LIMIT_BYTES + 1));
        assert_eq!(
            count_skipped_total(2001, 200 * 1024 * 1024),
            "≥2001 lines (file is 200.0MB, count skipped)"
        );
    }

    #[test]
    fn caps_are_spec_values() {
        assert_eq!(READ_CHUNK_BYTES, 64 * 1024);
        assert_eq!(MAX_LINE_CHARS, 2000);
        assert_eq!(LINE_BYTE_CAP, 8000);
        assert_eq!(COUNT_SKIP_LIMIT_BYTES, 64 * 1024 * 1024);
    }

    #[tokio::test]
    async fn overflow_chars_count_codepoints_not_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        // ASCII: chars == bytes.
        let path = dir.path().join("a.txt");
        std::fs::write(&path, vec![b'x'; 20_000]).unwrap();
        let mut s = LineStream::open(&path, "a.txt", token()).await.unwrap();
        let line = s.next_line().await.unwrap().unwrap();
        assert_eq!(line.overflow_bytes, 20_000 - (LINE_BYTE_CAP + 1) as u64);
        assert_eq!(line.overflow_chars, line.overflow_bytes);
        // Emoji (4 bytes each): 5000 chars over 20,000 bytes.
        let path = dir.path().join("e.txt");
        std::fs::write(&path, "😀".repeat(5000)).unwrap();
        let mut s = LineStream::open(&path, "e.txt", token()).await.unwrap();
        let line = s.next_line().await.unwrap().unwrap();
        let buffered_chars = line.text().chars().count() as u64;
        assert_eq!(
            buffered_chars + line.overflow_chars,
            5000,
            "prefix chars + discard chars = full count"
        );
    }

    #[tokio::test]
    async fn content_hash_matches_whole_file_read() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::testkit::write_fixtures(dir.path(), false).unwrap();
        for name in ["long.txt", "crlf.txt", "bom.txt", "minified.js"] {
            let path = dir.path().join(name);
            let raw = std::fs::read(&path).unwrap();
            let mut s = LineStream::open(&path, name, token()).await.unwrap();
            // Drain the way the driver does (some lines + the count pass).
            s.next_line().await.unwrap();
            s.count_rest_lines().await.unwrap();
            assert_eq!(
                s.content_hash(),
                crate::ledger::FileLedger::hash_bytes(&raw),
                "{name}"
            );
        }
        // Empty file hashes like hash_bytes(b"").
        let path = dir.path().join("empty.txt");
        let mut s = LineStream::open(&path, "empty.txt", token()).await.unwrap();
        assert_eq!(s.content_hash(), crate::ledger::FileLedger::hash_bytes(b""));
    }

    #[tokio::test]
    async fn count_rest_lines_adds_one_for_unterminated_tail() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("u.txt");
        std::fs::write(&p, b"a\nb\nc").unwrap();
        let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
        s.next_line().await.unwrap().unwrap();
        assert_eq!(s.count_rest().await.unwrap(), 1);
        let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
        s.next_line().await.unwrap().unwrap();
        assert_eq!(s.count_rest_lines().await.unwrap(), 2);
        // Terminated tail: lines == newlines.
        std::fs::write(&p, b"a\nb\n").unwrap();
        let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
        s.next_line().await.unwrap().unwrap();
        assert_eq!(s.count_rest_lines().await.unwrap(), 1);
        // Nothing left: zero.
        let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
        s.next_line().await.unwrap().unwrap();
        let l2 = s.next_line().await.unwrap().unwrap();
        assert!(l2.had_newline);
        assert_eq!(s.count_rest_lines().await.unwrap(), 0);
    }
}
