use std::process::Command;
use std::sync::mpsc::Sender;
use std::thread;

#[derive(Clone, Debug)]
pub struct HubEntry {
    pub name: String,
    pub kind: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub official: bool,
    pub verified: bool,
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

pub enum PluginMsg {
    Hub(Result<Vec<HubEntry>, String>),
    Installed(Vec<Installed>),
    Done(String, Result<String, String>),
}

pub const DEFAULT_HUB: &str = "https://gray.alignment.id/hub/index.json";

fn gray_bin() -> String {
    std::env::var("GRAY_GUI_GRAY_BIN").unwrap_or_else(|_| "gray".into())
}

pub fn hub_url() -> String {
    std::env::var("GRAY_HUB_INDEX").unwrap_or_else(|_| DEFAULT_HUB.into())
}

fn read_hub(url: &str) -> Result<String, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read_to_string(path).map_err(|e| e.to_string());
    }
    if std::path::Path::new(url).is_file() {
        return std::fs::read_to_string(url).map_err(|e| e.to_string());
    }
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "15", url])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn parse_hub(text: &str) -> Result<Vec<HubEntry>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let map = v
        .get("plugins")
        .and_then(|p| p.as_object())
        .ok_or("index has no plugins")?;
    let s = |e: &serde_json::Value, k: &str| {
        e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let b = |e: &serde_json::Value, k: &str| e.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let mut out: Vec<HubEntry> = map
        .iter()
        .map(|(name, e)| HubEntry {
            name: name.clone(),
            kind: Some(s(e, "kind"))
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "plugin".into()),
            version: s(e, "version"),
            description: s(e, "description"),
            author: s(e, "author"),
            official: b(e, "official"),
            verified: b(e, "verified"),
        })
        .collect();
    out.sort_by(|a, b| b.official.cmp(&a.official).then(a.name.cmp(&b.name)));
    Ok(out)
}

pub fn parse_list(text: &str) -> Vec<Installed> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("no plugins"))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next()?.to_string();
            let version = parts.next().unwrap_or("").to_string();
            Some(Installed {
                name,
                version,
                enabled: !l.contains("[disabled]"),
            })
        })
        .collect()
}

fn list() -> Vec<Installed> {
    Command::new(gray_bin())
        .args(["plugin", "list"])
        .output()
        .map(|o| parse_list(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

pub fn refresh(tx: Sender<PluginMsg>) {
    thread::spawn(move || {
        let _ = tx.send(PluginMsg::Installed(list()));
        let _ = tx.send(PluginMsg::Hub(
            read_hub(&hub_url()).and_then(|t| parse_hub(&t)),
        ));
    });
}

pub fn run(tx: Sender<PluginMsg>, label: String, args: Vec<String>) {
    thread::spawn(move || {
        let result = Command::new(gray_bin())
            .arg("plugin")
            .args(&args)
            .output()
            .map_err(|e| e.to_string())
            .and_then(|o| {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                let line = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .to_string();
                if o.status.success() {
                    Ok(line)
                } else {
                    Err(line)
                }
            });
        let _ = tx.send(PluginMsg::Done(label, result));
        let _ = tx.send(PluginMsg::Installed(list()));
    });
}
