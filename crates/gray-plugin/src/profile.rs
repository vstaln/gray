//! `gray.yml` profile loader: ordered plugin entries (builtin or sidecar).
//!
//! Format:
//! ```yaml
//! plugins:
//!   - builtin: tools-basic
//!   - sidecar: ~/.gray/plugins/my-tools
//!   - sidecar: [npx, -y, my-tools]
//! ```
//! Bare names (`- tools-basic`) are accepted as builtins.

// Hand-rolled line parser instead of a YAML dependency;
// switch to serde_yaml_ng if profiles grow beyond a flat entry list.

/// A sidecar plugin spec: argv to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSpec(pub Vec<String>);

/// One ordered entry in the `plugins:` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEntry {
    Builtin(String),
    Sidecar(SidecarSpec),
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim_matches('\'')
}

/// Parse a single `- ...` entry line into a [`PluginEntry`].
fn parse_entry(entry: &str) -> Option<PluginEntry> {
    let entry = entry.trim();
    if let Some(v) = entry.strip_prefix("builtin:") {
        let name = unquote(v.trim());
        return (!name.is_empty()).then(|| PluginEntry::Builtin(name.to_string()));
    }
    if let Some(v) = entry.strip_prefix("sidecar:") {
        let v = v.trim();
        // Argv list form: `sidecar: [npx, -y, my-tools]`
        if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let argv: Vec<String> = inner
                .split(',')
                .map(|a| unquote(a.trim()).to_string())
                .filter(|a| !a.is_empty())
                .collect();
            return (!argv.is_empty()).then_some(PluginEntry::Sidecar(SidecarSpec(argv)));
        }
        // String path form: `sidecar: ~/.gray/plugins/my-tools` (single argv).
        let path = unquote(v);
        // Expand `~` to $HOME so spawn gets a real path.
        let expanded = match path.strip_prefix("~/") {
            Some(rest) => match std::env::var("HOME") {
                Ok(home) => format!("{home}/{rest}"),
                Err(_) => path.to_string(),
            },
            None if path == "~" => std::env::var("HOME").unwrap_or_else(|_| path.to_string()),
            None => path.to_string(),
        };
        return (!expanded.is_empty()).then(|| PluginEntry::Sidecar(SidecarSpec(vec![expanded])));
    }
    // Bare `- name` form: builtin.
    let name = unquote(entry);
    (!name.is_empty()).then(|| PluginEntry::Builtin(name.to_string()))
}

/// Load the ordered [`PluginEntry`] list from a profile file.
/// This is the boot path: it preserves both builtin and sidecar entries
/// in file order. Missing file / no `plugins:` section is an error
/// (callers warn + fall back to builtins).
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
