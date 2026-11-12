#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSpec(pub Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEntry {
    Builtin(String),
    Sidecar(SidecarSpec),
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim_matches('\'')
}

fn parse_entry(entry: &str) -> Option<PluginEntry> {
    let entry = entry.trim();
    if let Some(v) = entry.strip_prefix("builtin:") {
        let name = unquote(v.trim());
        return (!name.is_empty()).then(|| PluginEntry::Builtin(name.to_string()));
    }
    if let Some(v) = entry.strip_prefix("sidecar:") {
        let v = v.trim();

        if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let argv: Vec<String> = inner
                .split(',')
                .map(|a| unquote(a.trim()).to_string())
                .filter(|a| !a.is_empty())
                .collect();
            return (!argv.is_empty()).then_some(PluginEntry::Sidecar(SidecarSpec(argv)));
        }

        let path = unquote(v);

        let expanded = gray_core::paths::expand_tilde(path).map_or_else(
            || path.to_string(),
            |home| home.to_string_lossy().into_owned(),
        );
        return (!expanded.is_empty()).then(|| PluginEntry::Sidecar(SidecarSpec(vec![expanded])));
    }

    let name = unquote(entry);
    (!name.is_empty()).then(|| PluginEntry::Builtin(name.to_string()))
}

pub fn load_entries(path: &str) -> anyhow::Result<Vec<PluginEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    let mut in_plugins = false;
    let mut saw_plugins = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_plugins = t == "plugins:";
            saw_plugins |= in_plugins;
            continue;
        }
        if !in_plugins {
            continue;
        }
        let Some(entry) = t.strip_prefix("- ") else {
            continue;
        };
        if let Some(e) = parse_entry(entry) {
            entries.push(e);
        }
    }
    if !saw_plugins || entries.is_empty() {
        anyhow::bail!("no `plugins:` entries in {path}");
    }
    Ok(entries)
}
