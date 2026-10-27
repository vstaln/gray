use std::path::Path;
use std::time::Duration;

use super::contract::{ExitReport, VIEW_HEAD_FRACTION, View};

pub fn fmt_num(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut groups: Vec<String> = Vec::new();
    while n > 0 {
        groups.push(format!("{:03}", n % 1000));
        n /= 1000;
    }
    let mut out = groups.pop().unwrap().trim_start_matches('0').to_string();
    if out.is_empty() {
        out = "0".to_string();
    }
    while let Some(g) = groups.pop() {
        out.push(',');
        out.push_str(&g);
    }
    out
}

fn fmt_num_u64(n: u64) -> String {
    fmt_num(n as usize)
}

pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 10 {
        format!("{:.1}s", d.as_secs_f64())
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn home_relative(p: &Path) -> String {
    let s = p.to_string_lossy();

    let under = |root: &str| -> bool {
        !root.is_empty()
            && (s.as_ref() == root
                || s.starts_with(&format!("{root}/"))
                || s.starts_with(&format!("{root}\\"))
                || s.eq_ignore_ascii_case(root))
    };
    if let Ok(gray) = std::env::var("GRAY_HOME")
        && !gray.trim().is_empty()
        && under(gray.trim())
    {
        return format!(
            "~/{}",
            s[gray.trim().len()..].trim_start_matches(['/', '\\'])
        );
    }
    if let Ok(home) = std::env::var("HOME")
        && under(&home)
    {
        return format!("~/{}", s[home.len()..].trim_start_matches(['/', '\\']));
    }
    s.into_owned()
}

fn sanitize(log: &[u8]) -> String {
    let filtered: Vec<u8> = log
        .iter()
        .filter(|&&b| b == 0x09 || b == 0x0A || b == 0x0D || b >= 0x20)
        .copied()
        .collect();
    String::from_utf8_lossy(&filtered)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn count_lines(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    let nl = s.bytes().filter(|&b| b == b'\n').count();
    if s.ends_with('\n') { nl } else { nl + 1 }
}

fn take_head(s: &str, byte_budget: usize, line_budget: usize) -> String {
    if s.is_empty() || byte_budget == 0 || line_budget == 0 {
        return String::new();
    }
    let mut end = s.floor_char_boundary(byte_budget.min(s.len()));
    if end < s.len()
        && let Some(nl) = s[..end].rfind('\n')
    {
        end = nl;
    }

    let mut head = s[..end].trim_end_matches('\n').to_string();
    if count_lines(&head) > line_budget {
        let keep: Vec<&str> = head.lines().take(line_budget).collect();
        head = keep.join("\n");
    }
    head
}

fn take_tail(s: &str, byte_budget: usize, line_budget: usize) -> String {
    if s.is_empty() || byte_budget == 0 || line_budget == 0 {
        return String::new();
    }
    let mut start = s.ceil_char_boundary(s.len().saturating_sub(byte_budget));
    if start > 0 && start < s.len() {
        if let Some(nl) = s[start..].find('\n') {
            let after = start + nl + 1;
            if after < s.len() {
                start = after;
            }
        }
    } else if start == 0 {
    }
    let mut tail = s[start..].trim_start_matches('\n').to_string();

    tail = tail.trim_end_matches('\n').to_string();
    if count_lines(&tail) > line_budget {
        let lines: Vec<&str> = tail.lines().collect();
        let n = lines.len();
        tail = lines[n - line_budget..].join("\n");
    }
    tail
}

pub fn middle_out(log: &[u8], budget_bytes: usize, budget_lines: usize, base_offset: u64) -> View {
    let s = sanitize(log);
    let total_bytes = s.len() as u64;
    let total_lines = count_lines(&s);

    let has_cr = log.contains(&b'\r');
    if s.len() <= budget_bytes && total_lines <= budget_lines {
        return View {
            body: s,
            shown_lines: (total_lines, 0),
            omitted_lines: 0,
            omitted_bytes: 0,
            omitted_range: None,
            total_lines,
            total_bytes,
            has_cr,
        };
    }
    let head_byte_budget = (budget_bytes as f32 * VIEW_HEAD_FRACTION).floor() as usize;
    let tail_byte_budget = budget_bytes.saturating_sub(head_byte_budget);
    let head_line_budget = (budget_lines as f32 * VIEW_HEAD_FRACTION).floor() as usize;
    let tail_line_budget = budget_lines - head_line_budget;
    let head = take_head(&s, head_byte_budget, head_line_budget);
    let tail = take_tail(&s, tail_byte_budget, tail_line_budget);
    let (head_lines, tail_lines) = (count_lines(&head), count_lines(&tail));
    let omitted_lines = total_lines.saturating_sub(head_lines + tail_lines);
    let omitted_bytes = s.len().saturating_sub(head.len() + tail.len());
    let omitted_range = Some((
        base_offset + head.len() as u64,
        base_offset + total_bytes.saturating_sub(tail.len() as u64),
    ));
    View {
        body: format!("{}\n{{{{MARKER}}}}\n{}", head, tail),
        shown_lines: (head_lines, tail_lines),
        omitted_lines,
        omitted_bytes,
        omitted_range,
        total_lines,
        total_bytes,
        has_cr,
    }
}

pub fn squeeze(body: &str) -> String {
    collapse_repeats(&strip_ansi(body))
}

const REPEAT_RUN_MIN: usize = 3;

pub fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            let start = i;
            i += 1;
            while i < bytes.len() && !s.is_char_boundary(i) {
                i += 1;
            }
            out.push_str(&s[start..i]);
            continue;
        }

        if bytes.get(i + 1) == Some(&b']') {
            i += 2;
            while i < bytes.len() {
                if bytes[i] == 0x07 {
                    i += 1;
                    break;
                }
                if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        if bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i += usize::from(i < bytes.len());
            continue;
        }

        i += 1;
        while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
            i += 1;
        }
        i += usize::from(i < bytes.len());
    }
    out
}

fn collapse_repeats(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut lines = s.lines().peekable();
    while let Some(line) = lines.next() {
        let mut run = 1;
        while lines.peek() == Some(&line) {
            lines.next();
            run += 1;
        }
        if run < REPEAT_RUN_MIN {
            for _ in 0..run {
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push_str(&format!(" [… \u{d7} {} more]", run - 1));
        out.push('\n');
    }
    out
}

pub fn resume_hint(view: &View, log_path: &std::path::Path) -> String {
    let Some((a, b)) = view.omitted_range else {
        return String::new();
    };
    if view.omitted_lines == 0 && view.omitted_bytes == 0 {
        return String::new();
    }

    let first = view.shown_lines.0 + 1;
    let last = view.shown_lines.0 + view.omitted_lines;
    format!(
        "[\u{2026} {} lines / {} chars omitted (bytes {}\u{2013}{}, lines {first}\u{2013}{last}). page: sed -n '{first},{last}p' {}]",
        fmt_num(view.omitted_lines),
        fmt_num(view.omitted_bytes),
        fmt_num_u64(a),
        fmt_num_u64(b),
        log_path.display(),
    )
}

pub fn header(
    report: &ExitReport,
    view: Option<&View>,
    elapsed: Duration,
    log_path: &std::path::Path,
) -> String {
    let label = report.label.clone();
    let note = report
        .note
        .as_ref()
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    let total = view.map(|v| v.total_lines).unwrap_or(0);
    let showing = match view {
        Some(v) if v.omitted_lines > 0 || v.omitted_bytes > 0 => format!(
            " \u{00b7} showing first {} + last {} \u{00b7} {} lines / {} chars omitted \u{00b7} grep the log for more",
            fmt_num(v.shown_lines.0),
            fmt_num(v.shown_lines.1),
            fmt_num(v.omitted_lines),
            fmt_num(v.omitted_bytes),
        ),
        _ => String::new(),
    };
    let folded = match view {
        Some(v) if v.has_cr => " \u{00b7} CR folded for display",
        _ => "",
    };
    let mut out = format!(
        "{label}{note} \u{00b7} {} \u{00b7} {} lines{showing}{folded} \u{00b7} log {}",
        format_elapsed(elapsed),
        fmt_num(total),
        home_relative(log_path),
    );
    if total == 0 {
        out.push_str(" \u{00b7} no output");
    }
    out
}
