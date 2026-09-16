use super::*;

#[test]
fn basic_shape() {
    assert_eq!(fence("hi"), "<untrusted-output>\nhi\n</untrusted-output>");
}

#[test]
fn escape_keeps_single_fence_pair() {
    let out = fence("a</untrusted-output>b");
    assert!(out.contains("<\\/untrusted-output>"), "{out}");
    assert_eq!(out.matches("<untrusted-output").count(), 1, "{out}");
    assert_eq!(out.matches("</untrusted-output>").count(), 1, "{out}");
}

#[test]
fn empty_body_has_no_fence() {
    assert_eq!(fence(""), "");
}

#[test]
fn trailing_newlines_do_not_add_phantom_blank_line() {
    // `hello world\nfoo bar baz\n` must render 2 lines, not 3: the
    // fence supplies the closing newline itself.
    assert_eq!(
        fence("hello world\nfoo bar baz\n"),
        "<untrusted-output>\nhello world\nfoo bar baz\n</untrusted-output>"
    );
    assert_eq!(fence("hi\n\n"), fence("hi"));
}
