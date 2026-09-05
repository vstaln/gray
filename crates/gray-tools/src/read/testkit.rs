//! T0.1 scaffold — hostile-file fixture zoo + golden-output harness.
//!
//! Planned wiring (T1.1, which splits `read.rs` into `read/mod.rs`):
//! add `#[cfg(test)] pub(crate) mod testkit;` to `read/mod.rs`.
//!
//! Fixtures are generated at test time into a `TempDir` and never committed.
//! `crates/gray-tools/tests/read_zoo.rs` carries a self-contained copy of the
//! builders until T1.1 dedups the two (this file uses `crate::` paths, the
//! integration test uses `gray_tools::`).

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use gray_core::agent::{ToolContext, ToolOutput};
use serde_json::Value;
use tempfile::TempDir;

use crate::Tool;
use crate::read::ReadTool;

/// `GRAY_ZOO_BIG=1` also builds the 200 MiB sparse single-line fixture.
pub fn big_enabled() -> bool {
    std::env::var("GRAY_ZOO_BIG").as_deref() == Ok("1")
}

/// Logical size of the sparse single-line fixture.
pub const SPARSE_BYTES: u64 = 200 * 1024 * 1024;

/// A tempdir populated with every hostile file shape from the T0.1 spec.
pub struct Zoo {
    // Owns the tempdir (deleted on drop); reach it via `root()`.
    dir: TempDir,
}

impl Zoo {
    /// Build every fixture, plus `sparse.txt` when [`big_enabled`].
    pub fn build() -> std::io::Result<Self> {
        let dir = TempDir::new()?;
        write_fixtures(dir.path(), big_enabled())?;
        Ok(Self { dir })
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Run `ReadTool` with a `ToolContext` rooted at the zoo tempdir.
    pub async fn read(&self, path: &str, offset: Option<u64>, limit: Option<u64>) -> ToolOutput {
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

/// Generate every T0.1 fixture shape into `root` (test-time only, never committed).
pub fn write_fixtures(root: &Path, big: bool) -> std::io::Result<()> {
    // long.txt: 3,000 short lines (crosses the 2,000-line window).
    let long: Vec<String> = (1..=3000).map(|i| format!("line {i:04}")).collect();
    std::fs::write(root.join("long.txt"), long.join("\n") + "\n")?;

    // lockfile.txt: 80,000 short lines (deep-offset reads).
    let lock: Vec<String> = (1..=80_000)
        .map(|i| format!("lock entry {i:06} sha=abcdef"))
        .collect();
    std::fs::write(root.join("lockfile.txt"), lock.join("\n") + "\n")?;

    // minified.js: one 3,900-char line + 3 normal lines.
    let head = "!function(e){var t={};";
    let first = format!("{head}{}", "x".repeat(3900 - head.len()));
    assert_eq!(first.len(), 3900);
    std::fs::write(
        root.join("minified.js"),
        format!("{first}\n//# sourceMappingURL=app.js.map\nconsole.log(\"ok\");\nconst x = 1;\n"),
    )?;

    // wide.log: 500 lines x 300 chars (crosses the 50 KiB cap before 2,000 lines).
    let wide: Vec<String> = (1..=500)
        .map(|i| format!("{:<300}", format!("log line {i:04} ")))
        .collect();
    std::fs::write(root.join("wide.log"), wide.join("\n") + "\n")?;

    std::fs::write(root.join("empty.txt"), b"")?;
    std::fs::write(root.join("crlf.txt"), b"alpha\r\nbeta\r\ngamma\r\n")?;
    std::fs::write(root.join("bom.txt"), "\u{FEFF}fn main() {}\nprintln!(\"hi\");\n")?;

    // emoji.txt: multibyte chars straddling the byte cap (400 bytes/line).
    let emoji: Vec<String> = (1..=200).map(|_| "\u{1F600}".repeat(100)).collect();
    std::fs::write(root.join("emoji.txt"), emoji.join("\n") + "\n")?;

    // fake.png: plain text wearing a .png extension (sniff must say text).
    std::fs::write(
        root.join("fake.png"),
        "this is plain text wearing a .png extension\nsecond line\n",
    )?;

    // real.png: 8-byte PNG magic + junk (sniff must say binary).
    let mut real = b"\x89PNG\r\n\x1a\n".to_vec();
    real.extend((0..1024).map(|i| (i % 256) as u8));
    std::fs::write(root.join("real.png"), real)?;

    // nul.bin: 4 KiB laced with NUL bytes.
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

    // Narrow no-break space (U+202F) macOS-screenshot shape.
    std::fs::write(
        root.join("Screenshot 3.04\u{202F}PM.png"),
        "screenshot bytes stand-in\n",
    )?;

    // cafe in NFD (e + combining acute) and a sibling in NFC.
    std::fs::write(root.join("cafe\u{301}.txt"), "nfd spelling\n")?;
    std::fs::write(root.join("caf\u{e9}.txt"), "nfc spelling\n")?;

    // For the AGENT.md miss (did-you-mean target).
    std::fs::write(root.join("AGENTS.md"), "# Agents\n\nRead this first.\n")?;

    if big {
        make_sparse(&root.join("sparse.txt"))?;
    }
    Ok(())
}

/// 200 MiB single line via seek+write (instant, sparse — never committed).
pub fn make_sparse(path: &Path) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.seek(SeekFrom::Start(SPARSE_BYTES - 1))?;
    f.write_all(b"x")?;
    Ok(())
}

/// Byte-for-byte golden comparison; the notice text is part of the contract,
/// so it is compared too. Panics with a line diff on drift.
pub fn assert_golden(actual: &str, expected: &str) {
    if actual == expected {
        return;
    }
    panic!("golden mismatch:\n{}", diff_lines(expected, actual));
}

// ponytail: naive O(n) line diff, no LCS — good enough for goldens under ~80k lines.
fn diff_lines(expected: &str, actual: &str) -> String {
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
    if expected.ends_with('\n') != actual.ends_with('\n') {
        out.push_str(&format!(
            "@@ trailing newline differs: expected {}, actual {} @@\n",
            expected.ends_with('\n'),
            actual.ends_with('\n')
        ));
    }
    out
}
