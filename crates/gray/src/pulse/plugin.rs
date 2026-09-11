use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

use crate::pulse::{config, goal, job};

const TOOL_NAME: &str = "pulse";

/// The manifest reply advertised during the host handshake.
fn manifest() -> Value {
    json!({
        "name": TOOL_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [{
            "name": TOOL_NAME,
            "description": "Manage gray's 24/7 standing-goal pulse: status, goal, on/off, sync.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "goal_get", "goal_set", "on", "off", "sync"]
                    },
                    "text": {"type": "string", "description": "Goal text for goal_set."},
                    "schedule": {"type": "string", "description": "Schedule for on, e.g. 'every 30m'."},
                    "deliver": {"type": "string", "description": "Delivery target for on: local | origin | <target>."}
                },
                "required": ["action"]
            }
        }],
        "commands": [],
        "hooks": []
    })
}

/// Handle one NDJSON request line, returning the JSON reply line when one is
/// due. Returns `None` for `plugin/shutdown` and for notifications (no `id`).
pub fn handle_line(line: &str) -> Option<String> {
    handle_line_at(line, gray_gateway::config::gray_home_dir().ok().as_deref())
}

fn handle_line_at(line: &str, home: Option<&Path>) -> Option<String> {
    let req: Value = serde_json::from_str(line).ok()?;
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "plugin/shutdown" {
        return None;
    }
    let id = req.get("id").and_then(Value::as_u64)?;
    let result = match method {
        "plugin/manifest" => manifest(),
        "tool/call" => tool_call(req.get("params"), home),
        _ => return None,
    };
    Some(json!({"id": id, "result": result}).to_string())
}

fn tool_call(params: Option<&Value>, home: Option<&Path>) -> Value {
    let empty = Value::Null;
    let params = params.unwrap_or(&empty);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != TOOL_NAME {
        return call_reply(Err(anyhow::anyhow!("unknown tool: {name}")));
    }
    let args = params.get("args").cloned().unwrap_or(Value::Null);
    call_reply(run_action_at(&args, home))
}

fn call_reply(result: Result<String>) -> Value {
    match result {
        Ok(content) => json!({"content": content, "is_error": false}),
        Err(e) => json!({"content": format!("{e:#}"), "is_error": true}),
    }
}

fn run_action_at(args: &Value, home: Option<&Path>) -> Result<String> {
    let home = home.ok_or_else(|| anyhow::anyhow!("cannot resolve home"))?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .context("pulse action is required")?;
    let str_arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    match action {
        "status" => {
            let cfg = config::load_config_at(&config::config_path_at(home))?;
            Ok(match job::job_status_at(&cfg, &job::cron_dir_at(home))? {
                job::JobStatus::Disabled => "disabled".into(),
                job::JobStatus::Missing => "enabled (job missing — run sync)".into(),
                job::JobStatus::Live { next_run_at } => match next_run_at {
                    Some(t) => format!("enabled, next run {}", fmt_ts(t)),
                    None => "enabled".into(),
                },
            })
        }
        "goal_get" => goal::read_goal_at(&goal::goal_path_at(home)),
        "goal_set" => {
            let text = str_arg("text").context("text is required for goal_set")?;
            goal::write_goal_at(&goal::goal_path_at(home), &text)?;
            Ok("goal set".into())
        }
        "on" => {
            let mut cfg = config::load_config_at(&config::config_path_at(home))?;
            job::enable_at(&mut cfg, str_arg("schedule"), str_arg("deliver"), home)?;
            Ok(format!("pulse on ({})", cfg.schedule))
        }
        "off" => {
            let mut cfg = config::load_config_at(&config::config_path_at(home))?;
            cfg.enabled = false;
            let goal = goal::read_goal_at(&goal::goal_path_at(home))?;
            job::sync_job_at(&cfg, &goal, &job::cron_dir_at(home))?;
            config::save_config_at(&config::config_path_at(home), &cfg)?;
            Ok("pulse off".into())
        }
        "sync" => {
            let cfg = config::load_config_at(&config::config_path_at(home))?;
            let goal = goal::read_goal_at(&goal::goal_path_at(home))?;
            job::sync_job_at(&cfg, &goal, &job::cron_dir_at(home))?;
            Ok("synced".into())
        }
        other => Err(anyhow::anyhow!("unknown action: {other}")),
    }
}

fn fmt_ts(epoch: i64) -> String {
    match chrono::DateTime::from_timestamp(epoch, 0) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => epoch.to_string(),
    }
}

fn is_shutdown(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
        .as_deref()
        == Some("plugin/shutdown")
}

/// Read NDJSON requests on stdin, write replies on stdout, and return when the
/// host sends `plugin/shutdown` or closes the stream.
pub fn serve_plugin() -> Result<()> {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match handle_line(&line) {
            Some(reply) => {
                writeln!(out, "{reply}")?;
                out.flush()?;
            }
            // Unknown methods and id-less notifications are ignored; only an
            // explicit shutdown ends the loop.
            None if is_shutdown(&line) => return Ok(()),
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_advertises_the_pulse_tool() {
        let out = handle_line_at(r#"{"id":1,"method":"plugin/manifest"}"#, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["tools"][0]["name"], "pulse");
    }

    #[test]
    fn tool_call_status_and_goal_set() {
        let dir = tempfile::tempdir().unwrap();
        let out = handle_line_at(
            r#"{"id":2,"method":"tool/call","params":{"name":"pulse","args":{"action":"goal_set","text":"do x"}}}"#,
            Some(dir.path()),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["is_error"], false);
        assert_eq!(
            goal::read_goal_at(&goal::goal_path_at(dir.path())).unwrap(),
            "do x"
        );
    }

    #[test]
    fn shutdown_is_a_notification_with_no_reply() {
        assert!(handle_line_at(r#"{"method":"plugin/shutdown"}"#, None).is_none());
    }
}
