//! Hot stroke heads for the intro: cells that just gained ink are drawn
//! with denser braille (more dots lit = brighter), then cool back to the
//! real glyph over a few frames. Shape and colour of the settled mark are
//! untouched; there is no halo outside the strokes.

/// Frames a freshly inked cell stays hot.
const COOL_FRAMES: usize = 4;

fn bits(ch: char) -> u32 {
    match ch {
        '\u{2800}'..='\u{28ff}' => ch as u32 - 0x2800,
        _ => 0,
    }
}

/// Heat per cell (0 = cold, 1 = just drawn) for `frames[idx]`.
pub(super) fn heat_map(frames: &[(Vec<String>, Vec<String>)], idx: usize) -> Vec<Vec<f32>> {
    let Some((mark, edge)) = frames.get(idx) else {
        return Vec::new();
    };
    let cell = |f: usize, r: usize, c: usize| -> u32 {
        frames
            .get(f)
            .and_then(|(m, _)| m.get(r))
            .and_then(|l| l.chars().nth(c))
            .map_or(0, bits)
    };
    mark.iter()
        .enumerate()
        .map(|(r, line)| {
            let tips: Vec<char> = edge.get(r).map(|l| l.chars().collect()).unwrap_or_default();
            line.chars()
                .enumerate()
                .map(|(c, ch)| {
                    let now = bits(ch);
                    if now == 0 {
                        return 0.0;
                    }
                    if tips.get(c) == Some(&'1') {
                        return 1.0;
                    }
                    // How many frames back did this cell last gain dots?
                    (1..=COOL_FRAMES.min(idx))
                        .find(|&k| now & !cell(idx - k, r, c) != 0)
                        .map_or(0.0, |k| 1.0 - (k - 1) as f32 / COOL_FRAMES as f32)
                })
                .collect()
        })
        .collect()
}

/// `ch` thickened toward a full cell by `heat`: hot cells read whiter
/// because more dots are lit.
pub(super) fn heat_glyph(ch: char, heat: f32) -> char {
    let b = bits(ch);
    if b == 0 || heat <= 0.0 {
        return ch;
    }
    // Only the stroke tip itself (heat 1) thickens; the cooling trail
    // keeps its real glyph and is just brighter.
    let full = if heat >= 1.0 {
        // Each lit dot also lights the dots above/below and beside it.
        let col = |b: u32, l: [u32; 4]| -> u32 {
            let lit: Vec<bool> = l.iter().map(|&m| b & m != 0).collect();
            (0..4)
                .filter(|&i| lit[i] || (i > 0 && lit[i - 1]) || (i < 3 && lit[i + 1]))
                .fold(0, |a, i| a | l[i])
        };
        let (left, right) = ([0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]);
        let (l, r) = (col(b, left), col(b, right));
        let side = |from: u32, a: [u32; 4], b: [u32; 4]| {
            (0..4).filter(|&i| from & a[i] != 0).fold(0, |acc, i| acc | b[i])
        };
        l | r | side(l, left, right) | side(r, right, left)
    } else {
        b
    };
    char::from_u32(0x2800 + (b | full)).unwrap_or(ch)
}
