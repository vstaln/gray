//! `gray view` — show an image file as an image, not as text.
//!
//! The bash tool claims `gray view <path>` before the shell runs, exactly as
//! it claims `cat <path>`, so in an agent session the image is attached as a
//! vision block. When a human runs `gray view` directly, there is no vision
//! block to ride on, so `run_cli` draws each image inline with the Kitty
//! graphics protocol on terminals that speak it (Kitty, Ghostty, WezTerm)
//! and falls back to validated names elsewhere. The decode, the 2000px
//! downscale and the error wording live in `gray_tools::view` so both
//! surfaces stay in agreement.

use std::io::{IsTerminal, Write};
use std::path::Path;

/// Load every path, keeping successes and per-path errors side by side so
/// one bad file never sinks the good ones.
fn load_all(
    paths: &[String],
    frames: Option<usize>,
) -> (Vec<gray_tools::view::Shown>, Vec<String>) {
    let mut shown = Vec::new();
    let mut failed = Vec::new();
    for raw in paths {
        match gray_tools::view::load_with_frames(Path::new(raw), frames) {
            Ok(part) => shown.push(part),
            Err(e) => failed.push(e.to_string()),
        }
    }
    (shown, failed)
}

/// One report line per shown path. A contact sheet is derived from a video,
/// not the file itself, so it says so — "viewed clip.mp4" alone would read
/// as if the video had been handed over.
fn shown_line(part: &gray_tools::view::Shown) -> String {
    if part.derived_from_video {
        format!("viewed {} (contact sheet)", part.path.display())
    } else {
        format!("viewed {}", part.path.display())
    }
}

/// What `gray view PATH...` reports: the lines to print for the images it
/// showed and the errors for the ones it could not. Kept as data, so the
/// exit code and the wording are testable without capturing stdout.
pub fn view_lines(paths: &[String]) -> (Vec<String>, Vec<String>) {
    let (shown, failed) = load_all(paths, None);
    (shown.iter().map(shown_line).collect(), failed)
}

/// Kitty transmit-and-display sequence for one base64 PNG, in 4096-byte
/// chunks like the compositor background upload. Pure (no terminal needed)
/// so tests cover the framing.
pub fn kitty_sequence(png_base64: &str) -> String {
    let mut out = String::new();
    let mut chunks = png_base64.as_bytes().chunks(4096).peekable();
    if chunks.peek().is_none() {
        return out;
    }
    while let Some(chunk) = chunks.next() {
        let more = usize::from(chunks.peek().is_some());
        if out.is_empty() {
            out.push_str(&format!("\x1b_Ga=T,f=100,m={more};"));
        } else {
            out.push_str(&format!("\x1b_Gm={more};"));
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\x1b\\");
    }
    out.push('\n');
    out
}

/// True on terminals speaking the Kitty graphics protocol (Kitty, Ghostty,
/// WezTerm). Conservative on purpose: on anything else an escape sequence
/// would print as garbage, so `run_cli` keeps the validated-names fallback.
pub fn terminal_supports_images() -> bool {
    if std::env::var("KITTY_WINDOW_ID").is_ok() || std::env::var("WEZTERM_VERSION").is_ok() {
        return true;
    }
    match std::env::var("TERM_PROGRAM") {
        Ok(prog) => {
            let norm: String = prog
                .chars()
                .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
                .map(|c| c.to_ascii_lowercase())
                .collect();
            matches!(norm.as_str(), "kitty" | "ghostty" | "wezterm")
        }
        Err(_) => false,
    }
}

/// PNG bytes for inline display. Normalized data is already PNG except that
/// JPEG stays JPEG, and Kitty inline display takes PNG — so JPEG is
/// re-encoded through the `image` crate.
fn display_png(shown: &gray_tools::view::Shown) -> Option<String> {
    if shown.media_type == "image/png" {
        return Some(shown.data.clone());
    }
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&shown.data)
        .ok()?;
    let img = image::load_from_memory(&raw).ok()?;
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// `gray view PATH...`: draw each image inline where the terminal allows it,
/// name what was shown, non-zero exit if any path failed. A failed draw
/// reads as a failure, never as a view.
pub fn run_cli(paths: &[String], frames: Option<usize>) -> anyhow::Result<()> {
    let draw = terminal_supports_images() && std::io::stdout().is_terminal();
    let (shown, mut failed) = load_all(paths, frames);
    let mut names = Vec::with_capacity(shown.len());
    if draw {
        let mut out = std::io::stdout().lock();
        for part in &shown {
            match display_png(part).map(|png| kitty_sequence(&png)) {
                Some(seq) if !seq.is_empty() && out.write_all(seq.as_bytes()).is_ok() => {
                    names.push(shown_line(part));
                }
                _ => failed.push(format!(
                    "{}: terminal could not display the image",
                    part.path.display()
                )),
            }
        }
        let _ = out.flush();
    } else {
        names.extend(shown.iter().map(shown_line));
    }
    for line in names {
        println!("{line}");
    }
    for err in &failed {
        eprintln!("gray view: {err}");
    }
    if !failed.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

#[path = "view_tests.rs"]
#[cfg(test)]
mod tests;
