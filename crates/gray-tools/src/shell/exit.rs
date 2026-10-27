#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use super::contract::ExitReport;

use super::split::split_pipeline;

pub fn exit_report(
    status: std::process::ExitStatus,
    command: &str,
    pipe_stages: Option<&[i32]>,
) -> ExitReport {
    let code = status.code();
    #[cfg(unix)]
    let signal: Option<i32> = status.signal();
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let (effective, label) = match (code, signal) {
        (Some(n), _) => (n, format!("exit {n}")),
        (None, Some(s)) => {
            let eff = 128 + s;

            #[allow(unused_mut)]
            let mut label = format!("exit {eff} ({})", signal_name(s));
            #[cfg(unix)]
            if status.core_dumped() {
                label.push_str(" (core dumped)");
            }
            (eff, label)
        }
        (None, None) => (1, "exit unknown".to_string()),
    };
    let note = if effective == 0 {
        masked_note(command, pipe_stages)
    } else {
        None
    };
    ExitReport {
        effective,
        label,
        note,
    }
}

fn signal_name(sig: i32) -> String {
    let name = match sig {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        6 => "ABRT",
        9 => "KILL",
        11 => "SEGV",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        24 => "XCPU",
        25 => "XFSZ",
        _ => return format!("SIG{sig}"),
    };
    format!("SIG{name}")
}

fn base_head(cmd: &str) -> &str {
    let head = cmd.split_whitespace().next().unwrap_or("");
    head.rsplit('/').next().unwrap_or(head)
}

const TAIL_CMDS: [&str; 8] = ["tail", "head", "grep", "tee", "sort", "wc", "less", "cat"];

pub fn masks_pipeline(command: &str) -> bool {
    let segs = split_pipeline(command);
    if segs.len() < 2 {
        return false;
    }
    let last = base_head(&segs[segs.len() - 1]);
    UNMASKABLE_TAILS.contains(&last) || TAIL_CMDS.contains(&last)
}

fn masked_note(command: &str, stages: Option<&[i32]>) -> Option<String> {
    let segs = split_pipeline(command);
    let stages = stages.filter(|s| segs.len() >= 2 && s.len() == segs.len())?;
    let sigpipe = 128 + 13;
    if !stages[..stages.len() - 1]
        .iter()
        .any(|c| *c != 0 && *c != sigpipe)
    {
        return None;
    }
    let last = base_head(&segs[segs.len() - 1]);
    if !UNMASKABLE_TAILS.contains(&last) && !TAIL_CMDS.contains(&last) {
        return None;
    }
    if segs.len() > 2 {
        return Some(format!(
            "`{last}` masks earlier stages' exit; rerun without the pipe to check"
        ));
    }
    let first = base_head(&segs[0]);
    Some(format!(
        "`{last}` masks `{first}`'s exit; rerun `{first}` alone for its status"
    ))
}

const UNMASKABLE_TAILS: [&str; 9] = [
    "head", "tail", "cat", "less", "awk", "sed", "tr", "cut", "column",
];
