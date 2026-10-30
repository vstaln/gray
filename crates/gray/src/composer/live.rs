use super::*;

pub(crate) fn format_thought_line(
    verb: &str,
    elapsed: &str,
    out_tokens: Option<usize>,
    tps: Option<u64>,
) -> String {
    let mut line = format!("✻ {verb} {elapsed}");
    if let Some(c) = out_tokens {
        line.push_str(&format!(" · {} tokens", crate::repl::fmt_usage(c)));
        if let Some(t) = tps {
            line.push_str(&format!(" · {t} tps"));
        }
    }
    line
}

pub(crate) fn turn_footer_line(
    had_thinking: bool,
    elapsed: Duration,
    turn_toks: Option<usize>,
    stream_ms: u64,
) -> String {
    let elapsed_str =
        crate::repl::format::fmt_duration_ms(elapsed.as_millis().min(u128::from(u64::MAX)) as u64);
    let verb = if had_thinking {
        "Thought for"
    } else {
        "Worked for"
    };

    let tps = turn_toks.and_then(|toks| crate::repl::turn_tokens_per_second(toks, stream_ms));
    format_thought_line(verb, &elapsed_str, turn_toks, tps)
}

pub(crate) fn pill_elapsed(
    turn_started: Option<Instant>,
    status_started: Instant,
    running: bool,
) -> Duration {
    if running {
        turn_started
            .map(|t| t.elapsed())
            .unwrap_or_else(|| status_started.elapsed())
    } else {
        status_started.elapsed()
    }
}

pub(crate) fn pill_context_tokens(u: &gray_core::event::Usage) -> usize {
    let input_part = if u.non_cached_input_tokens == 0
        && u.cache_read_input_tokens == 0
        && u.cache_write_input_tokens == 0
        && u.cached_tokens == 0
    {
        u.input_tokens
    } else {
        u.non_cached_input_tokens
    };
    input_part
        .saturating_add(u.output_tokens)
        .saturating_add(u.cache_read_input_tokens.max(u.cached_tokens))
        .saturating_add(u.cache_write_input_tokens)
}

pub(crate) fn live_context_total(
    usage: Option<gray_core::event::Usage>,
    streamed_bytes: u64,
) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    match usage {
        None => est,

        Some(u) => pill_context_tokens(&u).saturating_add(est),
    }
}

pub(crate) fn live_pill_suffix(turn_output_accum: usize, streamed_bytes: u64) -> String {
    let total = live_turn_output_tokens(turn_output_accum, streamed_bytes);
    if total == 0 {
        String::new()
    } else {
        format!(" · {} tokens", crate::repl::fmt_usage(total))
    }
}

pub(crate) fn live_turn_output_tokens(turn_output_accum: usize, streamed_bytes: u64) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    turn_output_accum.saturating_add(est)
}

pub(crate) fn live_tps_suffix(
    turn_output_accum: usize,
    streamed_bytes: u64,
    stream_ms: u64,
) -> String {
    let out = live_turn_output_tokens(turn_output_accum, streamed_bytes);
    crate::repl::turn_tokens_per_second(out, stream_ms)
        .map(|t| format!(" · {t} tps"))
        .unwrap_or_default()
}

pub(crate) fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

pub(crate) fn live_tool_rows(
    tools: &[LiveTool],
    elapsed: Duration,
    w: usize,
) -> Vec<Line<'static>> {
    let text_budget = w.saturating_sub(8).max(1);
    tools
        .iter()
        .take(MAX_LIVE_TOOLS)
        .flat_map(|t| {
            let mut headers = t.header.clone();

            if let Some(line) = headers.first_mut().filter(|_| t.running) {
                let filled = format!("{} ", crate::tui::glyph("\u{2b22}", "#"));
                let prefix = usize::from(
                    line.spans
                        .first()
                        .is_some_and(|s| s.content == filled.as_str()),
                );
                if prefix == 1 {
                    line.spans[0].content =
                        format!("{} ", crate::tui::glyph("\u{2b21}", "o")).into();
                }

                let live = line
                    .spans
                    .get(prefix)
                    .and_then(|s| crate::tool_fmt::live_bash_verb(&s.content));
                let end = prefix + usize::from(live.is_some());
                line.spans.splice(
                    prefix..end,
                    draw::shimmer_spans(live.unwrap_or("Running "), elapsed),
                );
            }
            headers
                .into_iter()
                .chain(t.tail.rows().into_iter().map(|text| {
                    let chars: Vec<char> = text.chars().collect();
                    let text =
                        if crate::text_width::fit_char_count(&chars, text_budget) < chars.len() {
                            let fit = crate::text_width::fit_char_count(
                                &chars,
                                text_budget.saturating_sub(1),
                            );
                            format!("{}…", chars[..fit].iter().collect::<String>())
                        } else {
                            text
                        };
                    Line::from(Span::styled(
                        format!("    {text}"),
                        Style::default().fg(crate::theme::theme().tool_dim),
                    ))
                }))
        })
        .collect()
}

pub(crate) fn live_tool_overflow(len: usize) -> usize {
    len.saturating_sub(MAX_LIVE_TOOLS)
}

impl LiveToolTail {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let text = crate::tui::strip_ansi(&String::from_utf8_lossy(bytes));
        for ch in text.chars() {
            match ch {
                '\n' => {
                    let line = std::mem::take(&mut self.carry);
                    if !line.trim().is_empty() {
                        self.lines.push_back(line);
                        while self.lines.len() > LIVE_TAIL_LINES {
                            self.lines.pop_front();
                        }
                    }
                }
                '\r' => self.carry.clear(),
                c => {
                    self.carry.push(c);
                    if self.carry.len() > LIVE_TAIL_CARRY_MAX {
                        let keep = LIVE_TAIL_CARRY_MAX / 2;
                        let mut start = self.carry.len() - keep;
                        while !self.carry.is_char_boundary(start) {
                            start += 1;
                        }
                        self.carry.drain(..start);
                    }
                }
            }
        }
    }

    pub(super) fn rows(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .lines
            .iter()
            .map(|l| crate::tool_fmt::expand_tabs(l))
            .collect();
        if !self.carry.trim().is_empty() {
            out.push(crate::tool_fmt::expand_tabs(&self.carry));
        }
        if out.len() > LIVE_TAIL_LINES {
            out.drain(..out.len() - LIVE_TAIL_LINES);
        }
        out
    }
}
