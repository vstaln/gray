use super::*;

fn row_text(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn live_row(name: &str, args_so_far: &str) -> Line<'static> {
    format_live_tool_header(name, args_so_far, None, None, None)
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn bash_args(cmd: &str) -> serde_json::Value {
    serde_json::json!({"command": cmd})
}

#[test]
fn bash_heredoc_write_renders_code_then_output() {
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

    assert_eq!(lines.len(), 25);
    assert!(
        first_rows.iter().any(|r| r.contains("… +36 lines")),
        "must collapse, got {first_rows:?}"
    );
}

#[test]
fn ls_caps_home_dir_flooding() {
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

    assert_eq!(lines.len(), 25);
    assert!(row_text(&lines[18]).contains("… +26 lines"));
}

#[test]
fn live_header_empty_args_shows_name_only() {
    let text = row_text(&live_row("bash", ""));
    assert!(text.contains("bash"), "got {text:?}");
}

#[test]
fn live_header_full_json_matches_final_header() {
    let raw = r#"{"command":"cargo test -p gray"}"#;
    let v: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        row_text(&live_row("bash", raw)),
        row_text(&format_tool_call_header("bash", &v, None, None)),
    );
}

#[test]
fn live_header_partial_bash_streams_command() {
    let text = row_text(&live_row("bash", r#"{"command":"cargo test --li"#));
    assert!(text.contains("cargo test"), "got {text:?}");
}

#[test]
fn live_header_partial_read_streams_path() {
    let text = row_text(&live_row("read", r#"{"path":"src/ma"#));
    assert!(text.contains("src/ma"), "got {text:?}");
}

#[test]
fn live_header_garbage_never_panics_and_names_tool() {
    for raw in ["{{{", "\"unclosed", "   ", ",,,"] {
        let text = row_text(&live_row("grep", raw));
        assert!(text.contains("grep"), "got {text:?} for {raw:?}");
    }
}

#[test]
fn live_header_huge_raw_still_streams_prefix() {
    let big = format!(r#"{{"command":"{}"#, "x".repeat(9000));
    let full = format!(r#"{{"command":"{}"}}"#, "x".repeat(9000));
    let v: serde_json::Value = serde_json::from_str(&full).unwrap();
    assert_eq!(
        row_text(&live_row("bash", &big)),
        row_text(&format_tool_call_header("bash", &v, None, None)),
    );
}

#[test]
fn truncate_cmd_never_splits_a_multibyte_char() {
    let head = "114.3k/300k · 0.0% cache ";
    assert_eq!(head.len(), 26);
    let pad = "x".repeat(53);
    let cmd = format!("{head}{pad}\u{1f7e0} High · up to 553c7cd\nsecond line");
    let cut = super::truncate_cmd(&cmd);

    assert!(cmd.is_char_boundary(cut.len()), "cut mid-char");
    assert!(cmd.starts_with(cut), "cut is not a prefix: {cut:?}");
    assert!(
        crate::text_width::display_width(cut) <= 80,
        "cut not within 80 cells: {} cells",
        crate::text_width::display_width(cut)
    );

    assert!(!cut.contains("second line"));
}

#[test]
fn truncate_cmd_ascii_still_cuts_at_exactly_80() {
    let cmd = format!("{}\nrest", "a".repeat(200));
    let cut = super::truncate_cmd(&cmd);
    assert_eq!(cut.len(), 80, "ascii cut moved: {cut:?}");
    assert_eq!(cut, "a".repeat(80));
}

#[test]
fn truncate_cmd_short_and_wide_text_is_untouched() {
    assert_eq!(super::truncate_cmd("echo hi"), "echo hi");

    let wide = "café ☕ ok";
    assert_eq!(super::truncate_cmd(wide), wide);

    let cjk = "日本語".repeat(40);
    let cut = super::truncate_cmd(&cjk);
    assert_eq!(
        crate::text_width::display_width(cut),
        80,
        "wide cut: {cut:?}"
    );
}

#[test]
fn live_bash_header_survives_multibyte_command() {
    let head = "Merge Risk: _";
    let pad = "y".repeat(64);
    let partial = format!("{{\"command\":\"{head}{pad}\u{1f7e0} High");
    let line = live_row("bash", &partial);
    let text = row_text(&line);
    assert!(text.contains("Merge Risk:"), "{text}");
    assert!(crate::text_width::display_width(&text) > 0);
}

#[test]
fn the_cards_header_drops_the_log_path_but_keeps_later_fields() {
    let out = "exit 143 (SIGTERM) \u{00b7} 2.1s \u{00b7} 0 lines \u{00b7} log ~/.gray/shell/s/x.log \u{00b7} no output\n<untrusted-output task=\"t1\">\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(!rendered.contains(".gray/shell"), "{rendered:?}");
    assert!(
        rendered.contains("0 lines \u{00b7} no output"),
        "{rendered:?}"
    );
    let last = "exit 0 \u{00b7} 0.0s \u{00b7} 1 lines \u{00b7} log ~/.gray/shell/s/t1.log\n<untrusted-output task=\"t1\">\nhi\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, last, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        rendered.contains("exit 0 \u{00b7} 0.0s \u{00b7} 1 lines"),
        "{rendered:?}"
    );
    assert!(!rendered.contains("log ~"), "{rendered:?}");
}

#[test]
fn a_cancelled_runs_second_line_header_drops_the_log_path_too() {
    let out = "cancelled by user after 2.1s\nexit 143 (SIGTERM) (terminated) \u{00b7} 2.1s \u{00b7} 0 lines \u{00b7} log ~/.gray/shell/s/x.log \u{00b7} no output\n<untrusted-output task=\"t1\">\n</untrusted-output>";
    let lines = format_tool_result_lines_with_context("bash", None, out, false, None);
    let rendered: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(!rendered.contains(".gray/shell"), "{rendered:?}");
    assert!(rendered.contains("cancelled by user"), "{rendered:?}");
}

#[test]
fn web_search_renders_a_verb_not_the_wire_name() {
    let args = serde_json::json!({"query": "LibreWolf AppImage download", "max_results": 5});
    let text = row_text(&format_tool_call_header("web_search", &args, None, None));
    assert!(!text.contains("web_search"), "wire name leaked: {text:?}");
    assert!(text.contains("Searched"), "verb missing: {text:?}");
    assert!(
        text.contains("LibreWolf AppImage download"),
        "query missing: {text:?}"
    );
    assert!(
        !text.contains("max_results"),
        "non-headline arg leaked: {text:?}"
    );
}

#[test]
fn web_fetch_renders_a_verb_with_the_url() {
    let args =
        serde_json::json!({"url": "https://librewolf.net/installation/linux/", "max_chars": 8000});
    let text = row_text(&format_tool_call_header("web_fetch", &args, None, None));
    assert!(!text.contains("web_fetch"), "wire name leaked: {text:?}");
    assert!(text.contains("Fetched"), "verb missing: {text:?}");
    assert!(
        text.contains("https://librewolf.net/installation/linux/"),
        "url missing: {text:?}"
    );
}

#[test]
fn web_search_body_renders_like_bash() {
    let out = r#"{"provider":"bing","results":[{"title":"t","url":"https://example.com","snippet":"hi"}]}"#;
    let lines = format_tool_result_lines_with_context("web_search", None, out, false, None);
    assert!(
        !lines.is_empty(),
        "web_search body must render, got header-only"
    );
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains("1 | "),
        "body not numbered like bash: {text:?}"
    );
    assert!(text.contains("example.com"), "result lost: {text:?}");
    assert!(!text.contains('{'), "bare json leaked: {text:?}");
}

#[test]
fn web_fetch_body_renders_like_bash() {
    let out = "line one\nline two";
    let lines = format_tool_result_lines_with_context("web_fetch", None, out, false, None);
    assert!(
        !lines.is_empty(),
        "web_fetch body must render, got header-only"
    );
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains("1 | "),
        "body not numbered like bash: {text:?}"
    );
    assert!(text.contains("line one"), "body lost: {text:?}");
}

#[test]
fn web_search_caps_long_output_like_bash() {
    let out: String = (1..=60)
        .map(|i| format!("result {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = format_tool_result_lines_with_context("web_search", None, &out, false, None);
    assert_eq!(lines.len(), 25, "must cap like bash, got {}", lines.len());
}

#[test]
fn web_search_results_render_readable_and_decode_entities() {
    let out = r#"{"provider":"bing","results":[
        {"title":"First title","url":"https://a.example","snippet":"Sep 22, 2026 &nbsp;&#0183;&#32;LibreWolf is a browser"},
        {"title":"Second title","url":"https://b.example","snippet":"plain snippet"}
    ]}"#;
    let lines = format_tool_result_lines_with_context("web_search", None, out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    for needle in [
        "First title",
        "https://a.example",
        "Second title",
        "https://b.example",
        "\u{00b7}",
    ] {
        assert!(text.contains(needle), "missing {needle:?}: {text:?}");
    }
    for banned in ["&#0183;", "\"provider\"", "\"results\"", "{"] {
        assert!(!text.contains(banned), "leaked {banned:?}: {text:?}");
    }
}

#[test]
fn web_search_unrecognized_json_falls_back_to_pretty() {
    let out = r#"{"foo":1}"#;
    let lines = format_tool_result_lines_with_context("web_search", None, out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("\"foo\""), "pretty json lost: {text:?}");
}

#[test]
fn web_search_empty_results_says_no_results() {
    let out = r#"{"provider":"bing","results":[]}"#;
    let lines = format_tool_result_lines_with_context("web_search", None, out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("no results"), "got: {text:?}");
}

#[test]
fn web_fetch_decodes_entities_and_keeps_unknown_literal() {
    let out = "don&#x27;t &quot;x&quot; &#27;";
    let lines = format_tool_result_lines_with_context("web_fetch", None, out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("don't \"x\""), "entities undecoded: {text:?}");
    assert!(
        text.contains("&#27;"),
        "control-decoding ref must stay literal: {text:?}"
    );
}

#[test]
fn web_fetch_giant_line_is_char_truncated_not_row_flooded() {
    let out = "word ".repeat(1000);
    let lines = format_tool_result_lines_with_context("web_fetch", None, &out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("… +"), "no truncation marker: {text:?}");
    assert!(text.contains("chars"), "no char count: {text:?}");
    assert!(
        lines.len() < 40,
        "giant fetch flooded the card: {} rows",
        lines.len()
    );
}

#[test]
fn web_search_many_results_hit_the_omission_marker() {
    let results: Vec<String> = (1..=20)
        .map(|i| {
            format!(r#"{{"title":"title {i}","url":"https://r{i}.example","snippet":"s{i}"}}"#)
        })
        .collect();
    let out = format!(r#"{{"provider":"bing","results":[{}]}}"#, results.join(","));
    let lines = format_tool_result_lines_with_context("web_search", None, &out, false, None);
    let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("… +"), "no omission marker: {text:?}");
}

#[test]
fn an_unknown_tool_humanizes_its_name_and_shows_args() {
    let args = serde_json::json!({"content": "hello channel"});
    let text = row_text(&format_tool_call_header(
        "my_channel_send",
        &args,
        None,
        None,
    ));
    assert!(
        !text.contains("my_channel_send"),
        "wire name leaked: {text:?}"
    );
    assert!(text.contains("My Channel Send"), "name missing: {text:?}");
    assert!(text.contains("hello channel"), "arg missing: {text:?}");
}

#[test]
fn plugin_preview_path_renders_first_string_at_that_path() {
    let args = serde_json::json!({"document": {"title": "  ## Hello  "}});
    let text = row_text(&format_tool_call_header(
        "some_surface_tool",
        &args,
        None,
        Some("document.title"),
    ));
    assert!(
        text.contains("Some Surface Tool"),
        "headline missing: {text:?}"
    );
    assert!(text.contains("## Hello"), "preview missing: {text:?}");
    assert!(
        !text.contains("preview="),
        "preview key leaked into dump: {text:?}"
    );
}

#[test]
fn plugin_preview_path_missing_falls_back_to_generic_dump() {
    let args = serde_json::json!({"document": {}});
    let text = row_text(&format_tool_call_header(
        "some_surface_tool",
        &args,
        None,
        Some("document.title"),
    ));
    assert!(
        text.contains("Some Surface Tool"),
        "headline missing: {text:?}"
    );
    assert!(
        text.contains("document="),
        "fallback dump missing: {text:?}"
    );
}

#[test]
fn unknown_tool_surfaces_first_string_arg_and_flags_the_rest() {
    let args = serde_json::json!({"all": true, "limit": 10});
    let text = row_text(&format_tool_call_header("recall", &args, None, None));
    assert!(text.contains("recall"), "headline missing: {text:?}");
    assert!(text.contains("all"), "flag missing: {text:?}");
    assert!(text.contains("limit=10"), "scalar missing: {text:?}");
    assert!(!text.contains("all=true"), "raw bool leaked: {text:?}");

    let args = serde_json::json!({"query": "wifi password", "all": true, "limit": 5});
    let text = row_text(&format_tool_call_header("recall", &args, None, None));
    assert!(
        text.contains("\"wifi password\""),
        "query preview missing: {text:?}"
    );
    assert!(text.contains("all"), "flag missing: {text:?}");
    assert!(text.contains("limit=5"), "scalar missing: {text:?}");
}

#[test]
fn preview_at_walks_object_keys_only() {
    let args = serde_json::json!({"document": {"title": "  Hi  "}});
    assert_eq!(preview_at(&args, "document.title").as_deref(), Some("Hi"));
    assert!(preview_at(&args, "document.missing").is_none());
    assert!(preview_at(&args, "document.title.deeper").is_none());
    assert!(preview_at(&args, "").is_none());
    assert!(preview_at(&args, "  ").is_none());
    let ws = serde_json::json!({"document": {"title": "   "}});
    assert!(preview_at(&ws, "document.title").is_none());
}

#[test]
fn unknown_tools_humanize_and_labels_override() {
    let args = serde_json::json!({"foo": "bar"});
    let text = row_text(&format_tool_call_header(
        "my_custom_tool",
        &args,
        None,
        None,
    ));
    assert!(text.contains("My Custom Tool"), "not humanized: {text:?}");
    assert!(
        !text.contains("my_custom_tool"),
        "wire name leaked: {text:?}"
    );
    let labeled = with_tool_label(&args, Some("Notify Owner"));
    let labeled_text = row_text(&format_tool_call_header(
        "my_custom_tool",
        &labeled,
        None,
        None,
    ));
    assert!(
        labeled_text.contains("Notify Owner"),
        "label ignored: {labeled_text:?}"
    );
    assert!(
        !labeled_text.contains("label="),
        "label leaked into preview: {labeled_text:?}"
    );
}

#[test]
fn live_header_streams_plugin_scalars() {
    let text = row_text(&live_row("web_search", r#"{"query":"LibreWolf AppI"#));
    assert!(text.contains("LibreWolf AppI"), "got {text:?}");

    let text = row_text(&live_row("my_surface_tool", r#"{"path":"/tmp/gra"#));
    assert!(text.contains("/tmp/gra"), "got {text:?}");
}

#[test]
fn a_cut_bash_header_ends_in_an_ellipsis() {
    let long = serde_json::json!({"command": format!("seq 20 | xargs sh -c '{}' | sort | uniq -c", "x".repeat(80))});
    let text = row_text(&format_tool_call_header("bash", &long, None, None));
    assert!(text.ends_with('\u{2026}'), "{text:?}");
    let multi = serde_json::json!({"command": "cd x\nmake"});
    assert!(
        row_text(&format_tool_call_header("bash", &multi, None, None)).ends_with("cd x\u{2026}")
    );
    let short = serde_json::json!({"command": "make"});
    assert!(row_text(&format_tool_call_header("bash", &short, None, None)).ends_with("Ran make"));
}

fn header(args: serde_json::Value) -> String {
    row_text(&format_tool_call_header("bash", &args, None, None))
}

fn body(args: serde_json::Value, out: &str) -> String {
    format_tool_result_lines_with_context("bash", Some(&args), out, false, None)
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_run_header_shows_the_command_without_its_setup() {
    let text = header(
        serde_json::json!({"command": "cd ~/wt/provider-chat && CARGO_BUILD_BUILD_DIR=target nice -n 19 ionice -c3 flock /tmp/l cargo check -p gray-provider --tests"}),
    );
    assert!(
        text.ends_with("Ran cargo check -p gray-provider --tests \u{00b7} ~/wt/provider-chat"),
        "{text:?}"
    );

    let risky = header(serde_json::json!({"command": "rm -rf build && cargo check"}));
    assert!(
        risky.ends_with("Ran rm -rf build && cargo check"),
        "{risky:?}"
    );
}

#[test]
fn each_action_gets_its_own_verb() {
    let cases = [
        (
            serde_json::json!({"command": "cargo test", "background": true}),
            "Started cargo test \u{00b7} in background",
        ),
        (
            serde_json::json!({"action": "output", "job_id": "cargo-check"}),
            "Checked cargo-check",
        ),
        (
            serde_json::json!({"action": "status", "job_id": "cargo-check", "wait_ms": 30000}),
            "Waited on cargo-check \u{00b7} up to 30s",
        ),
        (
            serde_json::json!({"action": "output", "job_id": "cargo-check", "wait_ms": 900000}),
            "Waited on cargo-check \u{00b7} up to 15m",
        ),
        (
            serde_json::json!({"action": "cancel", "job_id": "npm-test"}),
            "Stopped npm-test",
        ),
        (
            serde_json::json!({"action": "list"}),
            "Listed background jobs",
        ),
    ];
    for (args, want) in cases {
        let text = header(args);
        assert!(text.ends_with(want), "{text:?} should end with {want:?}");
        assert!(!text.contains("Ran"), "{text:?}");
    }
}

#[test]
fn old_random_job_ids_shrink_to_a_short_tag() {
    let text = header(
        serde_json::json!({"action": "output", "job_id": "bash-bbc7b00c1f4e48aa842a5ad1e7ada886"}),
    );
    assert!(text.ends_with("Checked job bbc7b0"), "{text:?}");
}

#[test]
fn every_verb_has_a_live_form() {
    for (done, live) in [
        ("Ran ", "Running "),
        ("Started ", "Starting "),
        ("Waited on ", "Waiting on "),
        ("Checked ", "Checking "),
        ("Stopped ", "Stopping "),
        ("Listed ", "Listing "),
        ("Viewed ", "Viewing "),
        ("Read ", "Reading "),
        ("Searched ", "Searching "),
        ("Skipped ", "Skipping "),
    ] {
        assert_eq!(live_bash_verb(done), Some(live));
    }
    assert_eq!(live_bash_verb("Edited "), None);
}

#[test]
fn a_yield_notice_reads_as_running_in_background() {
    let out = "still running \u{00b7} job cargo-check \u{00b7} yielded after 10s \u{00b7} timeout 900s \u{00b7} log /tmp/bash-x.log\nContinue other work. Use bash action:output/status with job_id:cargo-check and wait_ms (e.g. 30000) to await it in one call instead of polling; completion will also be reported between model rounds or on the next user turn.";
    let text = body(serde_json::json!({"command": "cargo check"}), out);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for part in [
        "running in background as cargo-check",
        "after 10s",
        "limit 15m",
    ] {
        assert!(flat.contains(part), "{part:?} missing: {text:?}");
    }
    assert!(!text.contains("Continue other work"), "{text:?}");
    assert!(!text.contains("action:output"), "{text:?}");
    assert!(!text.contains("/tmp/bash-x.log"), "{text:?}");
}

#[test]
fn a_finished_job_drops_its_id_line_and_keeps_the_output() {
    let out = "job cargo-check\nexit 0 \u{00b7} 12s \u{00b7} 2 lines\n<untrusted-output>\nerror[E0308]: mismatched types\nContinue other work is fine inside output\n</untrusted-output>";
    let text = body(
        serde_json::json!({"action": "output", "job_id": "cargo-check"}),
        out,
    );
    assert!(!text.contains("job cargo-check"), "{text:?}");
    let piped = body(
        serde_json::json!({"action": "output", "job_id": "x"}),
        "exit 0 (`head` masks earlier stages' exit; rerun without the pipe to check) \u{00b7} 1s",
    );
    assert!(
        piped.contains("(`head` masks earlier stages' exit) \u{00b7} 1s"),
        "{piped:?}"
    );
    assert!(text.contains("exit 0"), "{text:?}");
    assert!(text.contains("error[E0308]"), "{text:?}");

    assert!(
        text.contains("Continue other work is fine inside output"),
        "{text:?}"
    );
}

#[test]
fn a_snapshot_reads_plainly() {
    let out = "job cargo-check \u{00b7} running \u{00b7} elapsed 1m 3s \u{00b7} log /tmp/l.log\nLiveness: producing (2.0KB new output in the last 30s) \u{2014} still working, keep waiting or do other work.\nPartial output (snapshot):\n<untrusted-output>\nCompiling gray\n</untrusted-output>";
    let text = body(
        serde_json::json!({"action": "output", "job_id": "cargo-check", "wait_ms": 30000}),
        out,
    );
    assert!(
        text.contains("cargo-check \u{00b7} running \u{00b7} 1m 3s"),
        "{text:?}"
    );
    assert!(
        text.contains("producing (2.0KB new output in the last 30s)"),
        "{text:?}"
    );
    assert!(!text.contains("keep waiting"), "{text:?}");
    assert!(text.contains("output so far:"), "{text:?}");
    assert!(text.contains("Compiling gray"), "{text:?}");
    let stop = body(
        serde_json::json!({"action": "cancel", "job_id": "npm-test"}),
        "cancellation requested for job npm-test; use action:status/output for final result",
    );
    assert!(stop.contains("stopping npm-test"), "{stop:?}");
}

#[test]
fn an_error_lines_up_with_numbered_output() {
    let ok = format_tool_result_lines_with_context("bash", None, "exit 0 \u{00b7} 1s", false, None);
    let err = format_tool_result_lines_with_context(
        "bash",
        None,
        "unknown job in this session: x\nsecond line",
        true,
        None,
    );
    let text_col = |l: &Line<'_>, needle: &str| {
        crate::text_width::display_width(row_text(l).split(needle).next().unwrap())
    };
    assert_eq!(text_col(&err[0], "unknown"), text_col(&ok[0], "exit"));
    assert_eq!(text_col(&err[1], "second"), text_col(&ok[0], "exit"));
    assert!(row_text(&err[0]).contains('\u{2717}'));
}

#[test]
fn an_unknown_tool_renders_its_args_verbatim() {
    let args = serde_json::json!({"content": "**updated**"});
    let text = row_text(&format_tool_call_header(
        "my_channel_send",
        &args,
        None,
        None,
    ));
    assert!(text.contains("My Channel Send"), "name missing: {text:?}");
    assert!(text.contains("**updated**"), "arg missing: {text:?}");
}

#[test]
fn read_only_commands_name_what_they_looked_at() {
    for (cmd, want) in [
        (
            "cat /tmp/charts/g1_products.jpg",
            "Viewed /tmp/charts/g1_products.jpg",
        ),
        (
            "sed -n '120,180p' crates/gray/src/lib.rs",
            "Read crates/gray/src/lib.rs \u{00b7} lines 120\u{2013}180",
        ),
        (
            "head -n 5 a.rs b.rs",
            "Read a.rs, b.rs \u{00b7} first 5 lines",
        ),
        ("cd ~/gray && ls crates", "Listed crates \u{00b7} ~/gray"),
        (
            "grep -rn announce crates/gray-core/src",
            "Searched announce \u{00b7} crates/gray-core/src",
        ),
    ] {
        let text = header(serde_json::json!({ "command": cmd }));
        assert!(text.ends_with(want), "{text:?} should end with {want:?}");
    }
}

#[test]
fn reads_shorten_paths_against_the_cwd() {
    let args = serde_json::json!({"command": "cat /work/repo/src/main.rs"});
    let text = row_text(&format_tool_call_header(
        "bash",
        &args,
        Some(std::path::Path::new("/work/repo")),
        None,
    ));
    assert!(text.ends_with("Read src/main.rs"), "{text:?}");
}

#[test]
fn writes_and_background_runs_keep_their_verbs() {
    for (args, want) in [
        (
            serde_json::json!({"command": "cat a.rs > b.rs"}),
            "Ran cat a.rs > b.rs",
        ),
        (
            serde_json::json!({"command": "sed -i 's/a/b/' f.rs"}),
            "Ran sed -i 's/a/b/' f.rs",
        ),
        (
            serde_json::json!({"command": "tail -f app.log", "background": true}),
            "Started tail -f app.log \u{00b7} in background",
        ),
    ] {
        let text = header(args);
        assert!(text.ends_with(want), "{text:?} should end with {want:?}");
    }
}

#[test]
fn a_view_drops_the_shown_echo_but_keeps_its_notes() {
    let args = serde_json::json!({"command": "cat /tmp/a.png"});
    assert_eq!(body(args.clone(), "Shown: /tmp/a.png"), "");
    let noted = body(args, "Shown: /tmp/a.png; skipped: /tmp/b.png: no such file");
    assert!(noted.contains("skipped"), "{noted:?}");

    let echo = body(serde_json::json!({"command": "make"}), "Shown: x");
    assert!(echo.contains("Shown: x"), "{echo:?}");
}

#[test]
fn discord_send_preview_strips_markdown_markers() {
    let args = serde_json::json!({"content": "**gray updated + Discord restarted**"});
    let text = row_text(&format_tool_call_header("discord_send", &args, None, None));
    assert!(text.contains("Sent Discord"), "verb missing: {text:?}");
    assert!(
        text.contains("gray updated + Discord restarted"),
        "words missing: {text:?}"
    );
    assert!(!text.contains("**"), "literal markers leaked: {text:?}");
}

#[test]
fn discord_send_preview_strips_pairs_but_keeps_singles() {
    for (raw, want) in [
        ("`code` and **bold**", "code and bold"),
        ("__under__ plus ~~gone~~", "under plus gone"),
        ("a * b and 5_6 stay", "a * b and 5_6 stay"),
    ] {
        let args = serde_json::json!({"content": raw});
        let text = row_text(&format_tool_call_header("discord_send", &args, None, None));
        assert!(text.contains(want), "{raw:?} became {text:?}");
    }
}

fn spawn_args() -> serde_json::Value {
    serde_json::json!({
        "name": "e2e-theme",
        "model": "claude-haiku-5-5",
        "effort": "max",
        "prompt": "verify the theme fixes\nmore",
    })
}

fn short_model_name(id: &str) -> String {
    let friendly = crate::setup::friendly_model_name(id);
    friendly
        .strip_prefix("Claude ")
        .unwrap_or(friendly.as_str())
        .to_string()
}

#[test]
fn template_renders_headline_suffix_and_second_line() {
    let spec = "{name} · {model|model} · {effort}\n{prompt|firstline}";
    let args = with_tool_label(&spawn_args(), Some("Started agent"));
    let lines = format_tool_call_lines("subagent_spawn", &args, None, Some(spec));
    assert_eq!(lines.len(), 2, "second line missing: {lines:?}");
    let want = format!(
        "\u{2b22} Started agent e2e-theme · {} · max",
        short_model_name("claude-haiku-5-5")
    );
    assert_eq!(row_text(&lines[0]), want);
    assert_eq!(row_text(&lines[1]).trim(), "verify the theme fixes");
    let indent = " ".repeat(crate::text_width::display_width("\u{2b22} "));
    assert!(
        row_text(&lines[1]).starts_with(indent.as_str()),
        "second line not under the headline text: {lines:?}"
    );

    assert!(
        !row_text(&lines[0]).contains("name="),
        "dump leaked: {lines:?}"
    );
    assert!(
        !row_text(&lines[0]).contains("prompt"),
        "dump leaked: {lines:?}"
    );

    assert_eq!(
        row_text(&format_tool_call_header(
            "subagent_spawn",
            &args,
            None,
            Some(spec)
        )),
        want
    );
}

#[test]
fn template_headline_falls_back_to_the_humanized_name() {
    let lines = format_tool_call_lines("subagent_spawn", &spawn_args(), None, Some("{name}"));
    assert_eq!(row_text(&lines[0]), "\u{2b22} Subagent Spawn e2e-theme");
}

#[test]
fn template_drops_segments_whose_placeholders_are_all_empty() {
    let args = serde_json::json!({"name": "e2e-theme", "model": "claude-haiku-5-5"});

    assert_eq!(
        render_template(&args, "{name} · {model|model} · effort {effort}"),
        (
            format!("e2e-theme · {}", short_model_name("claude-haiku-5-5")),
            None
        )
    );

    assert_eq!(
        render_template(&args, "Started · run {name} · tier ({effort})"),
        ("Started · run e2e-theme".to_string(), None)
    );

    assert_eq!(
        render_template(&args, "{missing} · ({also})"),
        (String::new(), None)
    );
}

#[test]
fn template_filters_model_firstline_and_unknown_filters() {
    let args = serde_json::json!({
        "model": "claude-haiku-5-5",
        "prompt": "\n  \n  first real line  \nsecond line",
        "long": "x".repeat(120),
        "name": "e2e-theme",
    });
    assert_eq!(
        render_template(&args, "{model|model}").0,
        short_model_name("claude-haiku-5-5")
    );
    assert_eq!(
        render_template(&args, "{prompt|firstline}").0,
        "first real line"
    );

    let long = render_template(&args, "{long|firstline}").0;
    assert_eq!(long.chars().count(), 81, "not capped: {long:?}");
    assert!(long.ends_with('\u{2026}'), "no ellipsis: {long:?}");

    assert_eq!(render_template(&args, "{name|shout}").0, "e2e-theme");

    assert_eq!(
        render_template(&args, "{prompt}").0,
        "first real line second line"
    );
}

#[test]
fn template_arrays_join_scalars_and_objects_and_missing_are_empty() {
    let args = serde_json::json!({
        "tags": ["a", " b ", "", {"x": 1}, 3, null],
        "none": [],
        "objs": [{"x": 1}],
        "flag": false,
        "count": 0,
        "meta": {"k": "v"},
        "gone": null,
    });
    assert_eq!(render_template(&args, "{tags}").0, "a, b, 3");
    assert_eq!(render_template(&args, "{flag} {count}").0, "false 0");
    assert_eq!(render_template(&args, "{none}").0, "");
    assert_eq!(render_template(&args, "{objs}").0, "");
    assert_eq!(render_template(&args, "{meta} {gone} {nope.deep}").0, "");
}

#[test]
fn template_second_line_splits_on_either_break_and_omits_empty() {
    let args = serde_json::json!({"name": "e2e", "prompt": "do it", "p": "a\nb"});
    let pair = ("e2e".to_string(), Some("do it".to_string()));
    assert_eq!(render_template(&args, "{name}\n{prompt}"), pair);
    assert_eq!(render_template(&args, "{name}\\n{prompt}"), pair);
    assert_eq!(
        render_template(&args, "{name}\n{missing}"),
        ("e2e".to_string(), None)
    );

    assert_eq!(
        render_template(&args, "{name}\n{prompt}\\n{name}"),
        ("e2e".to_string(), Some("do it e2e".to_string()))
    );

    assert_eq!(render_template(&args, "{p}"), ("a b".to_string(), None));
}

#[test]
fn template_preview_flattens_for_json_and_dot_paths_stay_put() {
    let args = spawn_args();
    assert_eq!(
        preview_at(&args, "{name} · {effort}\n{prompt|firstline}").as_deref(),
        Some("e2e-theme · max · verify the theme fixes")
    );
    assert_eq!(preview_at(&args, "{missing}"), None);
    let doc = serde_json::json!({"document": {"title": "  Hi  "}});
    assert_eq!(preview_at(&doc, "document.title").as_deref(), Some("Hi"));
}

#[test]
fn dot_path_preview_header_is_unchanged() {
    let args = serde_json::json!({"document": {"title": "## Hello"}});
    let lines = format_tool_call_lines("some_surface_tool", &args, None, Some("document.title"));
    assert_eq!(lines.len(), 1, "a dot path adds no second line");
    assert_eq!(
        row_text(&lines[0]),
        "\u{2b22} Some Surface Tool \"## Hello\""
    );
}

#[test]
fn template_that_renders_empty_leaves_only_the_headline() {
    let args = with_tool_label(
        &serde_json::json!({"path": "/tmp/x.rs", "foo": "bar"}),
        Some("Started agent"),
    );
    let lines = format_tool_call_lines(
        "subagent_spawn",
        &args,
        None,
        Some("{missing} · ({also})\n{nope}"),
    );
    assert_eq!(lines.len(), 1, "empty second line rendered: {lines:?}");
    assert_eq!(row_text(&lines[0]), "\u{2b22} Started agent");
}

#[test]
fn built_in_tools_ignore_a_template_preview() {
    let args = serde_json::json!({"command": "ls -la"});
    let plain = format_tool_call_lines("bash", &args, None, None);
    let templated = format_tool_call_lines("bash", &args, None, Some("{command}\nx"));
    assert_eq!(
        templated.len(),
        1,
        "built-in arm grew a line: {templated:?}"
    );
    assert_eq!(row_text(&templated[0]), row_text(&plain[0]));
}

fn row_texts(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(row_text).collect()
}

#[test]
fn manifest_label_beats_a_wire_label_arg() {
    let args = serde_json::json!({"label": "wire-value", "name": "x"});
    let lines = format_tool_card_lines("my_tool", &args, None, Some("Manifest Label"), None);
    let head = row_text(&lines[0]);
    assert!(head.contains("Manifest Label"), "{head:?}");
    assert!(!head.contains("wire-value"), "{head:?}");
    assert_eq!(args["label"], "wire-value");
}

#[test]
fn live_header_with_label_and_template_matches_the_final_card() {
    let raw = r#"{"name":"e2e-theme","model":"claude-haiku-5-5","effort":"max","prompt":"verify the theme fixes\nmore"}"#;
    let v: serde_json::Value = serde_json::from_str(raw).unwrap();
    let spec = "{name} · {model|model} · {effort}\n{prompt|firstline}";
    let live = format_live_tool_header(
        "subagent_spawn",
        raw,
        None,
        Some("Started agent"),
        Some(spec),
    );
    let fin = format_tool_card_lines(
        "subagent_spawn",
        &v,
        None,
        Some("Started agent"),
        Some(spec),
    );
    assert_eq!(row_texts(&live), row_texts(&fin));
    assert_eq!(live.len(), 2, "{live:?}");
}

#[test]
fn plain_header_keeps_the_template_second_line() {
    let args = serde_json::json!({"name": "e2e-theme", "prompt": "verify the theme fixes\nmore"});
    let plain = format_tool_call_lines_plain(
        "subagent_spawn",
        &args,
        None,
        Some("{name}\n{prompt|firstline}"),
    );
    let rows: Vec<&str> = plain.split('\n').collect();
    assert_eq!(rows.len(), 2, "{plain:?}");
    assert!(rows[0].contains("e2e-theme"), "{plain:?}");
    assert!(rows[1].contains("verify the theme fixes"), "{plain:?}");
}

#[test]
fn mcp_tool_header_names_server_and_tool() {
    let line = format_tool_call_header(
        "mcp__treg__catalog_search",
        &serde_json::json!({"query": "send an sms"}),
        None,
        None,
    );
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("MCP treg catalog search"), "{text}");
    assert!(text.contains("send an sms"), "{text}");
    assert_eq!(
        humanize_tool_name("mcp__codegraph__codegraph_explore"),
        "codegraph \u{b7} codegraph explore"
    );
}
