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
    assert_eq!(line.overflow_chars, 20_000 - (LINE_BYTE_CAP + 1) as u64);
    assert!(s.next_line().await.unwrap().is_none());
    // Same with a trailing newline: terminator excluded from the overflow count.
    std::fs::write(&path, [vec![b'y'; 20_000], vec![b'\n']].concat()).unwrap();
    let mut s = LineStream::open(&path, "big.txt", token()).await.unwrap();
    let line = s.next_line().await.unwrap().unwrap();
    assert_eq!(line.bytes.len(), LINE_BYTE_CAP + 1);
    assert_eq!(line.overflow_chars, 20_000 - (LINE_BYTE_CAP + 1) as u64);
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
        // Oracle: the whole-file hygiene composition (BOM → sniff → lossy
        // decode → LF normalize), matching the stream's per-line rules.
        let bytes = super::super::hygiene::strip_bom(&data);
        match super::super::hygiene::sniff(bytes, name) {
            Ok(()) => {
                assert_eq!(s.binary_note(), None, "{name}");
                let text = String::from_utf8_lossy(bytes)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                let expected: Vec<&str> = text.lines().collect();
                let mut got = Vec::new();
                let mut n = 0;
                while let Some(line) = s.next_line().await.unwrap() {
                    n += 1;
                    assert_eq!(line.line_no, n, "{name} line_no");
                    assert_eq!(line.overflow_chars, 0, "{name} no fixture overflows");
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
        line.overflow_chars,
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
        super::super::notices::cancelled_note(s.line_no()),
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
        super::super::notices::cancelled_note(s.line_no()),
        "[read: cancelled after 2 lines]"
    );
}

#[test]
fn count_gate_and_skipped_fragment_are_contract_exact() {
    assert!(should_count_exact(COUNT_SKIP_LIMIT_BYTES));
    assert!(!should_count_exact(COUNT_SKIP_LIMIT_BYTES + 1));
    assert_eq!(
        super::super::notices::count_skipped_total(2001, 200 * 1024 * 1024),
        "≥2001 lines (file is 200.0MB, count skipped)"
    );
}

#[test]
fn caps_are_spec_values() {
    assert_eq!(READ_CHUNK_BYTES, 64 * 1024);
    assert_eq!(super::super::window::MAX_LINE_CHARS, 2000);
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
    assert_eq!(line.overflow_chars, 20_000 - (LINE_BYTE_CAP + 1) as u64);
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
        s.count_rest_lines_capped(MAX_COUNT_LINES).await.unwrap();
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
    assert_eq!(
        s.count_rest_lines_capped(MAX_COUNT_LINES).await.unwrap(),
        (2, false)
    );
    // Terminated tail: lines == newlines.
    std::fs::write(&p, b"a\nb\n").unwrap();
    let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
    s.next_line().await.unwrap().unwrap();
    assert_eq!(
        s.count_rest_lines_capped(MAX_COUNT_LINES).await.unwrap(),
        (1, false)
    );
    // Nothing left: zero.
    let mut s = LineStream::open(&p, "u.txt", token()).await.unwrap();
    s.next_line().await.unwrap().unwrap();
    s.next_line().await.unwrap().unwrap();
    assert_eq!(
        s.count_rest_lines_capped(MAX_COUNT_LINES).await.unwrap(),
        (0, false)
    );
}

#[tokio::test]
async fn count_capped_stops_early_and_reports() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("big.txt");
    std::fs::write(&p, "x\n".repeat(200_000)).unwrap();
    let mut s = LineStream::open(&p, "big.txt", token()).await.unwrap();
    let (count, capped) = s.count_rest_lines_capped(100_000).await.unwrap();
    assert!(capped);
    assert!(count <= 100_001);
    // Small stream under the cap: exact, uncapped.
    let p2 = dir.path().join("small.txt");
    std::fs::write(&p2, "x\n".repeat(10)).unwrap();
    let mut s2 = LineStream::open(&p2, "small.txt", token()).await.unwrap();
    let (count2, capped2) = s2.count_rest_lines_capped(100_000).await.unwrap();
    assert!(!capped2);
    assert_eq!(count2, 10);
}
