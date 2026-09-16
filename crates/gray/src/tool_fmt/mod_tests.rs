use super::*;

fn row_text(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn bash_args(cmd: &str) -> serde_json::Value {
    serde_json::json!({"command": cmd})
}

#[test]
fn bash_heredoc_write_renders_code_then_output() {
    // Bash-only harness: file writes arrive as heredocs whose bodies the
    // one-line header never shows. The box must carry the written code
    // (like the `write` arm) plus the command output.
    let cmd = "cat <<'EOF' > pid.rs\nfn main() {}\nEOF";
    let out = "exit 0 · patched pid.rs";
    let lines =
        format_tool_result_lines_with_context("bash", Some(&bash_args(cmd)), out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("fn main()"), "written code missing: {text:?}");
    assert!(text.contains("patched pid.rs"), "output missing: {text:?}");
    assert!(text.contains("1 | "), "code not numbered: {text:?}");
}

#[test]
fn bash_python_heredoc_without_redirect_renders_body() {
    let cmd = "python3 - <<'PY'\nprint(1)\nPY";
    let lines =
        format_tool_result_lines_with_context("bash", Some(&bash_args(cmd)), "", false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("print(1)"), "ran code missing: {text:?}");
}

#[test]
fn bash_unclosed_heredoc_falls_back_to_output_only() {
    let cmd = "cat <<'EOF'\npartial";
    let lines =
        format_tool_result_lines_with_context("bash", Some(&bash_args(cmd)), "hi", false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("hi"), "output missing: {text:?}");
}

#[test]
fn bash_plain_output_is_numbered() {
    let lines = format_tool_result_lines_with_context("bash", None, "hello\nworld", false, None);
    assert_eq!(lines.len(), 2);
    assert!(
        row_text(&lines[0]).contains("1 | "),
        "got {:?}",
        row_text(&lines[0])
    );
    assert!(row_text(&lines[0]).contains("hello"));
    assert!(row_text(&lines[1]).contains("2 | "));
}

#[test]
fn bash_shell_fence_is_stripped_for_display() {
    let out = "<untrusted-output task=\"t72\">\nexit 0 · hi\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(!text.contains("untrusted-output"), "fence leaked: {text:?}");
    assert!(text.contains("exit 0 · hi"), "body lost: {text:?}");
}

#[test]
fn bash_half_fence_from_truncation_still_strips() {
    // Budget truncation may keep only one half of the fence.
    let open_only = "<untrusted-output task=\"t3\">\npartial body";
    let text: String = format_tool_result_lines_with_context("bash", None, open_only, false, None)
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("untrusted-output"), "got {text:?}");

    let close_only = "partial body\n</untrusted-output>";
    let text: String = format_tool_result_lines_with_context("bash", None, close_only, false, None)
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("untrusted-output"), "got {text:?}");
    assert!(text.contains("partial body"), "body lost: {text:?}");
}

#[test]
fn bash_header_plus_fence_is_stripped_for_display() {
    // Real bash shape is header + fence(body), not fence-first:
    // `exit …` on line 1, opener on line 2, body, closer last.
    // The old stripper only handled fence-first and leaked the opener.
    let out = "exit 0 \u{00b7} 0.0s \u{00b7} 1 lines \u{00b7} log ~/.gray/shell/nosession/t1.log\n<untrusted-output task=\"t1\">\nhi\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        !rendered.contains("untrusted-output"),
        "fence leaked: {rendered:?}"
    );
    assert!(rendered.contains("exit 0"), "header lost: {rendered:?}");
    assert!(rendered.contains("hi"), "body lost: {rendered:?}");
}

#[test]
fn bash_promotion_trailer_keeps_body_but_drops_fence() {
    // Promotion shape: header, fence, trailer after the closer.
    // Closer is not the last line, so end-anchored stripping missed it.
    let out = "still running after 1s \u{2192} promoted to background as t6 \u{00b7} pid 4242 \u{00b7} log ~/.gray/shell/nosession/t6.log\n<untrusted-output task=\"t6\">\ntick 1\n</untrusted-output>\nshell_output(task_id=\"t6\", from_offset=12) for the rest \u{00b7} next_offset=12";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        !rendered.contains("untrusted-output"),
        "fence leaked: {rendered:?}"
    );
    assert!(rendered.contains("tick 1"), "body lost: {rendered:?}");
    assert!(
        rendered.contains("still running"),
        "header lost: {rendered:?}"
    );
    assert!(
        rendered.contains("shell_output"),
        "trailer lost: {rendered:?}"
    );
}

#[test]
fn bash_escaped_closer_in_body_is_restored() {
    // Body containing a literal closer is escaped as `<\\/` by fence();
    // display should restore the original text, not leak plumbing.
    let out = "exit 0 \u{00b7} 0.0s \u{00b7} 2 lines \u{00b7} log ~/.gray/shell/nosession/t1.log\n<untrusted-output task=\"t1\">\nfirst\n<\\/untrusted-output> tail\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        !rendered.contains("untrusted-output task="),
        "fence leaked: {rendered:?}"
    );
    assert!(rendered.contains("tail"), "body lost: {rendered:?}");
}

#[test]
fn bash_empty_output_returns_nothing() {
    assert!(format_tool_result_lines_with_context("bash", None, "   \n  ", false, None).is_empty());
}

#[test]
fn bash_caps_long_output_like_code_blocks() {
    let out: String = (1..=60)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = format_tool_result_lines_with_context("bash", None, &out, false, None);
    let first_rows: Vec<String> = lines.iter().map(row_text).collect();
    // 18 head + 1 omission marker + 6 tail
    assert_eq!(lines.len(), 25);
    assert!(
        first_rows.iter().any(|r| r.contains("… +36 lines")),
        "must collapse, got {first_rows:?}"
    );
}

#[test]
fn ls_caps_home_dir_flooding() {
    // Screenshot case: `ls ~` with 180 entries dumped literally
    // everything into the TUI transcript.
    let out: String = (1..=180)
        .map(|i| format!("entry-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = format_tool_result_lines_with_context("ls", None, &out, false, None);
    assert_eq!(lines.len(), 25, "must cap, got {}", lines.len());
}

#[test]
fn bash_json_is_pretty_printed() {
    let lines =
        format_tool_result_lines_with_context("bash", None, r#"{"a":1,"b":[1,2]}"#, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(lines.len() > 1);
    assert!(text.contains("\"a\": 1"), "got {text:?}");
}

#[test]
fn bash_html_is_split_one_tag_per_line() {
    let html = "<!DOCTYPE html><html><head><title>Vercel Security</title></head><body><p>hi</p></body></html>";
    let lines = format_tool_result_lines_with_context("bash", None, html, false, None);
    assert!(lines.len() > 1);
    for l in &lines {
        assert!(
            !row_text(l).contains("><"),
            "still minified: {:?}",
            row_text(l)
        );
    }
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("<title>Vercel Security</title>"));
}

#[test]
fn render_code_block_608_line_file_has_unique_gutters() {
    // Screenshot case: a 608-line Write box painted head rows on the
    // left and tail rows AGAIN on the right. Gutter numbers must each
    // appear exactly once: 1..=18 head, 603..=608 tail.
    let mut src: Vec<String> = vec![
        "<!DOCTYPE html>".to_string(),
        "<html lang=\"en\">".to_string(),
        "<head>".to_string(),
        "<meta charset=\"UTF-8\" />".to_string(),
        "<title>HorseTinder</title>".to_string(),
        "<style>".to_string(),
    ];
    while src.len() < 602 {
        let i = src.len();
        src.push(format!("  .filler-{i}{{color:#ff00{i:04};}}"));
    }
    src.push("  function rebuildDeck(){ renderMatches(); }".to_string());
    src.push("  document.addEventListener(\"x\", rebuildDeck);".to_string());
    src.push("  }})();".to_string());
    src.push("  </script>".to_string());
    src.push("</body>".to_string());
    src.push("</html>".to_string());
    assert_eq!(src.len(), 608);
    let content = src.join("\n");
    let lines = render_code_block(&content, None);
    assert_eq!(lines.len(), 25, "18 head + marker + 6 tail");
    let mut gutters: Vec<usize> = Vec::new();
    for l in &lines {
        let t: String = row_text(l);
        let num = t.split('|').next().unwrap_or("").trim();
        if num.is_empty() || num.starts_with('…') {
            continue;
        }
        gutters.push(num.parse::<usize>().expect("gutter must be numeric"));
    }
    let mut expected: Vec<usize> = (1..=18).chain(603..=608).collect();
    let mut got = gutters.clone();
    got.sort_unstable();
    expected.sort_unstable();
    assert_eq!(got, expected, "duplicated or missing gutters: {gutters:?}");
}

#[test]
fn render_code_block_cap_unchanged() {
    let content: String = (1..=50)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = render_code_block(&content, None);
    // 18 head + 1 omission marker + 6 tail
    assert_eq!(lines.len(), 25);
    assert!(row_text(&lines[18]).contains("… +26 lines"));
}
