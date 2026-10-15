use super::*;

#[cfg(unix)]
#[test]
fn file_url_percent_decoding() {
    assert_eq!(
        decoded_paste_path("file:///tmp/my%20photo.png"),
        "/tmp/my photo.png"
    );
    assert_eq!(
        decoded_paste_path("file://localhost/tmp/my%20photo.png"),
        "/tmp/my photo.png"
    );
    assert_eq!(
        decoded_paste_path("\"file:///tmp/a%20b.png\""),
        "/tmp/a b.png"
    );
}

#[test]
fn raw_paths_pass_through() {
    assert_eq!(decoded_paste_path("/tmp/100%.png"), "/tmp/100%.png");
    assert_eq!(decoded_paste_path("/tmp/plain.png"), "/tmp/plain.png");
}

#[test]
fn attach_image_appends_marker_and_pushes_caret_after_it() {
    let mut tui = Tui::test_tui(80, 30);
    tui.textarea.set_text("prefix suffix");
    tui.textarea.set_cursor(7);
    let path = PathBuf::from("/tmp/pic one.png");
    let marker = crate::repl::attachments::attachment_marker(&path);
    attach_image(&mut tui, path.clone());
    assert_eq!(tui.textarea.text(), format!("prefix suffix{marker}"));
    assert_eq!(tui.textarea.cursor(), tui.textarea.text().len());
    assert_eq!(tui.attachments, vec![path]);
}

#[test]
fn attach_image_appends_each_paste_in_order() {
    let mut tui = Tui::test_tui(80, 30);
    tui.textarea.set_text("middle ");
    tui.textarea.set_cursor(7);
    let pa = PathBuf::from("/tmp/a one.png");
    let pb = PathBuf::from("/tmp/b.png");
    let ma = crate::repl::attachments::attachment_marker(&pa);
    let mb = crate::repl::attachments::attachment_marker(&pb);
    attach_image(&mut tui, pa.clone());
    tui.textarea.insert_str("tail");
    attach_image(&mut tui, pb.clone());
    assert_eq!(tui.textarea.text(), format!("middle {ma}tail{mb}"));
    let end = tui.textarea.text().len();
    tui.textarea.replace_range(end - mb.len()..end, "");
    super::sync_attachment_markers(&mut tui);
    assert_eq!(tui.attachments, vec![pa]);
}
