//! Install/list/remove/update over the plugin lockfile.
//!
//! Honest but thin: only gray-native-shaped sources install until the
//! adapters land in Task 2.3.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub schema: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LockEntry>,
}

fn default_true() -> bool {
    true
}

// TODO(2.4): switch to gray_plugin::lock
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    #[serde(default)]
    pub ecosystem: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub adapter_version: String,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for LockEntry {
    fn default() -> Self {
        Self {
            ecosystem: String::new(),
            version: String::new(),
            hash: String::new(),
            source: String::new(),
            argv: Vec::new(),
            adapter_version: String::new(),
            installed_at: String::new(),
            scope: String::new(),
            enabled: true,
        }
    }
}

/// Install target: index name, https URL, npm package, ClawHub skill,
/// or Claude marketplace plugin.
#[derive(Debug, Clone)]
pub enum NameOrUrl {
    Name(String),
    Url(String),
    Npm {
        name: String,
        version: Option<String>,
    },
    Git {
        url: String,
        git_ref: Option<String>,
    },
    ClawHub {
        slug: String,
    },
    Claude {
        plugin: String,
        marketplace: Option<String>,
    },
}

/// Split an `npm:<pkg>[@<version>]` body on the LAST `@` (regex-free).
/// Scoped names keep their leading `@`: `@scope/bar` is unpinned,
/// `@scope/bar@1.2.3` pins `1.2.3`. A trailing `@` (`foo@`) is unpinned.
fn parse_npm_spec(body: &str) -> NameOrUrl {
    match body.rfind('@') {
        Some(i) if i > 0 => {
            let ver = &body[i + 1..];
            if ver.is_empty() {
                NameOrUrl::Npm {
                    name: body[..i].to_string(),
                    version: None,
                }
            } else {
                NameOrUrl::Npm {
                    name: body[..i].to_string(),
                    version: Some(ver.to_string()),
                }
            }
        }
        _ => NameOrUrl::Npm {
            name: body.to_string(),
            version: None,
        },
    }
}

/// Split a `claude:<plugin>[@<marketplace>]` body on the LAST `@`
/// (marketplace names never contain one). A trailing `@` leaves the
/// marketplace unpinned, mirroring [`parse_npm_spec`].
fn parse_claude_spec(body: &str) -> NameOrUrl {
    match body.rfind('@') {
        Some(i) if i > 0 && !body[i + 1..].is_empty() => NameOrUrl::Claude {
            plugin: body[..i].to_string(),
            marketplace: Some(body[i + 1..].to_string()),
        },
        _ => {
            let plugin = body.strip_suffix('@').unwrap_or(body).to_string();
            NameOrUrl::Claude {
                plugin,
                marketplace: None,
            }
        }
    }
}

pub fn parse_spec(s: &str) -> NameOrUrl {
    let t = s.trim();
    if let Some(body) = t.strip_prefix("clawhub:") {
        return NameOrUrl::ClawHub {
            slug: body.trim().to_string(),
        };
    }
    if let Some(body) = t.strip_prefix("claude:") {
        return parse_claude_spec(body.trim());
    }
    if let Some(body) = t.strip_prefix("npm:") {
        return parse_npm_spec(body);
    }
    if let Some(body) = t.strip_prefix("git:") {
        return parse_git_spec(body);
    }
    if t.starts_with("ssh://") || t.starts_with("git://") || t.starts_with("git@") {
        return parse_git_spec(t);
    }
    if (t.starts_with("http://") || t.starts_with("https://")) && https_has_git_suffix(t) {
        return parse_git_spec(t);
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        NameOrUrl::Url(t.to_string())
    } else {
        NameOrUrl::Name(t.to_string())
    }
}

/// Split `scheme://authority` off; returns `(head, remainder)`.
fn split_authority(s: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = s.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((&s[..scheme.len() + 3 + end], &rest[end..]))
}

/// Parse a git spec body (after `git:`, or the raw spec itself): strip
/// scheme/authority first (so `user@host` never reads as a ref marker),
/// then split the REMAINDER on the last `@` for the ref (R16). A trailing
/// `@` is unpinned, mirroring [`parse_npm_spec`].
fn parse_git_spec(body: &str) -> NameOrUrl {
    let (head, remainder) = match split_authority(body) {
        Some((h, r)) => (h, r),
        None => match body.find(':') {
            // scp-like `git@host:path`.
            Some(i) => (&body[..=i], &body[i + 1..]),
            // Bare local path.
            None => ("", body),
        },
    };
    let (path, git_ref) = match remainder.rfind('@') {
        Some(i) if !remainder[i + 1..].is_empty() => {
            (&remainder[..i], Some(remainder[i + 1..].to_string()))
        }
        Some(i) => (&remainder[..i], None),
        None => (remainder, None),
    };
    NameOrUrl::Git {
        url: format!("{head}{path}"),
        git_ref,
    }
}

/// Raw `http(s)` URL with a `.git` path suffix (after R16 ref-stripping)
/// is a git source; anything else stays a tarball [`NameOrUrl::Url`].
fn https_has_git_suffix(t: &str) -> bool {
    let Some((_, remainder)) = split_authority(t) else {
        return false;
    };
    let path = match remainder.rfind('@') {
        Some(i) => &remainder[..i],
        None => remainder,
    };
    path.split(['?', '#'])
        .next()
        .unwrap_or(path)
        .ends_with(".git")
}

/// Install name from a git URL: last path segment minus `.git`.
fn name_from_git_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let after_host = match path.split_once("://") {
        Some((_, rest)) => rest.find('/').map(|i| &rest[i + 1..]).unwrap_or(""),
        None => match path.find(':') {
            Some(i) => &path[i + 1..],
            None => path,
        },
    };
    let last = after_host.rsplit('/').next().unwrap_or(after_host);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

#[derive(Debug, Default, Clone)]
pub struct InstallOpts {
    pub argv: Vec<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
}

fn lock_path() -> PathBuf {
    crate::plugins_dir().join("lock.json")
}

pub(crate) fn now_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

fn read_lock() -> anyhow::Result<Option<LockFile>> {
    match std::fs::read_to_string(lock_path()) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_lock(lock: &LockFile) -> anyhow::Result<()> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock = lock.clone();
    lock.schema = 1;
    std::fs::write(&path, serde_json::to_string_pretty(&lock)?)?;
    Ok(())
}

/// Record one install in the lockfile, preserving any existing `enabled`
/// flag (a reinstall must not silently re-enable a disabled plugin).
fn record_install(
    name: &str,
    ecosystem: &str,
    version: &str,
    hash: &str,
    source: &str,
    scope: String,
    argv: Vec<String>,
) -> anyhow::Result<()> {
    let mut lock = read_lock()?.unwrap_or_default();
    let enabled = lock.plugins.get(name).map(|e| e.enabled).unwrap_or(true);
    lock.plugins.insert(
        name.to_string(),
        LockEntry {
            ecosystem: ecosystem.to_string(),
            version: version.to_string(),
            hash: hash.to_string(),
            source: source.to_string(),
            argv,
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: now_secs(),
            scope,
            enabled,
        },
    );
    write_lock(&lock)
}

fn ensure_gray_native(ecosystem: &str, type_: &str) -> anyhow::Result<()> {
    if ecosystem != "gray-native" {
        anyhow::bail!(
            "unsupported ecosystem '{ecosystem}' (only gray-native sources are installable until Task 2.3)"
        );
    }
    if type_ != "tarball" {
        anyhow::bail!(
            "unsupported source type '{type_}' (only gray-native tarballs are installable until Task 2.3)"
        );
    }
    Ok(())
}

/// Derive a plugin name from a URL's last path segment.
fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_suffix(".tar.gz")
        .or_else(|| last.strip_suffix(".tgz"))
        .unwrap_or(last)
        .to_string()
}

/// Default npm registry (overridden by [`NPM_REGISTRY_ENV`]).
pub const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// Env var overriding the npm registry base URL (tests point at loopback).
pub const NPM_REGISTRY_ENV: &str = "GRAY_NPM_REGISTRY";

pub fn npm_registry_base() -> String {
    std::env::var(NPM_REGISTRY_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_NPM_REGISTRY.to_string())
}

/// Registry metadata URL for a package (`/` in scoped names is `%2F`).
fn npm_metadata_url(base: &str, name: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        name.replace('/', "%2F")
    )
}

#[derive(Debug)]
struct NpmResolved {
    tarball: String,
    integrity: String,
    version: String,
}
async fn npm_resolve(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<NpmResolved> {
    if name.is_empty() {
        anyhow::bail!("npm package name is empty");
    }
    let url = npm_metadata_url(&npm_registry_base(), name);
    crate::fetch::check_url(&url)?;
    log::debug!(
        "resolving npm package {name} via {}",
        crate::fetch::redact(&url)
    );
    let meta: serde_json::Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let want = match version {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => meta
            .get("dist-tags")
            .and_then(|t| t.get("latest"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("npm package {name} has no latest version"))?
            .to_string(),
    };
    let dist = meta
        .get("versions")
        .and_then(|v| v.get(&want))
        .and_then(|v| v.get("dist"))
        .ok_or_else(|| anyhow::anyhow!("npm package {name} has no version {want}"))?;
    let tarball = dist
        .get("tarball")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("npm package {name}@{want} has no tarball"))?
        .to_string();
    let integrity = match dist.get("integrity").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            let shasum = dist
                .get("shasum")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("npm package {name}@{want} has no integrity"))?;
            format!("sha256:{shasum}")
        }
    };
    Ok(NpmResolved {
        tarball,
        integrity,
        version: want,
    })
}

/// Verified + unpacked npm tarball awaiting P2-2's extractor. `dir` owns the
/// staging tempdir (deleted on drop); the lock write happens in P2-2 after
/// extraction, so a failed extract leaves no half-state.
#[derive(Debug)]
pub(crate) struct StagedPkg {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) integrity: String,
    pub(crate) tarball: String,
}

pub(crate) async fn stage_npm_package(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<StagedPkg> {
    let resolved = npm_resolve(client, name, version).await?;
    let spec = crate::index::HashSpec::Single(resolved.integrity.clone());
    let archive = crate::fetch::download(client, &resolved.tarball, Some(&spec)).await?;
    let tmp_root = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_root)?;
    let dir = tempfile::tempdir_in(&tmp_root)?;
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, dir.path()) {
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    Ok(StagedPkg {
        dir,
        name: name.to_string(),
        version: resolved.version,
        integrity: resolved.integrity,
        tarball: resolved.tarball,
    })
}

// --- P2-2 pi skills extraction + lock record ---
//
// Layout probe 2026-09-06 (3 real tarballs into /tmp, uncommitted):
// - `@normful/picadillo@6.0.0`: `skills/mulch/SKILL.md` and
//   `skills/run-in-tmux/SKILL.md` (plus a sibling `scripts/` dir, ignored),
//   `extensions/*.ts` (skipped, P3).
// - `pi-subagents-j0k3r@1.5.13`: `skills/subagents-configuration/SKILL.md`,
//   manifest `"pi": {"skills": ["./skills"], "extensions": ["./index.ts"]}`,
//   top-level `*.md` are docs (README/CHANGELOG — still copied per R14).
// - `pi-skill-dollar@0.2.1`: NO skills at all, extension-only
//   (`"pi": {"extensions": ["./dist/extension.js"]}`) → honest bail (R12).
// Confirmed patterns: `skills/*/SKILL.md`, `*/SKILL.md`, top-level `*.md`,
// manifest `pi.skills` globs. NEVER execute package code: only `.md` files
// are copied; everything else is left behind (and counted honestly).

/// What a pi install took vs skipped, printed by [`emit_pi_summary`];
/// extensions/themes are P3 and never executed. `taken` holds
/// loadable skill dirs only; dest-top-level `.md` files are listed in
/// `docs` (copied for reference — the loader recurses with
/// `include_root_files=false`, so they never load as skills).
#[derive(Debug, Clone, Default)]
pub struct PiInstallSummary {
    pub taken: Vec<String>,
    pub docs: Vec<String>,
    pub skipped_ext: bool,
    pub skipped_themes: bool,
}

/// Lock key + `pi/` dir name for an npm package: `@scope/name` →
/// `scope-name` (strip leading `@`, `/` → `-`, `..` → `-`). Sanitized
/// keys never contain `/` or `..`, so [`remove`]'s traversal rejection
/// still holds. Callers must still run [`validate_install_key`] (via
/// [`install_key`]) — sanitize alone cannot turn `.`, empty, or `\`
/// inputs into safe keys, so those are rejected, not mangled. This
/// closes the parked npm-side `..` gap as well as the git `..` path:
/// both arms derive keys through [`install_key`].
pub fn sanitize_npm_key(name: &str) -> String {
    name.strip_prefix('@')
        .unwrap_or(name)
        .replace('/', "-")
        .replace("..", "-")
}

/// Reject install keys that would escape `<plugins>/pi/`: empty, `.`,
/// `..`, or anything still holding `/` or `\` after [`sanitize_npm_key`].
pub(crate) fn validate_install_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() || key == "." || key == ".." || key.contains('/') || key.contains('\\') {
        anyhow::bail!("cannot derive a safe plugin name from package (got {key:?})");
    }
    Ok(())
}

/// Shared npm+git key derivation: sanitize then reject destructive keys
/// before any clone/extract work.
pub(crate) fn install_key(name: &str) -> anyhow::Result<String> {
    let key = sanitize_npm_key(name);
    validate_install_key(&key)?;
    Ok(key)
}

/// npm tarballs wrap everything in `package/`; use it when present.
pub(crate) fn stage_root(dir: &Path) -> PathBuf {
    let wrapped = dir.join("package");
    if wrapped.is_dir() {
        wrapped
    } else {
        dir.to_path_buf()
    }
}

/// `(skills, extensions, themes)` globs/entries from `package.json`'s
/// `"pi"` key. Missing manifest or key → all empty (patterns still apply).
fn pi_manifest_lists(root: &Path) -> (Vec<String>, Vec<String>, Vec<String>) {
    let raw = std::fs::read_to_string(root.join("package.json"));
    let Ok(raw) = raw else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let get = |key: &str| {
        manifest
            .get("pi")
            .and_then(|pi| pi.get(key))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    (get("skills"), get("extensions"), get("themes"))
}

/// One `.md` file to copy: `src` under the staging root → `rel` under the
/// install dest, with `label` for the `taken` list.
struct SkillMatch {
    src: PathBuf,
    rel: PathBuf,
    label: String,
}

/// A `rel` built only from single file names is traversal-safe.
fn safe_rel(parts: &[&str]) -> Option<PathBuf> {
    let mut rel = PathBuf::new();
    for p in parts {
        if p.is_empty() || *p == "." || *p == ".." || p.contains(['/', '\\']) {
            return None;
        }
        rel.push(p);
    }
    Some(rel)
}

/// Copy `dir`'s `.md` files as one skill: `<label>/<file>.md`,
/// flattening the `skills/` container regardless of tarball depth.
fn push_skill_dir(dir: &Path, out: &mut Vec<SkillMatch>, seen: &mut HashSet<PathBuf>) {
    let Some(label) = dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| Path::new(n).extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort_by_key(|n| (n != "SKILL.md", n.clone()));
    for file in files {
        // Flatten the `skills/` container: rel is `<label>/<file>.md`
        // regardless of how deep the skill dir sat in the tarball.
        let flat = PathBuf::from(label).join(&file);
        if seen.insert(flat.clone()) {
            out.push(SkillMatch {
                src: dir.join(&file),
                rel: flat,
                label: label.to_string(),
            });
        }
    }
}

/// A manifest `pi.skills` glob base is safe only when it stays under the
/// staging root: reject absolute paths (which would discard `root` on
/// join) and any non-`Normal` component (`..`, prefixes, root markers).
fn glob_base_is_safe(base: &str) -> bool {
    if base.is_empty() {
        return false;
    }
    let p = Path::new(base);
    if p.is_absolute() {
        return false;
    }
    p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Collect skill matches: `skills/*/SKILL.md`, `*/SKILL.md`, manifest
/// `pi.skills` globs, top-level `*.md` (R14: top-level `.md` are copied
/// for reference and listed in `docs`, not `taken`). Deduped, `taken`
/// order stable.
fn collect_skill_matches(root: &Path, globs: &[String]) -> Vec<SkillMatch> {
    let mut out: Vec<SkillMatch> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // `skills/*/SKILL.md` (the common pi layout).
    if let Ok(rd) = std::fs::read_dir(root.join("skills")) {
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            if dir.join("SKILL.md").is_file() {
                push_skill_dir(&dir, &mut out, &mut seen);
            }
        }
    }

    // `*/SKILL.md` (one level under root; dedupe covers `skills/`).
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            if dir.join("SKILL.md").is_file() {
                push_skill_dir(&dir, &mut out, &mut seen);
            }
        }
    }

    // Manifest `pi.skills` globs (`./skills`, `skills/*`, single files).
    // Confined to the staging root: skip absolute bases and any base
    // with `..`/prefix components, then belt-and-suspenders verify the
    // joined candidate still strips to `root` before touching the fs.
    for glob in globs {
        let base = glob.strip_prefix("./").unwrap_or(glob);
        let base = base.strip_suffix("/*").unwrap_or(base);
        if base.is_empty() || !glob_base_is_safe(base) {
            continue;
        }
        let candidate = root.join(base);
        if candidate.strip_prefix(root).is_err() {
            continue;
        }
        let path = candidate;
        if path.is_dir() {
            if path.join("SKILL.md").is_file() {
                push_skill_dir(&path, &mut out, &mut seen);
            }
            if let Ok(rd) = std::fs::read_dir(&path) {
                let mut dirs: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                dirs.sort();
                for dir in dirs {
                    if dir.join("SKILL.md").is_file() {
                        push_skill_dir(&dir, &mut out, &mut seen);
                    }
                }
            }
        } else if path.is_file()
            && path.extension().is_some_and(|e| e == "md")
            && let Some(file) = path.file_name().and_then(|n| n.to_str())
        {
            if file == "SKILL.md" {
                if let Some(parent) = path.parent() {
                    push_skill_dir(parent, &mut out, &mut seen);
                }
            } else if let Some(rel) = safe_rel(&[file])
                && seen.insert(rel.clone())
            {
                out.push(SkillMatch {
                    src: path.clone(),
                    rel,
                    label: file.to_string(),
                });
            }
        }
    }

    // Top-level `*.md`.
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut files: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| Path::new(n).extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        for file in files {
            let Some(rel) = safe_rel(&[&file]) else {
                continue;
            };
            if seen.insert(rel.clone()) {
                out.push(SkillMatch {
                    src: root.join(&file),
                    rel,
                    label: file,
                });
            }
        }
    }

    out
}

/// Count skipped (never copied, never executed) payload: code files outside
/// skill dirs (`.ts/.js/.mjs/.cjs/.tsx/.jsx`, or anything under
/// `extensions/`/`dist/`) count as extensions; files under `themes/` or
/// `theme/` count as themes. Manifest/doc/markdown files don't count.
fn count_skipped(root: &Path) -> (usize, usize) {
    const CODE_EXT: &[&str] = &["ts", "js", "mjs", "cjs", "tsx", "jsx"];
    let mut ext = 0usize;
    let mut themes = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.') || n == "node_modules")
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            // Never count skill content: anything under a dir with SKILL.md.
            let mut under_skill = false;
            for anc in rel.ancestors().skip(1) {
                if anc.as_os_str().is_empty() {
                    break;
                }
                if root.join(anc).join("SKILL.md").is_file() {
                    under_skill = true;
                    break;
                }
            }
            if under_skill {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "package.json" {
                continue;
            }
            if path.extension().is_some_and(|e| e == "md") {
                continue;
            }
            let in_dir = |d: &str| rel.components().any(|c| c.as_os_str() == d);
            if in_dir("themes") || in_dir("theme") {
                themes += 1;
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| CODE_EXT.contains(&e))
                || in_dir("extensions")
                || in_dir("dist")
            {
                ext += 1;
            }
        }
    }
    (ext, themes)
}

/// Copy matched `.md` files into `dest` (parents created; non-`md`
/// refused defensively — matches are `.md` by construction).
fn copy_skill_matches(dest: &Path, matches: &[SkillMatch]) -> anyhow::Result<()> {
    for m in matches {
        if m.src.extension().is_some_and(|e| e != "md") || m.src.extension().is_none() {
            continue;
        }
        if m.rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            anyhow::bail!("refusing unsafe skill path: {}", m.rel.display());
        }
        let target = dest.join(&m.rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&m.src, &target)?;
    }
    Ok(())
}

/// Copy pi skills from a staged root into `<plugins_dir>/pi/<key>/`
/// (`.md` only — package code is never executed) and write ONE lock entry.
/// Zero skills → honest bail with nothing written (R12); any copy/lock
/// failure removes `dest` and writes nothing (no half-state). Shared by
/// the npm (P2-1 staging) and git (P2-4 clone) arms.
/// One shared summary printer: loadable skill dirs vs reference docs.
pub(crate) fn emit_pi_summary(summary: &PiInstallSummary) {
    eprintln!("skills taken: {}", summary.taken.join(", "));
    if !summary.docs.is_empty() {
        eprintln!("docs copied for reference: {}", summary.docs.join(", "));
    }
}

pub(crate) fn extract_pi_skills(
    root: &Path,
    key: &str,
    version: &str,
    hash: &str,
    source: &str,
    ecosystem: &str,
    opts: &InstallOpts,
) -> anyhow::Result<(PathBuf, PiInstallSummary)> {
    // Belt-and-suspenders: both arms validate via `install_key` first, but
    // the destructive `remove_dir_all(dest)` below must never run on `..`.
    validate_install_key(key)?;
    let (skill_globs, manifest_ext, manifest_themes) = pi_manifest_lists(root);
    let matches = collect_skill_matches(root, &skill_globs);
    if matches.is_empty() {
        anyhow::bail!(
            "package {key}@{version} ships no skills (nothing to install; extensions/themes need P3)"
        );
    }
    let dest = crate::plugins_dir().join("pi").join(key);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    if let Err(e) = copy_skill_matches(&dest, &matches) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }
    let (ext_n, theme_n) = count_skipped(root);
    // Single-component rels are dest-top-level `.md` (undiscoverable by
    // the loader, which recurses with `include_root_files=false`) → docs.
    let mut taken: Vec<String> = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    for m in &matches {
        if m.rel.components().count() == 1 {
            docs.push(m.label.clone());
        } else {
            taken.push(m.label.clone());
        }
    }
    taken.sort();
    taken.dedup();
    docs.sort();
    docs.dedup();
    let summary = PiInstallSummary {
        taken,
        docs,
        skipped_ext: !manifest_ext.is_empty() || root.join("extensions").is_dir() || ext_n > 0,
        skipped_themes: !manifest_themes.is_empty()
            || root.join("themes").is_dir()
            || root.join("theme").is_dir()
            || theme_n > 0,
    };
    if summary.skipped_ext {
        eprintln!("skipped {ext_n} extension files (P3)");
    }
    if summary.skipped_themes {
        eprintln!("skipped {theme_n} theme files (P3)");
    }
    let scope = opts.scope.clone().unwrap_or_else(|| "user".to_string());
    if let Err(e) = record_install(
        key,
        ecosystem,
        version,
        hash,
        source,
        scope,
        opts.argv.clone(),
    ) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }
    Ok((dest, summary))
}

/// `Npm` arm: resolve → verified download → staging unpack → skills
/// extraction + ONE lock write (no half-state on failure).
async fn install_npm(
    client: &reqwest::Client,
    name: &str,
    version: Option<&str>,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    let staged = stage_npm_package(client, name, version).await?;
    let key = install_key(&staged.name)?;
    let root = stage_root(staged.dir.path());
    let (dest, summary) = extract_pi_skills(
        &root,
        &key,
        &staged.version,
        &staged.integrity,
        &staged.tarball,
        "pi-gallery",
        &opts,
    )?;
    emit_pi_summary(&summary);
    Ok(Report {
        name: key,
        version: staged.version,
        path: dest,
    })
}

/// Shallow-clone `url` (under the plugins tmp dir) via the shared
/// `sources::clone_into_tmp` helper and return the keeper tempdir, the clone
/// dir, and the cloned HEAD commit sha. `--branch` only when pinned.
pub(crate) fn clone_git_repo(
    url: &str,
    git_ref: Option<&str>,
) -> anyhow::Result<(tempfile::TempDir, PathBuf, String)> {
    let branch = git_ref.filter(|r| !r.is_empty());
    let mut extra: Vec<&str> = Vec::new();
    if let Some(b) = branch {
        extra.push("--branch");
        extra.push(b);
    }
    let (staging, clone_dir) = crate::sources::clone_into_tmp(url, &extra, true)?;
    let sha_out = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()?;
    if !sha_out.status.success() {
        anyhow::bail!(
            "cloning {} failed: cannot read commit sha",
            crate::fetch::redact(url)
        );
    }
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    if sha.is_empty() {
        anyhow::bail!(
            "cloning {} failed: empty commit sha",
            crate::fetch::redact(url)
        );
    }
    Ok((staging, clone_dir, sha))
}

/// `Git` arm: shallow clone → P2-2 extractor over the clone → ONE lock
/// write (R18: same `pi/` dest, same taken/skipped honesty, same
/// zero-skills bail). No hash verify: honest `unverified` warning mirroring
/// [`install_url`]; lock `hash` is the raw post-clone commit sha (R17).
async fn install_git(
    url: &str,
    git_ref: Option<&str>,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    if url.trim().is_empty() {
        anyhow::bail!("git URL is empty");
    }
    eprintln!(
        "warning: unverified install from {} (no index hash; use an index name for verified installs)",
        crate::fetch::redact(url)
    );
    let raw = name_from_git_url(url);
    if raw.trim().is_empty() {
        anyhow::bail!(
            "cannot derive a plugin name from git URL: {}",
            crate::fetch::redact(url)
        );
    }
    let key = install_key(&raw).map_err(|_| {
        anyhow::anyhow!(
            "cannot derive a plugin name from git URL: {}",
            crate::fetch::redact(url)
        )
    })?;
    let (_stage, clone_dir, sha) = clone_git_repo(url, git_ref)?;
    let version = git_ref
        .filter(|r| !r.is_empty())
        .unwrap_or("0.0.0")
        .to_string();
    let (dest, summary) =
        extract_pi_skills(&clone_dir, &key, &version, &sha, url, "pi-gallery", &opts)?;
    emit_pi_summary(&summary);
    Ok(Report {
        name: key,
        version,
        path: dest,
    })
}

/// `(source, item)` identity for install failures, from the spec kind —
/// the honest attribution when the ecosystem isn't known yet.
fn install_identity(spec: &NameOrUrl) -> (String, String) {
    match spec {
        NameOrUrl::Name(n) => ("index".to_string(), n.clone()),
        NameOrUrl::Url(u) => ("url".to_string(), u.clone()),
        NameOrUrl::Npm { name, .. } => ("npm".to_string(), name.clone()),
        NameOrUrl::Git { url, .. } => ("git".to_string(), url.clone()),
        NameOrUrl::ClawHub { slug } => ("clawhub".to_string(), slug.clone()),
        NameOrUrl::Claude {
            plugin,
            marketplace,
        } => (
            "claude".to_string(),
            match marketplace {
                Some(m) => format!("{plugin}@{m}"),
                None => plugin.clone(),
            },
        ),
    }
}

/// Best-effort ecosystem attribution for the error registry: the lock
/// entry's ecosystem when known, `"plugin"` otherwise (misses,
/// unreadable lock).
fn ecosystem_of(name: &str) -> String {
    read_lock()
        .ok()
        .flatten()
        .and_then(|l| l.plugins.get(name).map(|e| e.ecosystem.clone()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "plugin".to_string())
}

pub async fn install(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let (source, item) = install_identity(&spec);
    install_inner(spec, opts).await.map_err(|e| {
        crate::errors::record(&source, &item, format!("{e:#}"));
        e
    })
}

async fn install_inner(spec: NameOrUrl, opts: InstallOpts) -> anyhow::Result<Report> {
    let client = crate::fetch::client()?;
    match spec {
        NameOrUrl::Name(name) => install_index(&client, &name, opts).await,
        NameOrUrl::Url(url) => install_url(&client, &url, opts).await,
        NameOrUrl::Npm { name, version } => {
            install_npm(&client, &name, version.as_deref(), opts).await
        }
        NameOrUrl::Git { url, git_ref } => install_git(&url, git_ref.as_deref(), opts).await,
        NameOrUrl::ClawHub { slug } => install_clawhub(&client, &slug, opts).await,
        NameOrUrl::Claude {
            plugin,
            marketplace,
        } => install_claude(&client, &plugin, marketplace.as_deref(), opts).await,
    }
}

/// `ClawHub` arm: shared detail → download → unpack → verify staging →
/// shared skills extraction + ONE lock write (`ecosystem: "clawhub"`).
async fn install_clawhub(
    client: &reqwest::Client,
    slug: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    let crate::sources::StagedClawHub {
        root,
        detail,
        verified,
        _staging,
    } = crate::sources::stage_clawhub_bundle(client, slug).await?;
    let source_url = crate::sources::clawhub_canonical_url(&detail.owner, &detail.slug);
    if !verified {
        eprintln!(
            "warning: unverified install from {} (no index hash; use an index name for verified installs)",
            crate::fetch::redact(&source_url)
        );
    }
    let version = if detail.version.trim().is_empty() {
        "0.0.0".to_string()
    } else {
        detail.version.clone()
    };
    // Owner-qualified key (`owner-slug` after sanitizing): two owners
    // shipping the same slug no longer collide on one `pi/<slug>` dir,
    // and the key derives from the searched display name.
    let key = install_key(&crate::sources::clawhub_key_input(
        &detail.owner,
        &detail.slug,
    ))?;
    let (dest, summary) =
        extract_pi_skills(&root, &key, &version, "", &source_url, "clawhub", &opts)?;
    emit_pi_summary(&summary);
    Ok(Report {
        name: key,
        version,
        path: dest,
    })
}

/// `Claude` arm: resolve via the marketplace adapter → shared skills
/// extraction + ONE lock write (`ecosystem: "claude"`). Unpinned/local
/// sources keep the honest unverified warning.
async fn install_claude(
    client: &reqwest::Client,
    plugin: &str,
    marketplace: Option<&str>,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    let resolved = crate::sources::claude_resolve(client, plugin, marketplace).await?;
    if resolved.unverified {
        eprintln!(
            "warning: unverified install from {} (no index hash; use an index name for verified installs)",
            crate::fetch::redact(&resolved.source_url)
        );
    }
    let key = install_key(plugin)?;
    let (dest, summary) = extract_pi_skills(
        &resolved.root,
        &key,
        &resolved.version,
        &resolved.hash,
        &resolved.source_url,
        "claude",
        &opts,
    )?;
    emit_pi_summary(&summary);
    Ok(Report {
        name: key,
        version: resolved.version,
        path: dest,
    })
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    let index = crate::index::fetch_index(client).await?;
    let entry = crate::index::lookup(&index, name)?;
    ensure_gray_native(&entry.ecosystem, &entry.source.type_)?;
    let archive = crate::fetch::download(client, &entry.source.url, Some(&entry.hash)).await?;
    let dest = crate::plugins_dir().join(name);
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    let scope = if entry.scope.is_empty() {
        opts.scope.clone().unwrap_or_else(|| "user".to_string())
    } else {
        entry.scope.clone()
    };
    record_install(
        name,
        &entry.ecosystem,
        &entry.version,
        entry.hash.primary().unwrap_or_default(),
        &entry.source.url,
        scope,
        opts.argv.clone(),
    )?;
    Ok(Report {
        name: name.to_string(),
        version: entry.version.clone(),
        path: dest,
    })
}

async fn install_url(
    client: &reqwest::Client,
    url: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    eprintln!(
        "warning: unverified install from {} (no index hash; use an index name for verified installs)",
        crate::fetch::redact(url)
    );
    let name = name_from_url(url);
    if name.is_empty() {
        anyhow::bail!(
            "cannot derive a plugin name from URL: {}",
            crate::fetch::redact(url)
        );
    }
    let archive = crate::fetch::download(client, url, None).await?;
    let dest = crate::plugins_dir().join(&name);
    if let Err(e) = crate::fetch::unpack_tar_gz(&archive, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    record_install(
        &name,
        "url",
        "0.0.0",
        "",
        url,
        opts.scope.clone().unwrap_or_else(|| "user".to_string()),
        opts.argv.clone(),
    )?;
    Ok(Report {
        name,
        version: "0.0.0".to_string(),
        path: dest,
    })
}

pub fn list() -> anyhow::Result<BTreeMap<String, LockEntry>> {
    Ok(read_lock()?.map(|l| l.plugins).unwrap_or_default())
}

pub fn remove(name: &str) -> anyhow::Result<()> {
    remove_inner(name).map_err(|e| {
        crate::errors::record(&ecosystem_of(name), name, format!("{e:#}"));
        e
    })
}

fn remove_inner(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("not installed: {name}");
    }
    let mut lock = read_lock()?.unwrap_or_default();
    if lock.plugins.remove(name).is_none() {
        anyhow::bail!("not installed: {name}");
    }
    // Gray-native/URL installs live at `<plugins_dir>/<name>`; pi installs
    // at `<plugins_dir>/pi/<name>` (R13: sanitized keys hold no `/`).
    for dir in [
        crate::plugins_dir().join(name),
        crate::plugins_dir().join("pi").join(name),
    ] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    write_lock(&lock)?;
    Ok(())
}

/// Flip a plugin's `enabled` flag (boot skips disabled entries).
/// Miss message matches `remove`.
pub fn set_enabled(name: &str, on: bool) -> anyhow::Result<()> {
    set_enabled_inner(name, on).map_err(|e| {
        crate::errors::record(&ecosystem_of(name), name, format!("{e:#}"));
        e
    })
}

fn set_enabled_inner(name: &str, on: bool) -> anyhow::Result<()> {
    let mut lock = read_lock()?.unwrap_or_default();
    let Some(entry) = lock.plugins.get_mut(name) else {
        anyhow::bail!("not installed: {name}");
    };
    entry.enabled = on;
    write_lock(&lock)?;
    Ok(())
}

/// Update one plugin (`target` = name) or all (`target` = `"all"`).
/// Only gray-native lock entries with an index entry are considered;
/// anything else is skipped with a warning. Returns per-plugin reports
/// for the plugins that actually changed.
pub async fn update(target: &str) -> anyhow::Result<Vec<Report>> {
    update_inner(target).await.map_err(|e| {
        crate::errors::record(&ecosystem_of(target), target, format!("{e:#}"));
        e
    })
}

async fn update_inner(target: &str) -> anyhow::Result<Vec<Report>> {
    let lock = read_lock()?.unwrap_or_default();
    let names: Vec<String> = if target == "all" {
        lock.plugins.keys().cloned().collect()
    } else {
        if !lock.plugins.contains_key(target) {
            anyhow::bail!("not installed: {target}");
        }
        vec![target.to_string()]
    };
    let client = crate::fetch::client()?;
    let index = crate::index::fetch_index(&client).await?;
    let mut out = Vec::new();
    for name in &names {
        let installed = &lock.plugins[name];
        if installed.ecosystem != "gray-native" {
            eprintln!("warning: skipping update of {name} (non-index source)");
            continue;
        }
        let entry = match crate::index::lookup(&index, name) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("warning: skipping update of {name} (not in index)");
                continue;
            }
        };
        if entry.version == installed.version {
            continue;
        }
        let argv = installed.argv.clone();
        let scope = Some(installed.scope.clone());
        out.push(install_index(&client, name, InstallOpts { argv, scope }).await?);
    }
    Ok(out)
}

// --- P2-3 `search` fan-out (Gray Index + Pi Gallery preview) ---
//
// Probe 2026-09-06: pi.dev/packages is SSR HTML (no JSON search API);
// npm `/-/v1/search` recalls known pi packages 3/3
// (@braintrust/pi-extension, bigpowers, context-mode), so the pi side
// reads npm search on the shared registry base. No per-hit filtering:
// matches are listed labeled `(preview)`; install stays skills-only.

/// Where a search hit came from. Labels are display-time copy only:
/// pi hits read exactly `Pi Gallery (preview)` (never stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSource {
    Gray,
    Pi,
    Claude,
    ClawHub,
}

impl SearchSource {
    /// Display label for a source (`Pi Gallery (preview)` is exact copy).
    pub fn label(self) -> &'static str {
        match self {
            SearchSource::Gray => "Gray Index",
            SearchSource::Pi => "Pi Gallery (preview)",
            SearchSource::Claude => "Claude",
            SearchSource::ClawHub => "ClawHub",
        }
    }
}

/// One merged search hit. Gray entries carry no description (the index
/// has none); pi hits carry npm's description (possibly empty).
/// `version_detail` (e.g. a source qualifier), `files`, and `trust`
/// (e.g. ClawHub `official/community + scan status`) feed the
/// preview+confirm pane; [`format_search_hit`] ignores them by design.
/// `popularity` is npm's `score.detail.popularity` (0..1) on pi hits,
/// 0.0 everywhere else (never fails a search on a stat).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub name: String,
    pub version: String,
    pub desc: String,
    pub source: SearchSource,
    pub version_detail: String,
    pub files: Vec<String>,
    pub trust: String,
    pub popularity: f32,
}

/// Advisory line printed when the pi side fails; search still exits 0.
pub const PI_UNREACHABLE_LINE: &str = "Pi Gallery (preview): unreachable";

/// Advisory line printed when the Gray Index fetch fails (network error,
/// 404, timeout); search continues with pi hits and still exits 0.
/// Exact mirror of the pi-side voice.
pub const GRAY_UNREACHABLE_LINE: &str = "Gray Index: unreachable";

/// Advisory line printed when the ClawHub side fails; search still exits 0.
pub const CLAWHUB_UNREACHABLE_LINE: &str = "ClawHub: unreachable";

/// Advisory line printed when the Claude side fails; search still exits 0.
pub const CLAUDE_UNREACHABLE_LINE: &str = "Claude: unreachable";

/// Render one hit: `name version [source] - desc`, with the desc suffix
/// omitted when empty (always the case for Gray Index hits). Pi and
/// ClawHub hits whose lock key differs from the display name (e.g.
/// `@scope/bar` → `scope-bar`, `arein/test` → `arein-test`) append
/// `(install as <key>)` via the shared sanitizer.
pub fn format_search_hit(hit: &SearchHit) -> String {
    let mut base = format!("{} {} [{}]", hit.name, hit.version, hit.source.label());
    if hit.source == SearchSource::Pi || hit.source == SearchSource::ClawHub {
        let key = sanitize_npm_key(&hit.name);
        if key != hit.name {
            base.push_str(&format!(" (install as {key})"));
        }
    }
    let desc = hit.desc.trim();
    if desc.is_empty() {
        base
    } else {
        format!("{base} - {desc}")
    }
}

/// Outcome of [`search_all`]: merged hits (Gray, Pi, Claude, ClawHub —
/// Gray wins name collisions, first wins thereafter) plus whether any
/// side failed. Callers print the `*_UNREACHABLE_LINE` consts when the
/// corresponding flag is set — never an error exit for a fetch failure.
#[derive(Debug)]
pub struct SearchOutput {
    pub hits: Vec<SearchHit>,
    pub pi_unreachable: bool,
    pub gray_unreachable: bool,
    pub clawhub_unreachable: bool,
    pub claude_unreachable: bool,
}

/// Query the pi side via npm search (`/-/v1/search`, `size=20`) on the
/// shared registry base ([`npm_registry_base`], so tests point this at
/// loopback). Any failure is an `Err` for [`search_all`] to downgrade
/// to the advisory path.
async fn search_pi(client: &reqwest::Client, query: &str) -> anyhow::Result<Vec<SearchHit>> {
    let url = format!("{}/-/v1/search", npm_registry_base().trim_end_matches('/'));
    crate::fetch::check_url(&url)?;
    log::debug!("searching pi gallery via {}", crate::fetch::redact(&url));
    let resp: serde_json::Value = client
        .get(&url)
        .query(&[("text", query), ("size", "20")])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut hits = Vec::new();
    if let Some(objects) = resp.get("objects").and_then(|v| v.as_array()) {
        for obj in objects {
            let pkg = obj.get("package");
            let name = pkg
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                continue;
            }
            let version = pkg
                .and_then(|p| p.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let desc = pkg
                .and_then(|p| p.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Free npm signal (`score.detail.popularity`, 0..1); missing
            // or odd shapes degrade to 0.0, never fail the search.
            let popularity = obj
                .get("score")
                .and_then(|s| s.get("detail"))
                .and_then(|d| d.get("popularity"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let popularity = popularity.clamp(0.0, 1.0);
            hits.push(SearchHit {
                name: name.to_string(),
                version,
                desc,
                source: SearchSource::Pi,
                version_detail: String::new(),
                files: Vec::new(),
                trust: String::new(),
                popularity,
            });
        }
    }
    hits.truncate(20);
    Ok(hits)
}

/// Fan out `query` over Gray Index (substring over the `fetch_index`
/// cache), pi ([`search_pi`]), Claude marketplaces, and ClawHub. Order is
/// Gray, Pi, Claude, ClawHub; Gray wins name collisions and every later
/// duplicate is suppressed (first wins). A fetch failure on any side sets
/// the corresponding `*_unreachable` flag instead of erroring; only
/// corrupt local state (not a fetch failure) still returns `Err`.
pub async fn search_all(query: &str) -> anyhow::Result<SearchOutput> {
    let client = crate::fetch::client()?;
    let (index, gray_unreachable) = match crate::index::fetch_index(&client).await {
        Ok(index) => (index, false),
        Err(e) => {
            log::debug!("gray index fetch failed: {e:#}");
            (
                crate::index::Index {
                    plugins: Default::default(),
                },
                true,
            )
        }
    };
    // `plugins` is a BTreeMap, so gray hits come out name-sorted.
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (name, entry) in index.plugins.iter().filter(|(n, _)| n.contains(query)) {
        seen.insert(name.clone());
        hits.push(SearchHit {
            name: name.clone(),
            version: entry.version.clone(),
            desc: String::new(),
            source: SearchSource::Gray,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
            popularity: 0.0,
        });
    }
    let pi_unreachable = match search_pi(&client, query).await {
        Ok(pi_hits) => {
            for hit in pi_hits {
                if seen.insert(hit.name.clone()) {
                    hits.push(hit);
                }
            }
            false
        }
        Err(e) => {
            log::debug!("pi gallery search failed: {e:#}");
            true
        }
    };
    // Claude (local catalog reads; git clones stay inside the adapter).
    let (claude_entries, claude_unreachable) = crate::sources::claude_search_entries(query);
    for e in claude_entries {
        if seen.insert(e.name.clone()) {
            hits.push(SearchHit {
                name: e.name,
                version: e.version,
                desc: e.description,
                source: SearchSource::Claude,
                version_detail: e.qualifier,
                files: Vec::new(),
                trust: String::new(),
                popularity: 0.0,
            });
        }
    }
    if claude_unreachable {
        log::debug!("claude marketplace search partially or fully failed");
    }
    // ClawHub (HTTP; 429 honored inside the adapter).
    let clawhub_unreachable = match crate::sources::clawhub_search(&client, query).await {
        Ok(entries) => {
            for e in entries {
                if seen.insert(e.name.clone()) {
                    hits.push(SearchHit {
                        name: e.name,
                        version: e.version,
                        desc: e.summary,
                        source: SearchSource::ClawHub,
                        version_detail: String::new(),
                        files: Vec::new(),
                        trust: crate::sources::clawhub_trust(e.official, &e.scan),
                        popularity: 0.0,
                    });
                }
            }
            false
        }
        Err(e) => {
            log::debug!("clawhub search failed: {e:#}");
            true
        }
    };
    Ok(SearchOutput {
        hits,
        pi_unreachable,
        gray_unreachable,
        clawhub_unreachable,
        claude_unreachable,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::await_holding_lock)]
    use super::*;

    // Serializes the process-global GRAY_HOME mutation within this test
    // binary (cargo runs tests in one process on multiple threads).
    // Shared with `errors::tests` (same process-global env).
    pub(crate) static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_entry_is_enabled() {
        assert!(LockEntry::default().enabled);
    }

    #[test]
    fn spec_splits_names_and_urls() {
        assert!(matches!(parse_spec("foo"), NameOrUrl::Name(_)));
        assert!(matches!(parse_spec("  foo  "), NameOrUrl::Name(_)));
        assert!(matches!(
            parse_spec("https://h/x.tar.gz"),
            NameOrUrl::Url(_)
        ));
        assert!(matches!(parse_spec("http://h/x.tar.gz"), NameOrUrl::Url(_)));
        assert_eq!(name_from_url("https://h/plugins/foo.tar.gz?x=1"), "foo");
    }

    #[test]
    fn spec_parses_npm_forms() {
        match parse_spec("npm:pi-foo") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "pi-foo");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("npm:@scope/bar@1.2.3") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "@scope/bar");
                assert_eq!(version, Some("1.2.3".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn npm_spec_edge_cases() {
        // Scoped without version stays unpinned.
        match parse_spec("npm:@scope/bar") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "@scope/bar");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // Trailing `@` is unpinned, not an empty version.
        match parse_spec("npm:foo@") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "foo");
                assert_eq!(version, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // Pinned unscoped.
        match parse_spec("npm:pi-foo@0.2.0") {
            NameOrUrl::Npm { name, version } => {
                assert_eq!(name, "pi-foo");
                assert_eq!(version, Some("0.2.0".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn spec_parses_git_forms() {
        // `git:` prefix with and without ref.
        match parse_spec("git:https://host/o/r.git@main") {
            NameOrUrl::Git { url, git_ref } => {
                assert_eq!(url, "https://host/o/r.git");
                assert_eq!(git_ref, Some("main".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("git:https://host/o/r.git") {
            NameOrUrl::Git { url, git_ref } => {
                assert_eq!(url, "https://host/o/r.git");
                assert_eq!(git_ref, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // Raw forms are git by shape (R15); raw https stays tarball Url.
        assert!(matches!(
            parse_spec("ssh://git@host/o/r.git"),
            NameOrUrl::Git { .. }
        ));
        assert!(matches!(
            parse_spec("git://host/o/r.git"),
            NameOrUrl::Git { .. }
        ));
        assert!(matches!(
            parse_spec("git@host:o/r.git"),
            NameOrUrl::Git { .. }
        ));
        assert!(matches!(
            parse_spec("https://host/o/r.git"),
            NameOrUrl::Git { .. }
        ));
        assert!(matches!(
            parse_spec("https://h/x.tar.gz"),
            NameOrUrl::Url(_)
        ));
        // `@ref` alone never flips a tarball URL to git (R15).
        assert!(matches!(
            parse_spec("https://h/x.tar.gz@main"),
            NameOrUrl::Url(_)
        ));
    }

    #[test]
    fn npm_metadata_url_encodes_scope() {
        assert_eq!(
            npm_metadata_url("https://registry.npmjs.org", "@scope/bar"),
            "https://registry.npmjs.org/@scope%2Fbar"
        );
        assert_eq!(
            npm_metadata_url("http://127.0.0.1:1/", "pi-foo"),
            "http://127.0.0.1:1/pi-foo"
        );
    }

    // --- npm registry stub helpers (loopback only, no live registry) ---

    fn b64_encode(bytes: &[u8]) -> String {
        const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for ch in bytes.chunks(3) {
            let mut n: u32 = 0;
            for &b in ch {
                n = (n << 8) | u32::from(b);
            }
            n <<= 8 * (3 - ch.len());
            s.push(ALPH[((n >> 18) & 63) as usize] as char);
            s.push(ALPH[((n >> 12) & 63) as usize] as char);
            s.push(if ch.len() > 1 {
                ALPH[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            s.push(if ch.len() > 2 {
                ALPH[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        s
    }

    fn sha512_integrity(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha512::new();
        h.update(bytes);
        format!("sha512-{}", b64_encode(&h.finalize()))
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        format!("sha256:{:x}", h.finalize())
    }

    fn tiny_tgz() -> Vec<u8> {
        use std::io::Write;
        let bytes = br#"{"name":"pi-foo"}"#;
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(bytes.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_mtime(0);
        hdr.set_cksum();
        let mut ar = tar::Builder::new(Vec::new());
        ar.append_data(&mut hdr, "package/package.json", &bytes[..])
            .unwrap();
        let tar_bytes = ar.into_inner().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    async fn spawn_tarball(tgz: Vec<u8>) -> String {
        use axum::{Router, routing::get};
        let router = Router::new().route(
            "/pi-foo.tgz",
            get(move || {
                let tgz = tgz.clone();
                async move { tgz }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/pi-foo.tgz")
    }

    async fn spawn_registry(meta: serde_json::Value) -> String {
        use axum::{Json, Router, routing::get};
        let router = Router::new().route(
            "/pi-foo",
            get(move || {
                let meta = meta.clone();
                async move { Json(meta) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    /// Point `GRAY_HOME` at a fresh tempdir and the npm registry at the stub.
    /// Must be called under `ENV_GUARD`.
    fn use_npm_env(registry_base: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
            std::env::set_var(NPM_REGISTRY_ENV, registry_base);
        }
        home
    }

    #[tokio::test]
    async fn npm_resolve_picks_exact_version() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "2.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
                "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let resolved = npm_resolve(&client, "pi-foo", Some("1.0.0")).await.unwrap();
        assert_eq!(resolved.tarball, tarball);
        assert_eq!(resolved.integrity, sha512_integrity(b"old"));
        assert_eq!(resolved.version, "1.0.0");
    }

    #[tokio::test]
    async fn npm_resolve_falls_back_to_latest() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "2.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
                "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let resolved = npm_resolve(&client, "pi-foo", None).await.unwrap();
        assert_eq!(resolved.tarball, tarball);
        assert_eq!(resolved.integrity, sha512_integrity(b"new"));
        assert_eq!(resolved.version, "2.0.0");
    }

    #[tokio::test]
    async fn npm_resolve_shasum_fallback() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let resolved = npm_resolve(&client, "pi-foo", None).await.unwrap();
        assert_eq!(resolved.integrity, "sha256:abc123");
    }

    #[tokio::test]
    async fn npm_resolve_missing_version_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = npm_resolve(&client, "pi-foo", Some("9.9.9"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("9.9.9"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn npm_resolve_missing_integrity_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = npm_resolve(&client, "pi-foo", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no integrity"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn stage_npm_package_downloads_verifies_unpacks() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = tiny_tgz();
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let staged = stage_npm_package(&client, "pi-foo", None).await.unwrap();
        assert_eq!(staged.name, "pi-foo");
        assert_eq!(staged.version, "1.0.0");
        assert_eq!(staged.integrity, integrity);
        assert_eq!(
            std::fs::read(staged.dir.path().join("package/package.json")).unwrap(),
            br#"{"name":"pi-foo"}"#
        );
    }

    #[tokio::test]
    async fn stage_npm_package_hash_mismatch_bails() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tarball = spawn_tarball(tiny_tgz()).await;
        let meta = serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"other bytes")}},
            }
        });
        let base = spawn_registry(meta).await;
        let _home = use_npm_env(&base);

        let client = crate::fetch::client().unwrap();
        let err = stage_npm_package(&client, "pi-foo", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("hash mismatch"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn download_verifies_sha512_base64() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let body = b"plugin bytes".to_vec();
        let url = spawn_tarball(body.clone()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha512_integrity(&body));
        let path = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn download_verifies_sha256_hex() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let body = b"plugin bytes".to_vec();
        let url = spawn_tarball(body.clone()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha256_hex(&body));
        let path = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn download_rejects_sha512_mismatch() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let url = spawn_tarball(b"plugin bytes".to_vec()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single(sha512_integrity(b"other bytes"));
        let err = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("hash mismatch"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn download_rejects_unknown_algorithm() {
        let _guard = ENV_GUARD.lock().unwrap();
        let _home = use_npm_env("http://127.0.0.1:1");
        let url = spawn_tarball(b"plugin bytes".to_vec()).await;

        let client = crate::fetch::client().unwrap();
        let want = crate::index::HashSpec::Single("md5:abc".to_string());
        let err = crate::fetch::download(&client, &url, Some(&want))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported hash algorithm"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn sanitize_npm_key_cases() {
        assert_eq!(sanitize_npm_key("pi-foo"), "pi-foo");
        assert_eq!(sanitize_npm_key("@scope/bar"), "scope-bar");
        // Sanitized keys can never trip `remove`'s traversal rejection.
        for key in [sanitize_npm_key("pi-foo"), sanitize_npm_key("@scope/bar")] {
            assert!(!key.contains('/'));
            assert!(!key.contains(".."));
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn install_key_confines_dotdot_and_backslash() {
        // `..`, `a/../b`, `foo..bar` sanitize to safe keys …
        for raw in ["..", "a/../b", "foo..bar"] {
            let key = sanitize_npm_key(raw);
            assert!(!key.contains(".."), "raw {raw:?} → {key:?}");
            assert!(!key.contains('/'), "raw {raw:?} → {key:?}");
            assert!(!key.contains('\\'), "raw {raw:?} → {key:?}");
            assert!(!key.is_empty(), "raw {raw:?}");
            assert_ne!(key, "..");
            assert_ne!(key, ".");
            // … and the shared validator accepts the sanitized form.
            validate_install_key(&key).unwrap();
            install_key(raw).unwrap();
        }
        // … while empty, `.`, and backslash keys are rejected, not mangled.
        assert!(install_key("").is_err());
        assert!(install_key(".").is_err());
        assert!(validate_install_key("..").is_err());
        assert!(validate_install_key("a/b").is_err());
        assert!(validate_install_key("a\\b").is_err());
        assert!(install_key("a\\b").is_err());
        // `remove()` works on the sanitized forms (no traversal trap).
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        for raw in ["..", "a/../b", "foo..bar"] {
            let key = sanitize_npm_key(raw);
            let mut lock = LockFile::default();
            lock.plugins.insert(
                key.clone(),
                LockEntry {
                    ecosystem: "pi-gallery".into(),
                    version: "1.0.0".into(),
                    ..LockEntry::default()
                },
            );
            write_lock(&lock).unwrap();
            let dir = crate::plugins_dir().join("pi").join(&key);
            std::fs::create_dir_all(&dir).unwrap();
            remove(&key).unwrap();
            assert!(!list().unwrap().contains_key(&key));
            assert!(!dir.exists());
        }
    }

    /// Build a fixture tarball with `package/`-prefixed entries (npm layout).
    fn skill_tgz(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut ar = tar::Builder::new(Vec::new());
        for (name, content) in files {
            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(content.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_mtime(0);
            hdr.set_cksum();
            ar.append_data(&mut hdr, format!("package/{name}"), content.as_bytes())
                .unwrap();
        }
        let tar_bytes = ar.into_inner().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    /// Registry stub serving one metadata doc on ANY path (scoped names).
    async fn spawn_registry_any(meta: serde_json::Value) -> String {
        use axum::{Json, Router, routing::get};
        let router = Router::new().route(
            "/*rest",
            get(move || {
                let meta = meta.clone();
                async move { Json(meta) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    const MULCH_SKILL: &str = "---\ndescription: mulch skill\n---\nMulch body";
    const TMUX_SKILL: &str = "---\ndescription: tmux skill\n---\nTmux body";

    fn pi_foo_meta(tarball: &str, integrity: &str) -> serde_json::Value {
        serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
            }
        })
    }

    #[tokio::test]
    async fn install_npm_extracts_skills_and_writes_lock() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = skill_tgz(&[
            (
                "package.json",
                r#"{"name":"pi-foo","version":"1.0.0","pi":{"extensions":["./dist/extension.js"]}}"#,
            ),
            ("skills/mulch/SKILL.md", MULCH_SKILL),
            ("skills/run-in-tmux/SKILL.md", TMUX_SKILL),
            (
                "skills/run-in-tmux/scripts/run-in-tmux",
                "#!/bin/sh\necho hi\n",
            ),
            ("extensions/mulch.ts", "export const x = 1;\n"),
            ("themes/dark.css", "body {}\n"),
            ("README.md", "# pi-foo\n"),
        ]);
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
        let _home = use_npm_env(&base);

        let report = install(parse_spec("npm:pi-foo"), InstallOpts::default())
            .await
            .unwrap();
        assert_eq!(report.name, "pi-foo");
        assert_eq!(report.version, "1.0.0");

        // `.md` only: skills + top-level doc land, code never does.
        assert_eq!(
            std::fs::read(report.path.join("mulch/SKILL.md")).unwrap(),
            MULCH_SKILL.as_bytes()
        );
        assert!(report.path.join("run-in-tmux/SKILL.md").is_file());
        assert!(report.path.join("README.md").is_file());
        assert!(!report.path.join("run-in-tmux/scripts/run-in-tmux").exists());
        assert!(!report.path.join("extensions").exists());
        assert!(!report.path.join("package.json").exists());

        // ONE lock entry, exact shape.
        let entry = list().unwrap().remove("pi-foo").expect("lock entry");
        assert_eq!(entry.ecosystem, "pi-gallery");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.hash, integrity);
        assert_eq!(entry.source, tarball);
        assert_eq!(entry.scope, "user");
        assert!(entry.enabled);
    }

    #[tokio::test]
    async fn install_npm_scoped_name_sanitizes_key_and_dir() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = skill_tgz(&[("skills/a/SKILL.md", MULCH_SKILL)]);
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let meta = serde_json::json!({
            "dist-tags": {"latest": "2.0.0"},
            "versions": {
                "2.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
            }
        });
        let base = spawn_registry_any(meta).await;
        let _home = use_npm_env(&base);

        let report = install(parse_spec("npm:@scope/bar"), InstallOpts::default())
            .await
            .unwrap();
        assert_eq!(report.name, "scope-bar");
        assert!(
            report.path.ends_with("pi/scope-bar"),
            "{}",
            report.path.display()
        );
        assert!(report.path.join("a/SKILL.md").is_file());
        let entry = list().unwrap().remove("scope-bar").expect("lock entry");
        assert_eq!(entry.ecosystem, "pi-gallery");
        assert_eq!(entry.version, "2.0.0");
    }

    #[tokio::test]
    async fn install_npm_honors_manifest_skill_globs() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = skill_tgz(&[
            (
                "package.json",
                r#"{"name":"pi-foo","version":"1.0.0","pi":{"skills":["./custom"]}}"#,
            ),
            ("custom/weird/SKILL.md", MULCH_SKILL),
        ]);
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
        let _home = use_npm_env(&base);

        let report = install(parse_spec("npm:pi-foo"), InstallOpts::default())
            .await
            .unwrap();
        assert_eq!(report.name, "pi-foo");
        assert!(report.path.join("weird/SKILL.md").is_file());
    }

    #[test]
    fn manifest_glob_bases_stay_in_staging_root() {
        assert!(glob_base_is_safe("skills"));
        assert!(glob_base_is_safe("custom/weird"));
        assert!(!glob_base_is_safe("/etc/x.md"));
        assert!(!glob_base_is_safe("../../evil.md"));
        assert!(!glob_base_is_safe("a/../../b"));
        assert!(!glob_base_is_safe(""));
    }

    #[test]
    fn manifest_escape_globs_are_skipped_but_legit_match_survives() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("skills/legit")).unwrap();
        std::fs::write(root.path().join("skills/legit/SKILL.md"), MULCH_SKILL).unwrap();
        // Sibling outside the staging root that an escape glob would target.
        let outside = root.path().join("evil.md");
        std::fs::write(&outside, "evil").unwrap();
        let matches = collect_skill_matches(
            root.path(),
            &[
                "/etc/x.md".to_string(),
                "../../evil.md".to_string(),
                "./skills".to_string(),
            ],
        );
        let labels: Vec<_> = matches.iter().map(|m| m.label.clone()).collect();
        assert!(
            labels.contains(&"legit".to_string()),
            "legit skill survives: {labels:?}"
        );
        // Every match still resolves under the staging root.
        for m in &matches {
            assert!(
                m.src.strip_prefix(root.path()).is_ok(),
                "escape: {}",
                m.src.display()
            );
        }
    }

    // --- P2-4 git fixture repos (local commits only, no network) ---

    /// `git` fixture repo with `files` committed on the default branch.
    /// Returns the fixture dir (deleted on drop) and its `file://` URL.
    fn init_git_fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "fixture"]);
        let url = format!("file://{}", dir.path().display());
        (dir, url)
    }

    fn git_fixture_sha(dir: &tempfile::TempDir) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// `GRAY_HOME` at a fresh tempdir (git installs need no registry stub).
    /// Must be called under `ENV_GUARD`.
    fn use_git_env() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        home
    }

    fn git_fixture_key(dir: &tempfile::TempDir) -> String {
        sanitize_npm_key(dir.path().file_name().unwrap().to_str().unwrap())
    }

    #[tokio::test]
    async fn install_git_clones_and_extracts_skills() {
        let _guard = ENV_GUARD.lock().unwrap();
        let (repo, url) = init_git_fixture(&[
            ("skills/mulch/SKILL.md", MULCH_SKILL),
            ("extensions/mulch.ts", "export const x = 1;\n"),
            ("README.md", "# demo\n"),
        ]);
        let sha = git_fixture_sha(&repo);
        let key = git_fixture_key(&repo);
        let _home = use_git_env();

        let report = install(parse_spec(&format!("git:{url}")), InstallOpts::default())
            .await
            .unwrap();
        assert_eq!(report.name, key);
        assert_eq!(report.version, "0.0.0");

        assert_eq!(
            std::fs::read(report.path.join("mulch/SKILL.md")).unwrap(),
            MULCH_SKILL.as_bytes()
        );
        assert!(report.path.join("README.md").is_file());
        assert!(!report.path.join("extensions").exists());

        // R17 lock shape: raw 40-hex sha, source is the clone URL.
        assert_eq!(sha.len(), 40);
        let entry = list().unwrap().remove(&key).expect("lock entry");
        assert_eq!(entry.ecosystem, "pi-gallery");
        assert_eq!(entry.version, "0.0.0");
        assert_eq!(entry.hash, sha);
        assert_eq!(entry.source, url);
        assert_eq!(entry.scope, "user");
        assert!(entry.enabled);
    }

    #[tokio::test]
    async fn install_git_pinned_ref_records_version() {
        let _guard = ENV_GUARD.lock().unwrap();
        let (repo, url) = init_git_fixture(&[("skills/a/SKILL.md", MULCH_SKILL)]);
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["checkout", "-qb", "feature"]);
        std::fs::create_dir_all(repo.path().join("skills/b")).unwrap();
        std::fs::write(repo.path().join("skills/b/SKILL.md"), TMUX_SKILL).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "feature"]);
        let sha = git_fixture_sha(&repo);
        let key = git_fixture_key(&repo);
        let _home = use_git_env();

        let report = install(
            parse_spec(&format!("git:{url}@feature")),
            InstallOpts::default(),
        )
        .await
        .unwrap();
        assert_eq!(report.name, key);
        assert_eq!(report.version, "feature");
        assert!(report.path.join("b/SKILL.md").is_file());
        let entry = list().unwrap().remove(&key).expect("lock entry");
        assert_eq!(entry.version, "feature");
        assert_eq!(entry.hash, sha);
        assert_eq!(entry.source, url);
    }

    #[tokio::test]
    async fn install_git_without_skills_bails_without_half_state() {
        let _guard = ENV_GUARD.lock().unwrap();
        let (repo, _url) = init_git_fixture(&[("notes.txt", "no skills here\n")]);
        let key = git_fixture_key(&repo);
        let _home = use_git_env();

        let err = install(
            parse_spec(&format!("git:file://{}", repo.path().display())),
            InstallOpts::default(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("no skills"), "honest reason: {err}");
        assert!(list().unwrap().is_empty());
        assert!(!crate::plugins_dir().join("pi").join(&key).exists());
    }

    #[tokio::test]
    async fn install_npm_without_skills_bails_without_half_state() {
        let _guard = ENV_GUARD.lock().unwrap();
        let tgz = tiny_tgz();
        let tarball = spawn_tarball(tgz.clone()).await;
        let integrity = sha512_integrity(&tgz);
        let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
        let _home = use_npm_env(&base);

        let err = install(parse_spec("npm:pi-foo"), InstallOpts::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("pi-foo"), "names package: {err}");
        assert!(err.contains("no skills"), "honest reason: {err}");
        // Nothing recorded, nothing left behind.
        assert!(list().unwrap().is_empty());
        assert!(!crate::plugins_dir().join("pi").exists());
    }

    #[test]
    fn remove_deletes_pi_subdir() {
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "scope-bar".to_string(),
            LockEntry {
                ecosystem: "pi-gallery".into(),
                version: "2.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        let dir = crate::plugins_dir().join("pi").join("scope-bar");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "x").unwrap();
        remove("scope-bar").unwrap();
        assert!(!list().unwrap().contains_key("scope-bar"));
        assert!(!dir.exists());
    }

    #[test]
    fn lock_roundtrips_exact_shape() {
        let mut lock = LockFile {
            schema: 1,
            plugins: BTreeMap::new(),
        };
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                hash: "sha256:abc".into(),
                source: "https://h/demo.tar.gz".into(),
                argv: vec![],
                adapter_version: "0.1.0".into(),
                installed_at: "1".into(),
                scope: "user".into(),
                enabled: true,
            },
        );
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
        assert_eq!(v["schema"], 1);
        let e = &v["plugins"]["demo"];
        for k in [
            "ecosystem",
            "version",
            "hash",
            "source",
            "argv",
            "adapter_version",
            "installed_at",
            "scope",
        ] {
            assert!(e.get(k).is_some(), "missing {k}");
        }
    }

    #[test]
    fn lock_enabled_defaults_true_and_roundtrips_false() {
        // Old locks without `enabled` load as enabled.
        let old: LockFile = serde_json::from_str(
            r#"{"schema":1,"plugins":{"demo":{"ecosystem":"gray-native","version":"1.0.0","hash":"sha256:abc","source":"https://h/demo.tar.gz","argv":[],"adapter_version":"0.1.0","installed_at":"1","scope":"user"}}}"#,
        )
        .unwrap();
        assert!(old.plugins["demo"].enabled);
        // New locks round-trip an explicit `false`.
        let mut lock = old.clone();
        lock.plugins.get_mut("demo").unwrap().enabled = false;
        let back: LockFile = serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
        assert!(!back.plugins["demo"].enabled);
    }

    #[test]
    fn set_enabled_flips_flag_and_bails_on_miss() {
        // GRAY_HOME points at a tempdir so the real lockfile is never
        // disturbed (serialized by ENV_GUARD).
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                enabled: true,
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        set_enabled("demo", false).unwrap();
        assert!(!list().unwrap()["demo"].enabled);
        set_enabled("demo", true).unwrap();
        assert!(list().unwrap()["demo"].enabled);
        let err = set_enabled("nope", false).unwrap_err();
        assert_eq!(err.to_string(), "not installed: nope");
    }

    #[test]
    fn remove_rejects_traversal_and_empty_names() {
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        for bad in ["../evil", "a/b", "..", ""] {
            let err = remove(bad).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("not installed: {bad}"),
                "name {bad:?}"
            );
        }
        // The lock entry and the plugins dir survive the rejections.
        assert!(list().unwrap().contains_key("demo"));
    }

    #[test]
    fn remove_deletes_entry_and_dir() {
        let _guard = ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let mut lock = LockFile::default();
        lock.plugins.insert(
            "demo".to_string(),
            LockEntry {
                ecosystem: "gray-native".into(),
                version: "1.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        let dir = crate::plugins_dir().join("demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.sh"), "#!/bin/sh\n").unwrap();
        remove("demo").unwrap();
        assert!(!list().unwrap().contains_key("demo"));
        assert!(!dir.exists());
    }

    // --- P2-3 search fan-out fixtures (loopback only, no live network) ---

    async fn spawn_index_stub(index: serde_json::Value) -> String {
        use axum::{Json, Router, routing::get};
        let router = Router::new().route(
            "/index.json",
            get(move || {
                let index = index.clone();
                async move { Json(index) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/index.json")
    }

    /// Gray Index stub that always 404s (the unpublished production index).
    async fn spawn_index_404_stub() -> String {
        use axum::{Router, http::StatusCode, routing::get};
        let router = Router::new().route("/index.json", get(|| async { StatusCode::NOT_FOUND }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/index.json")
    }

    async fn spawn_search_stub(objects: serde_json::Value) -> String {
        use axum::{Json, Router, routing::get};
        let router = Router::new().route(
            "/-/v1/search",
            get(move || {
                let objects = objects.clone();
                async move { Json(serde_json::json!({"objects": objects})) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    fn index_fixture(names: &[(&str, &str)]) -> serde_json::Value {
        let mut plugins = serde_json::Map::new();
        for (name, version) in names {
            plugins.insert(
                (*name).to_string(),
                serde_json::json!({
                    "ecosystem": "gray-native",
                    "version": version,
                    "source": {"type": "https", "url": "https://h/x.tar.gz"},
                    "hash": "sha256:abc",
                }),
            );
        }
        serde_json::json!({"schema": 1, "generated": "", "plugins": plugins})
    }

    fn search_objects(pkgs: &[(&str, &str, &str)]) -> serde_json::Value {
        pkgs.iter()
            .map(|(name, version, desc)| {
                serde_json::json!({"package": {"name": name, "version": version, "description": desc}})
            })
            .collect()
    }

    /// Point `GRAY_HOME` at a fresh tempdir and the index + npm search at
    /// stubs. ClawHub + Claude point at guaranteed-unreachable endpoints
    /// so these pre-Task-2 tests stay hermetic (no live network).
    /// Must be called under `ENV_GUARD`.
    fn use_search_env(index_url: &str, registry_base: &str) -> tempfile::TempDir {
        use_market_env(
            index_url,
            registry_base,
            "http://127.0.0.1:1",
            "file:///nonexistent-gray-fixture",
        )
    }

    /// Full search env: index + npm registry + ClawHub base + Claude
    /// marketplace specs. Must be called under `ENV_GUARD`.
    fn use_market_env(
        index_url: &str,
        registry_base: &str,
        clawhub_base: &str,
        claude_markets: &str,
    ) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
            std::env::set_var(crate::index::INDEX_URL_ENV, index_url);
            std::env::set_var(NPM_REGISTRY_ENV, registry_base);
            std::env::set_var(crate::sources::CLAWHUB_BASE_ENV, clawhub_base);
            std::env::set_var(crate::sources::CLAUDE_MARKETPLACES_ENV, claude_markets);
        }
        home
    }

    #[tokio::test]
    async fn search_merges_gray_first_with_source_labels() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[("gray-foo", "1.0.0")])).await;
        let registry =
            spawn_search_stub(search_objects(&[("pi-bar", "2.0.0", "does things")])).await;
        let _home = use_search_env(&index_url, &registry);

        let out = search_all("gray").await.unwrap();
        assert!(!out.pi_unreachable);
        assert_eq!(out.hits.len(), 2);
        assert_eq!(out.hits[0].source, SearchSource::Gray);
        assert_eq!(out.hits[0].name, "gray-foo");
        assert_eq!(out.hits[1].source, SearchSource::Pi);
        assert_eq!(out.hits[1].name, "pi-bar");
        assert_eq!(
            format_search_hit(&out.hits[0]),
            "gray-foo 1.0.0 [Gray Index]"
        );
        assert_eq!(
            format_search_hit(&out.hits[1]),
            "pi-bar 2.0.0 [Pi Gallery (preview)] - does things"
        );
    }

    #[tokio::test]
    async fn search_pi_reports_npm_popularity() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[])).await;
        let objects = serde_json::json!([
            {"package": {"name": "pi", "version": "1.0.0", "description": "d"},
             "score": {"detail": {"popularity": 0.83}}}
        ]);
        let registry = spawn_search_stub(objects).await;
        let _home = use_search_env(&index_url, &registry);

        let out = search_all("pi").await.unwrap();
        let hit = out
            .hits
            .iter()
            .find(|h| h.source == SearchSource::Pi)
            .unwrap();
        assert!((hit.popularity - 0.83).abs() < 1e-6);
    }

    #[tokio::test]
    async fn search_collision_prefers_gray() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[("gray-foo", "1.0.0")])).await;
        let registry = spawn_search_stub(search_objects(&[("gray-foo", "9.9.9", "pi copy")])).await;
        let _home = use_search_env(&index_url, &registry);

        let out = search_all("gray").await.unwrap();
        assert!(!out.pi_unreachable);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].name, "gray-foo");
        assert_eq!(out.hits[0].version, "1.0.0");
        assert_eq!(out.hits[0].source, SearchSource::Gray);
    }

    #[tokio::test]
    async fn search_pi_unreachable_is_advisory_not_error() {
        let _guard = ENV_GUARD.lock().unwrap();
        // Closed loopback port: connection refused, instantly.
        let index_url = spawn_index_stub(index_fixture(&[("gray-foo", "1.0.0")])).await;
        let _home = use_search_env(&index_url, "http://127.0.0.1:1");

        let out = search_all("gray").await.unwrap();
        assert!(out.pi_unreachable);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].source, SearchSource::Gray);
        assert_eq!(PI_UNREACHABLE_LINE, "Pi Gallery (preview): unreachable");
    }

    #[tokio::test]
    async fn search_pi_unreachable_with_gray_miss_still_ok() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[])).await;
        let _home = use_search_env(&index_url, "http://127.0.0.1:1");

        // Ok (not Err): callers print the advisory line and exit 0.
        let out = search_all("nothing-matches").await.unwrap();
        assert!(out.pi_unreachable);
        assert!(out.hits.is_empty());
    }

    #[tokio::test]
    async fn search_gray_404_is_advisory_with_pi_hits() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_404_stub().await;
        let registry =
            spawn_search_stub(search_objects(&[("pi-bar", "2.0.0", "does things")])).await;
        let _home = use_search_env(&index_url, &registry);

        let out = search_all("pi").await.unwrap();
        assert!(out.gray_unreachable);
        assert!(!out.pi_unreachable);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].source, SearchSource::Pi);
        assert_eq!(GRAY_UNREACHABLE_LINE, "Gray Index: unreachable");
    }

    #[tokio::test]
    async fn search_gray_404_and_pi_down_is_advisory_not_error() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_404_stub().await;
        // Closed loopback port: connection refused, instantly.
        let _home = use_search_env(&index_url, "http://127.0.0.1:1");

        // Ok (not Err): callers print both advisories and exit 0.
        let out = search_all("nothing-matches").await.unwrap();
        assert!(out.gray_unreachable);
        assert!(out.pi_unreachable);
        assert!(out.hits.is_empty());
    }

    #[tokio::test]
    async fn search_corrupt_cache_still_degrades_not_errors() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_404_stub().await;
        let registry =
            spawn_search_stub(search_objects(&[("pi-bar", "2.0.0", "does things")])).await;
        let home = use_search_env(&index_url, &registry);
        // Ruling 1b: corrupt local cache stays swallowed (`.ok()`), so a
        // fetch failure still degrades to advisory instead of erroring.
        let cache = home.path().join("plugins/index-cache.json");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "{not json").unwrap();

        let out = search_all("pi").await.unwrap();
        assert!(out.gray_unreachable);
        assert_eq!(out.hits.len(), 1);
    }

    #[tokio::test]
    async fn search_empty_both_sides_reports_no_hits() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[])).await;
        let registry = spawn_search_stub(search_objects(&[])).await;
        let _home = use_search_env(&index_url, &registry);

        // Ok + empty + reachable: callers keep the `not in index` miss.
        let out = search_all("nothing-matches").await.unwrap();
        assert!(!out.pi_unreachable);
        assert!(out.hits.is_empty());
    }

    #[test]
    fn search_copy_rules_are_exact() {
        assert_eq!(SearchSource::Gray.label(), "Gray Index");
        assert_eq!(SearchSource::Pi.label(), "Pi Gallery (preview)");
        assert!(SearchSource::Pi.label().contains("(preview)"));
        assert_eq!(PI_UNREACHABLE_LINE, "Pi Gallery (preview): unreachable");
        // Empty desc omits the suffix; surrounding whitespace is trimmed.
        let bare = SearchHit {
            name: "n".to_string(),
            version: "1.0.0".to_string(),
            desc: String::new(),
            source: SearchSource::Gray,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
            popularity: 0.0,
        };
        assert_eq!(format_search_hit(&bare), "n 1.0.0 [Gray Index]");
        let padded = SearchHit {
            desc: "  padded  ".to_string(),
            source: SearchSource::Pi,
            ..bare
        };
        assert_eq!(
            format_search_hit(&padded),
            "n 1.0.0 [Pi Gallery (preview)] - padded"
        );
    }

    #[test]
    fn search_pi_scoped_hit_hints_install_key() {
        // Same shared sanitizer, no duplication: raw `@scope/bar` hints the
        // lock key `scope-bar`; unsanitized names stay bare.
        let scoped = SearchHit {
            name: "@scope/bar".to_string(),
            version: "1.2.3".to_string(),
            desc: "does things".to_string(),
            source: SearchSource::Pi,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
            popularity: 0.0,
        };
        assert_eq!(
            format_search_hit(&scoped),
            "@scope/bar 1.2.3 [Pi Gallery (preview)] (install as scope-bar) - does things"
        );
        let plain = SearchHit {
            name: "pi-bar".to_string(),
            version: "2.0.0".to_string(),
            desc: String::new(),
            source: SearchSource::Pi,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
            popularity: 0.0,
        };
        assert_eq!(
            format_search_hit(&plain),
            "pi-bar 2.0.0 [Pi Gallery (preview)]"
        );
        // Gray hits never hint (index names install verbatim).
        let gray = SearchHit {
            name: "@scope/bar".to_string(),
            version: "1.0.0".to_string(),
            desc: String::new(),
            source: SearchSource::Gray,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
            popularity: 0.0,
        };
        assert_eq!(format_search_hit(&gray), "@scope/bar 1.0.0 [Gray Index]");
    }

    #[test]
    fn spec_parses_clawhub_and_claude_forms() {
        match parse_spec("clawhub:arein/test") {
            NameOrUrl::ClawHub { slug } => assert_eq!(slug, "arein/test"),
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("clawhub:test") {
            NameOrUrl::ClawHub { slug } => assert_eq!(slug, "test"),
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("claude:my-plugin@fixture-market") {
            NameOrUrl::Claude {
                plugin,
                marketplace,
            } => {
                assert_eq!(plugin, "my-plugin");
                assert_eq!(marketplace, Some("fixture-market".to_string()));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        // No `@marketplace` stays unpinned; a trailing `@` is unpinned too.
        match parse_spec("claude:my-plugin") {
            NameOrUrl::Claude {
                plugin,
                marketplace,
            } => {
                assert_eq!(plugin, "my-plugin");
                assert_eq!(marketplace, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
        match parse_spec("claude:my-plugin@") {
            NameOrUrl::Claude {
                plugin,
                marketplace,
            } => {
                assert_eq!(plugin, "my-plugin");
                assert_eq!(marketplace, None);
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn new_source_labels_and_lines_are_exact() {
        assert_eq!(SearchSource::Claude.label(), "Claude");
        assert_eq!(SearchSource::ClawHub.label(), "ClawHub");
        assert_eq!(CLAWHUB_UNREACHABLE_LINE, "ClawHub: unreachable");
        assert_eq!(CLAUDE_UNREACHABLE_LINE, "Claude: unreachable");
        // New fields never leak into the rendered hit (Task 1 copy frozen).
        let hit = SearchHit {
            name: "x".to_string(),
            version: "1.0.0".to_string(),
            desc: "d".to_string(),
            source: SearchSource::ClawHub,
            version_detail: "github:o/r@main".to_string(),
            files: vec!["SKILL.md".to_string()],
            trust: "community".to_string(),
            popularity: 0.0,
        };
        assert_eq!(format_search_hit(&hit), "x 1.0.0 [ClawHub] - d");
    }

    // --- Task 2 search/install fixtures (loopback + local dirs only) ---

    /// Minimal stored-ZIP builder (ClawHub serves ZIPs, not tarballs).
    fn skill_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, content) in files {
            let data = content.as_bytes();
            let off = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            for _ in 0..5 {
                out.extend_from_slice(&0u16.to_le_bytes());
            }
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            for _ in 0..3 {
                central.extend_from_slice(&0u16.to_le_bytes());
            }
            central.extend_from_slice(&0u16.to_le_bytes());
            for _ in 0..2 {
                central.extend_from_slice(&0u16.to_le_bytes());
            }
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            for _ in 0..4 {
                central.extend_from_slice(&0u16.to_le_bytes());
            }
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// ClawHub stub: fixed `/search` results plus a `fixture/demo` skill
    /// (detail + empty version files + ZIP download). The download 429s
    /// once (with `Retry-After: 0`) to prove the retry routing; the
    /// verdicts endpoint answers for `claw-foo` only, so `claw-bar` pins
    /// the search-payload fallback path.
    #[derive(Clone)]
    struct ClawStub {
        zip: Vec<u8>,
        downloads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn spawn_clawhub_stub(zip: Vec<u8>) -> String {
        use axum::{
            Json, Router,
            extract::State,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
        };
        use std::sync::atomic::Ordering;
        // `claw-foo` carries NO payload trust: its `scan:clean` must come
        // from the verdicts batch or the test fails.
        let search = serde_json::json!({"results": [
            {"slug": "claw-foo", "displayName": "Claw Foo",
             "summary": "does claw things", "version": "4.0.0",
             "ownerHandle": "fixture", "official": false},
            {"slug": "claw-bar", "displayName": "Claw Bar",
             "summary": "payload trust only", "version": "1.0.0",
             "official": false, "trust": {"clawHubVerdict": "clean"}},
            {"slug": "gray-foo", "displayName": "Gray Copy",
             "summary": "suppressed duplicate", "version": "9.9.9"},
        ]});
        let verdicts = serde_json::json!({"items": [
            {"ok": true, "decision": "pass", "requestedSlug": "claw-foo",
             "requestedOwnerHandle": "fixture", "requestedVersion": "4.0.0",
             "version": "4.0.0", "security": {"status": "clean", "passed": true}},
        ]});
        let detail = serde_json::json!({
            "skill": {"slug": "demo", "displayName": "Demo", "summary": "demo skill"},
            "latestVersion": {"version": "1.0.0"},
            "owner": {"handle": "fixture"},
        });
        let versions = serde_json::json!(
            {"version": {"version": "1.0.0", "files": [], "security": {"status": "clean"}}}
        );
        let router = Router::new()
            .route(
                "/api/v1/search",
                get(move || {
                    let search = search.clone();
                    async move { Json(search) }
                }),
            )
            .route(
                "/api/v1/skills/demo",
                get(move || {
                    let detail = detail.clone();
                    async move { Json(detail) }
                }),
            )
            .route(
                "/api/v1/skills/demo/versions/1.0.0",
                get(move || {
                    let versions = versions.clone();
                    async move { Json(versions) }
                }),
            )
            .route(
                "/api/v1/download",
                get(|State(st): State<ClawStub>| async move {
                    if st.downloads.fetch_add(1, Ordering::SeqCst) == 0 {
                        let mut h = HeaderMap::new();
                        h.insert("retry-after", "0".parse().unwrap());
                        return (StatusCode::TOO_MANY_REQUESTS, h, Vec::new());
                    }
                    (StatusCode::OK, HeaderMap::new(), st.zip.clone())
                }),
            )
            .route(
                "/api/v1/skills/-/security-verdicts",
                post(move || {
                    let verdicts = verdicts.clone();
                    async move { Json(verdicts) }
                }),
            )
            .with_state(ClawStub {
                zip,
                downloads: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/api/v1")
    }

    /// Local Claude marketplace fixture (no network): one path plugin
    /// with a skill plus one command plugin (install must refuse it).
    fn init_claude_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".claude-plugin/marketplace.json"),
            r#"{"name":"fixture-market","plugins":[
                {"name":"claude-foo","description":"does claude things","version":"3.0.0",
                 "source":"./plugins/claude-foo"},
                {"name":"claude-cmd","description":"command thing",
                 "source":{"source":"command","command":"make plugin"}}
            ]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("plugins/claude-foo/skills/greeter")).unwrap();
        std::fs::write(
            dir.path()
                .join("plugins/claude-foo/skills/greeter/SKILL.md"),
            MULCH_SKILL,
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn search_fans_out_gray_pi_claude_clawhub_in_order() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[("gray-foo", "1.0.0")])).await;
        let registry =
            spawn_search_stub(search_objects(&[("pi-foo", "2.0.0", "does pi things")])).await;
        let clawhub = spawn_clawhub_stub(skill_zip(&[("skills/a/SKILL.md", MULCH_SKILL)])).await;
        let market = init_claude_fixture();
        let markets = format!("file://{}", market.path().display());
        let _home = use_market_env(&index_url, &registry, &clawhub, &markets);

        let out = search_all("foo").await.unwrap();
        assert!(!out.pi_unreachable);
        assert!(!out.gray_unreachable);
        assert!(!out.clawhub_unreachable);
        assert!(!out.claude_unreachable);
        // Order Gray, Pi, Claude, ClawHub; the clawhub `gray-foo`
        // duplicate is suppressed (Gray wins).
        let names: Vec<_> = out
            .hits
            .iter()
            .map(|h| (h.name.as_str(), h.source))
            .collect();
        assert_eq!(
            names,
            vec![
                ("gray-foo", SearchSource::Gray),
                ("pi-foo", SearchSource::Pi),
                ("claude-foo", SearchSource::Claude),
                ("fixture/claw-foo", SearchSource::ClawHub),
                ("claw-bar", SearchSource::ClawHub),
            ]
        );
        // `claw-foo` ships no payload trust: `scan:clean` proves the
        // verdicts batch ran. `claw-bar` has no verdict: payload fallback.
        let claw = out
            .hits
            .iter()
            .find(|h| h.name == "fixture/claw-foo")
            .unwrap();
        assert_eq!(claw.trust, "community + scan:clean");
        assert_eq!(
            format_search_hit(claw),
            "fixture/claw-foo 4.0.0 [ClawHub] (install as fixture-claw-foo) - does claw things"
        );
        let bar = out.hits.iter().find(|h| h.name == "claw-bar").unwrap();
        assert_eq!(bar.trust, "community + scan:clean");
        let claude = out.hits.iter().find(|h| h.name == "claude-foo").unwrap();
        assert_eq!(claude.version, "3.0.0");
        assert_eq!(claude.version_detail, "./plugins/claude-foo");
        // Command plugins never surface (not installable).
        assert!(!out.hits.iter().any(|h| h.name == "claude-cmd"));
    }

    #[tokio::test]
    async fn search_clawhub_and_claude_down_is_advisory() {
        let _guard = ENV_GUARD.lock().unwrap();
        let index_url = spawn_index_stub(index_fixture(&[("gray-foo", "1.0.0")])).await;
        let registry =
            spawn_search_stub(search_objects(&[("pi-foo", "2.0.0", "does pi things")])).await;
        let _home = use_market_env(
            &index_url,
            &registry,
            "http://127.0.0.1:1",
            "file:///nonexistent-gray-fixture",
        );

        let out = search_all("foo").await.unwrap();
        assert!(out.clawhub_unreachable);
        assert!(out.claude_unreachable);
        assert!(!out.pi_unreachable);
        assert!(!out.gray_unreachable);
        assert_eq!(out.hits.len(), 2);
        assert_eq!(CLAWHUB_UNREACHABLE_LINE, "ClawHub: unreachable");
        assert_eq!(CLAUDE_UNREACHABLE_LINE, "Claude: unreachable");
    }

    #[tokio::test]
    async fn install_clawhub_downloads_and_writes_lock() {
        let _guard = ENV_GUARD.lock().unwrap();
        let zip = skill_zip(&[
            ("skills/greeter/SKILL.md", MULCH_SKILL),
            ("README.md", "# demo\n"),
        ]);
        let clawhub = spawn_clawhub_stub(zip).await;
        let market = init_claude_fixture();
        let markets = format!("file://{}", market.path().display());
        let index_url = spawn_index_stub(index_fixture(&[])).await;
        let _home = use_market_env(&index_url, "http://127.0.0.1:1", &clawhub, &markets);

        let report = install(parse_spec("clawhub:fixture/demo"), InstallOpts::default())
            .await
            .unwrap();
        // Owner-qualified key: no collision with another owner's `demo`.
        assert_eq!(report.name, "fixture-demo");
        assert_eq!(report.version, "1.0.0");
        // No version file list from the stub → honest unverified path.
        // (The stub 429s the download once: success proves the retry.)
        assert!(report.path.join("greeter/SKILL.md").is_file());
        let entry = list().unwrap().remove("fixture-demo").expect("lock entry");
        assert_eq!(entry.ecosystem, "clawhub");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.source, "https://clawhub.ai/fixture/skills/demo");
    }

    #[tokio::test]
    async fn install_claude_path_source_and_command_refusal() {
        let _guard = ENV_GUARD.lock().unwrap();
        let market = init_claude_fixture();
        let markets = format!("file://{}", market.path().display());
        let index_url = spawn_index_stub(index_fixture(&[])).await;
        let _home = use_market_env(
            &index_url,
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
            &markets,
        );

        let report = install(
            parse_spec("claude:claude-foo@fixture-market"),
            InstallOpts::default(),
        )
        .await
        .unwrap();
        assert_eq!(report.name, "claude-foo");
        assert_eq!(report.version, "3.0.0");
        assert!(report.path.join("greeter/SKILL.md").is_file());
        let entry = list().unwrap().remove("claude-foo").expect("lock entry");
        assert_eq!(entry.ecosystem, "claude");

        // `command` sources refuse with the warning string (never exec)…
        let err = install(parse_spec("claude:claude-cmd"), InstallOpts::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("command source"), "warns honestly: {err}");
        // …and the failure is recorded (Task 1 ruling holds for new arms).
        let recorded = crate::errors::list();
        assert!(
            recorded
                .iter()
                .any(|e| e.source == "claude" && e.item == "claude-cmd"),
            "registry keeps the failure: {recorded:?}"
        );
        // Unknown marketplace filters miss honestly too.
        let err = install(
            parse_spec("claude:claude-foo@no-such-market"),
            InstallOpts::default(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in claude marketplaces"), "miss: {err}");
    }
}
