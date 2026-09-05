//! T0.1 — hostile-file fixture zoo + golden-output harness.
//!
//! Self-contained until T1.1 wires `src/read/testkit.rs` into the crate and
//! dedups the builders (that module uses `crate::` paths, this file `gray_tools::`).

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gray_core::agent::{ToolContext, ToolOutput};
use gray_tools::{ReadTool, Tool};
use serde_json::Value;
use tempfile::TempDir;

const SPARSE_BYTES: u64 = 200 * 1024 * 1024;

fn big_enabled() -> bool {
    std::env::var("GRAY_ZOO_BIG").as_deref() == Ok("1")
}

struct Zoo {
    dir: TempDir,
}

impl Zoo {
    fn build() -> std::io::Result<Self> {
        let dir = TempDir::new()?;
        write_fixtures(dir.path(), big_enabled())?;
        Ok(Self { dir })
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    async fn read(&self, path: &str, offset: Option<u64>, limit: Option<u64>) -> ToolOutput {
        let mut map = serde_json::Map::new();
        map.insert("path".to_string(), Value::String(path.to_string()));
        if let Some(o) = offset {
            map.insert("offset".to_string(), Value::from(o));
        }
        if let Some(l) = limit {
            map.insert("limit".to_string(), Value::from(l));
        }
        let ctx = ToolContext {
            cwd: self.root(),
            ..ToolContext::default()
        };
        ReadTool::default().execute(&ctx, Value::Object(map)).await
    }
}

fn write_fixtures(root: &Path, big: bool) -> std::io::Result<()> {
    let long: Vec<String> = (1..=3000).map(|i| format!("line {i:04}")).collect();
    std::fs::write(root.join("long.txt"), long.join("\n") + "\n")?;

    let lock: Vec<String> = (1..=80_000)
        .map(|i| format!("lock entry {i:06} sha=abcdef"))
        .collect();
    std::fs::write(root.join("lockfile.txt"), lock.join("\n") + "\n")?;

    let head = "!function(e){var t={};";
    let first = format!("{head}{}", "x".repeat(3900 - head.len()));
    assert_eq!(first.len(), 3900);
    std::fs::write(
        root.join("minified.js"),
        format!("{first}\n//# sourceMappingURL=app.js.map\nconsole.log(\"ok\");\nconst x = 1;\n"),
    )?;

    let wide: Vec<String> = (1..=500)
        .map(|i| format!("{:<300}", format!("log line {i:04} ")))
        .collect();
    std::fs::write(root.join("wide.log"), wide.join("\n") + "\n")?;

    std::fs::write(root.join("empty.txt"), b"")?;
    std::fs::write(root.join("crlf.txt"), b"alpha\r\nbeta\r\ngamma\r\n")?;
    std::fs::write(
        root.join("bom.txt"),
        "\u{FEFF}fn main() {}\nprintln!(\"hi\");\n",
    )?;

    let emoji: Vec<String> = (1..=200).map(|_| "\u{1F600}".repeat(100)).collect();
    std::fs::write(root.join("emoji.txt"), emoji.join("\n") + "\n")?;

    std::fs::write(
        root.join("fake.png"),
        "this is plain text wearing a .png extension\nsecond line\n",
    )?;

    let mut real = b"\x89PNG\r\n\x1a\n".to_vec();
    real.extend((0..1024).map(|i| (i % 256) as u8));
    std::fs::write(root.join("real.png"), real)?;

    let nul: Vec<u8> = (0..4096)
        .map(|i| if i % 8 == 7 { 0 } else { b'A' + (i % 26) as u8 })
        .collect();
    std::fs::write(root.join("nul.bin"), nul)?;

    std::fs::write(
        root.join("Screenshot 3.04\u{202F}PM.png"),
        "screenshot bytes stand-in\n",
    )?;
    std::fs::write(root.join("cafe\u{301}.txt"), "nfd spelling\n")?;
    std::fs::write(root.join("caf\u{e9}.txt"), "nfc spelling\n")?;
    std::fs::write(root.join("AGENTS.md"), "# Agents\n\nRead this first.\n")?;

    if big {
        make_sparse(&root.join("sparse.txt"))?;
    }
    Ok(())
}

fn make_sparse(path: &Path) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.seek(SeekFrom::Start(SPARSE_BYTES - 1))?;
    f.write_all(b"x")?;
    Ok(())
}

fn assert_golden(actual: &str, expected: &str) {
    if actual == expected {
        return;
    }
    let mut out = String::from("--- expected\n+++ actual\n");
    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();
    for (i, (e, a)) in exp.iter().zip(act.iter()).enumerate() {
        if e != a {
            out.push_str(&format!("@@ line {} @@\n- {e}\n+ {a}\n", i + 1));
        }
    }
    let n = exp.len().min(act.len());
    for line in &exp[n..] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &act[n..] {
        out.push_str(&format!("+ {line}\n"));
    }
    panic!("golden mismatch:\n{out}");
}

fn read_file(root: &Path, name: &str) -> Vec<u8> {
    std::fs::read(root.join(name)).unwrap_or_else(|e| panic!("fixture {name} missing: {e}"))
}

#[tokio::test]
async fn zoo_builds_every_fixture_with_expected_shape() {
    let zoo = Zoo::build().expect("zoo builds");
    let root = zoo.root();

    let long = String::from_utf8(read_file(&root, "long.txt")).unwrap();
    assert_eq!(long.lines().count(), 3000);
    let lock = String::from_utf8(read_file(&root, "lockfile.txt")).unwrap();
    assert_eq!(lock.lines().count(), 80_000);

    let minified = String::from_utf8(read_file(&root, "minified.js")).unwrap();
    let mut lines = minified.lines();
    assert_eq!(lines.next().unwrap().len(), 3900);
    assert_eq!(minified.lines().count(), 4);

    let wide = String::from_utf8(read_file(&root, "wide.log")).unwrap();
    assert_eq!(wide.lines().count(), 500);
    assert!(wide.lines().all(|l| l.len() == 300));

    assert_eq!(read_file(&root, "empty.txt").len(), 0);
    assert!(
        read_file(&root, "crlf.txt")
            .windows(2)
            .any(|w| w == b"\r\n")
    );
    assert!(read_file(&root, "bom.txt").starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(
        String::from_utf8(read_file(&root, "emoji.txt"))
            .unwrap()
            .contains('😀')
    );

    let fake = String::from_utf8(read_file(&root, "fake.png")).expect("fake.png is text");
    assert_eq!(fake.lines().count(), 2);
    assert_eq!(
        &read_file(&root, "real.png")[..8],
        b"\x89PNG\r\n\x1a\n",
        "real.png carries PNG magic"
    );
    let nul = read_file(&root, "nul.bin");
    assert_eq!(nul.len(), 4096);
    assert!(nul.contains(&0));

    assert!(root.join("Screenshot 3.04\u{202F}PM.png").exists());
    assert!(root.join("cafe\u{301}.txt").exists());
    assert!(root.join("caf\u{e9}.txt").exists());
    assert!(root.join("AGENTS.md").exists());

    if big_enabled() {
        assert_eq!(
            std::fs::metadata(root.join("sparse.txt")).unwrap().len(),
            SPARSE_BYTES
        );
    } else {
        assert!(
            !root.join("sparse.txt").exists(),
            "sparse.txt needs GRAY_ZOO_BIG=1"
        );
    }
}

#[test]
fn zoo_sparse_fixture_is_a_200mib_single_line() {
    let dir = TempDir::new().unwrap();
    make_sparse(&dir.path().join("sparse.txt")).unwrap();
    let bytes = std::fs::read(dir.path().join("sparse.txt")).unwrap();
    assert_eq!(bytes.len() as u64, SPARSE_BYTES);
    assert!(!bytes.contains(&b'\n'), "sparse fixture is one line");
}

#[tokio::test]
#[should_panic(expected = "golden mismatch")]
async fn zoo_golden_harness_fails_loudly_on_drift() {
    assert_golden("a\nb\n", "a\nc\n");
}

// INT-C wires T1.3: empty.txt returns the is_error=false empty note (absolute
// path, so assert shape not bytes).
#[tokio::test]
async fn zoo_golden_empty_file_matches_current_output() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("empty.txt", None, None).await;
    assert!(!out.is_error);
    assert!(out.content.starts_with("[read: "), "{}", out.content);
    assert!(
        out.content.ends_with("empty.txt is empty (0 bytes)]"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn zoo_golden_agents_md_round_trips_exact() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("AGENTS.md", None, None).await;
    assert!(!out.is_error);
    assert_golden(
        &out.content,
        "     1\t# Agents\n     2\t\n     3\tRead this first.",
    );
}

#[tokio::test]
async fn zoo_golden_long_txt_line_cut_exact() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("long.txt", None, None).await;
    assert!(!out.is_error);
    // T1.2: every shown line carries its absolute cat -n prefix.
    // INT-C wires T1.3: line-cap hint uses the notices::line_cap contract wording.
    let shown: Vec<String> = (1..=2000)
        .map(|i| format!("{:>6}\tline {i:04}", i))
        .collect();
    let expected = format!(
        "{}\n\n[read: showing lines 1-2000 of 3000. Continue with offset=2001.]",
        shown.join("\n")
    );
    assert_golden(&out.content, &expected);
}

// ── T2.2 deferred chunk-boundary truncation ──

async fn read_signed(
    root: &Path,
    path: &str,
    offset: Option<i64>,
    limit: Option<u64>,
) -> ToolOutput {
    let mut map = serde_json::Map::new();
    map.insert("path".to_string(), Value::String(path.to_string()));
    if let Some(o) = offset {
        map.insert("offset".to_string(), Value::from(o));
    }
    if let Some(l) = limit {
        map.insert("limit".to_string(), Value::from(l));
    }
    let ctx = ToolContext {
        cwd: root.to_path_buf(),
        ..ToolContext::default()
    };
    ReadTool::default().execute(&ctx, Value::Object(map)).await
}

/// Numbered content lines (`cat -n` prefixes). Notes never contain `\t`,
/// so every line with a numeric prefix is file content.
fn numbered_lines(content: &str) -> Vec<usize> {
    content
        .lines()
        .filter_map(|l| l.split_once('\t')?.0.trim().parse::<usize>().ok())
        .collect()
}

/// Resume offset from a cut/user hint (`Continue with offset=N` /
/// `Use offset=N`). Past-EOF notes are handled separately.
fn resume_offset(content: &str) -> Option<u64> {
    let i = content.find("offset=")?;
    content[i + 7..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .ok()
}

#[tokio::test]
async fn t22_exactly_filled_window_claims_no_more_remains() {
    // Line 2000 ends the file: the peek observes EOF → complete, no note.
    // (The old whole-file code agreed here; a naive streaming swap that sets
    // `cut` when the window fills would falsely claim offset=2001.)
    let dir = TempDir::new().unwrap();
    let lines: Vec<String> = (1..=2000).map(|i| format!("e{i:04}")).collect();
    std::fs::write(dir.path().join("exact.txt"), lines.join("\n") + "\n").unwrap();
    let out = read_signed(dir.path(), "exact.txt", None, None).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(!out.content.contains("Continue with"), "{}", out.content);
    assert!(!out.content.contains("more lines"), "{}", out.content);
    assert_eq!(numbered_lines(&out.content).len(), 2000);
}

#[tokio::test]
async fn t22_one_more_byte_names_offset_2001() {
    // Same file plus one byte (an unterminated 2001st line): the peek
    // observes it → line cut resuming exactly there.
    let dir = TempDir::new().unwrap();
    let lines: Vec<String> = (1..=2000).map(|i| format!("e{i:04}")).collect();
    std::fs::write(dir.path().join("plus1.txt"), lines.join("\n") + "\n").unwrap();
    let mut raw = std::fs::read(dir.path().join("plus1.txt")).unwrap();
    raw.push(b'z');
    std::fs::write(dir.path().join("plus1.txt"), raw).unwrap();
    let out = read_signed(dir.path(), "plus1.txt", None, None).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(resume_offset(&out.content), Some(2001), "{}", out.content);
    assert!(
        out.content.contains("of 2001. Continue with offset=2001."),
        "{}",
        out.content
    );
    // The property: re-reading at the named offset returns that line.
    let again = read_signed(dir.path(), "plus1.txt", Some(2001), None).await;
    assert!(
        again
            .content
            .lines()
            .next()
            .unwrap()
            .starts_with("  2001\tz"),
        "{}",
        again.content
    );
}

#[tokio::test]
async fn t22_byte_resume_points_at_the_unshown_line() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("wide.log", None, None).await;
    assert!(!out.is_error, "{}", out.content);
    let next = resume_offset(&out.content).expect("byte-cap note names a resume line");
    assert!(next > 1, "{next}");
    let shown = numbered_lines(&out.content);
    assert!(
        !shown.contains(&(next as usize)),
        "resume line was not shown"
    );
    assert_eq!(*shown.last().unwrap(), next as usize - 1);
    let again = read_signed(&zoo.root(), "wide.log", Some(next as i64), None).await;
    assert!(
        again
            .content
            .lines()
            .next()
            .unwrap()
            .starts_with(&format!("{:>6}\t", next)),
        "{}",
        again.content
    );
}

#[tokio::test]
async fn t22_cut_iff_reread_returns_a_line() {
    // Property over deterministic pseudo-random small files (line counts
    // straddle the 2000 boundary, lengths straddle the byte cap, offsets
    // cover head/tail/past-EOF, limits cover exact/over): a named resume
    // offset always yields content, and a note-free read really ended.
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x % n
        }
    }
    let mut rng = Rng(0x0022_CAFE);
    let dir = TempDir::new().unwrap();
    for case in 0..150 {
        let nlines = if case % 10 == 9 {
            1995 + rng.below(12) // 1995..=2006: the line-cap boundary
        } else if case % 25 == 24 {
            100 + rng.below(400) // long lines: the byte-cap zone
        } else {
            1 + rng.below(30)
        } as usize;
        let long_every = if case % 25 == 24 { 1 } else { 7 };
        let mut text = String::new();
        for i in 1..=nlines {
            let len = if i % long_every == 0 && case % 3 == 0 {
                300
            } else {
                1 + rng.below(40)
            } as usize;
            let ch = (b'a' + (i % 26) as u8) as char;
            text.push_str(&ch.to_string().repeat(len));
            text.push('\n');
        }
        if case % 7 == 3 {
            // One clamped line.
            text.push_str(&"Q".repeat(2500));
            text.push('\n');
        }
        std::fs::write(dir.path().join("p.txt"), &text).unwrap();
        let total = text.lines().count();
        let offset: Option<i64> = match case % 5 {
            0 => None,
            1 => Some(1),
            2 => Some(1 + rng.below(total as u64 + 3) as i64),
            3 => Some(-(1 + rng.below(total as u64 + 2) as i64)),
            _ => Some(total as i64 + 1 + rng.below(3) as i64), // past EOF
        };
        let limit: Option<u64> = match case % 4 {
            0 => None,
            1 => Some(1 + rng.below(10)),
            2 => Some(5000),
            _ => None,
        };
        let out = read_signed(dir.path(), "p.txt", offset, limit).await;
        assert!(
            !out.is_error,
            "case {case} off={offset:?} lim={limit:?}: {}",
            out.content
        );
        if out.content.contains("beyond the end") {
            // Past EOF: the suggested tail retry must yield lines.
            let retry = resume_offset(&out.content).expect("EOF note suggests a retry");
            let r2 = read_signed(dir.path(), "p.txt", Some(retry as i64), None).await;
            assert!(
                !numbered_lines(&r2.content).is_empty(),
                "case {case}: retry {retry}:\n{}",
                r2.content
            );
        } else if let Some(n) = resume_offset(&out.content) {
            // Cut/user hint: re-reading there returns that very line.
            let r2 = read_signed(dir.path(), "p.txt", Some(n as i64), None).await;
            let first = r2.content.lines().next().unwrap_or("");
            assert!(
                first.starts_with(&format!("{:>6}\t", n)),
                "case {case} off={offset:?} lim={limit:?}: resume {n} gave {first:?}\n{}",
                r2.content
            );
        } else {
            // Complete: the line after the last shown one is past EOF.
            let shown = numbered_lines(&out.content);
            assert!(
                !shown.is_empty(),
                "case {case}: no content and no note:\n{}",
                out.content
            );
            let r2 = read_signed(
                dir.path(),
                "p.txt",
                Some(*shown.last().unwrap() as i64 + 1),
                None,
            )
            .await;
            assert!(
                r2.content.contains("beyond the end"),
                "case {case}: claimed complete but more follows:\n{}",
                r2.content
            );
        }
    }
}

#[tokio::test]
async fn t22_huge_file_skips_count_but_resumes_and_guards_write() {
    // >64 MiB text: exact counting is skipped (fragment note) while the
    // resume offset still names an observed line; the partial view still
    // refuses a blind overwrite with that offset.
    const BIG_LINE: usize = 70 * 1024 * 1024;
    let dir = TempDir::new().unwrap();
    let mut data = "x\n".repeat(2000).into_bytes();
    data.extend(std::iter::repeat_n(b'y', BIG_LINE));
    std::fs::write(dir.path().join("huge.txt"), &data).unwrap();

    let ledger = Arc::new(gray_tools::FileLedger::new());
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = gray_tools::ReadTool::new(ledger.clone())
        .execute(&ctx, serde_json::json!({"path": "huge.txt"}))
        .await;
    assert!(
        !out.is_error,
        "{}",
        &out.content[..out.content.len().min(300)]
    );
    assert!(
        out.content.contains("count skipped"),
        "{}",
        &out.content[out.content.len().saturating_sub(300)..]
    );
    assert!(
        out.content.contains("offset=2001"),
        "{}",
        &out.content[out.content.len().saturating_sub(300)..]
    );

    // Resume lands on the observed over-long line, clamped exactly.
    let again = read_signed(dir.path(), "huge.txt", Some(2001), None).await;
    let first = again.content.lines().next().unwrap_or("");
    assert!(first.starts_with("  2001\t"), "{first:.80}");
    assert!(
        again
            .content
            .contains(&format!("…[+{} chars]", BIG_LINE - 2000)),
        "{}",
        &again.content[..again.content.len().min(200)]
    );

    // The partial view refuses a blind overwrite with the resume offset.
    let wout = gray_tools::WriteTool::new(ledger)
        .execute(
            &ctx,
            serde_json::json!({"path": "huge.txt", "content": "new\n"}),
        )
        .await;
    assert!(wout.is_error, "{}", wout.content);
    assert!(wout.content.contains("only part of"), "{}", wout.content);
    assert!(wout.content.contains("offset=2001"), "{}", wout.content);
}

#[tokio::test]
async fn zoo_smoke_every_small_fixture_reads() {
    let zoo = Zoo::build().unwrap();
    // INT-C wires T1.4: real.png returns an is_error=false mime note, not a failure.
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
        "nul.bin",
        "AGENTS.md",
    ] {
        let out = zoo.read(name, None, None).await;
        assert!(!out.is_error, "{name} should read today: {}", out.content);
        assert!(!out.content.is_empty() || name == "empty.txt");
    }
    let miss = zoo.read("real.png", None, None).await;
    assert!(
        !miss.is_error,
        "real.png returns a note today: {}",
        miss.content
    );
    assert!(miss.content.contains("is image/png"), "{}", miss.content);
}
