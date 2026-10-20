use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use gray_core::spill;
use gray_tools::shell::contract::PUMP_DRAIN_TIMEOUT;
use gray_tools::shell::pump::Pump;
use serde_json::{Value, json};

pub async fn run_prompt_child(cwd: &Path, prompt: &str) -> Value {
    // After `gray update` the running image reads as "<path> (deleted)";
    // the child must exec the replacement at <path>.
    let exe = match std::env::current_exe() {
        Ok(p) => p
            .to_str()
            .and_then(|s| s.strip_suffix(" (deleted)"))
            .map(std::path::PathBuf::from)
            .unwrap_or(p),
        Err(e) => return json!({"error": format!("host/run: current_exe: {e}")}),
    };
    let mut child = match tokio::process::Command::new(exe)
        .arg("-p")
        .arg(prompt)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return json!({"error": format!("host/run spawn: {e}")}),
    };

    let id = uuid::Uuid::new_v4().as_simple().to_string();
    let log_dir = spill::dir().unwrap_or_else(std::env::temp_dir);
    let log_path = log_dir.join(format!("{}.txt", &id[..spill::HANDLE_LEN]));
    let pump = Pump::start(
        child.stdout.take(),
        child.stderr.take(),
        log_path.clone(),
        Arc::new(AtomicU64::new(0)),
        None,
    );

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => return json!({"error": format!("host/run wait: {e}")}),
    };
    let summary = match tokio::time::timeout(PUMP_DRAIN_TIMEOUT, pump).await {
        Ok(Ok(s)) => s,
        _ => {
            return json!({"error": "host/run: output pump failed", "output_file": log_path.display().to_string()});
        }
    };
    if !status.success() {
        return json!({"error": format!("host/run: gray -p exited {status}"), "output_file": log_path.display().to_string()});
    }
    let mut text = String::from_utf8_lossy(&summary.tail).trim().to_string();
    if text.is_empty() {
        return json!({"error": "host/run: empty reply", "output_file": log_path.display().to_string()});
    }
    if summary.total_bytes <= summary.tail.len() as u64 {
        return json!({"text": text});
    }
    let path = log_path.display().to_string();
    text.push_str(&format!(
        "\n\n[Showing last {} of {}. Full output: {path}]",
        spill::fmt_bytes(summary.tail.len()),
        spill::fmt_bytes(summary.total_bytes as usize)
    ));
    json!({"text": text, "output_file": path})
}
