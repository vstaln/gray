use crate::render_markdown_ratatui_full;
use crate::style::test_style;

fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn pretty_lines(text: &str) -> Vec<String> {
    let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
    lines_to_text(&output.lines)
}

#[test]
fn dollar_inline_math_renders_unicode() {
    let lines = pretty_lines("Energy is $E = mc^2$ here.\n\n");
    assert_eq!(lines[0], "Energy is E = mc² here.", "got: {lines:#?}");
}

#[test]
fn dollar_inline_math_hides_delimiters_in_pretty_mode() {
    let lines = pretty_lines("So $x_1 + x_2$ holds.\n\n");
    assert!(!lines[0].contains('$'), "got: {lines:#?}");
    assert!(lines[0].contains("x₁ + x₂"), "got: {lines:#?}");
}

#[test]
fn raw_mode_preserves_inline_math_source() {
    let text = "Energy is $E = mc^2$ here.\n\n";
    let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, false, None);
    let lines = lines_to_text(&output.lines);
    assert!(lines[0].contains("$E = mc^2$"), "got: {lines:#?}");
}

#[test]
fn paren_inline_math_renders_unicode() {
    let lines = pretty_lines("Sum \\(\\alpha + \\beta\\) end.\n\n");
    assert_eq!(lines[0], "Sum α + β end.", "got: {lines:#?}");
}

#[test]
fn padded_paren_inline_math_renders_unicode() {
    // Regression: whitespace just inside `\( … \)` made the normalized
    // `$ … $` violate pulldown's dollar-math flanking rule, so it used to
    // render as raw `$ … $`. The normalizer now trims that padding.
    let lines = pretty_lines("Sum \\( x+y \\) end.\n\n");
    assert_eq!(lines[0], "Sum x+y end.", "got: {lines:#?}");
    assert!(
        !lines[0].contains('$'),
        "delimiters must be gone: {lines:#?}"
    );
}

#[test]
fn padded_paren_inline_math_with_braces_renders() {
    let lines = pretty_lines("Set \\( S = \\{ x : x > 0 \\} \\) defined.\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("x : x > 0"), "got: {lines:#?}");
    assert!(!joined.contains('$'), "no raw dollar math: {lines:#?}");
}

#[test]
fn paren_inline_math_in_list_item() {
    let lines = pretty_lines("- implies \\(p \\to q\\)\n- plain\n\n");
    assert!(lines[0].contains("implies p → q"), "got: {lines:#?}");
}

#[test]
fn paren_inline_math_in_heading() {
    let lines = pretty_lines("## About \\(\\pi^2\\)\n\n");
    assert!(lines[0].contains("About π²"), "got: {lines:#?}");
}

#[test]
fn dollar_inline_math_in_heading() {
    let lines = pretty_lines("# Energy $E=mc^2$\n\n");
    assert!(lines[0].contains("Energy E=mc²"), "got: {lines:#?}");
}

#[test]
fn bracket_display_math_in_heading() {
    // pulldown-cmark keeps heading content inside a `Heading` block (no
    // wrapping paragraph), so the `\[...\]` source scan must also run on
    // heading end. `$$...$$` in the same position already converts via
    // `Event::DisplayMath`.
    let lines = pretty_lines("## Identity \\[x^2 + y^2 = z^2\\]\n\nAfter.\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("x² + y² = z²"), "got: {lines:#?}");
    assert!(!joined.contains("\\["), "got: {lines:#?}");
}

#[test]
fn escaped_backslash_paren_is_not_math() {
    // `\\(` is a literal backslash followed by a paren — not a math open.
    let lines = pretty_lines("Literal \\\\(x\\\\) here.\n\n");
    let joined = lines.join("\n");
    // Pulldown renders the escapes; no Unicode conversion should occur
    // and the parens must survive.
    assert!(joined.contains("(x"), "got: {lines:#?}");
}

#[test]
fn emphasis_inside_paren_math_falls_back() {
    // `*nope*` becomes emphasis, splitting the text events, so the span
    // is not converted; content must still render.
    let lines = pretty_lines("a \\(*nope*\\) b\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("nope"), "got: {lines:#?}");
    assert!(!joined.contains('→'), "got: {lines:#?}");
}

#[test]
fn display_math_dollar_renders_block() {
    let lines = pretty_lines("Before.\n\n$$\n\\int_0^1 x \\, dx = \\frac{1}{2}\n$$\n\nAfter.\n\n");
    let math_line = lines
        .iter()
        .find(|l| l.contains('∫'))
        .expect("math block line");
    assert_eq!(math_line.trim(), "∫₀¹ x dx = ½", "got: {lines:#?}");
    // Block lines are indented.
    assert!(math_line.starts_with("  "), "got: {lines:#?}");
}

#[test]
fn display_math_dollar_inline_form_renders_block() {
    let lines = pretty_lines("text $$x^2 + y^2 = z^2$$ more\n\n");
    let idx_text = lines.iter().position(|l| l.contains("text")).unwrap();
    let idx_math = lines
        .iter()
        .position(|l| l.contains("x² + y² = z²"))
        .unwrap();
    let idx_more = lines.iter().position(|l| l.contains("more")).unwrap();
    assert!(idx_text < idx_math, "text before math: {lines:#?}");
    assert!(idx_math < idx_more, "math before trailing text: {lines:#?}");
}

#[test]
fn display_math_bracket_renders_block() {
    let text = "The AM-GM inequality:\n\n\\[\n\\frac{a+b}{2} \\ge \\sqrt{ab}\n\\]\n\nDone.\n\n";
    let lines = pretty_lines(text);
    let math_line = lines
        .iter()
        .find(|l| l.contains('≥'))
        .expect("math block line");
    assert_eq!(math_line.trim(), "(a+b)/2 ≥ √(ab)", "got: {lines:#?}");
    assert!(!lines.join("\n").contains("\\["), "got: {lines:#?}");
}

#[test]
fn display_math_bracket_single_line_renders_block() {
    let lines = pretty_lines("\\[E = mc^2\\]\n\nAfter.\n\n");
    let math_line = lines.iter().find(|l| l.contains("mc²")).expect("math line");
    assert_eq!(math_line.trim(), "E = mc²", "got: {lines:#?}");
}

#[test]
fn display_math_bracket_in_raw_mode_shows_canonical_dollars() {
    // The delimiter normalizer rewrites `\[…\]` → `$$…$$` before parsing, so
    // raw mode shows the canonical `$$` form (the math→Unicode conversion is
    // still a pretty-only overlay, so the TeX body itself is preserved).
    let text = "\\[E = mc^2\\]\n\n";
    let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, false, None);
    let joined = lines_to_text(&output.lines).join("\n");
    assert!(joined.contains("$$E = mc^2$$"), "got: {joined:?}");
    assert!(!joined.contains("\\["), "got: {joined:?}");
}

#[test]
fn display_math_with_lone_equals_line_renders_block() {
    // Symptom 1: a lone `=` line inside a display span is a
    // CommonMark setext underline; unjoined, the first line became an H1
    // and the math rendered as raw TeX.
    let text = "The loss:\n\n\\[\n\\boxed{\n\\mathcal{L}_{\\text{MTP}}\n=\n\\sum_{i=0}^{2}\n\\gamma^{i}\\,\n\\mathbb{E}_{\\text{positions, mask}}\n\\Big[\n\\mathrm{KL}\\big(\n  \\mathrm{softmax}(z_{\\text{torso}}^{(s_i)})\n  \\;\\big\\|\\;\n  \\mathrm{softmax}(z_{\\text{draft}}^{(i)})\n\\big)\n\\Big]\n}\n\\]\n\nAfter.\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    let math_line = lines
        .iter()
        .find(|l| l.contains('ℒ'))
        .expect("math block line");
    assert!(math_line.contains("ℒ_(MTP) = ∑ᵢ₌₀²"), "got: {lines:#?}");
    assert!(joined.contains("softmax(z_(torso)"), "got: {lines:#?}");
    assert!(!joined.contains('$'), "no raw delimiters: {lines:#?}");
    assert!(!joined.contains("\\["), "got: {lines:#?}");
    assert!(!joined.contains("boxed"), "got: {lines:#?}");
}

#[test]
fn dollar_display_math_with_lone_equals_line_renders_block() {
    let lines = pretty_lines("$$\nx\n=\ny\n$$\n\nAfter.\n\n");
    let math_line = lines
        .iter()
        .find(|l| l.contains("x = y"))
        .expect("math block line");
    assert!(math_line.starts_with("  "), "block indent: {lines:#?}");
    assert!(!lines.join("\n").contains('$'), "got: {lines:#?}");
}

#[test]
fn text_subscript_in_table_cell_renders_readable() {
    // Symptom 2: `p_{\text{torso}}` in a table cell became the
    // modifier-letter run `pₜₒᵣₛₒ`, which renders with visible gaps in
    // fonts lacking those glyphs.
    let text = "| Who | Soft-teacher |\n|-----|--------------|\n| **Torso** | \\(p_{\\text{torso}}(\\cdot \\mid T_0,\\ldots,T_i)\\) |\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(joined.contains("p_(torso)(⋅ ∣ T₀,…,Tᵢ)"), "got: {lines:#?}");
    assert!(!joined.contains('ₜ'), "no modifier-letter runs: {lines:#?}");
}

#[test]
fn aligned_environment_renders_multiple_lines() {
    let text =
        "\\[\n\\begin{aligned}\nf(x) &= x^2 \\\\\ng(x) &= 2x\n\\end{aligned}\n\\]\n\nEnd.\n\n";
    let lines = pretty_lines(text);
    let idx_f = lines.iter().position(|l| l.contains("f(x) = x²")).unwrap();
    let idx_g = lines.iter().position(|l| l.contains("g(x) = 2x")).unwrap();
    assert_eq!(idx_g, idx_f + 1, "consecutive block lines: {lines:#?}");
}

#[test]
fn cases_environment_renders_flat() {
    let text = "$$\n|x| = \\begin{cases} x & x \\ge 0 \\\\ -x & x < 0 \\end{cases}\n$$\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(
        joined.contains("|x| = {x  x ≥ 0; −x  x < 0}"),
        "got: {lines:#?}"
    );
}

#[test]
fn inline_math_in_table_cell_renders_unicode() {
    let text = "| Col | Math |\n|-----|------|\n| a | $x^2 + 1$ |\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(joined.contains("x² + 1"), "got: {lines:#?}");
    assert!(!joined.contains('$'), "got: {lines:#?}");
}

#[test]
fn paren_inline_math_in_table_cell_renders_unicode() {
    // `\(…\)` inside a table cell must convert. Previously the
    // backslash-form scanner was disabled inside tables, leaving raw TeX.
    // Normalization rewrites `\(…\)` → `$…$` before parsing, so the existing
    // in-cell `$` path converts it.
    let text = "| Mode | Metric |\n|------|--------|\n| Rate | \\(\\alpha + \\beta\\) |\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(joined.contains("α + β"), "got: {lines:#?}");
    assert!(
        !joined.contains("\\("),
        "raw TeX must not survive: {lines:#?}"
    );
    assert!(!joined.contains('$'), "delimiters hidden: {lines:#?}");
}

#[test]
fn bracket_display_math_in_table_cell_renders_unicode() {
    // `\[…\]` inside a cell renders single-line (no room for a block).
    let text = "| Col | Math |\n|-----|------|\n| a | \\[x^2\\] |\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(joined.contains("x²"), "got: {lines:#?}");
    assert!(!joined.contains("\\["), "got: {lines:#?}");
}

#[test]
fn paren_inline_math_in_blockquote_renders_unicode() {
    let lines = pretty_lines("> energy \\(E = mc^2\\) noted\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("E = mc²"), "got: {lines:#?}");
    assert!(!joined.contains("\\("), "got: {lines:#?}");
}

#[test]
fn equation_environment_converts_to_block() {
    let text = "Before.\n\n\\begin{equation}\nE = mc^2\n\\end{equation}\n\nAfter.\n\n";
    let lines = pretty_lines(text);
    let joined = lines.join("\n");
    assert!(joined.contains("E = mc²"), "got: {lines:#?}");
    assert!(!joined.contains("\\begin"), "got: {lines:#?}");
}

#[test]
fn latex_in_code_span_left_verbatim() {
    // Code spans are verbatim: `\(…\)` inside backticks must NOT convert.
    let lines = pretty_lines("inline `\\(x\\)` code\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("\\(x\\)"), "code must stay raw: {lines:#?}");
}

#[test]
fn display_math_in_blockquote_renders() {
    let lines = pretty_lines("> Einstein: $$E = mc^2$$\n\n");
    let joined = lines.join("\n");
    assert!(joined.contains("E = mc²"), "got: {lines:#?}");
}

#[test]
fn oversized_inline_math_falls_back_to_code_styling() {
    let body = "x".repeat(crate::latex::MAX_MATH_SOURCE_LEN + 10);
    let text = format!("Big ${body}$ end.\n\n");
    let lines = pretty_lines(&text);
    let joined = lines.join("\n");
    // Content is preserved verbatim (code-style fallback), delimiters
    // hidden in pretty mode.
    assert!(joined.contains(&body), "fallback must keep raw content");
}

#[test]
fn bracket_math_inside_link_label_keeps_link_target() {
    // Option A normalizes `\[x\]` → `$$x$$` everywhere outside code, so (like
    // a literal `$$…$$`) display math inside a link label now converts. This
    // construct — display math inside a link label — is degenerate and
    // exceedingly rare in model output; the invariant we keep is that the
    // link target survives.
    let lines = pretty_lines("See [\\[x\\] notes](https://example.com) now.\n\n");
    let joined = lines.join("\n");
    assert!(
        joined.contains("https://example.com"),
        "link must survive: {lines:#?}"
    );
}

#[test]
fn unclosed_math_renders_without_panic() {
    for text in [
        "open $a + b\n\n",
        "open $$a + b\n\n",
        "open \\(a + b\n\n",
        "open \\[a + b\n\n",
        "$$\n\\frac{1}{\n\n",
        "\\]\n\n",
        "\\)\n\n",
    ] {
        let _ = pretty_lines(text);
    }
}

#[test]
fn multiple_inline_math_spans_in_one_paragraph() {
    let lines = pretty_lines("Both $a^2$ and \\(b_1\\) and $c \\ne d$ work.\n\n");
    assert_eq!(
        lines[0], "Both a² and b₁ and c ≠ d work.",
        "got: {lines:#?}"
    );
}

#[test]
fn greek_and_symbols_inline() {
    let lines =
        pretty_lines("Rate $\\lambda \\approx 0.5$ and set $S \\subseteq \\mathbb{R}^n$.\n\n");
    assert_eq!(lines[0], "Rate λ ≈ 0.5 and set S ⊆ ℝⁿ.", "got: {lines:#?}");
}
