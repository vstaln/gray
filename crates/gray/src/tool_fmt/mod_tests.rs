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

#[test]
fn live_header_empty_args_shows_name_only() {
    let text = row_text(&format_live_tool_header("bash", "", None));
    assert!(text.contains("bash"), "got {text:?}");
}

#[test]
fn live_header_full_json_matches_final_header() {
    let raw = r#"{"command":"cargo test -p gray"}"#;
    let v: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        row_text(&format_live_tool_header("bash", raw, None)),
        row_text(&format_tool_call_header("bash", &v, None)),
    );
}

#[test]
fn live_header_partial_bash_streams_command() {
    let text = row_text(&format_live_tool_header(
        "bash",
        r#"{"command":"cargo test --li"#,
        None,
    ));
    assert!(text.contains("cargo test"), "got {text:?}");
}

#[test]
fn live_header_partial_read_streams_path() {
    let text = row_text(&format_live_tool_header("read", r#"{"path":"src/ma"#, None));
    assert!(text.contains("src/ma"), "got {text:?}");
}

#[test]
fn live_header_garbage_never_panics_and_names_tool() {
    for raw in ["{{{", "\"unclosed", "   ", ",,,"] {
        let text = row_text(&format_live_tool_header("grep", raw, None));
        assert!(text.contains("grep"), "got {text:?} for {raw:?}");
    }
}

#[test]
fn live_header_huge_raw_still_streams_prefix() {
    // No 1MB Box: oversized partials cap the extracted scalar, the header
    // still streams from the same prefix the final card truncates to.
    let big = format!(r#"{{"command":"{}"#, "x".repeat(9000));
    let full = format!(r#"{{"command":"{}"}}"#, "x".repeat(9000));
    let v: serde_json::Value = serde_json::from_str(&full).unwrap();
    assert_eq!(
        row_text(&format_live_tool_header("bash", &big, None)),
        row_text(&format_tool_call_header("bash", &v, None)),
    );
}

/// The reported crash: a bash command whose first line carries multi-byte
/// text (here the `🟠` from a pasted PR review) straddling byte offset 80.
/// `truncate_cmd` used to byte-slice at a fixed 80 and panic the whole
/// REPL mid-stream. It must cut on a char boundary at 80 cells instead.
#[test]
fn truncate_cmd_never_splits_a_multibyte_char() {
    // 79 ASCII bytes, then the 4-byte emoji at bytes 79..83 — the exact
    // geometry of the reported panic.
    let head = "114.3k/300k · 0.0% cache ";
    assert_eq!(head.len(), 26);
    let pad = "x".repeat(53);
    let cmd = format!("{head}{pad}\u{1f7e0} High · up to 553c7cd\nsecond line");
    let cut = super::truncate_cmd(&cmd);
    // The old code byte-sliced at 80 and died here; the cut must land on a
    // char boundary, be a prefix, and fit 80 cells (bytes may exceed 80 —
    // cells are what the terminal renders).
    assert!(cmd.is_char_boundary(cut.len()), "cut mid-char");
    assert!(cmd.starts_with(cut), "cut is not a prefix: {cut:?}");
    assert!(
        crate::text_width::display_width(cut) <= 80,
        "cut not within 80 cells: {} cells",
        crate::text_width::display_width(cut)
    );
    // Only the first line is ever shown.
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
    // Under the cap: no cut at all, multi-byte or not.
    let wide = "café ☕ ok";
    assert_eq!(super::truncate_cmd(wide), wide);
    // Wide text now fills the same 80 cells (4x more chars than a byte cut).
    let cjk = "日本語".repeat(40);
    let cut = super::truncate_cmd(&cjk);
    assert_eq!(
        crate::text_width::display_width(cut),
        80,
        "wide cut: {cut:?}"
    );
}

/// The live-header path is where the crash surfaced (streaming bash args),
/// so drive the real entry point with the same shape.
#[test]
fn live_bash_header_survives_multibyte_command() {
    let head = "Merge Risk: _";
    let pad = "y".repeat(64);
    let partial = format!("{{\"command\":\"{head}{pad}\u{1f7e0} High");
    let line = format_live_tool_header("bash", &partial, None);
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
    let text = row_text(&format_tool_call_header("web_search", &args, None));
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
    let text = row_text(&format_tool_call_header("web_fetch", &args, None));
    assert!(!text.contains("web_fetch"), "wire name leaked: {text:?}");
    assert!(text.contains("Fetched"), "verb missing: {text:?}");
    assert!(
        text.contains("https://librewolf.net/installation/linux/"),
        "url missing: {text:?}"
    );
}

#[test]
fn discord_send_renders_a_verb_with_the_content() {
    let args = serde_json::json!({"content": "hello channel"});
    let text = row_text(&format_tool_call_header("discord_send", &args, None));
    assert!(!text.contains("discord_send"), "wire name leaked: {text:?}");
    assert!(text.contains("Sent Discord"), "verb missing: {text:?}");
    assert!(text.contains("hello channel"), "content missing: {text:?}");
}

#[test]
fn plugin_preview_path_renders_first_string_at_that_path() {
    let args =
        serde_json::json!({"preview": "document.title", "document": {"title": "  ## Hello  "}});
    let text = row_text(&format_tool_call_header("some_surface_tool", &args, None));
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
    let args = serde_json::json!({"preview": "document.title", "document": {}});
    let text = row_text(&format_tool_call_header("some_surface_tool", &args, None));
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
    let text = row_text(&format_tool_call_header("my_custom_tool", &args, None));
    assert!(text.contains("My Custom Tool"), "not humanized: {text:?}");
    assert!(
        !text.contains("my_custom_tool"),
        "wire name leaked: {text:?}"
    );
    let labeled = with_tool_label(&args, Some("Notify Owner"));
    let labeled_text = row_text(&format_tool_call_header("my_custom_tool", &labeled, None));
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
    let text = row_text(&format_live_tool_header(
        "web_search",
        r#"{"query":"LibreWolf AppI"#,
        None,
    ));
    assert!(text.contains("LibreWolf AppI"), "got {text:?}");
    let text = row_text(&format_live_tool_header(
        "discord_send",
        r#"{"content":"hello"#,
        None,
    ));
    assert!(text.contains("hello"), "got {text:?}");
}

#[test]
fn a_cut_bash_header_ends_in_an_ellipsis() {
    let long = serde_json::json!({"command": format!("seq 20 | xargs sh -c '{}' | sort | uniq -c", "x".repeat(80))});
    let text = row_text(&format_tool_call_header("bash", &long, None));
    assert!(text.ends_with('\u{2026}'), "{text:?}");
    let multi = serde_json::json!({"command": "cd x\nmake"});
    assert!(row_text(&format_tool_call_header("bash", &multi, None)).ends_with("cd x\u{2026}"));
    let short = serde_json::json!({"command": "ls"});
    assert!(row_text(&format_tool_call_header("bash", &short, None)).ends_with("Ran ls"));
}

// --- bash: one tool, five actions; headers say which, jobs go by name. ---

fn header(args: serde_json::Value) -> String {
    row_text(&format_tool_call_header("bash", &args, None))
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
        text.ends_with("Ran cargo check -p gray-provider --tests \u{00b7} in ~/wt/provider-chat"),
        "{text:?}"
    );
    // Setup that could do work is never hidden.
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
    ] {
        assert_eq!(live_bash_verb(done), Some(live));
    }
    assert_eq!(live_bash_verb("Read "), None);
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
    // Command output is never rewritten, even when it looks like a hint.
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
fn discord_send_preview_strips_markdown_markers() {
    let args = serde_json::json!({"content": "**gray updated + Discord restarted**"});
    let text = row_text(&format_tool_call_header("discord_send", &args, None));
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
        let text = row_text(&format_tool_call_header("discord_send", &args, None));
        assert!(text.contains(want), "{raw:?} became {text:?}");
    }
}
