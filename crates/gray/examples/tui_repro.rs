//! Temporary PTY repro harness (deleted after the fix lands).
//! Replays one live turn: reasoning -> tool boundary -> reasoning ->
//! multi-paragraph answer -> stray trailing reasoning -> turn end.
//! Dumps `tui.transcript` to /tmp/opencode/tui_repro.txt for counting
//! "Thought for" lines and blank runs.

use std::io::Write;

use gray::composer::Tui;
use ratatui::text::Line;

fn row_text(l: &Line<'static>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn is_blank(l: &Line<'static>) -> bool {
    l.style.bg.is_none()
        && l.spans
            .iter()
            .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
}

fn main() -> anyhow::Result<()> {
    let mut tui = Tui::new()?;
    tui.push_user_prompt("do the thing", &[], true);
    tui.begin_turn("Thinking");

    // Round 1 reasoning.
    tui.stream_thinking("Let me think about this problem carefully.\n");
    tui.stream_thinking("I need to check the files first.\n");
    std::thread::sleep(std::time::Duration::from_millis(60));

    // Tool boundary (mirrors ToolCallStart/ToolCallEnd/ToolResult dispatch).
    tui.flush_markdown();
    tui.end_thinking();
    tui.set_status(Some("Preparing tool: read"));
    tui.set_status(Some("Working"));
    tui.push_tool_box(Line::from("read Cargo.toml"), vec![Line::from("[package]")]);
    std::thread::sleep(std::time::Duration::from_millis(30));

    // Round 2 reasoning.
    tui.stream_thinking("The file looks good, now I will answer.\n");
    std::thread::sleep(std::time::Duration::from_millis(60));

    // Multi-paragraph answer in small chunks (like token streaming).
    for chunk in [
        "First paragraph of",
        " the answer, streamed bit by bit.\n\n",
        "Second paragraph of the answer.\n\n",
        "Third paragraph ends it.\n",
    ] {
        tui.stream_text(chunk);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Stray trailing reasoning chunk after the answer (re-opens the run).
    tui.stream_thinking("trailing note");

    // TurnEnd dispatch tail.
    tui.flush_markdown();
    tui.end_thinking();
    tui.set_turn_billed(4045);
    tui.end_turn(1200);
    tui.shutdown();

    let mut out = String::new();
    let mut thought = 0usize;
    let mut blank_runs: Vec<usize> = Vec::new();
    let mut cur = 0usize;
    for (i, l) in tui.transcript.iter().enumerate() {
        let text = row_text(l);
        let blank = is_blank(l);
        if blank {
            cur += 1;
        } else {
            if cur > 0 {
                blank_runs.push(cur);
            }
            cur = 0;
        }
        if text.contains("Thought for") || text.contains("Worked for") {
            thought += 1;
        }
        out.push_str(&format!(
            "{i:3} {} {text:?}\n",
            if blank { "BLANK" } else { "     " }
        ));
    }
    if cur > 0 {
        blank_runs.push(cur);
    }
    out.push_str(&format!(
        "THOUGHT_LINES={thought} BLANK_RUNS={blank_runs:?} TOTAL_ROWS={}\n",
        tui.transcript.len()
    ));
    std::fs::create_dir_all("/tmp/opencode")?;
    let mut f = std::fs::File::create("/tmp/opencode/tui_repro.txt")?;
    f.write_all(out.as_bytes())?;
    Ok(())
}
