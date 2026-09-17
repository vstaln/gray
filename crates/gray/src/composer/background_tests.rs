use super::*;
use base64::Engine;

fn png() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 64]))
        .save_with_format(file.path(), image::ImageFormat::Png)
        .unwrap();
    file
}

#[test]
fn upload_once_replace_placement_on_resize_and_delete_owned_images() {
    let file = png();
    let mut background = Background::load(file.path()).unwrap();
    let mut output = Vec::new();
    background.draw(&mut output, 80, 24).unwrap();
    let first = String::from_utf8(output.clone()).unwrap();
    assert!(first.contains("a=t,f=100"));
    assert!(first.contains("c=80,r=24,z=-1,C=1"));
    assert!(first.ends_with("\x1b8"));
    output.clear();
    background.draw(&mut output, 100, 30).unwrap();
    let second = String::from_utf8(output.clone()).unwrap();
    assert!(
        !second.contains("a=t"),
        "resize must not upload image again"
    );
    assert!(second.contains("c=100,r=30"));
    output.clear();
    background.hide(&mut output).unwrap();
    assert!(
        String::from_utf8(output.clone())
            .unwrap()
            .contains("a=d,d=I,i=")
    );
    assert!(!String::from_utf8(output.clone()).unwrap().contains("d=A"));
    output.clear();
    background.draw(&mut output, 80, 24).unwrap();
    assert!(String::from_utf8(output).unwrap().contains("a=t,f=100"));
}

#[test]
fn invalid_files_fail_before_any_terminal_output() {
    let file = tempfile::NamedTempFile::new().unwrap();
    assert!(Background::load(file.path()).is_err());
    std::fs::write(file.path(), b"not PNG\x1b[2J").unwrap();
    assert!(Background::load(file.path()).is_err());
    assert!(Background::load(std::path::Path::new("relative.png")).is_err());
    assert!(Background::load(file.path().parent().unwrap()).is_err());
    file.as_file().set_len(9 * 1024 * 1024).unwrap();
    assert!(Background::load(file.path()).is_err());
}

#[test]
fn payload_round_trips_and_small_terminal_does_not_paint() {
    let file = png();
    let mut background = Background::load(file.path()).unwrap();
    let mut output = Vec::new();
    background.draw(&mut output, 0, 0).unwrap();
    assert!(output.is_empty());
    background.draw(&mut output, 1, 1).unwrap();
    let wire = String::from_utf8(output).unwrap();
    let payload = wire
        .split(';')
        .nth(1)
        .unwrap()
        .split("\x1b\\")
        .next()
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap();
    assert_eq!(bytes, std::fs::read(file.path()).unwrap());
}
