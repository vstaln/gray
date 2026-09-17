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
    if (t.starts_with("http://") || t.starts_with("https://"))
        && (https_has_git_suffix(t) || https_is_bare_github_repo(t))
    {
        return parse_git_spec(t);
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        NameOrUrl::Url(t.to_string())
    } else {
        NameOrUrl::Name(t.to_string())
    }
}

/// Split `scheme://authority` off; returns `(head, remainder)`.
/// A `file://C:\...`-style Windows path keeps its drive in the path: the
/// authority ends at the first separator *after* the drive, never at the
/// drive colon itself.
fn split_authority(s: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = s.split_once("://")?;
    let is_win_path =
        rest.len() >= 2 && rest.as_bytes()[1] == b':' && rest.as_bytes()[0].is_ascii_alphabetic();
    let end = if is_win_path {
        // Backslash is a Windows path separator: without it the drive path
        // never ends, the remainder collapses to empty, and @ref splitting
        // is lost (CI round 2: `file://C:\tmp\repo@feature` stayed whole).
        rest[2..]
            .find(['/', '\\', '?', '#'])
            .map_or(rest.len(), |i| i + 2)
    } else {
        rest.find(['/', '?', '#']).unwrap_or(rest.len())
    };
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

/// Raw `http(s)` URL pointing at a bare `github.com/<owner>/<repo>`
/// repo page (what users paste): a git source, so
/// `gray plugin install https://github.com/<owner>/<repo>` clones like the
/// `git:` form. Deeper paths (releases, trees, blobs — e.g. tarball download
/// URLs) stay tarball [`NameOrUrl::Url`].
fn https_is_bare_github_repo(t: &str) -> bool {
    let Some((head, remainder)) = split_authority(t) else {
        return false;
    };
    let host = head
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .to_ascii_lowercase();
    if host != "github.com" {
        return false;
    }
    // Strip `@ref` (R16) then query/fragment; allow an optional `.git`.
    let path = match remainder.rfind('@') {
        Some(i) => &remainder[..i],
        None => remainder,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() != 2 {
        return false;
    }
    let repo = segs[1].strip_suffix(".git").unwrap_or(segs[1]);
    !segs[0].is_empty() && !repo.is_empty()
}

/// Install name from a git URL: last path segment minus `.git`.
fn name_from_git_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    // After the scheme, treat everything up to the first separator as the
    // authority — except a Windows drive letter, which stays in the path.
    // `git@host:path` (no scheme) keeps its scp-style host split.
    let after_host = match path.split_once("://") {
        Some((_, rest)) => {
            let is_win = rest.len() >= 2
                && rest.as_bytes()[1] == b':'
                && rest.as_bytes()[0].is_ascii_alphabetic();
            if is_win {
                rest
            } else {
                rest.find('/').map(|i| &rest[i + 1..]).unwrap_or("")
            }
        }
        // Same drive-letter rule as split_authority: "C:..." stays whole,
        // while a later colon (scp-style git@host:path) splits.
        None => match path.find(':') {
            Some(i)
                if i == 1
                    && path.len() > 2
                    && (path.as_bytes()[2] == b'\\' || path.as_bytes()[2] == b'/') =>
            {
                path
            }
            Some(i) => &path[i + 1..],
            None => path,
        },
    };
    let after_host = after_host.trim_end_matches('/');
    // Windows paths may use backslashes; the name is the last segment of
    // either separator spelling.
    let last = after_host.rsplit(['/', '\\']).next().unwrap_or(after_host);
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
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temporary.write_all(serde_json::to_string_pretty(&lock)?.as_bytes())?;
    temporary.persist(&path)?;
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
            installed_at: crate::now_secs().to_string(),
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
            if dist
                .get("shasum")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                anyhow::bail!(
                    "npm package {name}@{want} provides only legacy SHA-1; a SHA-256/512 integrity digest is required"
                );
            }
            anyhow::bail!("npm package {name}@{want} has no integrity");
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

/// Lock key + `pi/` dir name for an npm package: `@scope/name` →
/// `scope-name` (strip leading `@`, `/` → `-`, `..` → `-`). Sanitized
/// keys never contain `/` or `..`, so [`remove`]'s traversal rejection
/// still holds. Callers must still run [`validate_install_key`] (via
/// [`install_key`]) — sanitize alone cannot turn `.`, empty, or `\`
/// inputs into safe keys, so those are rejected, not mangled. This
/// closes the parked npm-side `..` gap as well as the git `..` path:
/// both arms derive keys through [`install_key`].
pub fn sanitize_npm_key(name: &str) -> String {
    // Backslashes stay illegal: a name still carrying them (e.g. a whole
    // drive path) must be rejected by validate_install_key, never mangled
    // into a plausible key.
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

/// Sorted child directories of `dir` that contain a `SKILL.md`.
fn skill_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("SKILL.md").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Collect skill matches: `skills/*/SKILL.md`, `*/SKILL.md`, manifest
/// `pi.skills` globs, top-level `*.md` (R14: top-level `.md` are copied
/// for reference and listed in `docs`, not `taken`). Deduped, `taken`
/// order stable.
fn collect_skill_matches(root: &Path, globs: &[String]) -> Vec<SkillMatch> {
    let mut out: Vec<SkillMatch> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // `skills/*/SKILL.md` (the common pi layout).
    for dir in skill_dirs(&root.join("skills")) {
        push_skill_dir(&dir, &mut out, &mut seen);
    }

    // `*/SKILL.md` (one level under root; dedupe covers `skills/`).
    for dir in skill_dirs(root) {
        push_skill_dir(&dir, &mut out, &mut seen);
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
            for dir in skill_dirs(&path) {
                push_skill_dir(&dir, &mut out, &mut seen);
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
pub(crate) fn extract_pi_skills(
    root: &Path,
    key: &str,
    version: &str,
    hash: &str,
    source: &str,
    ecosystem: &str,
    opts: &InstallOpts,
) -> anyhow::Result<PathBuf> {
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
    let skipped_ext = !manifest_ext.is_empty() || root.join("extensions").is_dir() || ext_n > 0;
    let skipped_themes = !manifest_themes.is_empty()
        || root.join("themes").is_dir()
        || root.join("theme").is_dir()
        || theme_n > 0;
    if skipped_ext {
        eprintln!("skipped {ext_n} extension files (P3)");
    }
    if skipped_themes {
        eprintln!("skipped {theme_n} theme files (P3)");
    }
    eprintln!("skills taken: {}", taken.join(", "));
    if !docs.is_empty() {
        eprintln!("docs copied for reference: {}", docs.join(", "));
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
    Ok(dest)
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
    let dest = extract_pi_skills(
        &root,
        &key,
        &staged.version,
        &staged.integrity,
        &staged.tarball,
        "pi-gallery",
        &opts,
    )?;
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
    let dest = extract_pi_skills(&clone_dir, &key, &version, &sha, url, "pi-gallery", &opts)?;
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
    let dest = extract_pi_skills(&root, &key, &version, "", &source_url, "clawhub", &opts)?;
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
    let dest = extract_pi_skills(
        &resolved.root,
        &key,
        &resolved.version,
        &resolved.hash,
        &resolved.source_url,
        "claude",
        &opts,
    )?;
    Ok(Report {
        name: key,
        version: resolved.version,
        path: dest,
    })
}

/// Install a fully staged tree, rolling back the directory if lock recording fails.
fn replace_archive(
    archive: &Path,
    name: &str,
    record: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<PathBuf> {
    validate_install_key(name)?;
    anyhow::ensure!(
        !matches!(name, "tmp" | "pi" | "lock.json" | "index-cache.json"),
        "reserved plugin name"
    );
    let root = crate::plugins_dir();
    std::fs::create_dir_all(&root)?;
    let stage = tempfile::tempdir_in(&root)?;
    let next = stage.path().join("next");
    let previous = stage.path().join("previous");
    std::fs::create_dir(&next)?;
    let unpacked = crate::fetch::unpack_tar_gz(archive, &next);
    let _ = std::fs::remove_file(archive);
    unpacked?;
    let dest = root.join(name);
    let had_previous = dest.symlink_metadata().is_ok();
    if had_previous {
        std::fs::rename(&dest, &previous)?;
    }
    let installed = std::fs::rename(&next, &dest)
        .map_err(anyhow::Error::from)
        .and_then(|()| record());
    if let Err(error) = installed {
        let rollback = (|| -> std::io::Result<()> {
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            if had_previous {
                std::fs::rename(&previous, &dest)?;
            }
            Ok(())
        })();
        if let Err(rollback) = rollback {
            let recovery = stage.keep();
            anyhow::bail!(
                "{error}; rollback failed: {rollback}; previous install saved at {}",
                recovery.display()
            );
        }
        return Err(error);
    }
    Ok(dest)
}

async fn install_index(
    client: &reqwest::Client,
    name: &str,
    opts: InstallOpts,
) -> anyhow::Result<Report> {
    validate_install_key(name)?;
    let index = crate::index::fetch_index(client).await?;
    let entry = crate::index::lookup(&index, name)?;
    ensure_gray_native(&entry.ecosystem, &entry.source.type_)?;
    let archive = crate::fetch::download(client, &entry.source.url, Some(&entry.hash)).await?;
    let scope = if entry.scope.is_empty() {
        opts.scope.clone().unwrap_or_else(|| "user".to_string())
    } else {
        entry.scope.clone()
    };
    let dest = replace_archive(&archive, name, || {
        record_install(
            name,
            &entry.ecosystem,
            &entry.version,
            entry.hash.primary().unwrap_or_default(),
            &entry.source.url,
            scope,
            opts.argv.clone(),
        )
    })?;
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
    validate_install_key(&name)?;
    let archive = crate::fetch::download(client, url, None).await?;
    let dest = replace_archive(&archive, &name, || {
        record_install(
            &name,
            "url",
            "0.0.0",
            "",
            url,
            opts.scope.clone().unwrap_or_else(|| "user".to_string()),
            opts.argv.clone(),
        )
    })?;
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

#[path = "ops_tests.rs"]
#[cfg(test)]
pub(crate) mod tests;
