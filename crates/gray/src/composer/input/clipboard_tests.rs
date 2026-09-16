use super::*;

#[test]
fn normalize_paste_handles_crlf_and_lone_cr() {
    assert_eq!(normalize_paste("a\r\nb"), "a\nb");
    assert_eq!(normalize_paste("a\rb"), "a\nb");
    assert_eq!(normalize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
    assert_eq!(normalize_paste("plain\ntext"), "plain\ntext");
    assert_eq!(normalize_paste(""), "");
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn clipboard_chain_reads_through_fake_xclip() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = dir.path().join("xclip");
    std::fs::write(&shim, "#!/bin/sh\nprintf 'pasted-text'").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let paths = dir.path().to_string_lossy().into_owned();
    // wl-paste is absent so the chain must fall through to the xclip shim.
    assert_eq!(
        read_system_clipboard_text_with_paths(&paths),
        Some("pasted-text".to_string())
    );
}

#[test]
fn clipboard_chain_empty_path_gives_none_fast() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = dir.path().to_string_lossy().into_owned();
    assert_eq!(read_system_clipboard_text_with_paths(&paths), None);
}
