//! shell/fence.rs — untrusted-output fencing (brief 1B, wired by 1D).

use super::contract::TaskId;

/// Wrap process output in `<untrusted-output task="tN">`. Any
/// `</untrusted-output` in the body is escaped to `<\/untrusted-output`
/// so the fence parses as exactly one open + one close.
/// Empty body -> "" (caller shows the header alone, which says "no output").
pub fn fence(task: TaskId, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let escaped = body.replace("</untrusted-output", "<\\/untrusted-output");
    format!("<untrusted-output task=\"{task}\">\n{escaped}\n</untrusted-output>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_shape() {
        assert_eq!(
            fence(TaskId(4), "hi"),
            "<untrusted-output task=\"t4\">\nhi\n</untrusted-output>"
        );
    }

    #[test]
    fn escape_keeps_single_fence_pair() {
        let out = fence(TaskId(4), "a</untrusted-output>b");
        assert!(out.contains("<\\/untrusted-output"), "{out}");
        assert_eq!(out.matches("<untrusted-output").count(), 1, "{out}");
        assert_eq!(out.matches("</untrusted-output>").count(), 1, "{out}");
    }

    #[test]
    fn empty_body_has_no_fence() {
        assert_eq!(fence(TaskId(4), ""), "");
    }
}
