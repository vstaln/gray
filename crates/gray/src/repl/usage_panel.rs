//! `/usage` panel rows: pure builders shared by the styled TUI render and
//! the headless plain print ([`usage_plain`] strips the same rows' spans,
//! so the two never drift). `handle_usage` owns the `✓` action header and
//! resolves the [`PanelInput`].

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::SessionTotals;
use crate::setup::ModelRate;

/// Cells in the cache-share bar.
const BAR_CELLS: usize = 24;

/// Cache warmth at render time, read off the composer's `CacheTracker`.
/// `Unknown` — headless, or a provider that never reported caching —
/// omits the suffix entirely.
pub(crate) enum Warmth {
    Warm(std::time::Duration),
    Cold,
    Unknown,
}

/// Everything one `/usage` render needs, gathered by `handle_usage`.
pub(crate) struct PanelInput<'a> {
    pub(crate) totals: &'a SessionTotals,
    pub(crate) model: &'a str,
    /// Active model's rate; `None` = unpriced (fallback line).
    pub(crate) rate: Option<ModelRate>,
    pub(crate) warmth: Warmth,
}

/// `{model} · {n} turn{s}` — the action-line detail in both renders.
pub(crate) fn usage_header(input: &PanelInput<'_>) -> String {
    let n = input.totals.turns;
    format!(
        "{} · {n} turn{}",
        input.model,
        if n == 1 { "" } else { "s" }
    )
}

/// Every row below the header, in panel order.
pub(crate) fn usage_rows(input: &PanelInput<'_>) -> Vec<Line<'static>> {
    let t = input.totals;
    let fl = crate::setup::format_context_length;
    let mut rows = Vec::new();

    let mut tokens = vec![
        body(format!("{} in", fl(t.input))),
        sep(),
        body(format!("{} out", fl(t.output))),
    ];
    // An unpriced model's `cost` is 0 by absence, not by being free — the
    // Rate row's fallback says so; printing `$0` here would lie.
    if input.rate.is_some() {
        tokens.push(sep());
        tokens.push(body(crate::setup::format_cost(t.cost)));
    }
    rows.push(row("Tokens", tokens));

    if t.cache_reported {
        let fresh = t.input.saturating_sub(t.cache_read + t.cache_write);
        let mut cache = cache_bar(t.cache_read, t.cache_write, fresh);
        cache.push(Span::raw("  "));
        let hit = (t.cache_read as f64 * 100.0 / t.input.max(1) as f64).round() as usize;
        cache.push(styled(
            format!("{}% hit", hit.min(100)),
            crate::theme::theme().cache_hit,
            true,
        ));
        match &input.warmth {
            Warmth::Warm(left) => {
                cache.push(sep());
                cache.push(styled(
                    format!("warm {} left", crate::cache::format_remaining(*left)),
                    crate::theme::theme().success,
                    false,
                ));
            }
            Warmth::Cold => {
                cache.push(sep());
                cache.push(muted("cold"));
            }
            Warmth::Unknown => {}
        }
        rows.push(row("Cache", cache));
        rows.push(Line::from(vec![
            Span::raw(" ".repeat(11)),
            body(format!("{} read", fl(t.cache_read))),
            sep(),
            body(format!("{} written", fl(t.cache_write))),
            sep(),
            body(format!("{} fresh", fl(fresh))),
        ]));

        if let Some(r) = &input.rate
            && r.has_cache_prices
        {
            let saved = if t.saved < 0.0 {
                vec![muted(format!(
                    "−${:.2} (cache writes not yet paid back)",
                    -t.saved
                ))]
            } else {
                let mut v = vec![styled(
                    format!("≈{} vs uncached", crate::setup::format_cost(t.saved)),
                    crate::theme::theme().success,
                    false,
                )];
                if t.write_premium >= 0.01 {
                    v.push(sep());
                    v.push(styled(
                        format!("writes +{}", crate::setup::format_cost(t.write_premium)),
                        crate::theme::theme().success,
                        false,
                    ));
                }
                v
            };
            rows.push(row("Saved", saved));
        }

        let m = &t.misses;
        if m.count == 0 {
            rows.push(row(
                "Misses",
                vec![styled("none", crate::theme::theme().success, false)],
            ));
        } else {
            let mut v = vec![
                styled(m.count.to_string(), crate::theme::theme().error_soft, false),
                sep(),
                styled(
                    format!("{} re-billed", fl(m.tokens)),
                    crate::theme::theme().error_soft,
                    false,
                ),
            ];
            if m.cost >= 0.01 {
                v.push(sep());
                v.push(styled(
                    format!("≈{}", crate::setup::format_cost(m.cost)),
                    crate::theme::theme().error_soft,
                    false,
                ));
            }
            for (n, label) in [
                (m.idle, "idle"),
                (m.model_switch, "model switch"),
                (m.other, "other"),
            ] {
                if n > 0 {
                    v.push(sep());
                    v.push(body(format!("{label} {n}")));
                }
            }
            rows.push(row("Misses", v));
        }
    } else {
        rows.push(row("Cache", vec![muted("provider reports no cache usage")]));
    }

    if t.total_duration_ms > 0 && t.timed_turns > 0 {
        let avg = t.total_duration_ms / t.timed_turns as u64;
        let f = crate::repl::format::fmt_duration_ms;
        rows.push(row(
            "Time",
            vec![
                body(format!("{} total", f(t.total_duration_ms))),
                sep(),
                body(format!("{} avg", f(avg))),
            ],
        ));
    }

    let rate = match &input.rate {
        Some(r) => {
            let mut v = vec![body(format!(
                "{}/{} per 1M in/out",
                crate::setup::format_cost(r.input * 1_000_000.0),
                crate::setup::format_cost(r.output * 1_000_000.0)
            ))];
            if r.has_cache_prices {
                v.push(sep());
                v.push(body(format!(
                    "cached {}",
                    crate::setup::format_cost(r.cache_read * 1_000_000.0)
                )));
            }
            v
        }
        None => vec![muted(
            "unpriced (no rate yet — pricing tables lag new models)",
        )],
    };
    rows.push(row("Rate", rate));

    // What compression saved, over every result metered on this machine.
    // An empty counter is noise on a panel opened for tokens and cost.
    let spill = gray_core::spill::totals();
    if spill.events > 0 {
        rows.push(row(
            "Squeeze",
            vec![body(format!(
                "{} tool output never sent ({:.0}% of {} produced) · ≈{} tokens",
                gray_core::spill::fmt_bytes(spill.saved() as usize),
                spill.saved_pct(),
                gray_core::spill::fmt_bytes(spill.raw_bytes as usize),
                fl((spill.saved() / 4) as usize),
            ))],
        ));
    }

    rows
}

/// Headless render: the same rows with styles stripped — one source of
/// truth, so pipe output can never drift from the TUI panel.
pub(crate) fn usage_plain(input: &PanelInput<'_>) -> Vec<String> {
    usage_rows(input)
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

/// `  {label:<9}` gutter + value spans — one `Tokens  `/`Squeeze`-width row.
fn row(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![muted(format!("  {label:<9}"))];
    spans.extend(value);
    Line::from(spans)
}

/// Stacked read/written/fresh shares of session input: `█` hits, `▓`
/// writes, `░` fresh. Segments round to exactly [`BAR_CELLS`]; any nonzero
/// share keeps at least one cell. Empty when nothing was input.
fn cache_bar(read: usize, write: usize, fresh: usize) -> Vec<Span<'static>> {
    let total = read + write + fresh;
    if total == 0 {
        return Vec::new();
    }
    let parts = [read, write, fresh];
    let mut cells = [0usize; 3];
    for (i, &p) in parts.iter().enumerate() {
        if p > 0 {
            cells[i] = ((p * BAR_CELLS + total / 2) / total).max(1);
        }
    }
    let mut sum: usize = cells.iter().sum();
    while sum > BAR_CELLS {
        // Shave the largest segment that can give a cell up (never a
        // nonzero share's last cell).
        let Some(i) = (0..3).filter(|&i| cells[i] > 1).max_by_key(|&i| cells[i]) else {
            break;
        };
        cells[i] -= 1;
        sum -= 1;
    }
    while sum < BAR_CELLS {
        let i = (0..3).max_by_key(|&i| cells[i]).unwrap_or(0);
        cells[i] += 1;
        sum += 1;
    }
    let theme = crate::theme::theme();
    let glyphs = [
        ('█', theme.cache_hit),
        ('▓', theme.accent_soft),
        ('░', theme.text_faint),
    ];
    cells
        .iter()
        .zip(glyphs)
        .filter(|(n, _)| **n > 0)
        .map(|(n, (g, c))| styled(g.to_string().repeat(*n), c, false))
        .collect()
}

fn styled(s: impl Into<String>, color: ratatui::style::Color, bold: bool) -> Span<'static> {
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Span::styled(s.into(), style)
}

fn body(s: impl Into<String>) -> Span<'static> {
    styled(s, crate::theme::theme().text_body, false)
}

fn muted(s: impl Into<String>) -> Span<'static> {
    styled(s, crate::theme::theme().text_muted, false)
}

fn sep() -> Span<'static> {
    styled(" · ", crate::theme::theme().text_dim, false)
}

#[path = "usage_panel_tests.rs"]
#[cfg(test)]
mod tests;
