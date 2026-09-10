use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub key: String,
    pub display: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub install_hint: String,
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
        key: String::new(),
        display: String::new(),
        command: "npx".to_string(),
        args: vec!["-y".to_string(), pkg.to_string()],
        env: vec![],
        install_hint: String::new(),
    };
    let mut specs = vec![
        AgentSpec {
            key: "codex".to_string(),
            display: "Codex".to_string(),
            install_hint: "npm i -g @zed-industries/codex-acp or ensure node is installed"
                .to_string(),
            ..npx("@zed-industries/codex-acp")
        },
        AgentSpec {
            key: "claude".to_string(),
            display: "Claude Code".to_string(),
            install_hint: "npm i -g @zed-industries/claude-code-acp or ensure node is installed"
                .to_string(),
            ..npx("@zed-industries/claude-code-acp")
        },
        AgentSpec {
            key: "opencode".to_string(),
            display: "opencode".to_string(),
            command: "opencode".to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install opencode (https://opencode.ai)".to_string(),
        },
        AgentSpec {
            key: "cursor".to_string(),
            display: "Cursor".to_string(),
            command: "cursor-agent".to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install cursor-agent".to_string(),
        },
        AgentSpec {
            key: "gemini".to_string(),
            display: "Gemini".to_string(),
            command: "gemini".to_string(),
            args: vec!["--experimental-acp".to_string()],
            env: vec![],
            install_hint: "install the gemini CLI".to_string(),
        },
        AgentSpec {
            key: "copilot".to_string(),
            display: "Copilot".to_string(),
            command: "copilot".to_string(),
            args: vec!["--acp".to_string()],
            env: vec![],
            install_hint: "install the copilot CLI".to_string(),
        },
        AgentSpec {
            key: "grok".to_string(),
            display: "Grok Build".to_string(),
            command: "grok".to_string(),
            args: vec!["agent".to_string(), "stdio".to_string()],
            env: vec![("GROK_OAUTH2_REFERRER".to_string(), "gray".to_string())],
            install_hint: "install the grok CLI".to_string(),
        },
    ];
    for extra in ["goose", "kimi", "kiro"] {
        specs.push(AgentSpec {
            key: extra.to_string(),
            display: {
                let mut s = extra.to_string();
                if let Some(c) = s.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                s
            },
            command: extra.to_string(),
            args: vec!["acp".to_string()],
            env: vec![],
            install_hint: "install the agent CLI and ensure it supports ACP".to_string(),
        });
    }
    specs
}

pub fn prefer_native_binary(mut spec: AgentSpec) -> AgentSpec {
    if spec.command == "npx" {
        let bin = match spec.key.as_str() {
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
            key: name,
            display: String::new(),
            command: entry.command,
            args: entry.args,
            env: entry.env.into_iter().collect(),
            install_hint: "custom agent from ~/.gray/acp.json".to_string(),
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

/// Isolated `CODEX_HOME` for the `codex` adapter (`~/.gray/codex-home`).
/// The pinned `@zed-industries/codex-acp` predates the current
/// `~/.codex/config.toml` schema and crashes parsing it, so the adapter
/// runs with a clean home: defaults work, auth is carried over by copying
/// `~/.codex/auth.json` when present (refresh if the source is newer).
pub fn ensure_codex_home() -> PathBuf {
    let dir = gray_home_dir().join("codex-home");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(home) = std::env::var("HOME") {
        let src = PathBuf::from(home).join(".codex").join("auth.json");
        let dst = dir.join("auth.json");
        let copy = std::fs::metadata(&src).ok().is_some_and(|m| {
            std::fs::metadata(&dst)
                .ok()
                .and_then(|d| d.modified().ok())
                .zip(m.modified().ok())
                .is_none_or(|(d, s)| s > d)
        });
        if copy {
            let _ = std::fs::copy(&src, &dst);
        }
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npx_argv_is_tokenized() {
        // Each argv element separate: no combined "-y package" strings.
        for spec in builtin().iter().filter(|s| s.command == "npx") {
            assert!(
                spec.args.iter().all(|a| !a.contains(' ')),
                "combined argv element in {}: {:?}",
                spec.key,
                spec.args
            );
            assert_eq!(spec.args.first().map(String::as_str), Some("-y"));
        }
    }

    #[test]
    fn codex_home_isolates_auth_and_avoids_user_config() {
        let base =
            std::env::temp_dir().join(format!("gray-test-codex-home-{}", std::process::id()));
        let fake_home = base.join("home");
        let fake_gray = base.join("gray");
        std::fs::create_dir_all(fake_home.join(".codex")).unwrap();
        std::fs::write(fake_home.join(".codex").join("auth.json"), r#"{"t":1}"#).unwrap();
        // User config with a schema the old adapter chokes on — must NOT be copied.
        std::fs::write(
            fake_home.join(".codex").join("config.toml"),
            "model_reasoning_effort = \"max\"\n",
        )
        .unwrap();
        let prev_home = std::env::var("HOME").ok();
        let prev_gray = std::env::var("GRAY_HOME").ok();
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var("GRAY_HOME", &fake_gray);
        }
        let dir = ensure_codex_home();
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_gray {
                Some(v) => std::env::set_var("GRAY_HOME", v),
                None => std::env::remove_var("GRAY_HOME"),
            }
        }
        assert_eq!(dir, fake_gray.join("codex-home"));
        assert!(dir.join("auth.json").exists(), "auth must carry over");
        assert!(
            !dir.join("config.toml").exists(),
            "user config must NOT leak into the isolated home"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
