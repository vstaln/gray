use std::path::Path;
use std::time::Duration;

use crate::images::MediaError;

const SHEET_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub const DEFAULT_FRAMES: usize = 16;
const MAX_FRAMES: usize = 64;

pub(crate) fn run(
    cmd: &str,
    args: &[&str],
    limit: Duration,
) -> Result<std::process::Output, MediaError> {
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

pub(crate) fn detail(out: &std::process::Output, fallback: &str) -> MediaError {
    let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        MediaError::Extract(fallback.to_string())
    } else {
        MediaError::Extract(text.chars().take(200).collect())
    }
}

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

pub fn video_sheet(path: &Path, frames: usize) -> Result<Vec<u8>, MediaError> {
    let frames = frames.clamp(1, MAX_FRAMES);
    let p = path.display().to_string();

    if !path.is_file() {
        return Err(MediaError::Extract(format!(
            "{}: no such file",
            path.display()
        )));
    }

    let fps = match duration_secs(path) {
        Some(d) if d > 0.0 => format!("{:.6}", frames as f64 / d),
        _ => format!("{:.6}", frames as f64 / 30.0),
    };

    let cols = if frames <= 4 { frames } else { 4 };

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
