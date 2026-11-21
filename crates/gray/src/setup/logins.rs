use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::text_width::fit_chars;

const CMD_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RED: Color = Color::Rgb(232, 112, 112);

pub(crate) fn cli_argvs() -> HashMap<String, Vec<String>> {
    let Ok(home) = super::catalog::gray_home() else {
        return HashMap::new();
    };
    let Ok(lock) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(&home)) else {
        return HashMap::new();
    };
    lock.plugins
        .into_iter()
        .filter(|(_, e)| e.enabled)
        .filter_map(|(name, e)| {
            let argv = e
                .cli_argv
                .filter(|a| !a.is_empty())
                .or_else(|| Some(e.argv).filter(|a| !a.is_empty()))?;
            Some((name, argv))
        })
        .collect()
}

pub(crate) fn gray_bin() -> std::ffi::OsString {
    std::env::current_exe()
        .map(Into::into)
        .unwrap_or_else(|_| "gray".into())
}

fn first_err_line(stderr: &[u8], fallback: &str) -> String {
    let err = String::from_utf8_lossy(stderr);
    err.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("error: ").to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn run(argv: &[String], args: &[&str], timeout: Duration) -> Result<String, String> {
    let (prog, base) = argv.split_first().ok_or("empty argv")?;
    let mut child = std::process::Command::new(prog)
        .args(base)
        .args(args)
        .env("GRAY_BIN", gray_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !out.status.success() {
        return Err(first_err_line(&out.stderr, "command failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn account_head(
    pinned: bool,
    label: Option<&str>,
    email: Option<&str>,
    org: Option<&str>,
    lit: bool,
    w: usize,
) -> Vec<Span<'static>> {
    let t = crate::theme::theme();
    let pick = |c: Color| {
        if lit {
            Style::default().fg(t.on_selection)
        } else {
            Style::default().fg(c)
        }
    };
    let mut head = vec![Span::styled(
        if pinned { " ● " } else { "   " },
        pick(t.accent),
    )];
    let mut used = 3;
    match label.filter(|l| !l.is_empty()) {
        Some(l) => {
            head.push(Span::styled(
                fit_chars(l, w.saturating_sub(used)),
                pick(t.text_body).add_modifier(Modifier::BOLD),
            ));
            return head;
        }
        None if label.is_none() => {
            let who = email.unwrap_or("signed in").to_string();
            head.push(Span::styled(
                fit_chars(&who, w.saturating_sub(used)),
                pick(t.text_body),
            ));
            used = w;
        }
        None => {}
    }
    if used < w {
        let who = match (email, org) {
            (Some(e), Some(o)) => format!("{e} · {o}"),
            (Some(e), None) => e.to_string(),
            (None, Some(o)) => o.to_string(),
            (None, None) => String::new(),
        };
        head.push(Span::styled(
            fit_chars(&who, w.saturating_sub(used)),
            pick(t.text_dim),
        ));
    }
    head
}

pub(crate) fn portal_section(name: &str, who: Option<&str>, w: usize) -> Vec<Line<'static>> {
    let t = crate::theme::theme();
    let fg = |c: Color| Style::default().fg(c);
    let meta = "  ·  signed in";
    vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                fit_chars(name, w.saturating_sub(meta.chars().count() + 1)),
                fg(t.text_bright).add_modifier(Modifier::BOLD),
            ),
            Span::styled(meta, fg(t.text_dim)),
        ]),
        Line::from(account_head(true, None, who, None, false, w)),
    ]
}

pub(crate) fn account_argv() -> Option<Vec<String>> {
    cli_argvs().remove("account")
}

pub(crate) fn portal_cmd(args: &[&str]) -> Result<String, String> {
    let argv = account_argv().ok_or("the account plugin is not installed")?;
    run(&argv, args, CMD_TIMEOUT)
}
