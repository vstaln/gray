//! Agent skills backend: install/list/remove over `<agent_dir>/skills/`.
//!
//! Install resolves `clawhub:` / `github:` / `url:` / local-path specs
//! into `<agent_dir>/skills/<slug>/` (agent dir is [`crate::gray_home`],
//! exactly like [`crate::ops`]), reusing the Task 2 download/verify
//! paths. `update` is OUT (later task).

use std::path::{Path, PathBuf};

/// Install origin, written to `<skill>/.gray-origin.json` (mirrors ClawHub's
/// origin.json shape: version + registry/slug/owner + pinned version +
/// timestamp + inspectable URL).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillOrigin {
    pub version: u32,
    #[serde(default)]
    pub registry: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub installed_version: String,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub source_url: String,
}

/// One installed skill: `name` is the `<agent_dir>/skills/` dir name (the
/// install key); `version`/`source` come from the origin file when present
/// (`""`/`"local"` for hand-placed skills without one).
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub name: String,
    pub version: String,
    pub source: String,
    pub origin: Option<SkillOrigin>,
}

/// Origin sidecar filename inside each installed skill dir.
pub const ORIGIN_FILE: &str = ".gray-origin.json";

/// Agent skills dir: [`crate::gray_home`] + `skills` (same agent-dir
/// resolution [`crate::ops`] uses via [`crate::plugins_dir`]).
pub(crate) fn skills_dir() -> PathBuf {
    crate::gray_home().join("skills")
}

/// `(registry, item)` identity for install failures, from the spec kind.
fn spec_identity(spec: &str) -> (String, String) {
    let t = spec.trim();
    match t.split_once(':') {
        Some(("clawhub", _)) => ("clawhub".to_string(), t.to_string()),
        Some(("claude", _)) => ("claude".to_string(), t.to_string()),
        Some(("github", _)) => ("github".to_string(), t.to_string()),
        Some(("url", _)) => ("url".to_string(), t.to_string()),
        _ => ("local".to_string(), t.to_string()),
    }
}

pub async fn install(spec: &str) -> anyhow::Result<crate::ops::Report> {
    let (registry, item) = spec_identity(spec);
    install_inner(spec).await.map_err(|e| {
        crate::errors::record(&registry, &item, format!("{e:#}"));
        e
    })
}

async fn install_inner(spec: &str) -> anyhow::Result<crate::ops::Report> {
    let client = crate::fetch::client()?;
    let t = spec.trim();
    if let Some(body) = t.strip_prefix("clawhub:") {
        let pending = stage_clawhub(&client, body).await?;
        return finish_skill(pending);
    }
    if let Some(body) = t.strip_prefix("github:") {
        let pending = stage_github(body).await?;
        return finish_skill(pending);
    }
    if let Some(body) = t.strip_prefix("url:") {
        let pending = stage_url(&client, body).await?;
        return finish_skill(pending);
    }
    if t.is_empty() {
        anyhow::bail!("skill spec is empty");
    }
    finish_skill(stage_local(t)?)
}

/// A validated bundle awaiting the shared copy + origin write. `_staging`
/// keeps tempdirs (ClawHub ZIP unpack, git clone, url SKILL.md) alive until
/// the copy completes; local paths stage nothing.
struct PendingSkill {
    _staging: Option<tempfile::TempDir>,
    root: PathBuf,
    slug: String,
    version: String,
    unverified: bool,
    warn_source: Option<String>,
    registry: String,
    owner: String,
    source_url: String,
}

/// Validate the SKILL.md bundle, copy it into `<skills>/<slug>/`, write the
/// origin sidecar, and report. Failures leave no half-state (a partial dest
/// is removed).
fn finish_skill(p: PendingSkill) -> anyhow::Result<crate::ops::Report> {
    crate::ops::validate_install_key(&p.slug)?;
    if !p.root.join("SKILL.md").is_file() {
        anyhow::bail!(
            "skill bundle '{}' has no SKILL.md (not a skill bundle)",
            p.slug
        );
    }
    let text = std::fs::read_to_string(p.root.join("SKILL.md"))?;
    let (fm_name, fm_desc) = parse_skill_frontmatter(&text);
    if fm_name.trim().is_empty() {
        anyhow::bail!("SKILL.md for '{}' is missing a non-empty 'name'", p.slug);
    }
    if fm_desc.trim().is_empty() {
        anyhow::bail!(
            "SKILL.md for '{}' is missing a non-empty 'description'",
            p.slug
        );
    }
    if fm_name.trim() != p.slug {
        eprintln!(
            "warning: skill name '{}' does not match directory '{}' (installing as '{}')",
            fm_name.trim(),
            p.slug,
            p.slug
        );
    }
    let dest = skills_dir().join(&p.slug);
    if p.root == dest || dest.starts_with(&p.root) || p.root.starts_with(&dest) {
        anyhow::bail!("cannot install skill '{}' from itself", p.slug);
    }
    let origin = SkillOrigin {
        version: 1,
        registry: p.registry,
        slug: p.slug.clone(),
        owner: p.owner,
        installed_version: p.version.clone(),
        installed_at: crate::ops::now_secs(),
        source_url: p.source_url.clone(),
    };
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    if let Err(e) = copy_dir_all(&p.root, &dest).and_then(|()| {
        std::fs::write(
            dest.join(ORIGIN_FILE),
            serde_json::to_string_pretty(&origin)?,
        )?;
        Ok(())
    }) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }
    if p.unverified
        && let Some(src) = p.warn_source.as_deref()
    {
        eprintln!(
            "warning: unverified install from {} (no index hash; use an index name for verified installs)",
            crate::fetch::redact(src)
        );
    }
    Ok(crate::ops::Report {
        name: p.slug,
        version: p.version,
        path: dest,
    })
}

/// `clawhub:<owner/slug>` (or bare slug): detail → ZIP download → per-file
/// verify when the versions endpoint answers (else the honest unverified
/// path), mirroring the ops ClawHub arm's unpack/verify pipeline.
async fn stage_clawhub(client: &reqwest::Client, body: &str) -> anyhow::Result<PendingSkill> {
    let slug = body.trim();
    if slug.is_empty() {
        anyhow::bail!("clawhub skill slug is empty (want clawhub:<owner/slug>)");
    }
    let crate::sources::StagedClawHub {
        root,
        detail,
        verified,
        _staging,
    } = crate::sources::stage_clawhub_bundle(client, slug).await?;
    let version = if detail.version.trim().is_empty() {
        "0.0.0".to_string()
    } else {
        detail.version.clone()
    };
    crate::ops::validate_install_key(&detail.slug)?;
    let source_url = crate::sources::clawhub_canonical_url(&detail.owner, &detail.slug);
    Ok(PendingSkill {
        root,
        slug: detail.slug.clone(),
        version,
        unverified: !verified,
        warn_source: if verified {
            None
        } else {
            Some(source_url.clone())
        },
        registry: "clawhub".to_string(),
        owner: detail.owner.clone(),
        source_url,
        _staging: Some(_staging),
    })
}

/// Split `github:<owner/repo[/path]>` (path parts must be plain names —
/// no `.`/`..`/empties/backslashes, so the join stays under the clone).
fn parse_github_spec(body: &str) -> anyhow::Result<(String, String, Option<String>)> {
    let parts: Vec<&str> = body.trim().split('/').collect();
    if parts.len() < 2
        || parts
            .iter()
            .any(|p| p.trim().is_empty() || *p == "." || *p == ".." || p.contains('\\'))
    {
        anyhow::bail!("bad github skill spec: {body:?} (want github:<owner/repo[/path]>)");
    }
    let owner = parts[0].trim().to_string();
    let repo = parts[1].trim().to_string();
    let subpath = if parts.len() > 2 {
        Some(parts[2..].join("/"))
    } else {
        None
    };
    Ok((owner, repo, subpath))
}

/// `github:<owner/repo[/path]>`: shallow-clone via the shared git helper,
/// then take the repo root (or the subpath) as the bundle. No hash pin
/// exists, so this is always the honest unverified path.
async fn stage_github(body: &str) -> anyhow::Result<PendingSkill> {
    let (owner, repo, subpath) = parse_github_spec(body)?;
    let url = format!("https://github.com/{owner}/{repo}.git");
    let (_staging, clone_dir, _sha) = crate::ops::clone_git_repo(&url, None)?;
    let root = match &subpath {
        Some(s) => clone_dir.join(s),
        None => clone_dir.clone(),
    };
    if !root.join("SKILL.md").is_file() {
        match &subpath {
            Some(s) => {
                anyhow::bail!("github:{owner}/{repo}#{s} has no SKILL.md (not a skill bundle)")
            }
            None => anyhow::bail!(
                "github:{owner}/{repo} has no SKILL.md at its root (pass github:<owner/repo/path> to a skill bundle)"
            ),
        }
    }
    let slug = match &subpath {
        Some(s) => s.rsplit('/').next().unwrap_or(&repo).to_string(),
        None => repo.clone(),
    };
    crate::ops::validate_install_key(&slug)?;
    let source_url = match &subpath {
        Some(s) => format!("https://github.com/{owner}/{repo}#{s}"),
        None => format!("https://github.com/{owner}/{repo}"),
    };
    Ok(PendingSkill {
        root,
        slug,
        version: "0.0.0".to_string(),
        unverified: true,
        warn_source: Some(url),
        registry: "github".to_string(),
        owner,
        source_url,
        _staging: Some(_staging),
    })
}

/// Derive the install slug from a `url:` SKILL.md URL: `…/<slug>/SKILL.md`
/// → `<slug>`; `…/<name>.md` → `<name>`; anything else → the last segment.
fn slug_from_url(url: &str) -> anyhow::Result<String> {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = after_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Drop the authority (first segment) when a scheme is present, so a
    // bare host (`https://h/`) yields no slug instead of the hostname.
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let segs: &[&str] = if url.contains("://") && !segs.is_empty() {
        &segs[1..]
    } else {
        &segs[..]
    };
    let last = segs.last().copied().unwrap_or("");
    let slug = if last.eq_ignore_ascii_case("SKILL.md") {
        segs.get(segs.len().saturating_sub(2))
            .copied()
            .unwrap_or("")
    } else if let Some(stem) = last.strip_suffix(".md") {
        stem
    } else {
        last
    };
    crate::ops::validate_install_key(slug).map_err(|_| {
        anyhow::anyhow!(
            "cannot derive a skill name from URL: {}",
            crate::fetch::redact(url)
        )
    })?;
    Ok(slug.to_string())
}

/// `url:<https SKILL.md>`: download the file (https-only, 64 MiB cap via
/// the shared downloader) and install it as `<slug>/SKILL.md`. Always
/// unverified (no hash pin).
async fn stage_url(client: &reqwest::Client, body: &str) -> anyhow::Result<PendingSkill> {
    let url = body.trim();
    if url.is_empty() {
        anyhow::bail!("url skill spec is empty (want url:<https SKILL.md>)");
    }
    let slug = slug_from_url(url)?;
    let archive = crate::fetch::download(client, url, None).await?;
    let text = std::fs::read_to_string(&archive).map_err(|_| {
        anyhow::anyhow!(
            "url {} did not return SKILL.md text",
            crate::fetch::redact(url)
        )
    })?;
    let _ = std::fs::remove_file(&archive);
    let tmp_root = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_root)?;
    let staging = tempfile::tempdir_in(&tmp_root)?;
    std::fs::write(staging.path().join("SKILL.md"), text)?;
    Ok(PendingSkill {
        root: staging.path().to_path_buf(),
        slug,
        version: "0.0.0".to_string(),
        unverified: true,
        warn_source: Some(url.to_string()),
        registry: "url".to_string(),
        owner: String::new(),
        source_url: url.to_string(),
        _staging: Some(staging),
    })
}

/// Local path: a bundle dir (slug = dir name) or a SKILL.md file (bundle =
/// parent, slug = parent name). The user's own disk needs no verification.
fn stage_local(path_str: &str) -> anyhow::Result<PendingSkill> {
    let p = Path::new(path_str.trim());
    if path_str.trim().is_empty() {
        anyhow::bail!("skill path is empty");
    }
    let dir_name = |d: &Path| {
        d.file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    };
    let (root, slug) = if p.is_dir() {
        let slug = dir_name(p)
            .ok_or_else(|| anyhow::anyhow!("cannot derive a skill name from path: {path_str:?}"))?;
        (p.to_path_buf(), slug)
    } else if p.is_file() {
        if p.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
            anyhow::bail!(
                "skill file {path_str:?} is not SKILL.md (want a skill bundle dir or its SKILL.md)"
            );
        }
        let parent = p.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let slug = dir_name(&parent)
            .ok_or_else(|| anyhow::anyhow!("cannot derive a skill name from path: {path_str:?}"))?;
        (parent, slug)
    } else {
        anyhow::bail!("skill path does not exist: {path_str:?}");
    };
    crate::ops::validate_install_key(&slug)
        .map_err(|_| anyhow::anyhow!("cannot derive a safe skill name from path: {path_str:?}"))?;
    Ok(PendingSkill {
        root,
        slug,
        version: "0.0.0".to_string(),
        unverified: false,
        warn_source: None,
        registry: "local".to_string(),
        owner: String::new(),
        source_url: path_str.trim().to_string(),
        _staging: None,
    })
}

/// Minimal frontmatter parse (no new deps): `(name, description)` from the
/// `---` block, `""` when absent/unclosed. Mirrors the gray loader's
/// single-line `key: value` + quote-strip rules.
fn parse_skill_frontmatter(text: &str) -> (String, String) {
    let mut name = String::new();
    let mut desc = String::new();
    let trimmed = text.trim_start();
    let Some(after) = trimmed.strip_prefix("---") else {
        return (name, desc);
    };
    let after = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    let mut closed = false;
    let mut fm_lines = Vec::new();
    for line in after.lines() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(line);
    }
    if !closed {
        return (String::new(), String::new());
    }
    for line in fm_lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let mut val = val.trim().to_string();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = val[1..val.len() - 1].to_string();
        }
        match key.trim() {
            "name" if name.is_empty() => name = val,
            "description" if desc.is_empty() => desc = val,
            _ => {}
        }
    }
    (name, desc)
}

/// Recursive bundle copy (symlinks followed, like `cp -r`).
fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            // `target` is `dst` + one file name: confined by construction
            // (archive unpackers already rejected `..`/absolute entries).
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Read an origin sidecar; corrupt/missing → `None` (hand-placed skills
/// simply have no origin — list stays total).
fn read_origin(path: &Path) -> Option<SkillOrigin> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn list() -> anyhow::Result<Vec<InstalledSkill>> {
    list_inner().map_err(|e| {
        crate::errors::record("skills", "list", format!("{e:#}"));
        e
    })
}

/// Scan `<agent_dir>/skills/` in the existing discovery layout: each direct
/// child dir holding `SKILL.md` is one installed skill (dot-dirs skipped,
/// mirroring the loader).
fn list_inner() -> anyhow::Result<Vec<InstalledSkill>> {
    let rd = match std::fs::read_dir(skills_dir()) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() || !path.join("SKILL.md").is_file() {
            continue;
        }
        let origin = read_origin(&path.join(ORIGIN_FILE));
        out.push(InstalledSkill {
            version: origin
                .as_ref()
                .map(|o| o.installed_version.clone())
                .unwrap_or_default(),
            source: origin
                .as_ref()
                .map(|o| o.registry.clone())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "local".to_string()),
            origin,
            name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn remove(name: &str) -> anyhow::Result<()> {
    remove_inner(name).map_err(|e| {
        crate::errors::record("skills", name, format!("{e:#}"));
        e
    })
}

/// Delete `<agent_dir>/skills/<name>`; miss message matches `ops::remove`.
fn remove_inner(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("not installed: {name}");
    }
    let dir = skills_dir().join(name);
    if !dir.is_dir() {
        anyhow::bail!("not installed: {name}");
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]
    use super::*;

    const FIXTURE_SKILL: &str =
        "---\nname: demo-skill\ndescription: demo skill for tests\n---\nBody\n";

    /// Point `GRAY_HOME` at a fresh tempdir. Must run under `ENV_GUARD`.
    fn use_skills_env() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        home
    }

    /// Fixture bundle: `<root>/demo-skill/{SKILL.md, references/notes.md}`.
    fn fixture_bundle() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("demo-skill");
        std::fs::create_dir_all(bundle.join("references")).unwrap();
        std::fs::write(bundle.join("SKILL.md"), FIXTURE_SKILL).unwrap();
        std::fs::write(bundle.join("references/notes.md"), "# notes\n").unwrap();
        (root, bundle)
    }

    #[test]
    fn spec_identity_routes_each_scheme() {
        assert_eq!(
            spec_identity("clawhub:owner/slug"),
            ("clawhub".to_string(), "clawhub:owner/slug".to_string())
        );
        assert_eq!(
            spec_identity("claude:grep-skills"),
            ("claude".to_string(), "claude:grep-skills".to_string())
        );
        assert_eq!(
            spec_identity("claude:grep-skills@dogfood-fix"),
            (
                "claude".to_string(),
                "claude:grep-skills@dogfood-fix".to_string()
            )
        );
        assert_eq!(
            spec_identity("github:owner/repo"),
            ("github".to_string(), "github:owner/repo".to_string())
        );
        assert_eq!(
            spec_identity("url:https://h/x-skill/SKILL.md"),
            (
                "url".to_string(),
                "url:https://h/x-skill/SKILL.md".to_string()
            )
        );
        assert_eq!(
            spec_identity("/tmp/demo-skill"),
            ("local".to_string(), "/tmp/demo-skill".to_string())
        );
    }

    #[test]
    fn frontmatter_parses_name_and_description() {
        assert_eq!(
            parse_skill_frontmatter(FIXTURE_SKILL),
            ("demo-skill".to_string(), "demo skill for tests".to_string())
        );
        assert_eq!(
            parse_skill_frontmatter("---\nname: 'q'\ndescription: \"d\"\n---\n"),
            ("q".to_string(), "d".to_string())
        );
        // Absent/unclosed frontmatter degrades to empty (callers refuse).
        assert_eq!(
            parse_skill_frontmatter("no frontmatter\n"),
            (String::new(), String::new())
        );
        assert_eq!(
            parse_skill_frontmatter("---\nname: x\n"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn github_spec_parses_and_rejects() {
        assert_eq!(
            parse_github_spec("owner/repo").unwrap(),
            ("owner".to_string(), "repo".to_string(), None)
        );
        assert_eq!(
            parse_github_spec("owner/repo/path/to/skill").unwrap(),
            (
                "owner".to_string(),
                "repo".to_string(),
                Some("path/to/skill".to_string())
            )
        );
        for bad in ["", "justname", "a/../b", "a/./b", "a//b", "a/b\\c"] {
            assert!(parse_github_spec(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn url_slugs_derive_from_skill_path() {
        assert_eq!(
            slug_from_url("https://h/skills/my-skill/SKILL.md").unwrap(),
            "my-skill"
        );
        assert_eq!(
            slug_from_url("http://127.0.0.1:9/x-skill/SKILL.md").unwrap(),
            "x-skill"
        );
        assert_eq!(slug_from_url("https://h/x/foo.md?tok=1").unwrap(), "foo");
        assert!(slug_from_url("https://h/").is_err());
    }

    #[tokio::test]
    async fn install_local_copies_bundle_and_writes_origin() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let (_root, bundle) = fixture_bundle();
        let _home = use_skills_env();

        let report = install(bundle.to_str().unwrap()).await.unwrap();
        assert_eq!(report.name, "demo-skill");
        assert_eq!(report.version, "0.0.0");
        let dest = skills_dir().join("demo-skill");
        assert_eq!(report.path, dest);
        assert_eq!(
            std::fs::read(dest.join("SKILL.md")).unwrap(),
            FIXTURE_SKILL.as_bytes()
        );
        assert_eq!(
            std::fs::read(dest.join("references/notes.md")).unwrap(),
            b"# notes\n"
        );
        let origin: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dest.join(ORIGIN_FILE)).unwrap())
                .unwrap();
        assert_eq!(origin["version"], 1);
        assert_eq!(origin["registry"], "local");
        assert_eq!(origin["slug"], "demo-skill");
        assert_eq!(origin["installed_version"], "0.0.0");
        assert!(!origin["installed_at"].as_str().unwrap_or("").is_empty());
        assert_eq!(origin["source_url"], bundle.to_str().unwrap());
    }

    #[tokio::test]
    async fn install_from_skill_md_file_uses_parent_bundle() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let (_root, bundle) = fixture_bundle();
        let _home = use_skills_env();

        let skill_md = bundle.join("SKILL.md");
        let report = install(skill_md.to_str().unwrap()).await.unwrap();
        assert_eq!(report.name, "demo-skill");
        assert!(report.path.join("references/notes.md").is_file());
    }

    #[tokio::test]
    async fn list_round_trips_install() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let (_root, bundle) = fixture_bundle();
        let _home = use_skills_env();
        assert!(list().unwrap().is_empty());

        install(bundle.to_str().unwrap()).await.unwrap();
        let skills = list().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "demo-skill");
        assert_eq!(skills[0].version, "0.0.0");
        assert_eq!(skills[0].source, "local");
        let origin = skills[0].origin.as_ref().expect("origin");
        assert_eq!(origin.slug, "demo-skill");
        assert_eq!(origin.installed_version, "0.0.0");
    }

    #[tokio::test]
    async fn remove_deletes_dir_and_miss_matches_ops_voice() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let (_root, bundle) = fixture_bundle();
        let _home = use_skills_env();

        install(bundle.to_str().unwrap()).await.unwrap();
        remove("demo-skill").unwrap();
        assert!(list().unwrap().is_empty());
        assert!(!skills_dir().join("demo-skill").exists());
        for bad in ["demo-skill", "../evil", "a/b", ""] {
            let err = remove(bad).unwrap_err();
            assert_eq!(err.to_string(), format!("not installed: {bad}"));
        }
    }

    #[tokio::test]
    async fn install_refuses_bundle_without_skill_md() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("empty-skill");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("notes.txt"), "no skill here\n").unwrap();
        let _home = use_skills_env();

        let err = install(bundle.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no SKILL.md"), "clear reason: {err}");
        assert!(!skills_dir().join("empty-skill").exists());
        assert!(list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn install_refuses_missing_description() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("nodesc");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("SKILL.md"), "---\nname: nodesc\n---\nBody\n").unwrap();
        let _home = use_skills_env();

        let err = install(bundle.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("description"),
            "names the missing field: {err}"
        );
        assert!(!skills_dir().join("nodesc").exists());
    }

    #[tokio::test]
    async fn install_refuses_missing_name() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("noname");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join("SKILL.md"),
            "---\ndescription: has desc\n---\nBody\n",
        )
        .unwrap();
        let _home = use_skills_env();

        let err = install(bundle.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("'name'"), "names the missing field: {err}");
        assert!(!skills_dir().join("noname").exists());
    }

    #[tokio::test]
    async fn install_warns_not_refuses_on_name_dir_mismatch() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("dir-name");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join("SKILL.md"),
            "---\nname: other-name\ndescription: d\n---\nBody\n",
        )
        .unwrap();
        let _home = use_skills_env();

        // pi leniency: installs under the dir slug, warning on stderr.
        let report = install(bundle.to_str().unwrap()).await.unwrap();
        assert_eq!(report.name, "dir-name");
        assert!(report.path.join("SKILL.md").is_file());
    }

    #[tokio::test]
    async fn install_url_downloads_skill_md_over_loopback() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        use axum::{Router, routing::get};
        let router = Router::new().route("/x-skill/SKILL.md", get(|| async { FIXTURE_SKILL }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let _home = use_skills_env();

        // Fixture frontmatter names `demo-skill` while the URL slug is
        // `x-skill`: mismatch warns, install lands on the URL slug.
        let report = install(&format!("url:http://127.0.0.1:{port}/x-skill/SKILL.md"))
            .await
            .unwrap();
        assert_eq!(report.name, "x-skill");
        assert_eq!(
            std::fs::read(report.path.join("SKILL.md")).unwrap(),
            FIXTURE_SKILL.as_bytes()
        );
        let skills = list().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source, "url");
    }
}
