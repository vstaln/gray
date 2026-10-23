//! Kitty graphics protocol previews.
//!
//! On terminals that speak the kitty graphics protocol (ghostty, kitty,
//! wezterm) image previews render as real pixels instead of half-block
//! approximations. The image is transmitted once per preview size, then each
//! transcript cell carries U+10EEEE plus row/column diacritics so the
//! terminal paints the matching tile there. The placeholder cells live in
//! the normal text grid, so they scroll and reflow like any other line.
//! Unsupported terminals fall back to the half-block renderer.
//!
//! Cell pixel size comes from a CSI-16-t query on /dev/tty (cached once);
//! `GRAY_CELL_PX=WxH` overrides it when the probe loses the stdin race.

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::Engine;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const PLACEHOLDER: char = '\u{10EEEE}';

/// Diacritics that encode placeholder row/column indices, in the order the
/// protocol expects. From kitty's `gen/rowcolumn-diacritics.txt`.
const DIACRITICS: &[char] = &[
    '\u{305}',
    '\u{30d}',
    '\u{30e}',
    '\u{310}',
    '\u{312}',
    '\u{33d}',
    '\u{33e}',
    '\u{33f}',
    '\u{346}',
    '\u{34a}',
    '\u{34b}',
    '\u{34c}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35b}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36a}',
    '\u{36b}',
    '\u{36c}',
    '\u{36d}',
    '\u{36e}',
    '\u{36f}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59c}',
    '\u{59d}',
    '\u{59e}',
    '\u{59f}',
    '\u{5a0}',
    '\u{5a1}',
    '\u{5a8}',
    '\u{5a9}',
    '\u{5ab}',
    '\u{5ac}',
    '\u{5af}',
    '\u{5c4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65a}',
    '\u{65b}',
    '\u{65d}',
    '\u{65e}',
    '\u{6d6}',
    '\u{6d7}',
    '\u{6d8}',
    '\u{6d9}',
    '\u{6da}',
    '\u{6db}',
    '\u{6dc}',
    '\u{6df}',
    '\u{6e0}',
    '\u{6e1}',
    '\u{6e2}',
    '\u{6e4}',
    '\u{6e7}',
    '\u{6e8}',
    '\u{6eb}',
    '\u{6ec}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73a}',
    '\u{73d}',
    '\u{73f}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74a}',
    '\u{7eb}',
    '\u{7ec}',
    '\u{7ed}',
    '\u{7ee}',
    '\u{7ef}',
    '\u{7f0}',
    '\u{7f1}',
    '\u{7f3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81b}',
    '\u{81c}',
    '\u{81d}',
    '\u{81e}',
    '\u{81f}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82a}',
    '\u{82b}',
    '\u{82c}',
    '\u{82d}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{f82}',
    '\u{f83}',
    '\u{f86}',
    '\u{f87}',
    '\u{135d}',
    '\u{135e}',
    '\u{135f}',
    '\u{17dd}',
    '\u{193a}',
    '\u{1a17}',
    '\u{1a75}',
    '\u{1a76}',
    '\u{1a77}',
    '\u{1a78}',
    '\u{1a79}',
    '\u{1a7a}',
    '\u{1a7b}',
    '\u{1a7c}',
    '\u{1b6b}',
    '\u{1b6d}',
    '\u{1b6e}',
    '\u{1b6f}',
    '\u{1b70}',
    '\u{1b71}',
    '\u{1b72}',
    '\u{1b73}',
    '\u{1cd0}',
    '\u{1cd1}',
    '\u{1cd2}',
    '\u{1cda}',
    '\u{1cdb}',
    '\u{1ce0}',
    '\u{1dc0}',
    '\u{1dc1}',
    '\u{1dc3}',
    '\u{1dc4}',
    '\u{1dc5}',
    '\u{1dc6}',
    '\u{1dc7}',
    '\u{1dc8}',
    '\u{1dc9}',
    '\u{1dcb}',
    '\u{1dcc}',
    '\u{1dd1}',
    '\u{1dd2}',
    '\u{1dd3}',
    '\u{1dd4}',
    '\u{1dd5}',
    '\u{1dd6}',
    '\u{1dd7}',
    '\u{1dd8}',
    '\u{1dd9}',
    '\u{1dda}',
    '\u{1ddb}',
    '\u{1ddc}',
    '\u{1ddd}',
    '\u{1dde}',
    '\u{1ddf}',
    '\u{1de0}',
    '\u{1de1}',
    '\u{1de2}',
    '\u{1de3}',
    '\u{1de4}',
    '\u{1de5}',
    '\u{1de6}',
    '\u{1dfe}',
    '\u{20d0}',
    '\u{20d1}',
    '\u{20d4}',
    '\u{20d5}',
    '\u{20d6}',
    '\u{20d7}',
    '\u{20db}',
    '\u{20dc}',
    '\u{20e1}',
    '\u{20e7}',
    '\u{20e9}',
    '\u{20f0}',
    '\u{2cef}',
    '\u{2cf0}',
    '\u{2cf1}',
    '\u{2de0}',
    '\u{2de1}',
    '\u{2de2}',
    '\u{2de3}',
    '\u{2de4}',
    '\u{2de5}',
    '\u{2de6}',
    '\u{2de7}',
    '\u{2de8}',
    '\u{2de9}',
    '\u{2dea}',
    '\u{2deb}',
    '\u{2dec}',
    '\u{2ded}',
    '\u{2dee}',
    '\u{2def}',
    '\u{2df0}',
    '\u{2df1}',
    '\u{2df2}',
    '\u{2df3}',
    '\u{2df4}',
    '\u{2df5}',
    '\u{2df6}',
    '\u{2df7}',
    '\u{2df8}',
    '\u{2df9}',
    '\u{2dfa}',
    '\u{2dfb}',
    '\u{2dfc}',
    '\u{2dfd}',
    '\u{2dfe}',
    '\u{2dff}',
    '\u{a66f}',
    '\u{a67c}',
    '\u{a67d}',
    '\u{a6f0}',
    '\u{a6f1}',
    '\u{a8e0}',
    '\u{a8e1}',
    '\u{a8e2}',
    '\u{a8e3}',
    '\u{a8e4}',
    '\u{a8e5}',
    '\u{a8e6}',
    '\u{a8e7}',
    '\u{a8e8}',
    '\u{a8e9}',
    '\u{a8ea}',
    '\u{a8eb}',
    '\u{a8ec}',
    '\u{a8ed}',
    '\u{a8ee}',
    '\u{a8ef}',
    '\u{a8f0}',
    '\u{a8f1}',
    '\u{aab0}',
    '\u{aab2}',
    '\u{aab3}',
    '\u{aab7}',
    '\u{aab8}',
    '\u{aabe}',
    '\u{aabf}',
    '\u{aac1}',
    '\u{fe20}',
    '\u{fe21}',
    '\u{fe22}',
    '\u{fe23}',
    '\u{fe24}',
    '\u{fe25}',
    '\u{fe26}',
    '\u{10a0f}',
    '\u{10a38}',
    '\u{1d185}',
    '\u{1d186}',
    '\u{1d187}',
    '\u{1d188}',
    '\u{1d189}',
    '\u{1d1aa}',
    '\u{1d1ab}',
    '\u{1d1ac}',
    '\u{1d1ad}',
    '\u{1d242}',
    '\u{1d243}',
    '\u{1d244}',
];

/// Maximum placeholder rows/cols is the diacritic table length; previews are
/// far smaller, but clamp anyway.
fn diacritic(i: usize) -> char {
    DIACRITICS[i.min(DIACRITICS.len() - 1)]
}

/// Terminal cell size in pixels: (width, height).
fn cell_pixels() -> (u32, u32) {
    static CELL: OnceLock<(u32, u32)> = OnceLock::new();
    *CELL.get_or_init(|| {
        if let Some((w, h)) = std::env::var("GRAY_CELL_PX").ok().and_then(|v| {
            v.split_once(['x', ';', ','])
                .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        }) && w > 0
            && h > 0
        {
            return (w, h);
        }
        query_cell_pixels().unwrap_or((10, 20))
    })
}

/// Force the cell-size probe now, before any stdin reader exists. The
/// OnceLock caches the result (including the fallback), so a later preview
/// never races the event loop for the reply — a reply read by crossterm
/// mid-turn decodes as an Esc press plus stray characters.
pub(crate) fn warm_cell_pixels() {
    if supported() {
        let _ = cell_pixels();
    }
}

/// Ask the terminal how big a character cell is: CSI 16 t -> CSI 6 ; h ; w t.
/// Best-effort: the TUI's input thread can win the race for the reply, in
/// which case we give up and the caller uses the fallback geometry.
#[cfg(unix)]
fn query_cell_pixels() -> Option<(u32, u32)> {
    use std::os::unix::io::AsRawFd;
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    tty.write_all(b"\x1b[16t").ok()?;
    tty.flush().ok()?;
    let fd = tty.as_raw_fd();
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
    let mut buf: Vec<u8> = Vec::with_capacity(32);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let rc = unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) };
        if rc <= 0 {
            return None;
        }
        let mut chunk = [0u8; 64];
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if buf.contains(&b't') || buf.len() > 64 {
            break;
        }
    }
    let s = String::from_utf8_lossy(&buf);
    let body = s.strip_prefix("\x1b[6;")?.strip_suffix('t')?;
    let (h, w) = body.split_once(';')?;
    let h: u32 = h.parse().ok()?;
    let w: u32 = w.parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(not(unix))]
fn query_cell_pixels() -> Option<(u32, u32)> {
    None
}

fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        match std::env::var("GRAY_IMAGES").ok().as_deref() {
            Some("kitty") => return true,
            Some("off") | Some("blocks") | Some("halfblock") => return false,
            _ => {}
        }
        if std::env::var_os("TMUX").is_some() {
            return false;
        }
        if !std::io::stdout().is_terminal() {
            return false;
        }
        let prog = std::env::var("TERM_PROGRAM").unwrap_or_default();
        let term = std::env::var("TERM").unwrap_or_default();
        prog.contains("ghostty")
            || prog.contains("kitty")
            || prog == "WezTerm"
            || term.contains("kitty")
            || term.contains("ghostty")
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var_os("WEZTERM_PANE").is_some()
    })
}

fn image_id(path: &Path, cols: u32, rows: u32) -> u32 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    cols.hash(&mut h);
    rows.hash(&mut h);
    ((h.finish() as u32) & 0x00ff_ffff) | 1
}

/// (path, cols, rows) already pushed to the terminal this process.
fn transmitted() -> &'static Mutex<HashSet<(PathBuf, u32, u32)>> {
    static SET: OnceLock<Mutex<HashSet<(PathBuf, u32, u32)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `\x1b[3J` drops scrollback and the virtual placements tied to those
/// cells; the next preview must re-upload, so forget what we sent.
pub(crate) fn reset_transmitted() {
    if let Ok(mut set) = transmitted().lock() {
        set.clear();
    }
}

/// Queued image uploads. APC chunks must never interleave with frame bytes:
/// backends emit escapes as fmt-piece writes (a MoveTo is several write()
/// calls), so a transmit mid-frame tears a CSI — orphaning text like `;1H`
/// and eating the following output into a void. Drained between frames by
/// [`flush_transmits`].
type PendingTx = (u32, Vec<u8>, u32, u32, u32, u32);

fn pending() -> &'static Mutex<VecDeque<PendingTx>> {
    static P: OnceLock<Mutex<VecDeque<PendingTx>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Transmit raw RGBA as a kitty virtual placement: `a=T` stores and displays,
/// `U=1` keeps it virtual so only the unicode-placeholder cells render it.
fn transmit(id: u32, rgba: &[u8], w: u32, h: u32, cols: u32, rows: u32) -> std::io::Result<()> {
    pending()
        .lock()
        .map_err(|_| std::io::Error::other("pending lock"))?
        .push_back((id, rgba.to_vec(), w, h, cols, rows));
    Ok(())
}

/// Write all queued uploads as contiguous APC chunks under one stdout lock.
/// Called from `CustomTerminal::draw` after the frame flush, on the TUI
/// thread, so chunks can only ever sit between complete frames.
pub(crate) fn flush_transmits() {
    let Ok(mut q) = pending().lock() else {
        return;
    };
    if q.is_empty() {
        return;
    }
    let mut out = std::io::stdout().lock();
    const RAW_CHUNK: usize = 3072; // 4096 base64 chars
    while let Some((id, rgba, w, h, cols, rows)) = q.pop_front() {
        let n = rgba.len().div_ceil(RAW_CHUNK);
        for (i, chunk) in rgba.chunks(RAW_CHUNK).enumerate() {
            let more = u8::from(i + 1 < n);
            let head = if i == 0 {
                write!(
                    out,
                    "\x1b_Gq=2,i={id},a=T,U=1,f=32,t=d,s={w},v={h},c={cols},r={rows},m={more};"
                )
            } else {
                write!(out, "\x1b_Gq=2,m={more};")
            };
            let wrote = head
                .and_then(|_| {
                    out.write_all(
                        base64::engine::general_purpose::STANDARD
                            .encode(chunk)
                            .as_bytes(),
                    )
                })
                .and_then(|_| out.write_all(b"\x1b\\"));
            if wrote.is_err() {
                q.clear();
                return;
            }
        }
    }
    let _ = out.flush();
}

fn trim_transparent_margins(img: image::DynamicImage) -> Option<image::DynamicImage> {
    let rgba = img.to_rgba8();
    let (mut l, mut t, mut r, mut b) = (rgba.width(), rgba.height(), 0u32, 0u32);
    let mut any = false;
    for (x, y, p) in rgba.enumerate_pixels() {
        if p[3] > 8 {
            any = true;
            l = l.min(x);
            t = t.min(y);
            r = r.max(x);
            b = b.max(y);
        }
    }
    if !any {
        return None;
    }
    Some(img.crop_imm(l, t, r - l + 1, b - t + 1))
}

/// Transcript lines that render `path` as a kitty image, or `None` when the
/// terminal can't do it / the image won't load.
pub(crate) fn preview_lines(
    path: &Path,
    max_w: usize,
    hang_cells: usize,
) -> Option<Vec<Line<'static>>> {
    if !supported() {
        return None;
    }
    let img = image::open(path).ok()?;
    let img = trim_transparent_margins(img)?;
    let cols = max_w.clamp(8, 48) as u32;
    let (cw, ch) = cell_pixels();
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    // Natural footprint is 1 image px : 1 screen px; cap at `cols` cells wide
    // so big images shrink but small ones don't get upscaled to mush.
    let c_nat = iw.div_ceil(cw).clamp(1, cols);
    // Rows that keep the source aspect under the real cell geometry.
    let r_nat = ((ih as u64 * c_nat as u64 * cw as u64 + (iw as u64 * ch as u64) / 2)
        / (iw as u64 * ch as u64))
        .clamp(1, (DIACRITICS.len() - 1) as u64) as u32;
    const MAX_PREVIEW_ROWS: u32 = 12;
    let (c, r) = if r_nat > MAX_PREVIEW_ROWS {
        let c2 = ((iw as u64 * MAX_PREVIEW_ROWS as u64 * ch as u64 + (ih as u64 * cw as u64) / 2)
            / (ih as u64 * cw as u64))
            .clamp(1, c_nat as u64) as u32;
        (c2, MAX_PREVIEW_ROWS)
    } else {
        (c_nat, r_nat)
    };
    // Bake the thumbnail at exactly display resolution; kitty maps it onto
    // the c-by-r placeholder grid, so matching c*cw x r*ch keeps the aspect.
    let thumb =
        image::imageops::resize(&img, c * cw, r * ch, image::imageops::FilterType::Lanczos3);
    let (tw, th) = thumb.dimensions();
    if tw == 0 || th == 0 {
        return None;
    }
    let id = image_id(path, c, r);
    let key = (path.to_path_buf(), c, r);
    {
        let mut set = transmitted().lock().ok()?;
        if set.insert(key) {
            transmit(id, thumb.as_raw(), tw, th, c, r).ok()?;
        }
    }
    let [_, ir, ig, ib] = id.to_be_bytes();
    let id_color = Color::Rgb(ir, ig, ib);
    let surface = crate::theme::theme().surface_bg;
    let hang = " ".repeat(hang_cells);
    let mut lines = Vec::with_capacity(r as usize);
    for y in 0..r {
        let mut spans = vec![Span::styled(hang.clone(), Style::default().bg(surface))];
        for x in 0..c {
            let mut cell = String::with_capacity(8);
            cell.push(PLACEHOLDER);
            cell.push(diacritic(y as usize));
            cell.push(diacritic(x as usize));
            cell.push(diacritic(0));
            spans.push(Span::styled(
                cell,
                Style::default().fg(id_color).bg(surface),
            ));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reset_transmitted_forces_reupload() {
        super::transmitted()
            .lock()
            .unwrap()
            .insert((std::path::PathBuf::from("/tmp/x.png"), 8, 4));
        super::reset_transmitted();
        assert!(super::transmitted().lock().unwrap().is_empty());
    }

    #[test]
    fn placeholder_cells_are_single_width() {
        let mut cell = String::new();
        cell.push(super::PLACEHOLDER);
        cell.push(super::DIACRITICS[3]);
        cell.push(super::DIACRITICS[5]);
        cell.push(super::DIACRITICS[0]);
        assert_eq!(crate::text_width::display_width(&cell), 1);
    }
}
