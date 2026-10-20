use crate::style::test_style;
use crate::{StreamingMarkdownRenderer, render_markdown_ratatui_full};

fn lines_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn repro_soft_break_space_across_chunks() {
    let full = "Analyzing possible latency causes like API delay, cold start, network, and system load for a concise explanation.\nSeparating local execution from external API latency and noting the absence of internal timing data.\n\n";
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    let chars: Vec<char> = full.chars().collect();
    for w in chars.chunks(7) {
        let s: String = w.iter().collect();
        r.push_and_render(&s, None);
    }
    let view = r.finish(None);
    let flat: String = lines_text(&view.lines).join("");
    assert!(
        flat.contains("explanation. Separating"),
        "soft break must collapse to a space, got: {flat:?}"
    );
}

#[test]
fn torn_hyperlink_brackets_across_chunks_preserve_hyperlink_offset() {
    let full = "See [click](https://example.com) here.\n\n";
    let split = full.find("](").unwrap() + 1;
    let (a, b) = full.split_at(split);
    assert!(a.ends_with(']'), "a={a:?}");
    assert!(b.starts_with('('), "b={b:?}");

    let (expected, _) = render_markdown_ratatui_full(full, test_style::STYLE, true, None);
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    r.push_and_render(a, None);
    r.push_and_render(b, None);
    let view = r.finish(None);

    assert_eq!(lines_text(&view.lines), lines_text(&expected.lines));

    let line0: String = view.lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let hit = view
        .hyperlinks
        .iter()
        .find(|h| {
            h.url == "https://example.com" && {
                let slice: String = line0
                    .chars()
                    .skip(h.column_range.start)
                    .take(h.column_range.len())
                    .collect();
                slice == "click"
            }
        })
        .expect("hyperlink over link text should survive torn chunk");
    assert_eq!(hit.column_range.len(), 5);
}

fn norm(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| l.trim_end().to_string()).collect()
}

fn streamed_pushed_vs_final(full: &str, chunk_chars: usize) -> (Vec<String>, Vec<String>) {
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    let mut committed = 0usize;
    let mut pushed: Vec<String> = Vec::new();
    let chars: Vec<char> = full.chars().collect();
    for w in chars.chunks(chunk_chars.max(1)) {
        let s: String = w.iter().collect();
        r.push_and_render(&s, None);
        let frozen = r.frozen_lines_len();
        if frozen > committed {
            pushed.extend(lines_text(&r.view().lines[committed..frozen]));
            committed = frozen;
        }
    }
    let out = r.finish_tail_into_output(None);
    pushed.extend(lines_text(&out.lines[committed.min(out.lines.len())..]));
    (pushed, lines_text(&out.lines))
}

#[test]
fn emitted_stream_matches_output_for_documents() {
    let cases = [
        "Right — that reframes it. You're not asking whether gray can hitch a ride; gray is the thing you've been building, and this competition is literally \"ship your agent to 1,000 users on someone else's infra.\" The timing is dumb-lucky: the program started today (Oct 20), ends Dec 1.\n\nHow gray maps onto it:\n\n| Competition needs | Gray already has |\n|---|---|\n| Agent per user, persistent | Sessions, `~/.gray` state, cron, gateway — already a long-lived agent, not a request handler |\n| Runs on Maritime | Custom Docker container support; gray is a static binary, `/data` volume → `~/.gray` |\n| Reachable by real users | Keiki fronts SMS/WhatsApp/Telegram — gray's ACP/gateway sits behind it |\n| Model for 1,000 users | Maritime's LLM proxy = default provider; users don't need their own keys, and the prize credits spend through it anyway |\n\nThe actual work is the productization gap, not the agent:\n\n1. **Onboarding** — a fresh Maritime VM has no `auth.json`, no config, no providers. Need a boot path that lands a user into a working gray against the Maritime LLM proxy.\n2. **Headless interface** — Keiki talks messages, not TUI. Gray needs a clean non-interactive entry (gateway/ACP) that a channel adapter can drive.\n3. **Provisioning loop** — sign-up → `provision()` a Maritime machine per user → seed config → wire Keiki channel.\n4. **The actual hard part** — distribution. Six weeks, 1,000 real humans.\n",
        "# Heading\n\npara one with **bold** and `code` and a [link](https://example.com) inside\n\n- bullet a\n- bullet b\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\npara two\n\n1. first\n2. second\n\n> quote line\n\nfinal para\n",
        "para\n\n## mid heading\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\ntext after table\n",
    ];
    for (i, full) in cases.iter().enumerate() {
        for cs in [1usize, 3, 7, 13, 40, 1000] {
            let (pushed, final_lines) = streamed_pushed_vs_final(full, cs);
            assert_eq!(
                norm(&pushed),
                norm(&final_lines),
                "case {i} chunk={cs}\nfull={full:?}"
            );
        }
    }
}

#[test]
fn fuzz_emitted_stream_matches_output() {
    let frags = [
        "word ",
        "**bold** ",
        "`code` ",
        "[l](https://x.com) ",
        "\n",
        "\n\n",
        "## h\n",
        "- item\n",
        "1. n\n",
        "> q\n",
        "```\ncode line\n",
        "```rust\nx=1\n",
        "```\n",
        "| a | b |\n",
        "|---|---|\n",
        "| 1 | 2 |\n",
        "*it* ",
        "~~del~~ ",
        "### ",
        "text.",
        "(paren) ",
        "— dash ",
        "\"quote\" ",
        "$math$ ",
        "https://url.com ",
        "\\*esc ",
        "===\n",
        "---\n",
        "***\n",
        "text\n=",
        "text\n-",
        "setext\n===\n",
        "  \n",
        "\t",
        "&amp;",
        "&lt;tag&gt;",
        "[^1] ",
        "[^1]: note\n",
        "<b>html</b> ",
        "1) paren\n",
        "+ plus\n",
        "* star\n",
        "    indent\n",
        "~~~\nfence\n",
        "~~~\n",
        "text  \n",
        "a\\n",
        "__u__ ",
        "_i_ ",
        "***bi*** ",
        ">",
        "#",
    ];
    for seed in 0u64..20000 {
        let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        let mut next = move || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s >> 33
        };
        let nfrags = 3 + (next() % 15) as usize;
        let mut full = String::new();
        for _ in 0..nfrags {
            full.push_str(frags[(next() as usize) % frags.len()]);
        }
        let cs = 1 + (next() % 24) as usize;
        let (pushed, final_lines) = streamed_pushed_vs_final(&full, cs);
        if norm(&pushed) != norm(&final_lines) {
            panic!(
                "seed={seed} chunk={cs}\nfull={full:?}\npushed={pushed:?}\nfinal={final_lines:?}"
            );
        }
    }
}

#[test]
fn emitted_stream_equals_final_output() {
    let full = "<b>html</b> ***\n***bi*** 1. n\n1) paren\n";
    for cs in [1usize, 2, 3, 5] {
        let (pushed, final_lines) = streamed_pushed_vs_final(full, cs);
        assert_eq!(
            norm(&pushed),
            norm(&final_lines),
            "chunk={cs}: emitted stream diverged from output"
        );
    }
}

/// The composer emits `view().lines[committed..frozen]` as transcript rows and
/// later slices the finalized output at the same committed count. That only
/// works if the frozen prefix is immutable: a mid-stream table-width change
/// must not reset it, and finalizing must not re-segment it. This drives the
/// exact composer contract — emitted lines must equal the output verbatim.
#[test]
fn committed_prefix_survives_width_change_and_finish() {
    let full = "para before the table\n\n| head a | head b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\npara after the table\n\nfinal line\n";
    for (cs, w1, w2, flip_pct) in [
        (1usize, 80usize, 30usize, 50usize),
        (3, 80, 30, 50),
        (7, 80, 40, 30),
        (5, 30, 80, 60),
        (13, 60, 20, 40),
        (2, 80, 24, 90),
    ] {
        let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        r.set_max_table_width(Some(w1));
        let chars: Vec<char> = full.chars().collect();
        let total = chars.len();
        let mut committed = 0usize;
        let mut pushed: Vec<String> = Vec::new();
        let mut flipped = false;
        for (i, w) in chars.chunks(cs).enumerate() {
            let s: String = w.iter().collect();
            if !flipped && i * cs * 100 >= total * flip_pct {
                r.set_max_table_width(Some(w2));
                flipped = true;
            }
            r.push_and_render(&s, None);
            let frozen = r.frozen_lines_len();
            assert!(
                frozen >= committed,
                "cs={cs} w={w1}->{w2}: frozen went {committed}->{frozen}; \
                 committed rows would re-emit as duplicates"
            );
            if frozen > committed {
                pushed.extend(lines_text(&r.view().lines[committed..frozen]));
                committed = frozen;
            }
        }
        let out = r.finish_tail_into_output(None);
        if out.lines.len() > committed {
            pushed.extend(lines_text(&out.lines[committed..]));
        }
        assert_eq!(
            norm(&pushed),
            norm(&lines_text(&out.lines)),
            "cs={cs} w={w1}->{w2}: emitted stream must equal output exactly"
        );
    }
}
