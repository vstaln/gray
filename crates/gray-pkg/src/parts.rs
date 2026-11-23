use std::collections::BTreeMap;
use std::path::PathBuf;

pub type Overrides = BTreeMap<String, BTreeMap<String, bool>>;

pub fn path() -> PathBuf {
    crate::plugins_dir().join("parts.json")
}

pub fn read() -> Overrides {
    std::fs::read(path())
        .ok()
        .and_then(|raw| serde_json::from_slice::<Overrides>(&raw).ok())
        .unwrap_or_default()
}

pub fn bundle(bundle: &str) -> BTreeMap<String, bool> {
    read().remove(bundle).unwrap_or_default()
}

pub fn get(bundle: &str, part: &str) -> Option<bool> {
    self::bundle(bundle).get(part).copied()
}

pub fn set(bundle: &str, part: &str, on: bool) -> anyhow::Result<()> {
    let _guard = crate::ops::hold_registry_lock()?;
    let mut all = read();
    all.entry(bundle.to_string())
        .or_default()
        .insert(part.to_string(), on);
    write(&all)
}

pub fn replace(bundle: &str, on: &BTreeMap<String, bool>) -> anyhow::Result<()> {
    let _guard = crate::ops::hold_registry_lock()?;
    let mut all = read();
    if on.is_empty() {
        all.remove(bundle);
    } else {
        all.insert(bundle.to_string(), on.clone());
    }
    write(&all)
}

fn write(all: &Overrides) -> anyhow::Result<()> {
    let path = path();
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut tmp, all)?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}
