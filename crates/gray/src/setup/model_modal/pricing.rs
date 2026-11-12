pub(super) const METER_W: usize = 6;

pub(super) fn meter_fill(levels: &[&str], level: &str, width: usize) -> usize {
    let tiers: Vec<&str> = levels.iter().copied().filter(|l| *l != "off").collect();
    let Some(k) = tiers.iter().position(|l| *l == level) else {
        return 0;
    };
    let n = tiers.len();
    ((k + 1) * width * 2 + n) / (2 * n)
}

pub(super) fn list_window(total: usize, sel: usize, top: usize, cap: usize) -> (usize, usize) {
    if total <= cap {
        return (0, total);
    }
    let sel = sel.min(total - 1);
    if cap < 3 {
        let top = top.min(sel).max((sel + 1).saturating_sub(cap));
        return (top.min(total - cap), cap);
    }

    let mut top = top.min(sel).min(total - (cap - 1));
    loop {
        let room = cap - usize::from(top > 0);
        let shown = room - usize::from(top + room < total);
        if sel < top + shown {
            return (top, shown);
        }
        top += sel + 1 - (top + shown);
    }
}

pub(super) fn per_million(rate: f64) -> String {
    let s = format!("{:.3}", rate * 1_000_000.0);
    format!("${}", s.trim_end_matches('0').trim_end_matches('.'))
}

pub(crate) const PRICE_DOTS: usize = 5;

pub(crate) fn price_level(blended_per_million: f64) -> usize {
    const BANDS: [f64; PRICE_DOTS - 1] = [0.5, 2.0, 8.0, 20.0];
    1 + BANDS.iter().filter(|&&b| blended_per_million > b).count()
}

pub(crate) fn rate_text(r: &super::super::context::ModelRate, room: usize) -> String {
    let build = |cached: bool, suffix: bool| {
        let mut segs = vec![format!("in {}", per_million(r.input))];
        if cached && r.has_cache_prices {
            segs.push(format!("cached {}", per_million(r.cache_read)));
        }
        segs.push(format!("out {}", per_million(r.output)));
        let mut s = segs.join(" · ");
        if suffix {
            s.push_str(" / 1M");
        }
        s
    };
    for (cached, suffix) in [(true, true), (false, true), (false, false)] {
        let s = build(cached, suffix);
        if s.chars().count() <= room {
            return s;
        }
    }
    fit_chars(&build(false, false), room)
}

pub(crate) fn price_gradient(i: usize, w: usize) -> ratatui::style::Color {
    let t = crate::theme::theme();
    let stops = [t.success, t.accent, t.error_soft];
    let x = if w <= 1 {
        0.0
    } else {
        i as f64 / (w - 1) as f64
    };
    let seg = (x * 2.0).min(1.999);
    let (a, b, f) = (stops[seg as usize], stops[seg as usize + 1], seg.fract());
    crate::tui::blend_color(a, b, f as f32)
}

pub(crate) fn known_context(model: &str) -> Option<usize> {
    super::super::context::ensure_disk_loaded();
    let tail = model.rsplit('/').next().unwrap_or(model);
    [model, tail]
        .into_iter()
        .find_map(super::super::context::get_cached_model_context)
}

pub(crate) use crate::text_width::fit_chars;
