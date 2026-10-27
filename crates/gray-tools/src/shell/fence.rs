pub fn fence(body: &str) -> String {
    let trimmed = body.trim_end_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    let escaped = trimmed.replace("</untrusted-output>", "<\\/untrusted-output>");
    format!("<untrusted-output>\n{escaped}\n</untrusted-output>")
}
