//! `/feedback` support: save a local copy under `~/.gray/feedback/` and build
//! a prefilled GitHub issue URL for `vstaln/gray`.

use std::io::Write;
use std::path::{Path, PathBuf};

pub const ISSUES_NEW_URL: &str = "https://github.com/vstaln/gray/issues/new";
const MAX_URL_BODY_CHARS: usize = 3000;
const MAX_TITLE_CHARS: usize = 80;

/// First non-empty line of `text`, capped at [`MAX_TITLE_CHARS`] chars.
pub fn build_title(text: &str) -> String {
    for line in text.lines() {
        let clean = line.trim();
        if clean.is_empty() {
            continue;
        }
        if clean.chars().count() <= MAX_TITLE_CHARS {
            return clean.to_string();
        }
        let prefix: String = clean.chars().take(MAX_TITLE_CHARS).collect();
        let cut = prefix
            .rfind(' ')
            .map(|i| prefix[..i].to_string())
            .unwrap_or(prefix);
        return format!("{}…", cut.trim_end());
    }
    "Feedback".to_string()
}

/// Bug-report body: `Summary` carries the title, `Actual Behavior` the raw
/// feedback text; environment lines pin the version/OS/model/session.
pub fn build_body(text: &str, version: &str, os: &str, model: &str, session: &str) -> String {
    let title = build_title(text);
    format!(
        "## Summary\n{title}\n\n## Expected Behavior\n—\n\n## Actual Behavior\n{}\n\n## Steps to Reproduce\n—\n\n---\nCommand Code Version: {version}\nOperating System: {os}\nModel: {model}\nSession: {session}\n",
        text.trim()
    )
}

/// Prefilled "new issue" URL. The body is capped so the URL stays openable;
/// the local file always keeps the full text.
pub fn issue_url(title: &str, body: &str) -> String {
    let short = if body.chars().count() > MAX_URL_BODY_CHARS {
        let cut: String = body.chars().take(MAX_URL_BODY_CHARS).collect();
        format!("{cut}\n… [truncated for URL — full text in local file]")
    } else {
        body.to_string()
    };
    let enc = |s: &str| {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    format!("{ISSUES_NEW_URL}?title={}&body={}", enc(title), enc(&short))
}

/// `feedback-20260905T120000Z.md`-style stamp for filenames.
pub fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Writes `# title` + `body` into `dir`, appending `-N` on collision.
/// Returns the final path.
pub fn save_feedback(dir: &Path, title: &str, body: &str, stamp: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut path = dir.join(format!("feedback-{stamp}.md"));
    for n in 2..100 {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("feedback-{stamp}-{n}.md"));
    }
    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "# {title}")?;
    writeln!(f)?;
    writeln!(f, "{body}")?;
    Ok(path)
}

/// Best-effort browser open (`xdg-open`/`open`/`start`); failures are silent
/// since the URL is always printed too.
pub fn open_in_browser(url: &str) {
    use std::process::{Command, Stdio};
    let res = match std::env::consts::OS {
        "macos" => Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
        "windows" => Command::new("cmd")
            .args(["/c", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
        _ => Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
    };
    let _ = res;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_uses_first_line_and_caps() {
        assert_eq!(build_title("  hello world\nsecond"), "hello world");
        assert_eq!(build_title("\n\n  \nreal"), "real");
        assert_eq!(build_title(""), "Feedback");
        let long = "w".repeat(200);
        let t = build_title(&long);
        assert!(t.chars().count() <= MAX_TITLE_CHARS + 1, "{t}");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn body_carries_environment_footer() {
        let b = build_body("broken x", "0.1.0", "linux / x86_64", "m", "s1");
        assert!(b.contains("## Summary\nbroken x"), "{b}");
        assert!(b.contains("## Expected Behavior\n—"), "{b}");
        assert!(b.contains("## Actual Behavior\nbroken x"), "{b}");
        assert!(b.contains("## Steps to Reproduce\n—"), "{b}");
        assert!(b.contains("Command Code Version: 0.1.0"), "{b}");
        assert!(b.contains("Operating System: linux / x86_64"), "{b}");
        assert!(b.contains("Model: m"), "{b}");
        assert!(b.contains("Session: s1"), "{b}");
    }

    #[test]
    fn url_is_prefilled_and_encoded() {
        let u = issue_url("a b", "c&d");
        assert!(u.starts_with(ISSUES_NEW_URL));
        assert!(u.contains("title=a%20b"), "{u}");
        assert!(u.contains("body=c%26d"), "{u}");
    }

    #[test]
    fn url_body_truncates_but_file_does_not() {
        let big = "x".repeat(MAX_URL_BODY_CHARS + 10);
        let u = issue_url("t", &big);
        assert!(u.contains("truncated"), "{u}");
        let dir = tempfile::tempdir().unwrap();
        let p = save_feedback(dir.path(), "t", &big, "stamp").unwrap();
        let saved = std::fs::read_to_string(p).unwrap();
        assert!(saved.contains(&big));
    }

    #[test]
    fn save_dedups_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let a = save_feedback(dir.path(), "t", "b", "s").unwrap();
        let b = save_feedback(dir.path(), "t", "b", "s").unwrap();
        assert_ne!(a, b);
        assert!(b.to_string_lossy().contains("feedback-s-2.md"));
    }
}
