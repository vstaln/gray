//! T0.1 — hostile-file fixture zoo + golden-output harness.
//!
//! Self-contained until T1.1 wires `src/read/testkit.rs` into the crate and
//! dedups the builders (that module uses `crate::` paths, this file `gray_tools::`).

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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
        ReadTool.execute(&ctx, Value::Object(map)).await
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
    std::fs::write(root.join("bom.txt"), "\u{FEFF}fn main() {}\nprintln!(\"hi\");\n")?;

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
        .map(|i| {
            if i % 8 == 7 {
                0
            } else {
                b'A' + (i % 26) as u8
            }
        })
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
    assert!(read_file(&root, "crlf.txt").windows(2).any(|w| w == b"\r\n"));
    assert!(read_file(&root, "bom.txt").starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(String::from_utf8(read_file(&root, "emoji.txt")).unwrap().contains('😀'));

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
        assert!(!root.join("sparse.txt").exists(), "sparse.txt needs GRAY_ZOO_BIG=1");
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

// Current behavior pins (T1.1/T1.3 update these deliberately with a reason each):
// empty.txt returns silence today; T1.3 turns it into an is_error=false note.
#[tokio::test]
async fn zoo_golden_empty_file_matches_current_output() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("empty.txt", None, None).await;
    assert!(!out.is_error);
    assert_golden(&out.content, "");
}

#[tokio::test]
async fn zoo_golden_agents_md_round_trips_exact() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("AGENTS.md", None, None).await;
    assert!(!out.is_error);
    assert_golden(&out.content, "     1\t# Agents\n     2\t\n     3\tRead this first");
}

#[tokio::test]
async fn zoo_golden_long_txt_line_cut_exact() {
    let zoo = Zoo::build().unwrap();
    let out = zoo.read("long.txt", None, None).await;
    assert!(!out.is_error);
    // T1.2: every shown line carries its absolute cat -n prefix.
    let shown: Vec<String> = (1..=2000).map(|i| format!("{:>6}\tline {i:04}", i)).collect();
    let expected = format!(
        "{}\n\n[Showing lines 1-2000 of 3000. Use offset=2001 to continue.]",
        shown.join("\n")
    );
    assert_golden(&out.content, &expected);
}

#[tokio::test]
async fn zoo_smoke_every_small_fixture_reads() {
    let zoo = Zoo::build().unwrap();
    // real.png is invalid UTF-8 today so read fails; T1.4/T5.1 turns this into an ok-note.
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
    assert!(miss.is_error, "real.png fails as non-UTF8 until T1.4");
}
