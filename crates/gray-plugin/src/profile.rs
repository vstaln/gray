//! `gray.yml` profile loader: ordered builtin plugin names.
//!
//! Format (matches the repo `gray.yml` default profile):
//! ```yaml
//! plugins:
//!   - tools-basic
//!   - tools-search
//! ```
//! Mapping entries (`- builtin: tools-basic`) are also accepted.

// ponytail: hand-rolled line parser instead of a serde_yaml dependency;
// switch to serde_yaml if profiles grow beyond a flat name list.
pub fn load_profile(path: &str) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let mut names = Vec::new();
    let mut in_plugins = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_plugins = t == "plugins:";
            continue;
        }
        if !in_plugins {
            continue;
        }
        let Some(entry) = t.strip_prefix("- ") else {
            continue;
        };
        let entry = entry.trim().trim_matches('"').trim_matches('\'');
        // Accept `- builtin: name` mapping form; bare `- name` otherwise.
        let name = match entry.strip_prefix("builtin:") {
            Some(v) => v.trim().trim_matches('"').trim_matches('\''),
            None => entry,
        };
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    Ok(names)
}
