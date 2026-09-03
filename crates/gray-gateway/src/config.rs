//! Gateway config
use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform { Telegram, Discord, Slack }
impl std::str::FromStr for Platform {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "telegram" => Ok(Self::Telegram),
            "discord" => Ok(Self::Discord),
            "slack" => Ok(Self::Slack),
            _ => Err(format!("unknown platform {s}")),
        }
    }
}
impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Telegram => write!(f, "telegram"), Self::Discord => write!(f, "discord"), Self::Slack => write!(f, "slack") }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub token: Option<String>,
    #[serde(default)] pub app_token: Option<String>,
    #[serde(default)] pub home_channel: Option<String>,
}
impl Default for PlatformConfig { fn default() -> Self { Self { enabled: false, token: None, app_token: None, home_channel: None } } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default)] pub platforms: HashMap<Platform, PlatformConfig>,
    #[serde(default="default_group_per_user")] pub group_per_user: bool,
    #[serde(default)] pub thread_per_user: bool,
}
fn default_group_per_user() -> bool { true }
impl Default for GatewayConfig { fn default() -> Self { Self { platforms: HashMap::new(), group_per_user: true, thread_per_user: false } } }
pub fn gray_home_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).map_err(|_| anyhow::anyhow!("cannot resolve home"))?;
    Ok(PathBuf::from(base))
}
pub fn gray_gateway_path() -> anyhow::Result<PathBuf> {
    gray_home_dir().map(|b| b.join("gateway.yaml"))
}
pub fn load_gateway_config() -> GatewayConfig {
    let Ok(path) = gray_gateway_path() else { return GatewayConfig::default(); };
    std::fs::read_to_string(&path).ok().and_then(|s| serde_yaml::from_str(&s).ok()).unwrap_or_default()
}
pub fn save_gateway_config(cfg: &GatewayConfig) -> anyhow::Result<()> {
    let path = gray_gateway_path()?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let s = serde_yaml::to_string(cfg)?;
    std::fs::write(&path, s)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?; }
    Ok(())
}
