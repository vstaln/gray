use std::path::Path;
use std::time::Duration;

use gray_core::message::{ContentBlock, Message};

const SCRIPT: &str = r#"uname -srm 2>/dev/null;
(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME");
echo "cpus: $(nproc 2>/dev/null)  mem: $(free -m 2>/dev/null | awk '/Mem:/{print $2" MB"}')";
for t in python3 pip node npm go cargo rustc gcc g++ clang java make cmake docker git; do p=$(command -v $t 2>/dev/null) && echo "$t: $($t --version 2>&1 | head -n1)"; done;
[ -f package.json ] && echo "package.json scripts: $(grep -o '"[a-z:-]*": *"[^"]*"' package.json | head -8 | tr '\n' ' ')";
for f in Cargo.toml pyproject.toml setup.py pytest.ini go.mod Makefile CMakeLists.txt requirements.txt; do [ -f $f ] && echo "found $f"; done;
git status -sb 2>/dev/null | head -5"#;

const CAP: usize = 2000;

pub(crate) fn enabled() -> bool {
    std::env::var("GRAY_ENV_SNAPSHOT").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) async fn seed(message: &mut Message, cwd: &Path) {
    if !enabled() {
        return;
    }
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(SCRIPT)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(5), command.output()).await
    else {
        return;
    };
    let mut text = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    if text.is_empty() {
        return;
    }
    if text.len() > CAP {
        let mut end = CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    let block = format!("\n\n<environment>\n{text}\n</environment>");
    match message.content.iter_mut().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text),
        _ => None,
    }) {
        Some(existing) => existing.push_str(&block),
        None => message.content.push(ContentBlock::text(block)),
    }
}
