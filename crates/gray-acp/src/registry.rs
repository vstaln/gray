use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub key: &'static str,
    pub display: &'static str,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub install_hint: &'static str,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct UserAgentEntry {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct UserAcpFile {
    #[serde(default)]
    pub agent_servers: HashMap<String, UserAgentEntry>,
    #[serde(default)]
    pub auto_approve: bool,
}

pub fn builtin() -> Vec<AgentSpec> {
    let npx = |pkg: &str| AgentSpec {
        key: "",
        display: "",
        command: "npx".to_string(),
        args: vec!["-y".to_string(), pkg.to_string()],
        env: vec![],
        install_hint: "",
    };
    let mut specs = vec![
        AgentSpec {
            key: "codex",
            display: "Codex",
            install_hint: "npm i -g @zed-industries/codex-acp or ensure node is installed",
            ..npx("-y @zed-industries/codex-acp")
        },
        AgentSpec {
            key: "claude",
            display: "Claude Code",
            install_hint: "npm i -g @zed-industries/claude-code-acp or ensure node is installed",
            ..npx("-y @zed-industries/claude-code-acp")
        },
        AgentSpec {
            key: "opencode",
            display: "opencode",
            command: "opencode".to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install opencode (https://opencode.ai)",
        },
        AgentSpec {
            key: "cursor",
            display: "Cursor",
            command: "cursor-agent".to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install cursor-agent",
        },
        AgentSpec {
            key: "gemini",
            display: "Gemini",
            command: "gemini".to_string(),
            args: vec!["--experimental-acp".to_string()],
            env: vec![],
            install_hint: "install the gemini CLI",
        },
        AgentSpec {
            key: "copilot",
            display: "Copilot",
            command: "copilot".to_string(),
            args: vec!["--acp".to_string()],
            env: vec![],
            install_hint: "install the copilot CLI",
        },
        AgentSpec {
            key: "grok",
            display: "Grok Build",
            command: "grok".to_string(),
            args: vec!["agent".to_string(), "stdio".to_string()],
            env: vec![("GROK_OAUTH2_REFERRER".to_string(), "gray".to_string())],
            install_hint: "install the grok CLI",
        },
    ];
    for extra in ["goose", "kimi", "kiro"] {
        specs.push(AgentSpec {
            key: Box::leak(extra.to_string().into_boxed_str()),
            display: Box::leak({
                let mut s = extra.to_string();
                if let Some(c) = s.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                s.into_boxed_str()
            }),
            command: extra.to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install the agent CLI and ensure it supports ACP",
        });
    }
    specs
}

pub fn prefer_native_binary(mut spec: AgentSpec) -> AgentSpec {
    if spec.command == "npx" {
        let bin = match spec.key {
            "codex" => "codex-acp",
            "claude" => "claude-code-acp",
            _ => return spec,
        };
        if which::which(bin).is_ok() {
            spec.command = bin.to_string();
            spec.args = vec![];
        }
    }
    spec
}

pub fn load_user_agents(gray_home: &Path) -> Vec<AgentSpec> {
    let path = gray_home.join("acp.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let file: UserAcpFile = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    file.agent_servers
        .into_iter()
        .map(|(name, entry)| AgentSpec {
            key: Box::leak(name.into_boxed_str()),
            display: Box::leak(String::new().into_boxed_str()),
            command: entry.command,
            args: entry.args,
            env: entry.env.into_iter().collect(),
            install_hint: "custom agent from ~/.gray/acp.json",
        })
        .collect()
}

pub fn resolve(name: &str, gray_home: Option<&Path>) -> Option<AgentSpec> {
    if let Some(home) = gray_home {
        for spec in load_user_agents(home) {
            if spec.key.eq_ignore_ascii_case(name) {
                return Some(spec);
            }
        }
    }
    builtin()
        .into_iter()
        .find(|s| s.key.eq_ignore_ascii_case(name))
        .map(prefer_native_binary)
}

pub fn all_specs(gray_home: Option<&Path>) -> Vec<AgentSpec> {
    let mut specs: Vec<AgentSpec> = builtin().into_iter().map(prefer_native_binary).collect();
    if let Some(home) = gray_home {
        specs.extend(load_user_agents(home));
    }
    specs
}

pub fn installed(spec: &AgentSpec) -> bool {
    if spec.command == "npx" {
        which::which("npx").is_ok() || which::which("node").is_ok()
    } else {
        which::which(&spec.command).is_ok()
    }
}

pub fn gray_home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return PathBuf::from(h);
    }
    dirs_gray_home()
}

fn dirs_gray_home() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".gray");
    }
    PathBuf::from(".gray")
}
