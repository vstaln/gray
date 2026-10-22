use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::detect::{CredSource, McpSource, ToolDef};
use crate::auth::store::CredentialStore;
use crate::repl::TuiOpt;
use crate::setup::catalog::{self, AuthEntry};

struct SetupItem {
    label: String,
    desc: String,

    opt_in: bool,
    apply: SetupApply,
}

enum SetupApply {
    Rules {
        src: PathBuf,
    },
    Skills {
        src_dir: PathBuf,
    },
    Mcp {
        entries: Vec<(String, serde_json::Value)>,
    },
    Creds {
        entries: Vec<CredItem>,
    },
    Plugin {
        name: &'static str,
    },
}

struct CredItem {
    target: String,
    src: PathBuf,
    extract: CredExtract,
}

enum CredExtract {
    PoolAgentKey { provider: String },

    DotEnv { name: String },

    ProviderApiKey { provider: String },
}

impl CredExtract {
    fn read(&self, src: &Path) -> anyhow::Result<String> {
        match self {
            Self::PoolAgentKey { provider } => {
                let v = read_json(src)
                    .ok_or_else(|| anyhow::anyhow!("cannot parse {}", src.display()))?;
                if let Some(entries) = v
                    .get("credential_pool")
                    .and_then(|p| p.get(provider))
                    .and_then(|e| e.as_array())
                {
                    for e in entries {
                        if let Some(k) = e
                            .get("agent_key")
                            .and_then(|k| k.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            return Ok(k.to_string());
                        }
                    }
                }
                v.get("providers")
                    .and_then(|p| p.get(provider))
                    .and_then(|e| e.get("agent_key"))
                    .and_then(|k| k.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| anyhow::anyhow!("no agent_key for {provider}"))
            }
            Self::ProviderApiKey { provider } => read_json(src)
                .and_then(|v| {
                    v.get("providers")
                        .and_then(|p| p.get(provider))
                        .and_then(|e| e.get("apiKey"))
                        .and_then(|k| k.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .ok_or_else(|| anyhow::anyhow!("no apiKey for {provider}")),
            Self::DotEnv { name } => {
                let body = std::fs::read_to_string(src)
                    .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", src.display()))?;
                dotenv_pairs(&body)
                    .into_iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("{name} not set"))
            }
        }
    }
}

fn def(id: &str) -> Option<&'static ToolDef> {
    super::detect::TOOLS.iter().find(|t| t.id == id)
}

fn say(tui: &TuiOpt, msg: &str) {
    crate::repl::say(tui.as_ref().map(|(s, _)| s), msg);
}

fn agents_md_path() -> anyhow::Result<PathBuf> {
    crate::sys_prompt_path()
}

fn section_present(tool_id: &str) -> bool {
    let Ok(p) = agents_md_path() else {
        return false;
    };
    std::fs::read_to_string(p)
        .map(|b| b.contains(&format!("<!-- gray-setup:{tool_id} BEGIN -->")))
        .unwrap_or(false)
}

fn skill_entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                        && !e.file_name().to_string_lossy().starts_with('.')
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

fn plugin_installed(name: &str) -> bool {
    gray_pkg::ops::list()
        .map(|l| l.contains_key(name))
        .unwrap_or(false)
}

async fn plugin_resolves(name: &str) -> bool {
    let Ok(client) = gray_pkg::fetch::client() else {
        return false;
    };
    let hit = async {
        let (hub, comm) = tokio::join!(
            gray_pkg::index::fetch_hub_index(&client),
            gray_pkg::index::fetch_index(&client)
        );
        hub.map(|i| gray_pkg::index::lookup(&i, name).is_ok())
            .unwrap_or(false)
            || comm
                .map(|i| gray_pkg::index::lookup(&i, name).is_ok())
                .unwrap_or(false)
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), hit)
        .await
        .unwrap_or(false)
}

fn clip(s: String, n: usize) -> String {
    s.chars().take(n).collect()
}

fn mcp_json_path() -> Option<PathBuf> {
    crate::setup::gray_home().ok().map(|h| h.join("mcp.json"))
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

fn jsonc_strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_str = false;
    let mut esc = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2 == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for c2 in chars.by_ref() {
                if prev == '*' && c2 == '/' {
                    break;
                }
                prev = c2;
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn existing_mcp_names() -> std::collections::BTreeSet<String> {
    mcp_json_path()
        .and_then(|p| read_json(&p))
        .and_then(|v| {
            v.get("mcpServers")
                .and_then(|m| m.as_object())
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default()
}

fn claude_mcp_entries(path: &Path) -> Vec<(String, serde_json::Value)> {
    read_json(path)
        .and_then(|v| v.get("mcpServers").and_then(|m| m.as_object()).cloned())
        .map(|m| m.into_iter().filter(|(_, e)| e.is_object()).collect())
        .unwrap_or_default()
}

fn opencode_mcp_entries(home: &Path, rel: &str) -> Vec<(String, serde_json::Value)> {
    let mut path = home.join(rel);
    if !path.is_file() {
        let alt = path.with_extension("jsonc");
        if alt.is_file() {
            path = alt;
        }
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let root: serde_json::Value = serde_json::from_str(&raw)
        .or_else(|_| serde_json::from_str(&jsonc_strip_comments(&raw)))
        .unwrap_or(serde_json::Value::Null);
    let Some(map) = root.get("mcp").and_then(|m| m.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, e) in map {
        let Some(entry) = opencode_entry_convert(e) else {
            continue;
        };
        out.push((name.clone(), entry));
    }
    out
}

fn opencode_entry_convert(e: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = e.as_object()?;
    match obj.get("type").and_then(|t| t.as_str()) {
        Some("local") | None => {
            let cmd = obj.get("command").and_then(|c| c.as_array())?;
            let bin = cmd.first().and_then(|c| c.as_str())?;
            let args: Vec<serde_json::Value> = cmd.iter().skip(1).cloned().collect();
            let mut entry = serde_json::json!({
                "command": bin,
                "args": args,
            });
            if let Some(env) = obj
                .get("environment")
                .or_else(|| obj.get("env"))
                .filter(|v| v.is_object())
            {
                entry["env"] = env.clone();
            }
            Some(entry)
        }
        Some("remote") => {
            let url = obj.get("url").and_then(|u| u.as_str())?;
            let mut entry = serde_json::json!({ "url": url });
            if let Some(h) = obj.get("headers").filter(|v| v.is_object()) {
                entry["headers"] = h.clone();
            }
            Some(entry)
        }
        _ => None,
    }
}

fn mcp_candidates(d: &ToolDef, home: &Path) -> Vec<(String, serde_json::Value)> {
    let Some(src) = &d.mcp else {
        return Vec::new();
    };
    let entries = match src {
        McpSource::ClaudeJson(rel) => claude_mcp_entries(&home.join(rel)),
        McpSource::Opencode(rel) => opencode_mcp_entries(home, rel),
    };
    let existing = existing_mcp_names();
    entries
        .into_iter()
        .filter(|(n, _)| !existing.contains(n))
        .collect()
}

fn env_key_names(entry: &serde_json::Value) -> Vec<String> {
    entry
        .get("env")
        .and_then(|e| e.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn non_empty_str(v: Option<&serde_json::Value>) -> bool {
    v.and_then(|s| s.as_str()).is_some_and(|s| !s.is_empty())
}

fn map_provider(raw: &str) -> String {
    match raw {
        "qwen-portal" | "qwen-oauth" => "qwen".to_string(),
        other => other.to_lowercase().replace('_', "-"),
    }
}

fn env_cred_target(name: &str) -> Option<String> {
    for suf in ["_API_KEY", "_TOKEN", "_SECRET"] {
        if let Some(base) = name.strip_suffix(suf)
            && !base.is_empty()
        {
            return Some(map_provider(base));
        }
    }
    None
}

fn dotenv_pairs(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let v = value.trim();
        let v = if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            &v[1..v.len() - 1]
        } else {
            v.split(" #").next().unwrap_or(v).trim_end()
        };
        out.push((name.to_string(), v.to_string()));
    }
    out
}

fn push_cred(
    items: &mut Vec<CredItem>,
    seen: &mut BTreeSet<String>,
    target: String,
    src: &Path,
    extract: CredExtract,
) {
    if seen.insert(target.clone()) {
        items.push(CredItem {
            target,
            src: src.to_path_buf(),
            extract,
        });
    }
}

fn cred_candidates(d: &ToolDef, home: &Path) -> (Vec<CredItem>, Vec<String>) {
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped = Vec::new();
    for src in d.creds.unwrap_or(&[]) {
        let path = home.join(src.rel());
        match src {
            CredSource::HermesAuth(_) => {
                let Some(v) = read_json(&path) else { continue };
                if let Some(pool) = v.get("credential_pool").and_then(|p| p.as_object()) {
                    for (prov, entries) in pool {
                        let arr = entries.as_array();
                        if arr.is_some_and(|a| a.iter().any(|e| non_empty_str(e.get("agent_key"))))
                        {
                            push_cred(
                                &mut items,
                                &mut seen,
                                map_provider(prov),
                                &path,
                                CredExtract::PoolAgentKey {
                                    provider: prov.clone(),
                                },
                            );
                        } else if arr.is_some_and(|a| {
                            a.iter().any(|e| {
                                non_empty_str(e.get("access").or_else(|| e.get("access_token")))
                            })
                        }) {
                            skipped.push(format!("{prov} (OAuth)"));
                        }
                    }
                }
                if let Some(providers) = v.get("providers").and_then(|p| p.as_object()) {
                    for (prov, e) in providers {
                        if non_empty_str(e.get("agent_key")) {
                            push_cred(
                                &mut items,
                                &mut seen,
                                map_provider(prov),
                                &path,
                                CredExtract::PoolAgentKey {
                                    provider: prov.clone(),
                                },
                            );
                        }
                    }
                }
            }
            CredSource::DotEnv(_) => {
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (name, _) in dotenv_pairs(&body) {
                    let Some(target) = env_cred_target(&name) else {
                        continue;
                    };
                    push_cred(
                        &mut items,
                        &mut seen,
                        target,
                        &path,
                        CredExtract::DotEnv { name },
                    );
                }
            }
            CredSource::OpenclawProfiles(_) => {
                let Some(v) = read_json(&path) else { continue };
                if let Some(profiles) = v.get("profiles").and_then(|p| p.as_object()) {
                    for (name, e) in profiles {
                        if non_empty_str(e.get("access").or_else(|| e.get("access_token"))) {
                            skipped.push(format!("{name} (OAuth)"));
                        }
                    }
                }
            }
            CredSource::OpenclawModels(_) => {
                let Some(v) = read_json(&path) else { continue };
                if let Some(providers) = v.get("providers").and_then(|p| p.as_object()) {
                    for (prov, e) in providers {
                        if non_empty_str(e.get("apiKey")) {
                            push_cred(
                                &mut items,
                                &mut seen,
                                map_provider(prov),
                                &path,
                                CredExtract::ProviderApiKey {
                                    provider: prov.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    skipped.sort();
    skipped.dedup();
    (items, skipped)
}

pub(crate) fn cred_source_ids(src: &CredSource, path: &Path) -> Vec<String> {
    let mut ids = match src {
        CredSource::HermesAuth(_) => read_json(path)
            .and_then(|v| {
                v.get("credential_pool")
                    .and_then(|p| p.as_object())
                    .map(|pool| {
                        pool.iter()
                            .filter(|(_, e)| {
                                e.as_array().is_some_and(|a| {
                                    a.iter().any(|e| non_empty_str(e.get("agent_key")))
                                })
                            })
                            .map(|(k, _)| k.clone())
                            .collect::<Vec<_>>()
                    })
            })
            .unwrap_or_default(),
        CredSource::DotEnv(_) => std::fs::read_to_string(path)
            .map(|body| {
                dotenv_pairs(&body)
                    .into_iter()
                    .filter(|(n, _)| env_cred_target(n).is_some())
                    .map(|(n, _)| n)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        CredSource::OpenclawProfiles(_) => read_json(path)
            .and_then(|v| {
                v.get("profiles")
                    .and_then(|p| p.as_object())
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
            })
            .unwrap_or_default(),
        CredSource::OpenclawModels(_) => read_json(path)
            .and_then(|v| {
                v.get("providers").and_then(|p| p.as_object()).map(|m| {
                    m.iter()
                        .filter(|(_, e)| non_empty_str(e.get("apiKey")))
                        .map(|(k, _)| k.clone())
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default(),
    };
    ids.sort();
    ids
}

fn cred_store() -> anyhow::Result<CredentialStore> {
    Ok(CredentialStore::new(catalog::auth_store_path()?))
}

fn cred_fp(secret: &str) -> String {
    let hex = format!("{:x}", Sha256::digest(secret.as_bytes()));
    hex[..16].to_string()
}

fn entry_fp(entry: &AuthEntry) -> Option<String> {
    match entry {
        AuthEntry::Key(secret) => Some(cred_fp(secret)),
        _ => None,
    }
}

fn age_desc(age: std::time::Duration) -> Option<String> {
    let days = age.as_secs() / 86_400;
    if days >= 45 {
        Some(format!("last used ~{}mo ago", days / 30))
    } else if days >= 10 {
        Some(format!("last used ~{}w ago", days / 7))
    } else {
        None
    }
}

fn stale_hint(d: &ToolDef, home: &Path) -> Option<String> {
    let rel = d.stale_probe?;
    let modified = home.join(rel).symlink_metadata().ok()?.modified().ok()?;
    age_desc(modified.elapsed().ok()?)
}

async fn items_for(def: &ToolDef) -> Vec<SetupItem> {
    let mut items = Vec::new();
    let home = match gray_core::paths::user_home() {
        Some(h) => h,
        None => return items,
    };
    if let Some(rel) = def.rules {
        let src = home.join(rel);
        if src.is_file() && !section_present(def.id) {
            let body = std::fs::read_to_string(&src).unwrap_or_default();
            let lines = body.lines().count();
            let first = body
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(2)
                .collect::<Vec<_>>()
                .join(" · ");
            items.push(SetupItem {
                label: format!("global rules ({rel})"),
                desc: clip(
                    format!("{lines} lines / {} B → AGENTS.md · {first}", body.len()),
                    90,
                ),
                opt_in: false,
                apply: SetupApply::Rules { src },
            });
        }
    }
    if let Some(rel) = def.skills {
        let dir = home.join(rel);
        let entries = skill_entries(&dir);
        if !entries.is_empty() {
            let names: Vec<String> = entries
                .iter()
                .filter_map(|e| e.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            let dest = crate::setup::gray_home()
                .map(|h| h.join("skills").display().to_string())
                .unwrap_or_else(|_| "$GRAY_HOME/skills".to_string());
            items.push(SetupItem {
                label: format!(
                    "{} skill{}",
                    entries.len(),
                    if entries.len() == 1 { "" } else { "s" }
                ),
                desc: clip(
                    format!(
                        "symlink → {dest} · {}{}",
                        names.iter().take(4).cloned().collect::<Vec<_>>().join(", "),
                        if names.len() > 4 { ", …" } else { "" }
                    ),
                    90,
                ),
                opt_in: false,
                apply: SetupApply::Skills { src_dir: dir },
            });
        }
    }
    let mcp = mcp_candidates(def, &home);
    if !mcp.is_empty() {
        let needs = !plugin_installed("mcp");
        let names: Vec<String> = mcp
            .iter()
            .map(|(n, e)| {
                let keys = env_key_names(e);
                if keys.is_empty() {
                    n.clone()
                } else {
                    format!("{n} (env: {})", keys.join(", "))
                }
            })
            .collect();
        items.push(SetupItem {
            label: format!(
                "{} MCP server{}",
                mcp.len(),
                if mcp.len() == 1 { "" } else { "s" }
            ),
            desc: clip(
                format!(
                    "merge → $GRAY_HOME/mcp.json · {}{}",
                    names.join(", "),
                    if needs {
                        " · needs the mcp plugin"
                    } else {
                        ""
                    }
                ),
                90,
            ),
            opt_in: false,
            apply: SetupApply::Mcp { entries: mcp },
        });
    }
    let (mut cred_items, cred_skipped) = cred_candidates(def, &home);
    match cred_store().and_then(|s| s.load()) {
        Ok(map) => cred_items.retain(|c| !map.contains_key(&c.target)),

        Err(_) => cred_items.clear(),
    }
    if !cred_items.is_empty() {
        let names: Vec<String> = cred_items
            .iter()
            .map(|c| format!("{} (key)", c.target))
            .collect();
        let mut desc = format!(
            "→ $GRAY_HOME/auth.json · {} — values never shown",
            names.join(", ")
        );
        if !cred_skipped.is_empty() {
            desc.push_str(&format!(" · skipped: {}", cred_skipped.join(", ")));
        }
        items.push(SetupItem {
            label: format!(
                "{} credential{}",
                cred_items.len(),
                if cred_items.len() == 1 { "" } else { "s" }
            ),
            desc: clip(desc, 90),
            opt_in: true,
            apply: SetupApply::Creds {
                entries: cred_items,
            },
        });
    }
    if let Some(p) = def.plugin
        && !plugin_installed(p)
        && plugin_resolves(p).await
    {
        items.push(SetupItem {
            label: format!("{p} provider plugin"),
            desc: "gray plugin install + consent at first boot".to_string(),
            opt_in: false,
            apply: SetupApply::Plugin { name: p },
        });
    }
    if let Some(hint) = stale_hint(def, &home)
        && let Some(first) = items.first_mut()
    {
        first.desc = format!("{} · {hint}", first.desc);
    }
    items
}

fn apply_rules(tool: &ToolDef, src: &Path) -> anyhow::Result<Option<String>> {
    let body = std::fs::read_to_string(src)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", src.display()))?;
    let path = agents_md_path()?;
    crate::load_or_create_system_prompt_at(&path)?;
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(&format!("<!-- gray-setup:{} BEGIN -->", tool.id)) {
        return Ok(None);
    }
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    let block = format!(
        "\n<!-- gray-setup:{} BEGIN -->\n{}\n<!-- gray-setup:{} END -->\n",
        tool.id,
        body.trim_end(),
        tool.id
    );
    existing.push_str(&block);
    std::fs::write(&path, &existing)?;
    Ok(Some(format!(
        "AGENTS.md +{} B (marked section)",
        block.len()
    )))
}

fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?
            .write_all(body.as_bytes())?;
        let mode = std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .ok()
            .filter(|m| *m < 0o600)
            .unwrap_or(0o600);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

fn apply_mcp(
    d: &ToolDef,
    entries: &[(String, serde_json::Value)],
    state: &mut super::State,
    run: &str,
) -> anyhow::Result<String> {
    let path =
        mcp_json_path().ok_or_else(|| anyhow::anyhow!("cannot resolve $GRAY_HOME/mcp.json"))?;
    let mut root = read_json(&path)
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if root.get("mcpServers").and_then(|m| m.as_object()).is_none() {
        root["mcpServers"] = serde_json::json!({});
    }
    let servers = root["mcpServers"].as_object_mut().expect("set above");
    let mut added = 0usize;
    for (name, entry) in entries {
        if servers.contains_key(name) {
            continue;
        }
        servers.insert(name.clone(), entry.clone());
        state.imports.push(super::ImportEntry {
            run: run.to_string(),
            at: now_secs(),
            tool: d.id.to_string(),
            kind: "mcp".into(),
            target: name.clone(),
            detail: path.display().to_string(),
        });
        added += 1;
    }
    write_atomic(&path, &serde_json::to_string_pretty(&root)?)?;
    Ok(format!("{added} merged → {}", path.display()))
}

fn apply_skills_logged(
    d: &ToolDef,
    src_dir: &Path,
    state: &mut super::State,
    run: &str,
) -> anyhow::Result<String> {
    let dest_dir = crate::setup::gray_home()?.join("skills");
    std::fs::create_dir_all(&dest_dir)?;
    let mut linked = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for entry in skill_entries(src_dir) {
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let link = dest_dir.join(&name);
        if link.symlink_metadata().is_ok() {
            skipped.push(name);
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&entry, &link)?;
        #[cfg(not(unix))]
        {
            if entry.is_dir() {
                std::os::windows::fs::symlink_dir(&entry, &link)?;
            } else {
                std::os::windows::fs::symlink_file(&entry, &link)?;
            }
        }
        state.imports.push(super::ImportEntry {
            run: run.to_string(),
            at: now_secs(),
            tool: d.id.to_string(),
            kind: "skill".into(),
            target: link.display().to_string(),
            detail: entry.display().to_string(),
        });
        linked += 1;
    }
    Ok(format!(
        "{linked} linked{}{}",
        if skipped.is_empty() {
            ""
        } else {
            ", skipped existing: "
        },
        skipped.join(", ")
    ))
}

async fn apply_plugin(name: &str) -> anyhow::Result<String> {
    let home = crate::plugin_cli::home()?;
    let lines = crate::plugin_cli::install_spec_lines(&home, name, false).await?;
    Ok(lines.join(" · "))
}

fn apply_creds(
    d: &ToolDef,
    entries: &[CredItem],
    state: &mut super::State,
    run: &str,
) -> anyhow::Result<String> {
    apply_creds_at(&cred_store()?, d, entries, state, run)
}

fn apply_creds_at(
    store: &CredentialStore,
    d: &ToolDef,
    entries: &[CredItem],
    state: &mut super::State,
    run: &str,
) -> anyhow::Result<String> {
    let _lock = store.lock()?;
    let mut map = store.load()?;
    let mut added = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for e in entries {
        if map.contains_key(&e.target) {
            skipped.push(e.target.clone());
            continue;
        }
        match e.extract.read(&e.src) {
            Ok(secret) => {
                let fp = cred_fp(&secret);
                map.insert(e.target.clone(), AuthEntry::Key(secret));
                state.imports.push(super::ImportEntry {
                    run: run.to_string(),
                    at: now_secs(),
                    tool: d.id.to_string(),
                    kind: "creds".into(),
                    target: e.target.clone(),
                    detail: fp,
                });
                added += 1;
            }
            Err(_) => skipped.push(e.target.clone()),
        }
    }
    if added > 0 {
        store.replace_locked(&map)?;
    }
    Ok(format!(
        "{added} stored → {}{}",
        store.path().display(),
        if skipped.is_empty() {
            String::new()
        } else {
            format!(" · skipped: {}", skipped.join(", "))
        }
    ))
}

fn undo_entry(entry: &super::ImportEntry) -> (bool, String) {
    match entry.kind.as_str() {
        "rules" => {
            let path = Path::new(&entry.target);
            let Ok(body) = std::fs::read_to_string(path) else {
                return (true, "AGENTS.md missing — skipped".into());
            };
            let begin = format!("<!-- gray-setup:{} BEGIN -->", entry.detail);
            let end = format!("<!-- gray-setup:{} END -->", entry.detail);
            match (body.find(&begin), body.find(&end)) {
                (Some(b), Some(e)) if e > b => {
                    let tail = &body[e + end.len()..];
                    let head = body[..b].trim_end().to_string();
                    let merged = format!(
                        "{head}{}",
                        if tail.starts_with('\n') {
                            tail.to_string()
                        } else {
                            format!("\n{tail}")
                        }
                    );
                    match std::fs::write(path, merged) {
                        Ok(()) => (true, "marked section removed".into()),
                        Err(e) => (false, format!("write failed: {e}")),
                    }
                }
                _ => (true, "marked section not found — skipped".into()),
            }
        }
        "skill" => {
            let link = Path::new(&entry.target);
            match link.symlink_metadata() {
                Ok(m) if m.file_type().is_symlink() => match std::fs::remove_file(link) {
                    Ok(()) => (true, format!("symlink {} removed", link.display())),
                    Err(e) => (false, format!("remove failed: {e}")),
                },
                _ => (
                    true,
                    format!("{} not a symlink — left in place", link.display()),
                ),
            }
        }
        "mcp" => {
            let path = Path::new(&entry.detail);
            let Some(mut root) = read_json(path).filter(|v| v.is_object()) else {
                return (true, format!("{} unreadable — skipped", path.display()));
            };
            let removed = root
                .get_mut("mcpServers")
                .and_then(|m| m.as_object_mut())
                .map(|m| m.remove(&entry.target).is_some())
                .unwrap_or(false);
            if !removed {
                return (
                    true,
                    format!("mcp '{}' not present — skipped", entry.target),
                );
            }
            match write_atomic(
                path,
                &serde_json::to_string_pretty(&root).unwrap_or_default(),
            ) {
                Ok(()) => (true, format!("mcp '{}' removed", entry.target)),
                Err(e) => (false, format!("write failed: {e}")),
            }
        }
        "plugin" => match gray_pkg::ops::remove(&entry.target) {
            Ok(()) => (true, format!("plugin {} removed", entry.target)),
            Err(e) => (false, format!("remove failed: {e}")),
        },
        "creds" => match catalog::auth_store_path() {
            Ok(path) => undo_cred(&entry.target, &entry.detail, &CredentialStore::new(path)),
            Err(_) => (true, "no auth store — skipped".into()),
        },
        other => (true, format!("unknown kind {other} — skipped")),
    }
}

fn undo_cred(target: &str, detail: &str, store: &CredentialStore) -> (bool, String) {
    let _lock = match store.lock() {
        Ok(l) => l,
        Err(e) => return (false, format!("lock failed: {e}")),
    };
    let mut map = match store.load() {
        Ok(m) => m,
        Err(e) => return (false, format!("load failed: {e}")),
    };
    match map.get(target) {
        None => (true, format!("cred '{target}' not present — skipped")),
        Some(e) if entry_fp(e).as_deref() != Some(detail) => (
            true,
            format!("cred '{target}' changed since import — left in place"),
        ),
        Some(_) => {
            map.remove(target);
            match store.replace_locked(&map) {
                Ok(()) => (true, format!("cred '{target}' removed")),
                Err(e) => (false, format!("write failed: {e}")),
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run_id() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}{:06}", d.as_secs(), d.subsec_micros()))
        .unwrap_or_else(|_| "0".to_string())
}

pub(crate) async fn handle(tui: &TuiOpt, arg: Option<String>) {
    let rest = arg.unwrap_or_default().trim().to_string();
    if rest == "scanned" {
        say(tui, &super::detect::scanned_report());
        return;
    }
    if let Some(sub) = rest.strip_prefix("dismiss") {
        let id = sub.trim();
        match def(id) {
            Some(d) => {
                let mut state = super::load_state();
                state.dismissed.insert(d.id.to_string());
                match super::save_state(&state) {
                    Ok(()) => say(
                        tui,
                        &format!("{} dismissed — no more detection rows", d.name),
                    ),
                    Err(e) => say(tui, &format!("dismiss failed: {e}")),
                }
            }
            None => say(
                tui,
                &format!("unknown tool '{id}' — /setup scanned lists ids"),
            ),
        }
        return;
    }
    if let Some(sub) = rest.strip_prefix("undo") {
        let n: usize = sub.trim().parse().unwrap_or(1);
        let mut state = super::load_state();
        if state.imports.is_empty() {
            say(tui, "import log empty — nothing to undo");
            return;
        }
        let mut run_order: Vec<String> = Vec::new();
        for e in state.imports.iter().rev() {
            if !run_order.contains(&e.run) {
                run_order.push(e.run.clone());
            }
        }
        let mut out = String::new();
        for run in run_order.iter().take(n) {
            let mut i = state.imports.len();
            while i > 0 {
                i -= 1;
                if state.imports[i].run != *run {
                    continue;
                }
                let entry = state.imports[i].clone();
                let (done, res) = undo_entry(&entry);
                if done {
                    state.imports.remove(i);
                }
                out.push_str(&format!("undo {} {}: {}\n", entry.kind, entry.target, res));
            }
        }
        let _ = super::save_state(&state);
        say(tui, &out);
        return;
    }

    if !super::enabled() {
        say(
            tui,
            "quality-of-life disabled (\"quality_of_life\": false in config.json)",
        );
        return;
    }
    let detected = super::detect::scan();
    let tools: Vec<&ToolDef> = if rest.is_empty() {
        detected.iter().map(|d| d.def).collect()
    } else {
        match def(&rest) {
            Some(d) => vec![d],
            None => {
                say(
                    tui,
                    &format!(
                        "unknown tool '{rest}' — /setup [{}|undo|dismiss|scanned]",
                        super::detect::TOOLS
                            .iter()
                            .map(|t| t.id)
                            .collect::<Vec<_>>()
                            .join("|")
                    ),
                );
                return;
            }
        }
    };
    let mut plan: Vec<(&ToolDef, Vec<SetupItem>)> = Vec::new();
    for d in tools {
        let items = items_for(d).await;
        if !items.is_empty() {
            plan.push((d, items));
        }
    }
    if plan.is_empty() {
        say(
            tui,
            "nothing to import — no applicable rules/skills/credentials/plugins found (or already imported)",
        );
        return;
    }
    let Some(shared) = tui.as_ref().map(|(s, _)| s) else {
        let mut out = String::from("applicable imports:\n");
        for (d, items) in &plan {
            out.push_str(&format!("  {}:\n", d.name));
            for i in items {
                out.push_str(&format!("    - {} — {}\n", i.label, i.desc));
            }
        }
        say(tui, &out);
        return;
    };

    let questions: Vec<crate::ask::AskQuestion> = plan
        .iter()
        .map(|(d, items)| crate::ask::AskQuestion {
            id: d.id.to_string(),
            header: d.name.to_string(),
            question: format!("import from {}? (check what to apply)", d.name),
            detail: String::new(),
            options: items
                .iter()
                .map(|i| crate::ask::AskOption {
                    label: i.label.clone(),
                    description: i.desc.clone(),
                    opt_in: i.opt_in,
                    freeform: false,
                })
                .collect(),
            multi: true,
            default_all: true,
            no_other: false,
        })
        .collect();
    let Some(answers) = crate::ask::ask_questions_modal(shared, questions) else {
        say(tui, "cancelled — nothing changed");
        return;
    };
    let mut state = super::load_state();
    let run = run_id();
    let mut out = String::new();
    for (d, items) in &plan {
        let checked: Vec<String> = answers
            .iter()
            .find(|a| a.id == d.id)
            .map(|a| a.answers.clone())
            .unwrap_or_default();
        for item in items {
            if !checked.iter().any(|c| c == &item.label) {
                out.push_str(&format!("{}: {} — skipped by choice\n", d.name, item.label));
                continue;
            }
            let res: Result<String, String> = match &item.apply {
                SetupApply::Rules { src } => match apply_rules(d, src) {
                    Ok(Some(r)) => {
                        state.imports.push(super::ImportEntry {
                            run: run.clone(),
                            at: now_secs(),
                            tool: d.id.to_string(),
                            kind: "rules".into(),
                            target: agents_md_path()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                            detail: d.id.to_string(),
                        });
                        Ok(r)
                    }
                    Ok(None) => Ok("already present — skipped".to_string()),
                    Err(e) => Err(format!("{e:#}")),
                },
                SetupApply::Skills { src_dir } => {
                    apply_skills_logged(d, src_dir, &mut state, &run).map_err(|e| format!("{e:#}"))
                }
                SetupApply::Mcp { entries } => {
                    apply_mcp(d, entries, &mut state, &run).map_err(|e| format!("{e:#}"))
                }
                SetupApply::Creds { entries } => {
                    apply_creds(d, entries, &mut state, &run).map_err(|e| format!("{e:#}"))
                }
                SetupApply::Plugin { name } => match apply_plugin(name).await {
                    Ok(r) => {
                        state.imports.push(super::ImportEntry {
                            run: run.clone(),
                            at: now_secs(),
                            tool: d.id.to_string(),
                            kind: "plugin".into(),
                            target: name.to_string(),
                            detail: String::new(),
                        });
                        Ok(r)
                    }
                    Err(e) => Err(format!("{e:#}")),
                },
            };
            match res {
                Ok(r) => out.push_str(&format!("{}: {} — {}\n", d.name, item.label, r)),
                Err(e) => out.push_str(&format!("{}: {} — failed: {e}\n", d.name, item.label)),
            }
        }
    }
    let _ = super::save_state(&state);
    out.push_str("log: $GRAY_HOME/quality-of-life/state.json · /setup undo reverses");
    say(tui, &out);
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
