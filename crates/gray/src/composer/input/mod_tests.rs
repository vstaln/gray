use super::*;

#[test]
fn strip_escape_sequences_removes_paste_markers_and_keeps_text() {
    assert_eq!(
        strip_escape_sequences("\u{1b}[200~hello world\u{1b}[201~"),
        "hello world"
    );

    assert_eq!(
        strip_escape_sequences("\u{1b}]0;title\u{7}a\u{1b}[2Kb"),
        "ab"
    );

    assert_eq!(
        strip_escape_sequences("fn main() {\n\tprintln!(\"hi\");\n}"),
        "fn main() {\n\tprintln!(\"hi\");\n}"
    );

    assert_eq!(strip_escape_sequences("a\u{1b}b"), "ab");
    assert_eq!(strip_escape_sequences("\u{1b}[201~"), "");
}

#[cfg(unix)]
#[test]
fn pty_slave_reports_dead_only_after_master_closes() {
    unsafe {
        let mut master = 0;
        let mut slave = 0;
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
        assert!(!fd_dead(slave));
        assert!(!fd_dead_confirmed(slave));
        libc::close(master);
        assert!(fd_dead(slave));
        assert!(fd_dead_confirmed(slave));
        libc::close(slave);
    }
}
