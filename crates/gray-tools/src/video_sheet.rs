//! Video → a single contact-sheet JPEG, so `gray view clip.mp4` works on
//! every model regardless of whether it takes a native video part.
//!
//! ffmpeg does the work in one pass: `fps` picks `frames` stills spread
//! evenly over the duration, `scale` shrinks each to fit, `tile` lays them
//! out in a grid, and the result is capped by the caller's normalizer like
//! any other image. No ffprobe: `fps=frames/duration` needs the duration,
//! so it is probed with ffprobe, and a clip ffmpeg can decode but ffprobe
//! cannot describe falls back to a fixed rate.

use std::path::Path;
use std::time::Duration;

use crate::images::MediaError;

/// ffmpeg on a long clip is not instant; past this we return the error
/// rather than block a turn forever.
const SHEET_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Default tile count. 16 reads as a 4x4 sheet at a glance; more is just a
/// bigger image to the model.
pub const DEFAULT_FRAMES: usize = 16;
const MAX_FRAMES: usize = 64;

fn run(cmd: &str, args: &[&str], limit: Duration) -> Result<std::process::Output, MediaError> {
    let program = cmd.to_string();
    let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(
            std::process::Command::new(&program)
                .args(&argv)
                .output()
                .map_err(|e| std::io::Error::other(e.to_string())),
        );
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(MediaError::Extract(format!("{cmd} not available: {e}"))),
        Err(_) => Err(MediaError::Extract(format!(
            "{cmd} timed out after {}s",
            limit.as_secs()
        ))),
    }
}

fn detail(out: &std::process::Output, fallback: &str) -> MediaError {
    let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        MediaError::Extract(fallback.to_string())
    } else {
        MediaError::Extract(text.chars().take(200).collect())
    }
}

/// Clip duration in seconds via ffprobe, or `None` if it cannot be read.
fn duration_secs(path: &Path) -> Option<f64> {
    let p = path.display().to_string();
    let out = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &p,
        ],
        PROBE_TIMEOUT,
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// `frames` stills of `path` tiled into one JPEG. The returned bytes go
/// through the caller's image normalizer, so the 2000px / 5MB caps and the
/// PNG re-encode behave exactly as they do for a screenshot.
pub fn video_sheet(path: &Path, frames: usize) -> Result<Vec<u8>, MediaError> {
    let frames = frames.clamp(1, MAX_FRAMES);
    let p = path.display().to_string();

    // ffmpeg's own "no such file" is a fine error, but it arrives as three
    // lines of muxer chatter. Check first so a typo reads like a typo.
    if !path.is_file() {
        return Err(MediaError::Extract(format!(
            "{}: no such file",
            path.display()
        )));
    }

    // fps=frames/duration spreads the tiles over the whole clip; when the
    // duration is unreadable, assume a 30s clip rather than dropping to a
    // single frame, so a weird container still yields a useful sheet.
    let fps = match duration_secs(path) {
        Some(d) if d > 0.0 => format!("{:.6}", frames as f64 / d),
        _ => format!("{:.6}", frames as f64 / 30.0),
    };
    // 4 columns reads best in a square-ish grid for the usual counts;
    // `tile=Wx-1` lets the row count follow from the frame count.
    let cols = if frames <= 4 { frames } else { 4 };

    // One pass: sample, shrink to fit, tile, encode. `tile` emits a single
    // frame once it has `frames` inputs; -frames:v 1 stops it there even if
    // a longer clip yields extra tiles.
    // ffmpeg's tile takes a literal WxH and only accepts -1 for the *height*
    // (padding to the stream end), so the row count has to be computed here.
    // A partial last row is fine: tile pads the missing cells itself.
    let rows = frames.div_ceil(cols);
    let vf = format!("fps={fps},scale='min(640,iw)':-2,tile={cols}x{rows}");
    let out = run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &p,
            "-vf",
            &vf,
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "-",
        ],
        SHEET_TIMEOUT,
    )?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(detail(&out, "no video frame decoded"));
    }
    Ok(out.stdout)
}

#[path = "video_sheet_tests.rs"]
#[cfg(test)]
mod tests;
