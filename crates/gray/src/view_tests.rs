use super::*;

use std::io::Cursor;

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([3, 2, 1]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

#[test]
fn run_cli_reports_every_image_it_showed() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    std::fs::write(&a, png()).unwrap();
    std::fs::write(&b, png()).unwrap();

    let paths = vec![
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
    ];
    let (shown, failed) = view_lines(&paths);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown[0].starts_with("viewed "), "{shown:?}");
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
fn run_cli_keeps_going_and_reports_failures() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.png");
    std::fs::write(&good, png()).unwrap();
    let text = dir.path().join("notes.md");
    std::fs::write(&text, "not an image").unwrap();

    let paths = vec![
        text.to_string_lossy().into_owned(),
        good.to_string_lossy().into_owned(),
    ];
    let (shown, failed) = view_lines(&paths);
    assert_eq!(shown.len(), 1, "the good one still shows: {shown:?}");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("use cat for text files"), "{failed:?}");
}

#[test]
fn run_cli_reports_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let (shown, failed) = view_lines(&[dir.path().join("gone.png").to_string_lossy().into_owned()]);
    assert!(shown.is_empty());
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("view failed"), "{failed:?}");
}

#[test]
fn fallback_line_says_nothing_was_drawn() {
    // `viewed …` alone reads as success to an agent whose turn got no image,
    // so the non-drawing path must say what actually happened.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.png");
    std::fs::write(&p, png()).unwrap();
    let shown = gray_tools::view::load(&p).unwrap();
    let line = fallback_line(&shown);
    assert!(line.starts_with("viewed "), "{line}");
    assert!(line.contains("not shown"), "{line}");
    assert_eq!(shown_line(&shown), format!("viewed {}", p.display()));
}

#[test]
fn kitty_sequence_frames_one_png() {
    let seq = kitty_sequence("QUJD");
    assert!(seq.starts_with("\x1b_Ga=T,f=100,m=0;"), "{seq:?}");
    assert!(seq.contains("QUJD"), "{seq:?}");
    assert!(seq.ends_with("\x1b\\\n"), "{seq:?}");
}

#[test]
fn kitty_sequence_chunks_a_large_payload() {
    let big = "Q".repeat(5000);
    let seq = kitty_sequence(&big);
    // First chunk says more follows, last says it is done.
    assert!(seq.contains("\x1b_Ga=T,f=100,m=1;"), "{seq:?}");
    assert!(seq.contains("\x1b_Gm=0;"), "{seq:?}");
    assert_eq!(seq.matches("\x1b\\").count(), 2, "{seq:?}");
}

#[test]
fn kitty_sequence_of_nothing_is_nothing() {
    assert!(kitty_sequence("").is_empty());
}

#[test]
fn terminal_support_reads_known_env() {
    // One test, sequential cases: env is process-global and tests run in
    // parallel, so the save/mutate/restore cycle must not span tests.
    let keep_kitty = std::env::var("KITTY_WINDOW_ID").ok();
    let keep_wez = std::env::var("WEZTERM_VERSION").ok();
    let keep_prog = std::env::var("TERM_PROGRAM").ok();
    unsafe {
        std::env::remove_var("KITTY_WINDOW_ID");
        std::env::remove_var("WEZTERM_VERSION");
        std::env::remove_var("TERM_PROGRAM");
    }
    assert!(!terminal_supports_images(), "bare env draws nothing");

    unsafe { std::env::set_var("KITTY_WINDOW_ID", "0") };
    assert!(terminal_supports_images(), "kitty window id means kitty");
    unsafe { std::env::remove_var("KITTY_WINDOW_ID") };

    unsafe { std::env::set_var("TERM_PROGRAM", "ghostty") };
    assert!(terminal_supports_images(), "ghostty speaks kitty graphics");
    unsafe { std::env::set_var("TERM_PROGRAM", "Apple_Terminal") };
    assert!(!terminal_supports_images(), "apple terminal does not");

    match keep_kitty {
        Some(v) => unsafe { std::env::set_var("KITTY_WINDOW_ID", v) },
        None => unsafe { std::env::remove_var("KITTY_WINDOW_ID") },
    }
    match keep_wez {
        Some(v) => unsafe { std::env::set_var("WEZTERM_VERSION", v) },
        None => unsafe { std::env::remove_var("WEZTERM_VERSION") },
    }
    match keep_prog {
        Some(v) => unsafe { std::env::set_var("TERM_PROGRAM", v) },
        None => unsafe { std::env::remove_var("TERM_PROGRAM") },
    }
}
#[test]
fn run_cli_with_no_paths_is_not_an_error() {
    // The CLI layer requires >=1; the parser is the only guard, so the empty
    // case is a no-op rather than a crash.
    let (shown, failed) = view_lines(&[]);
    assert!(shown.is_empty() && failed.is_empty());
}
