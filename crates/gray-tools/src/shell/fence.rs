//! shell/fence.rs: untrusted-output fencing.

/// Wrap process output in <untrusted-output>. Any
/// closer in the body is escaped so the fence parses as exactly one
/// open + one close.
/// Trailing newlines are stripped first: the fence adds its own, so a body
/// ending in `\n` would otherwise render a phantom blank line and disagree
/// with the header's line count. (The header still reports the true count.)
/// Empty body -> "" (caller shows the header alone, which says "no output").
pub fn fence(body: &str) -> String {
    let trimmed = body.trim_end_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    let escaped = trimmed.replace("</untrusted-output>", "<\\/untrusted-output>");
    format!("<untrusted-output>\n{escaped}\n</untrusted-output>")
}

#[cfg(test)]
mod tests {
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
}
