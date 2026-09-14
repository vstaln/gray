//! shell/fence.rs: untrusted-output fencing.

/// Wrap process output in <untrusted-output>. Any
/// closer in the body is escaped so the fence parses as exactly one
/// open + one close.
/// Empty body -> "" (caller shows the header alone, which says "no output").
pub fn fence(body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let escaped = body.replace("</untrusted-output>", "<\\/untrusted-output>");
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
}
