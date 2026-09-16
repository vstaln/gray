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

#[path = "fence_tests.rs"]
#[cfg(test)]
mod tests;
