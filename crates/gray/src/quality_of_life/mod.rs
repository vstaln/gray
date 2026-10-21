pub(crate) mod detect;
pub(crate) mod kitty;
pub(crate) mod providers;
pub(crate) mod setup;
pub mod update;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub tools: BTreeMap<String, ToolStamp>,
    #[serde(default)]
    pub dismissed: BTreeSet<String>,
    #[serde(default)]
    pub imports: Vec<ImportEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ToolStamp {
    pub fp: u64,
    pub seen_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImportEntry {
    #[serde(default)]
    pub run: String,
    pub at: u64,
    pub tool: String,
    pub kind: String,
    pub target: String,
    #[serde(default)]
    pub detail: String,
}

pub(crate) fn state_path() -> Option<PathBuf> {
    crate::setup::gray_home()
        .ok()
        .map(|h| h.join("quality-of-life").join("state.json"))
}

fn legacy_state_path() -> Option<PathBuf> {
    crate::setup::gray_home()
        .ok()
        .map(|h| h.join("qol").join("state.json"))
}

pub(crate) fn load_state() -> State {
    let Some(p) = state_path() else {
        return State::default();
    };
    let body = std::fs::read_to_string(&p)
        .ok()
        .or_else(|| legacy_state_path().and_then(|l| std::fs::read_to_string(l).ok()));
    let Some(body) = body else {
        return State::default();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

pub(crate) fn save_state(state: &State) -> anyhow::Result<()> {
    let p = state_path()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve $GRAY_HOME/quality-of-life/state.json"))?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state)?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub(crate) fn enabled() -> bool {
    crate::setup::quality_of_life_enabled()
}
