use super::*;

use std::process::Command;

/// ffmpeg/ffprobe are the decoder here and are not guaranteed on a build
/// machine, so the one test that needs a real clip skips rather than fails.
fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A 2s solid-color clip: 64x64, one second per color, cheap to make.
fn tiny_clip(path: &std::path::Path) -> bool {
    Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x64:d=1",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64:d=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn a_missing_video_reports_a_typo_not_ffmpeg_chatter() {
    let err = video_sheet(std::path::Path::new("/tmp/definitely-not-here.mp4"), 4)
        .expect_err("missing file must fail");
    assert!(err.to_string().contains("no such file"), "{err}");
}

#[test]
fn garbage_bytes_fail_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("liar.mp4");
    std::fs::write(&path, b"definitely not a video").unwrap();
    assert!(video_sheet(&path, 4).is_err(), "garbage must not decode");
}

#[test]
fn a_real_clip_becomes_a_decodable_jpeg_sheet() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    if !tiny_clip(&path) {
        eprintln!("skipping: could not build the test clip");
        return;
    }

    let sheet = video_sheet(&path, 4).expect("a real clip must sheet");
    let decoded = image::load_from_memory_with_format(&sheet, image::ImageFormat::Jpeg)
        .expect("sheet must be a decodable JPEG");
    // 4 tiles of a 64-wide source at 2 rows: wider than tall, and not a
    // single-frame passthrough.
    let (w, h) = (decoded.width(), decoded.height());
    assert!(w > 64, "sheet must tile, got {w}x{h}");
    assert!(h >= 64, "sheet must tile, got {w}x{h}");
}

#[test]
fn a_bigger_tile_count_yields_a_taller_sheet() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    if !tiny_clip(&path) {
        eprintln!("skipping: could not build the test clip");
        return;
    }
    let dims = |n: usize| {
        let bytes = video_sheet(&path, n).expect("sheet");
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
        (img.width(), img.height())
    };
    let few = dims(4);
    let many = dims(16);
    assert!(
        many.1 > few.1 || many.0 > few.0,
        "more tiles must be a bigger sheet: {few:?} vs {many:?}"
    );
}
