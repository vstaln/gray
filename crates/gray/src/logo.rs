use std::sync::OnceLock;

pub(crate) const MARK_ROWS: usize = 12;

const SAMPLES: usize = 3;
const GLOW_REACH: f32 = 18.0;
const DOT_BITS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

#[derive(Clone, Copy)]
pub(crate) struct MarkCell {
    pub glyph: char,
    pub glow: f32,
}

#[derive(serde::Deserialize)]
struct Stroke {
    path: Vec<[f32; 2]>,
    start: f32,
    dur: f32,
    ease: [f32; 4],
}

#[derive(serde::Deserialize)]
struct Pens {
    band: f32,
    hull: Vec<[f32; 2]>,
    end: f32,
    strokes: Vec<Stroke>,
}

fn pens() -> &'static Pens {
    static PENS: OnceLock<Pens> = OnceLock::new();
    PENS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/logo-pens.json"))
            .expect("bundled logo-pens.json parses")
    })
}

fn cubic_bezier(x: f32, [x1, y1, x2, y2]: [f32; 4]) -> f32 {
    let curve = |s: f32, p1: f32, p2: f32| {
        3.0 * (1.0 - s) * (1.0 - s) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if curve(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve((lo + hi) / 2.0, y1, y2).clamp(0.0, 1.0)
}

fn drawn(path: &[[f32; 2]], frac: f32) -> Vec<[f32; 2]> {
    let lens: Vec<f32> = path
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .collect();
    let mut want = frac * lens.iter().sum::<f32>();
    let mut out = vec![path[0]];
    for (w, len) in path.windows(2).zip(lens) {
        if want <= len {
            let k = if len > 0.0 { want / len } else { 0.0 };
            out.push([
                w[0][0] + (w[1][0] - w[0][0]) * k,
                w[0][1] + (w[1][1] - w[0][1]) * k,
            ]);
            return out;
        }
        want -= len;
        out.push(w[1]);
    }
    out
}

fn inside_hull(hull: &[[f32; 2]], x: f32, y: f32) -> bool {
    (0..hull.len()).all(|i| {
        let [ax, ay] = hull[i];
        let [bx, by] = hull[(i + 1) % hull.len()];
        (bx - ax) * (y - ay) - (by - ay) * (x - ax) >= 0.0
    })
}

fn on_stroke(path: &[[f32; 2]], half: f32, x: f32, y: f32) -> bool {
    path.windows(2).enumerate().any(|(k, w)| {
        let [ax, ay] = w[0];
        let (dx, dy) = (w[1][0] - ax, w[1][1] - ay);
        let len = dx.hypot(dy);
        if k > 0 && (x - ax).hypot(y - ay) <= half {
            return true;
        }
        if len <= f32::EPSILON {
            return false;
        }
        let along = ((x - ax) * dx + (y - ay) * dy) / len;
        let across = ((x - ax) * dy - (y - ay) * dx).abs() / len;
        (0.0..=len).contains(&along) && across <= half
    })
}

fn cell_aspect() -> f32 {
    let px = std::env::var("GRAY_CELL_PX")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once(['x', ';', ','])?;
            Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
        })
        .or_else(|| {
            let size = crossterm::terminal::window_size().ok()?;
            (size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0).then(|| {
                (
                    size.width as f32 / size.columns as f32,
                    size.height as f32 / size.rows as f32,
                )
            })
        });
    match px {
        Some((w, h)) if w > 0.0 && h > 0.0 => (h / w).clamp(1.4, 3.0),
        _ => 2.0,
    }
}

pub(crate) fn intro_time(progress: f32) -> f32 {
    let pens = pens();
    let first = pens
        .strokes
        .iter()
        .map(|s| s.start)
        .fold(f32::INFINITY, f32::min);
    first + progress.clamp(0.0, 1.0) * (pens.end - first)
}

pub(crate) fn mark_at(time: f32) -> Vec<Vec<MarkCell>> {
    let pens = pens();
    let (x0, x1) = pens
        .hull
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[0]), hi.max(p[0]))
        });
    let (y0, y1) = pens
        .hull
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[1]), hi.max(p[1]))
        });
    let cols = (((x1 - x0) / (y1 - y0)) * MARK_ROWS as f32 * cell_aspect())
        .round()
        .max(1.0) as usize;
    let (cell_w, cell_h) = ((x1 - x0) / cols as f32, (y1 - y0) / MARK_ROWS as f32);

    let mut paths = Vec::with_capacity(pens.strokes.len());
    let mut tips: Vec<[f32; 2]> = Vec::new();
    for s in &pens.strokes {
        let frac = if time >= s.start + s.dur {
            1.0
        } else {
            cubic_bezier(((time - s.start) / s.dur).clamp(0.0, 1.0), s.ease)
        };
        if frac <= 1e-6 {
            continue;
        }
        let path = drawn(&s.path, frac);
        if frac < 1.0 {
            tips.extend(path.last().copied());
        }
        paths.push(path);
    }
    let half = pens.band / 2.0;
    let covered = |x: f32, y: f32| {
        inside_hull(&pens.hull, x, y) && paths.iter().any(|p| on_stroke(p, half, x, y))
    };
    let (dot_w, dot_h) = (cell_w / 2.0, cell_h / 4.0);
    let dot_lit = |col: usize, row: usize, dx: usize, dy: usize| -> bool {
        let left = x0 + col as f32 * cell_w + dx as f32 * dot_w;
        let top = y0 + row as f32 * cell_h + dy as f32 * dot_h;
        let hits = (0..SAMPLES * SAMPLES)
            .filter(|i| {
                let sx = left + ((i % SAMPLES) as f32 + 0.5) / SAMPLES as f32 * dot_w;
                let sy = top + ((i / SAMPLES) as f32 + 0.5) / SAMPLES as f32 * dot_h;
                covered(sx, sy)
            })
            .count();
        hits * 2 >= SAMPLES * SAMPLES
    };

    (0..MARK_ROWS)
        .map(|row| {
            (0..cols)
                .map(|col| {
                    let bits = (0..4)
                        .flat_map(|dy| (0..2).map(move |dx| (dx, dy)))
                        .filter(|&(dx, dy)| dot_lit(col, row, dx, dy))
                        .fold(0, |m, (dx, dy)| m | DOT_BITS[dy][dx]);
                    let cx = x0 + (col as f32 + 0.5) * cell_w;
                    let cy = y0 + (row as f32 + 0.5) * cell_h;
                    let glow = tips
                        .iter()
                        .map(|t| (1.0 - (t[0] - cx).hypot(t[1] - cy) / GLOW_REACH).max(0.0))
                        .fold(0.0f32, f32::max);
                    MarkCell {
                        glyph: if bits == 0 {
                            ' '
                        } else {
                            char::from_u32(0x2800 + bits).unwrap_or(' ')
                        },
                        glow: glow * glow,
                    }
                })
                .collect()
        })
        .collect()
}

pub(crate) fn mark() -> Vec<Vec<MarkCell>> {
    mark_at(pens().end)
}
