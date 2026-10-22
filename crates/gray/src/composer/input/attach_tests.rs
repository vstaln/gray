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

#[cfg(target_os = "windows")]
#[test]
#[ignore = "touches the real system clipboard"]
fn windows_clipboard_image_attaches() {
    // Put a real bitmap on the clipboard, then drive the full paste path.
    let set = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
            // SetImage sometimes throws "clipboard operation did not succeed"
            // even though the data lands — verify with ContainsImage instead.
            "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
             $b=New-Object System.Drawing.Bitmap 8,8; \
             for($i=0; $i -lt 10; $i++){ \
             try { [System.Windows.Forms.Clipboard]::SetImage($b) } catch {}; \
             try { if([System.Windows.Forms.Clipboard]::ContainsImage()){ exit 0 } } catch {}; \
             Start-Sleep -Milliseconds 100 }; \
             exit 1",
        ])
        .output()
        .expect("spawn powershell");
    assert!(set.status.success(), "SetImage failed: {:?}", set);

    let mut tui = Tui::test_tui(80, 30);
    assert!(try_attach_clipboard_image(&mut tui));
    assert_eq!(tui.attachments.len(), 1);
    let img = image::open(&tui.attachments[0]).expect("decode pasted png");
    assert_eq!((img.width(), img.height()), (8, 8));

    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Clipboard]::Clear()",
        ])
        .output()
        .ok();
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
