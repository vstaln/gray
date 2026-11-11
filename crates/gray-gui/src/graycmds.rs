use std::process::Command;

pub fn bin() -> String {
    std::env::var("GRAY_GUI_GRAY_BIN").unwrap_or_else(|_| "gray".to_string())
}

pub fn list() -> Vec<(String, String)> {
    let out = Command::new(bin()).args(["cmd", "list"]).output().ok();
    let Some(out) = out.filter(|o| o.status.success()) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    v.get("result")
        .and_then(|r| r.get("commands"))
        .and_then(|c| c.as_array())
        .map(|cmds| {
            cmds.iter()
                .filter_map(|c| {
                    let name = c.get("name")?.as_str()?.to_string();
                    let desc = c
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((name, desc))
                })
                .collect()
        })
        .unwrap_or_default()
}
