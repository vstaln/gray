//! ClawHub + Claude marketplace sources (search + install adapters).
//!
//! Live shapes verified 2026-09-06 (read-only):
//! - ClawHub `GET /api/v1/search?q=&limit=` → `{results:[{slug,
//!   displayName,summary,version,ownerHandle,official,...}]}` (bare slugs
//!   409 on detail/download: pass `ownerHandle` when known).
//! - ClawHub detail `GET /api/v1/skills/{slug}[?ownerHandle=]` → extended
//!   shape (not the doc's minimal sketch: `{skill,latestVersion,owner,
//!   moderation}`); per-file hashes live at
//!   `GET /api/v1/skills/{slug}/versions/{version}` (`files[].sha256`).
//! - ClawHub download `GET /api/v1/download?slug=&version=[&ownerHandle=]`
//!   streams a ZIP (`SKILL.md` at root); GitHub-backed skills answer the
//!   same route with a JSON handoff (`sourceRef: "public-github"` +
//!   `archiveUrl`) instead of bytes.
//! - ClawHub trust `POST /api/v1/skills/-/security-verdicts`
//!   (`{items:[{slug,ownerHandle?,version}]}`); 429s carry `Retry-After`.
//! - Claude `marketplace.json` source kinds: relative-path string,
//!   `github`, `url`, `git-subdir`, `npm`, `archive`, `command` (skip).
//! - pi.dev: SSR HTML only (no listing `/api/*` — `/api/packages` 501s,
//!   `preview-media` is per-package media; no embedded JSON), so the npm
//!   `/-/v1/search` proxy stays the listing layer and npm stays the
//!   artifact resolver. No HTML scraping (fragile, forbidden).
//!
//! No auth anywhere here (public read endpoints only). No new runtime
//! dependencies: reqwest/serde_json/git-CLI/std-fs only.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Source identity + reachability
// ---------------------------------------------------------------------------

/// Search/install source. Labels are display-time copy (match
/// [`crate::ops::SearchSource`] labels exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    GrayIndex,
    PiGallery,
    ClawHub,
    ClaudeRepo,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::GrayIndex => "Gray Index",
            Source::PiGallery => "Pi Gallery (preview)",
            Source::ClawHub => "ClawHub",
            Source::ClaudeRepo => "Claude",
        }
    }
}

/// Lightweight reachability per source: one short-timeout request each,
/// never an error (any failure is `false`, never a hard fail).
pub async fn status(source: Source) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match source {
        Source::GrayIndex => {
            let url = crate::index::index_url();
            client
                .head(&url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        }
        Source::PiGallery => {
            let url = crate::ops::npm_registry_base();
            client
                .get(&url)
                .send()
                .await
                .map(|r| r.status().is_success() || r.status().is_redirection())
                .unwrap_or(false)
        }
        Source::ClawHub => {
            let url = format!("{}/search", clawhub_base().trim_end_matches('/'));
            client
                .get(&url)
                .query(&[("q", "ping"), ("limit", "1")])
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        }
        Source::ClaudeRepo => {
            let markets = claude_marketplaces();
            let Some(first) = markets.first() else {
                return false;
            };
            if let Some(dir) = local_marketplace_dir(first) {
                return dir.join(".claude-plugin/marketplace.json").is_file();
            }
            // `ls-remote`-style would shell to git; a HEAD on the repo page
            // answers the same question in one short request.
            let url = match marketplace_repo_parts(first) {
                Ok((o, r)) => format!("https://github.com/{o}/{r}"),
                Err(_) => return false,
            };
            client
                .head(&url)
                .send()
                .await
                .map(|r| r.status().is_success() || r.status().is_redirection())
                .unwrap_or(false)
        }
    }
}

// ---------------------------------------------------------------------------
// ClawHub adapter
// ---------------------------------------------------------------------------

/// Default ClawHub API base (overridden by [`CLAWHUB_BASE_ENV`).
pub const DEFAULT_CLAWHUB_BASE: &str = "https://clawhub.ai/api/v1";
/// Env var overriding the ClawHub API base (tests point at loopback).
pub const CLAWHUB_BASE_ENV: &str = "GRAY_CLAWHUB_BASE";

pub fn clawhub_base() -> String {
    std::env::var(CLAWHUB_BASE_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLAWHUB_BASE.to_string())
}

/// One search hit: `name` is `owner/slug` when the owner is known (that is
/// the installable `clawhub:` spec), else the bare slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClawHubEntry {
    pub name: String,
    pub slug: String,
    pub owner: String,
    pub display_name: String,
    pub summary: String,
    pub version: String,
    pub official: bool,
    pub scan: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawHubResult {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    owner_handle: Option<String>,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    trust: serde_json::Value,
    /// Older/alternate shapes nest the same fields one level down.
    #[serde(default)]
    tags: serde_json::Value,
}

/// Parse a `GET /search` body (`{results:[...]}`). Tolerant: missing
/// version/official/trust degrade to empty/false, nameless rows drop.
pub fn parse_clawhub_search(raw: &serde_json::Value) -> Vec<ClawHubEntry> {
    let results = raw
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for r in &results {
        let r: ClawHubResult = serde_json::from_value(r.clone()).unwrap_or_default();
        let slug = r.slug.trim().to_string();
        if slug.is_empty() {
            continue;
        }
        let owner = r.owner_handle.as_deref().unwrap_or("").trim().to_string();
        let name = if owner.is_empty() || slug.contains('/') {
            slug.clone()
        } else {
            format!("{owner}/{slug}")
        };
        let version = r
            .version
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                r.tags
                    .get("latest")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let scan = r
            .trust
            .get("clawHubVerdict")
            .or_else(|| r.trust.get("verdict"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty() && *v != "null")
            .unwrap_or("")
            .to_string();
        out.push(ClawHubEntry {
            name,
            slug,
            owner,
            display_name: r.display_name,
            summary: r.summary,
            version,
            official: r.official,
            scan,
        });
    }
    out
}

/// Display trust: `official|community` plus ` + scan:<status>` when known.
pub fn clawhub_trust(official: bool, scan: &str) -> String {
    let base = if official { "official" } else { "community" };
    if scan.trim().is_empty() {
        base.to_string()
    } else {
        format!("{base} + scan:{}", scan.trim())
    }
}

/// Split `owner/slug` (install reference form) from a bare slug.
pub fn split_clawhub_slug(slug: &str) -> (Option<String>, String) {
    match slug.trim().split_once('/') {
        Some((o, s)) if !o.trim().is_empty() && !s.trim().is_empty() => {
            (Some(o.trim().to_string()), s.trim().to_string())
        }
        _ => (None, slug.trim().to_string()),
    }
}

/// GET with one 429 → honor `Retry-After` (capped) → retry once.
/// Shared by search, detail, versions, and the artifact download.
async fn clawhub_get(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, &str)],
    timeout_secs: u64,
) -> anyhow::Result<reqwest::Response> {
    crate::fetch::check_url(url)?;
    log::debug!("clawhub GET {}", crate::fetch::redact(url));
    let send = || {
        client
            .get(url)
            .query(query)
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .send()
    };
    let resp = send().await?;
    if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok(resp);
    }
    let wait = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(2)
        .min(30);
    log::debug!("clawhub 429, retrying after {wait}s");
    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    Ok(send().await?)
}

/// Search ClawHub (`limit=20`). Any failure is `Err` for the caller to
/// downgrade to the advisory path.
pub async fn clawhub_search(
    client: &reqwest::Client,
    query: &str,
) -> anyhow::Result<Vec<ClawHubEntry>> {
    let url = format!("{}/search", clawhub_base().trim_end_matches('/'));
    let resp = clawhub_get(client, &url, &[("q", query), ("limit", "20")], 10).await?;
    let body: serde_json::Value = resp.error_for_status()?.json().await?;
    let mut entries = parse_clawhub_search(&body);
    entries.truncate(20);
    // Exact-version verdicts (one batch POST, best-effort) are fresher
    // than the search-payload scan; the payload stays the fallback.
    let keys: Vec<(String, String, String)> = entries
        .iter()
        .map(|e| (e.slug.clone(), e.owner.clone(), e.version.clone()))
        .collect();
    let verdicts = clawhub_verdicts_batch(client, &keys).await;
    for e in &mut entries {
        if let Some(scan) = verdicts.get(&(e.slug.clone(), e.owner.clone(), e.version.clone())) {
            e.scan = scan.clone();
        }
    }
    Ok(entries)
}

/// Resolved ClawHub skill: pinned version plus the versions-endpoint file
/// list (empty when the endpoint won't answer — install then proceeds on
/// the unverified path).
#[derive(Debug, Clone, Default)]
pub struct ClawHubDetail {
    pub slug: String,
    pub owner: String,
    pub display_name: String,
    pub summary: String,
    pub version: String,
    pub files: Vec<ClawHubFile>,
}

/// One versions-endpoint file row (`path` + hex `sha256`).
#[derive(Debug, Clone, Default)]
pub struct ClawHubFile {
    pub path: String,
    pub sha256: String,
}

/// Detail + version files for `slug` (`owner/slug` or bare; bare 409s when
/// ambiguous — the caller surfaces "qualify with owner/").
pub async fn clawhub_detail(client: &reqwest::Client, slug: &str) -> anyhow::Result<ClawHubDetail> {
    let (owner_opt, bare) = split_clawhub_slug(slug);
    if bare.is_empty() {
        anyhow::bail!("clawhub slug is empty");
    }
    if bare.contains('/') {
        anyhow::bail!("bad clawhub slug: {slug:?}");
    }
    let base = clawhub_base();
    let detail_url = format!("{}/skills/{bare}", base.trim_end_matches('/'));
    let mut q: Vec<(&str, &str)> = Vec::new();
    let owner_owned;
    if let Some(o) = owner_opt.as_deref() {
        owner_owned = o.to_string();
        q.push(("ownerHandle", &owner_owned));
    }
    let resp = clawhub_get(client, &detail_url, &q, 10).await?;
    if resp.status() == reqwest::StatusCode::CONFLICT {
        anyhow::bail!("clawhub slug {bare:?} is ambiguous (qualify as owner/slug)");
    }
    let body: serde_json::Value = resp.error_for_status()?.json().await?;
    let skill = body.get("skill").unwrap_or(&body);
    let str_at = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let version = body
        .get("latestVersion")
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            skill
                .get("tags")
                .and_then(|t| t.get("latest"))
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let owner = owner_opt.clone().unwrap_or_else(|| {
        body.get("owner")
            .and_then(|o| o.get("handle"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    // Version files (install-time hashes); soft-fail to empty — the
    // install arm treats "no file list" as unverified, not fatal.
    // Trust display is served by the verdicts batch at search time, not
    // by this endpoint's security snapshot.
    let mut files = Vec::new();
    if !version.is_empty() {
        let ver_url = format!(
            "{}/skills/{bare}/versions/{version}",
            base.trim_end_matches('/')
        );
        if let Ok(resp) = clawhub_get(client, &ver_url, &q, 10).await
            && let Ok(vbody) = resp.error_for_status()
        {
            match vbody.json::<serde_json::Value>().await {
                Ok(v) => {
                    if let Some(arr) = v
                        .get("version")
                        .and_then(|x| x.get("files"))
                        .or_else(|| v.get("files"))
                        .and_then(|x| x.as_array())
                    {
                        for f in arr {
                            let path = f
                                .get("path")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let sha256 = f
                                .get("sha256")
                                .or_else(|| f.get("sha256hash"))
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !path.is_empty() && !sha256.is_empty() {
                                files.push(ClawHubFile { path, sha256 });
                            }
                        }
                    }
                }
                Err(e) => log::debug!("clawhub versions parse failed: {e:#}"),
            }
        }
    }
    Ok(ClawHubDetail {
        slug: bare,
        owner,
        display_name: str_at(skill, "displayName"),
        summary: str_at(skill, "summary"),
        version,
        files,
    })
}

/// Canonical listing URL for lock `source` (what users can inspect).
pub fn clawhub_canonical_url(owner: &str, slug: &str) -> String {
    if owner.trim().is_empty() {
        format!("clawhub:{slug}")
    } else {
        format!("https://clawhub.ai/{owner}/skills/{slug}")
    }
}

/// Lock/dir key input for a ClawHub skill: `owner/slug` when the owner is
/// known (the shared sanitizer turns it into `owner-slug`, matching the
/// owner-qualified search display name), else the bare slug.
pub fn clawhub_key_input(owner: &str, slug: &str) -> String {
    if owner.trim().is_empty() {
        slug.to_string()
    } else {
        format!("{}/{slug}", owner.trim())
    }
}

/// Download the skill ZIP to `$GRAY_HOME/plugins/tmp/` (handles the
/// GitHub-handoff JSON by following `archiveUrl`). Returns the kept path.
/// Goes through [`clawhub_get`] (429 + `Retry-After` honored); the bytes
/// must be sniffed for the handoff JSON before choosing the pipeline, so
/// only the handoff branch delegates to [`crate::fetch::download`] while
/// the ZIP branch keeps the same 64 MiB cap and tmp handling.
pub async fn clawhub_download_bundle(
    client: &reqwest::Client,
    detail: &ClawHubDetail,
) -> anyhow::Result<PathBuf> {
    let base = clawhub_base();
    let url = format!("{}/download", base.trim_end_matches('/'));
    let mut q: Vec<(&str, &str)> = vec![("slug", &detail.slug)];
    if !detail.version.is_empty() {
        q.push(("version", &detail.version));
    }
    let owner_owned = detail.owner.clone();
    if !owner_owned.trim().is_empty() {
        q.push(("ownerHandle", &owner_owned));
    }
    log::debug!("clawhub downloading {}", detail.slug);
    let resp = clawhub_get(client, &url, &q, 30)
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    if bytes.len() > crate::fetch::MAX_BYTES as usize {
        anyhow::bail!("plugin archive exceeds 64 MiB cap");
    }
    // GitHub-backed handoff: small JSON with `archiveUrl`, not a ZIP.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && v.get("sourceRef").and_then(|s| s.as_str()) == Some("public-github")
    {
        let archive = v
            .get("archiveUrl")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("clawhub handoff has no archiveUrl"))?;
        return crate::fetch::download(client, archive, None).await;
    }
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        anyhow::bail!("clawhub download for {} is not a ZIP archive", detail.slug);
    }
    let tmp_dir = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let tmp = tempfile::NamedTempFile::new_in(&tmp_dir)?;
    let temppath = tmp.into_temp_path();
    let path: PathBuf = temppath.to_path_buf();
    tokio::fs::write(&path, &bytes).await?;
    temppath
        .keep()
        .map_err(|e| anyhow::anyhow!("keeping download: {e}"))?;
    Ok(path)
}

/// Verify unpacked `root` against the versions-endpoint file list
/// (hex sha256 per file). Empty list → `Ok(false)` (nothing to check —
/// caller takes the unverified path); mismatch → `Err`.
pub fn verify_clawhub_files(root: &Path, files: &[ClawHubFile]) -> anyhow::Result<bool> {
    if files.is_empty() {
        return Ok(false);
    }
    use sha2::Digest;
    for f in files {
        // Confine to the staging root (no absolute/`..` entries).
        let rel = Path::new(&f.path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            anyhow::bail!("refusing unsafe clawhub file path: {}", f.path);
        }
        let bytes = std::fs::read(root.join(rel))?;
        let got = format!("{:x}", sha2::Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(&f.sha256) {
            anyhow::bail!("hash mismatch for {}", f.path);
        }
    }
    Ok(true)
}

/// Batch trust verdicts (`POST /skills/-/security-verdicts`, up to 100
/// `(slug, owner, version)` items in one call). Returns the scan status
/// for the items the endpoint answers `ok` on, keyed by the request
/// triple. Best-effort: any failure is an empty map and callers keep the
/// search-payload scan. Versionless entries are never queried.
pub async fn clawhub_verdicts_batch(
    client: &reqwest::Client,
    items: &[(String, String, String)],
) -> std::collections::BTreeMap<(String, String, String), String> {
    let mut out = std::collections::BTreeMap::new();
    let items: Vec<&(String, String, String)> = items
        .iter()
        .filter(|(_, _, v)| !v.trim().is_empty())
        .take(100)
        .collect();
    if items.is_empty() {
        return out;
    }
    let url = format!(
        "{}/skills/-/security-verdicts",
        clawhub_base().trim_end_matches('/')
    );
    if crate::fetch::check_url(&url).is_err() {
        return out;
    }
    let req_items: Vec<serde_json::Value> = items
        .iter()
        .map(|(slug, owner, version)| {
            let mut o = serde_json::json!({"slug": slug, "version": version});
            if !owner.trim().is_empty() {
                o["ownerHandle"] = serde_json::Value::String(owner.clone());
            }
            o
        })
        .collect();
    let resp = match client
        .post(&url)
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({"items": req_items}))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::debug!("clawhub verdicts failed: {e:#}");
            return out;
        }
    };
    // 429 on the write-bucket twin: honor and retry once like reads.
    let resp = match resp.status() {
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(2)
                .min(30);
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            match client
                .post(&url)
                .timeout(std::time::Duration::from_secs(10))
                .json(&serde_json::json!({"items": req_items}))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("clawhub verdicts retry failed: {e:#}");
                    return out;
                }
            }
        }
        _ => resp,
    };
    let body: serde_json::Value = match resp.error_for_status() {
        Ok(r) => match r.json().await {
            Ok(b) => b,
            Err(_) => return out,
        },
        Err(_) => return out,
    };
    let results = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for r in &results {
        if r.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let status = r
            .get("security")
            .and_then(|s| s.get("status"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("");
        if status.is_empty() {
            continue;
        }
        let (Some(rs), Some(rv)) = (
            r.get("requestedSlug").and_then(|v| v.as_str()),
            r.get("requestedVersion").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let ro = r
            .get("requestedOwnerHandle")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Match back to the request triple (owner echoes only when the
        // request qualified it).
        if let Some(key) = items
            .iter()
            .find(|(s, o, v)| s == rs && v == rv && (ro.is_empty() || o == ro))
        {
            out.insert((**key).clone(), status.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Claude marketplace adapter
// ---------------------------------------------------------------------------

/// Env var overriding the marketplace list (comma-separated `owner/repo`;
/// `file://<dir>` or plain local dirs are accepted for fixtures).
pub const CLAUDE_MARKETPLACES_ENV: &str = "GRAY_CLAUDE_MARKETPLACES";
/// Default marketplaces (official + community).
pub const DEFAULT_CLAUDE_MARKETPLACES: &str =
    "anthropics/claude-plugins-official,anthropics/claude-plugins-community";

/// Configured marketplace specs (env override or default).
pub fn claude_marketplaces() -> Vec<String> {
    let from_env = std::env::var(CLAUDE_MARKETPLACES_ENV)
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());
    from_env.unwrap_or_else(|| {
        DEFAULT_CLAUDE_MARKETPLACES
            .split(',')
            .map(str::to_string)
            .collect()
    })
}

/// One plugin entry's fetch location. `command` is OUT (never executed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// Relative path inside the marketplace repo (`./plugins/foo`).
    Path(String),
    Github {
        repo: String,
        ref_: String,
        sha: String,
    },
    Url {
        url: String,
        ref_: String,
        sha: String,
    },
    GitSubdir {
        url: String,
        path: String,
        ref_: String,
        sha: String,
    },
    Npm {
        package: String,
        version: String,
        registry: String,
    },
    Archive {
        url: String,
        sha256: String,
    },
    Command {
        command: String,
    },
}

fn opt_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Parse one plugin `source` value (string path or discriminator object).
/// `None` = missing/unknown shape (caller skips the plugin).
pub fn parse_plugin_source(v: &serde_json::Value) -> Option<PluginSource> {
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        return Some(PluginSource::Path(s.to_string()));
    }
    let obj = v.as_object()?;
    match obj.get("source").and_then(|s| s.as_str())? {
        "github" => Some(PluginSource::Github {
            repo: opt_str(v, "repo"),
            ref_: opt_str(v, "ref"),
            sha: opt_str(v, "sha"),
        }),
        "url" => Some(PluginSource::Url {
            url: opt_str(v, "url"),
            ref_: opt_str(v, "ref"),
            sha: opt_str(v, "sha"),
        }),
        "git-subdir" => Some(PluginSource::GitSubdir {
            url: opt_str(v, "url"),
            path: opt_str(v, "path"),
            ref_: opt_str(v, "ref"),
            sha: opt_str(v, "sha"),
        }),
        "npm" => Some(PluginSource::Npm {
            package: opt_str(v, "package"),
            version: opt_str(v, "version"),
            registry: opt_str(v, "registry"),
        }),
        "archive" => Some(PluginSource::Archive {
            url: opt_str(v, "url"),
            sha256: opt_str(v, "sha256"),
        }),
        "command" => Some(PluginSource::Command {
            command: opt_str(v, "command"),
        }),
        _ => None,
    }
}

/// One marketplace plugin (post-parse; `command` entries kept so the
/// install arm can refuse them with the warning string).
#[derive(Debug, Clone)]
pub struct MarketplacePlugin {
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: PluginSource,
}

/// Parsed marketplace catalog.
#[derive(Debug, Clone)]
pub struct MarketplaceCatalog {
    pub name: String,
    pub plugins: Vec<MarketplacePlugin>,
}

/// Parse `marketplace.json` text. Nameless plugins and unknown source
/// shapes are skipped (search stays total); `command` plugins are kept
/// for the install arm's explicit refusal.
pub fn parse_marketplace_json(raw: &str) -> anyhow::Result<MarketplaceCatalog> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("unknown")
        .to_string();
    let mut plugins = Vec::new();
    if let Some(arr) = v.get("plugins").and_then(|p| p.as_array()) {
        for p in arr {
            let pname = opt_str(p, "name");
            if pname.is_empty() {
                continue;
            }
            let Some(source) = p.get("source").and_then(parse_plugin_source) else {
                continue;
            };
            plugins.push(MarketplacePlugin {
                name: pname,
                description: opt_str(p, "description"),
                version: opt_str(p, "version"),
                source,
            });
        }
    }
    Ok(MarketplaceCatalog { name, plugins })
}

/// Compact source qualifier for `version_detail` (preview+confirm pane).
pub fn claude_qualifier(source: &PluginSource) -> String {
    match source {
        PluginSource::Path(s) => s.clone(),
        PluginSource::Github { repo, ref_, .. } if !ref_.is_empty() => {
            format!("github:{repo}@{ref_}")
        }
        PluginSource::Github { repo, .. } => format!("github:{repo}"),
        PluginSource::Url { url, ref_, .. } if !ref_.is_empty() => format!("{url}@{ref_}"),
        PluginSource::Url { url, .. } => url.clone(),
        PluginSource::GitSubdir { url, path, .. } => format!("{url}#{path}"),
        PluginSource::Npm {
            package, version, ..
        } if !version.is_empty() => {
            format!("npm:{package}@{version}")
        }
        PluginSource::Npm { package, .. } => format!("npm:{package}"),
        PluginSource::Archive { url, .. } => url.clone(),
        PluginSource::Command { .. } => "command source (not installable)".to_string(),
    }
}

/// A Claude search hit (mapped to [`crate::ops::SearchHit`] by the caller).
#[derive(Debug, Clone)]
pub struct ClaudeEntry {
    pub name: String,
    pub description: String,
    pub version: String,
    pub qualifier: String,
    pub marketplace: String,
}

/// Split an `owner/repo` spec (exactly two non-empty parts).
pub fn marketplace_repo_parts(spec: &str) -> anyhow::Result<(String, String)> {
    match spec.trim().split_once('/') {
        Some((o, r))
            if !o.trim().is_empty()
                && !r.trim().is_empty()
                && !r.contains('/')
                && !spec.contains("://") =>
        {
            Ok((o.trim().to_string(), r.trim().to_string()))
        }
        _ => anyhow::bail!("bad marketplace spec: {spec:?} (want owner/repo)"),
    }
}

/// A marketplace spec pointing at a local dir (`file://…` or a plain
/// existing path — fixtures use this; no network).
pub fn local_marketplace_dir(spec: &str) -> Option<PathBuf> {
    let s = spec.trim().strip_prefix("file://").unwrap_or(spec.trim());
    let p = Path::new(s);
    if !s.is_empty() && p.is_dir() {
        Some(p.to_path_buf())
    } else {
        None
    }
}

fn git_output(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| anyhow::anyhow!("git failed to run ({e})"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Shallow-clone `url` into `<tmp>/repo` (`extra` holds `--branch` /
/// `--filter` / `--sparse` flags) and return the keeper tempdir + path.
fn clone_into_tmp(
    url: &str,
    extra: &[&str],
    under_plugins_tmp: bool,
) -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
    let tmp = if under_plugins_tmp {
        let root = crate::plugins_dir().join("tmp");
        std::fs::create_dir_all(&root)?;
        tempfile::tempdir_in(&root)?
    } else {
        tempfile::tempdir()?
    };
    let dst = tmp.path().join("repo");
    let mut cmd = std::process::Command::new("git");
    cmd.args(["clone", "--depth", "1"])
        .args(extra)
        .arg("--")
        .arg(url)
        .arg(&dst)
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("git failed to run ({e})"))?;
    if !out.status.success() {
        anyhow::bail!(
            "cloning {} failed: {}",
            crate::fetch::redact(url),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok((tmp, dst))
}

/// Fetch one marketplace catalog: local dirs read straight off disk;
/// `owner/repo` shallow-clones (sparse+blobless, full-clone fallback) to
/// a tempdir that is dropped after parsing.
fn fetch_catalog(spec: &str) -> anyhow::Result<MarketplaceCatalog> {
    if let Some(dir) = local_marketplace_dir(spec) {
        let raw = std::fs::read_to_string(dir.join(".claude-plugin/marketplace.json"))?;
        return parse_marketplace_json(&raw);
    }
    let (owner, repo) = marketplace_repo_parts(spec)?;
    let url = format!("https://github.com/{owner}/{repo}.git");
    // Sparse blobless shallow clone: KB, not the ~10 MB working tree.
    if let Ok((tmp, dst)) = clone_into_tmp(&url, &["--filter=blob:none", "--sparse"], false)
        && git_output(
            &["sparse-checkout", "set", "--cone", "--", ".claude-plugin"],
            &dst,
        )
        .is_ok()
        && dst.join(".claude-plugin/marketplace.json").is_file()
    {
        let raw = std::fs::read_to_string(dst.join(".claude-plugin/marketplace.json"))?;
        let cat = parse_marketplace_json(&raw)?;
        drop(tmp);
        return Ok(cat);
    }
    // Fallback: full shallow clone (servers without filter support).
    let (tmp, dst) = clone_into_tmp(&url, &[], false)
        .map_err(|e| anyhow::anyhow!("cloning {owner}/{repo} failed: {e:#}"))?;
    let raw = std::fs::read_to_string(dst.join(".claude-plugin/marketplace.json"))?;
    let cat = parse_marketplace_json(&raw)?;
    drop(tmp);
    Ok(cat)
}

/// Search all configured marketplaces (substring over name/description,
/// mirroring the Gray arm). `command` plugins are skipped (not
/// installable — the install arm refuses them with the warning string).
/// Returns `(entries, any_failed)`: any single-marketplace failure sets
/// the flag (partial results stay usable).
pub fn claude_search_entries(query: &str) -> (Vec<ClaudeEntry>, bool) {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut any_failed = false;
    for spec in claude_marketplaces() {
        let cat = match fetch_catalog(&spec) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("claude marketplace {spec} failed: {e:#}");
                any_failed = true;
                continue;
            }
        };
        let mname = if cat.name.trim().is_empty() || cat.name == "unknown" {
            spec.rsplit('/').next().unwrap_or(&spec).to_string()
        } else {
            cat.name.clone()
        };
        for p in &cat.plugins {
            if matches!(p.source, PluginSource::Command { .. }) {
                continue;
            }
            if !p.name.contains(query) && !p.description.contains(query) {
                continue;
            }
            if !seen.insert(p.name.clone()) {
                continue;
            }
            out.push(ClaudeEntry {
                name: p.name.clone(),
                description: p.description.clone(),
                version: p.version.clone(),
                qualifier: claude_qualifier(&p.source),
                marketplace: mname.clone(),
            });
        }
    }
    (out, any_failed)
}

/// Does `plugin`'s marketplace filter match this catalog? The filter is
/// the catalog's `name`, the full `owner/repo` spec, or the repo suffix.
fn marketplace_matches(filter: &str, catalog_name: &str, spec: &str) -> bool {
    let f = filter.trim();
    f == catalog_name || f == spec.trim() || spec.trim().strip_suffix(&format!("/{f}")).is_some()
}

/// A resolved Claude plugin: `_staging` owns the tempdir (deleted on
/// drop), `root` is the plugin dir to extract over.
#[derive(Debug)]
pub struct ClaudeResolved {
    pub _staging: tempfile::TempDir,
    pub root: PathBuf,
    pub version: String,
    pub hash: String,
    pub source_url: String,
    pub unverified: bool,
}

fn git_url_for(source_url: &str) -> String {
    let s = source_url.trim();
    if s.contains("://") || s.starts_with("git@") {
        s.to_string()
    } else if s.contains('/') {
        format!("https://github.com/{s}.git")
    } else {
        s.to_string()
    }
}

/// Resolve `plugin[@marketplace]` to a staged bundle. `command` sources
/// bail with the warning string (never executed). `sha`-pinned git
/// sources verify the cloned HEAD against the pin; unpinned and local
/// sources install on the unverified path.
pub async fn claude_resolve(
    client: &reqwest::Client,
    plugin: &str,
    marketplace_filter: Option<&str>,
) -> anyhow::Result<ClaudeResolved> {
    if plugin.trim().is_empty() {
        anyhow::bail!("claude plugin name is empty");
    }
    let mut found: Option<(String, String, MarketplacePlugin)> = None;
    for spec in claude_marketplaces() {
        let cat = fetch_catalog(&spec)
            .map_err(|e| anyhow::anyhow!("claude marketplace {spec} failed: {e:#}"))?;
        let mname = if cat.name.trim().is_empty() || cat.name == "unknown" {
            spec.rsplit('/').next().unwrap_or(&spec).to_string()
        } else {
            cat.name.clone()
        };
        if let Some(f) = marketplace_filter
            && !marketplace_matches(f, &mname, &spec)
        {
            continue;
        }
        if let Some(p) = cat.plugins.iter().find(|p| p.name == plugin) {
            found = Some((spec, mname, p.clone()));
            break;
        }
    }
    let Some((spec, _mname, p)) = found else {
        let where_ = marketplace_filter.unwrap_or("configured marketplaces");
        anyhow::bail!("not in claude marketplaces: {plugin} (in {where_})");
    };
    match &p.source {
        PluginSource::Command { .. } => {
            eprintln!("warning: skipping command source for {plugin} (not executed)");
            anyhow::bail!("claude plugin {plugin} uses a command source (not installable)");
        }
        PluginSource::Path(rel) => {
            let rel = rel.strip_prefix("./").unwrap_or(rel);
            // Local marketplace: use the dir straight; remote: full
            // shallow clone (installs are rare — latency is fine).
            // `_staging` keeps the clone alive when there is one.
            let (staging, repo_dir) = match local_marketplace_dir(&spec) {
                Some(dir) => (tempfile::tempdir()?, dir),
                None => {
                    let (o, r) = marketplace_repo_parts(&spec)?;
                    let url = format!("https://github.com/{o}/{r}.git");
                    clone_into_tmp(&url, &[], true)?
                }
            };
            let root = repo_dir.join(rel);
            if !root.is_dir() {
                anyhow::bail!("claude plugin {plugin} path {rel:?} missing in marketplace");
            }
            Ok(ClaudeResolved {
                _staging: staging,
                root,
                version: if_version(&p.version),
                hash: String::new(),
                source_url: format!("{}#{rel}", spec.trim()),
                unverified: true,
            })
        }
        PluginSource::Github { repo, ref_, sha }
        | PluginSource::Url {
            url: repo,
            ref_,
            sha,
        } => {
            let url = if matches!(&p.source, PluginSource::Github { .. }) {
                format!(
                    "https://github.com/{}.git",
                    repo.trim().trim_end_matches(".git")
                )
            } else {
                repo.clone()
            };
            install_git_source(&url, ref_, sha, None, &p, plugin).await
        }
        PluginSource::GitSubdir {
            url,
            path,
            ref_,
            sha,
        } => install_git_source(&git_url_for(url), ref_, sha, Some(path), &p, plugin).await,
        PluginSource::Npm {
            package, version, ..
        } => {
            let ver = if version.trim().is_empty() {
                None
            } else {
                Some(version.as_str())
            };
            let staged = crate::ops::stage_npm_package(client, package, ver).await?;
            let root = crate::ops::stage_root(staged.dir.path());
            Ok(ClaudeResolved {
                root,
                version: staged.version,
                hash: staged.integrity,
                source_url: staged.tarball,
                unverified: false,
                _staging: staged.dir,
            })
        }
        PluginSource::Archive { url, sha256 } => {
            let expected = if sha256.trim().is_empty() {
                None
            } else if sha256.contains(':') {
                Some(crate::index::HashSpec::Single(sha256.clone()))
            } else {
                Some(crate::index::HashSpec::Single(format!("sha256:{sha256}")))
            };
            let archive = crate::fetch::download(client, url, expected.as_ref()).await?;
            let tmp_root = crate::plugins_dir().join("tmp");
            std::fs::create_dir_all(&tmp_root)?;
            let staging = tempfile::tempdir_in(&tmp_root)?;
            if let Err(e) = crate::fetch::unpack_tar_gz(&archive, staging.path()) {
                let _ = std::fs::remove_file(&archive);
                return Err(e);
            }
            let _ = std::fs::remove_file(&archive);
            let root = crate::ops::stage_root(staging.path());
            let unverified = sha256.trim().is_empty();
            Ok(ClaudeResolved {
                root,
                version: if_version(&p.version),
                hash: sha256.clone(),
                source_url: url.clone(),
                unverified,
                _staging: staging,
            })
        }
    }
}

fn if_version(v: &str) -> String {
    if v.trim().is_empty() {
        "0.0.0".to_string()
    } else {
        v.to_string()
    }
}

/// Check out the catalog-pinned commit: fresh clones usually sit on it
/// already (fast path, no network); when upstream moved on, fetch just that
/// commit so installs keep working instead of rotting. Fail-closed when the
/// pin doesn't resolve upstream.
fn checkout_pinned_commit(repo_dir: &Path, plugin: &str, want: &str) -> anyhow::Result<()> {
    if git_output(&["rev-parse", "HEAD"], repo_dir)? == want {
        return Ok(());
    }
    git_output(&["fetch", "--depth", "1", "origin", want], repo_dir).map_err(|e| {
        anyhow::anyhow!("claude plugin {plugin} pinned commit {want} unavailable upstream ({e:#})")
    })?;
    git_output(&["checkout", "--quiet", want], repo_dir).map_err(|e| {
        anyhow::anyhow!("claude plugin {plugin} cannot check out pinned commit {want} ({e:#})")
    })?;
    Ok(())
}

/// Clone a git plugin source (optional subdir), checking out the pinned
/// `sha` when present so the installed tree is exactly the catalog commit.
async fn install_git_source(
    url: &str,
    git_ref: &str,
    sha: &str,
    subdir: Option<&str>,
    p: &MarketplacePlugin,
    plugin: &str,
) -> anyhow::Result<ClaudeResolved> {
    if url.trim().is_empty() {
        anyhow::bail!("claude plugin {plugin} has an empty git URL");
    }
    let branch = if git_ref.trim().is_empty() {
        None
    } else {
        Some(git_ref.trim())
    };
    let (staging, repo_dir) = clone_plugin_repo(url, branch)?;
    if !sha.trim().is_empty() {
        checkout_pinned_commit(&repo_dir, plugin, sha.trim())?;
    }
    let root = match subdir {
        Some(s) if !s.trim().is_empty() => repo_dir.join(s.trim()),
        _ => repo_dir.clone(),
    };
    if !root.is_dir() {
        anyhow::bail!("claude plugin {plugin} subdir missing in repo");
    }
    let head = git_output(&["rev-parse", "HEAD"], &repo_dir)?;
    Ok(ClaudeResolved {
        root,
        version: if !p.version.trim().is_empty() {
            p.version.clone()
        } else if let Some(b) = branch {
            b.to_string()
        } else {
            "0.0.0".to_string()
        },
        hash: head,
        source_url: url.to_string(),
        unverified: sha.trim().is_empty(),
        _staging: staging,
    })
}

/// Shallow-clone a plugin repo under `$GRAY_HOME/plugins/tmp/`.
fn clone_plugin_repo(
    url: &str,
    branch: Option<&str>,
) -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
    let mut extra: Vec<&str> = Vec::new();
    let owned;
    if let Some(b) = branch {
        owned = b.to_string();
        extra.push("--branch");
        extra.push(&owned);
    }
    clone_into_tmp(url, &extra, true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]
    use super::*;

    #[test]
    fn source_labels_are_exact() {
        assert_eq!(Source::GrayIndex.label(), "Gray Index");
        assert_eq!(Source::PiGallery.label(), "Pi Gallery (preview)");
        assert_eq!(Source::ClawHub.label(), "ClawHub");
        assert_eq!(Source::ClaudeRepo.label(), "Claude");
    }

    #[test]
    fn clawhub_search_fixture_parses_brief_shape() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[
                {"slug":"gifgrep","displayName":"GifGrep","summary":"grep gifs","version":"1.2.3"},
                {"slug":"bare","displayName":"Bare","summary":"no version"},
                {"slug":"","displayName":"Nameless","summary":"dropped"}
            ]}"#,
        )
        .unwrap();
        let entries = parse_clawhub_search(&v);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].slug, "gifgrep");
        assert_eq!(entries[0].name, "gifgrep");
        assert_eq!(entries[0].version, "1.2.3");
        assert_eq!(entries[0].summary, "grep gifs");
        assert!(!entries[0].official);
        assert_eq!(entries[1].version, "");
    }

    #[test]
    fn clawhub_search_keeps_owner_and_scan() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[
                {"slug":"test","displayName":"Test","summary":"s","version":"0.0.1",
                 "ownerHandle":"arein","official":true,
                 "trust":{"clawHubVerdict":"clean"}}
            ]}"#,
        )
        .unwrap();
        let entries = parse_clawhub_search(&v);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "arein/test");
        assert_eq!(entries[0].owner, "arein");
        assert!(entries[0].official);
        assert_eq!(entries[0].scan, "clean");
        assert_eq!(clawhub_trust(true, "clean"), "official + scan:clean");
        assert_eq!(clawhub_trust(false, ""), "community");
    }

    #[test]
    fn clawhub_key_input_qualifies_owner() {
        assert_eq!(clawhub_key_input("arein", "test"), "arein/test");
        assert_eq!(clawhub_key_input("", "test"), "test");
        // Downstream sanitize turns it into the collision-free lock key.
        assert_eq!(
            crate::ops::sanitize_npm_key(&clawhub_key_input("arein", "test")),
            "arein-test"
        );
    }

    #[test]
    fn clawhub_slug_split_cases() {
        assert_eq!(
            split_clawhub_slug("arein/test"),
            (Some("arein".to_string()), "test".to_string())
        );
        assert_eq!(split_clawhub_slug("test"), (None, "test".to_string()));
        assert_eq!(split_clawhub_slug("  "), (None, String::new()));
    }

    #[test]
    fn marketplace_fixture_covers_all_six_plus_command() {
        let raw = r#"{
            "name": "fixture-market",
            "plugins": [
                {"name":"p-path","description":"d","version":"1.0.0","source":"./plugins/p-path"},
                {"name":"p-github","description":"d","source":{"source":"github","repo":"o/r","ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
                {"name":"p-url","description":"d","source":{"source":"url","url":"https://example.com/r.git"}},
                {"name":"p-subdir","description":"d","source":{"source":"git-subdir","url":"o/mono","path":"tools/p"}},
                {"name":"p-npm","description":"d","source":{"source":"npm","package":"@o/p","version":"2.0.0"}},
                {"name":"p-archive","description":"d","source":{"source":"archive","url":"https://example.com/p.zip","sha256":"abc123"}},
                {"name":"p-command","description":"d","source":{"source":"command","command":"make plugin"}},
                {"name":"","description":"nameless dropped","source":"./x"},
                {"name":"p-bogus","description":"unknown shape dropped","source":{"source":"teleport"}}
            ]
        }"#;
        let cat = parse_marketplace_json(raw).unwrap();
        assert_eq!(cat.name, "fixture-market");
        assert_eq!(cat.plugins.len(), 7);
        assert!(matches!(cat.plugins[0].source, PluginSource::Path(_)));
        assert!(matches!(cat.plugins[1].source, PluginSource::Github { .. }));
        assert!(matches!(cat.plugins[2].source, PluginSource::Url { .. }));
        assert!(matches!(
            cat.plugins[3].source,
            PluginSource::GitSubdir { .. }
        ));
        assert!(matches!(cat.plugins[4].source, PluginSource::Npm { .. }));
        assert!(matches!(
            cat.plugins[5].source,
            PluginSource::Archive { .. }
        ));
        // `command` parses but is never executed: the install arm refuses it.
        assert!(matches!(
            cat.plugins[6].source,
            PluginSource::Command { .. }
        ));
        assert_eq!(claude_qualifier(&cat.plugins[4].source), "npm:@o/p@2.0.0");
    }

    #[test]
    fn marketplace_specs_parse_and_reject() {
        assert_eq!(
            marketplace_repo_parts("anthropics/claude-plugins-official").unwrap(),
            (
                "anthropics".to_string(),
                "claude-plugins-official".to_string()
            )
        );
        assert!(marketplace_repo_parts("justname").is_err());
        assert!(marketplace_repo_parts("a/b/c").is_err());
        assert!(marketplace_repo_parts("https://github.com/a/b").is_err());
    }

    #[test]
    fn claude_marketplaces_env_override() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
        }
        assert_eq!(claude_marketplaces().len(), 2);
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var(CLAUDE_MARKETPLACES_ENV, "foo/bar, file:///tmp/x ");
        }
        assert_eq!(claude_marketplaces(), vec!["foo/bar", "file:///tmp/x"]);
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
        }
    }

    #[test]
    fn local_marketplace_dir_detects_dirs_only() {
        let dir = tempfile::tempdir().unwrap();
        assert!(local_marketplace_dir(dir.path().to_str().unwrap()).is_some());
        assert!(local_marketplace_dir(&format!("file://{}", dir.path().display())).is_some());
        assert!(local_marketplace_dir("anthropics/claude-plugins-official").is_none());
        assert!(local_marketplace_dir("").is_none());
    }

    #[tokio::test]
    async fn git_source_checks_out_stale_pin() {
        // Upstream moved past the catalog pin: resolve must check out the
        // pinned commit (not fail), fully offline over file://.
        // SAFETY: serialized by ENV_GUARD (process-global env).
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        let dir = tempfile::tempdir().unwrap();
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
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(&["init", "-q"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "one"]);
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let pin = String::from_utf8(out.stdout).unwrap().trim().to_string();
        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "two"]);
        let url = format!("file://{}", dir.path().display());
        let p = MarketplacePlugin {
            name: "pin-test".to_string(),
            description: String::new(),
            version: String::new(),
            source: PluginSource::Path("x".to_string()),
        };
        let resolved = install_git_source(&url, "", &pin, None, &p, "pin-test")
            .await
            .expect("stale pin must resolve via checkout");
        assert_eq!(resolved.hash, pin);
        assert!(!resolved.unverified);
        assert!(resolved.root.join("a.txt").is_file());
        assert!(!resolved.root.join("b.txt").exists());
    }

    #[tokio::test]
    async fn status_never_hard_fails() {
        // Closed loopback port: every source reports false, none panics.
        // SAFETY: serialized by ENV_GUARD (process-global env).
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var(CLAUDE_MARKETPLACES_ENV, "127.0.0.1:9/none");
            std::env::set_var(CLAWHUB_BASE_ENV, "http://127.0.0.1:9");
            std::env::set_var(crate::ops::NPM_REGISTRY_ENV, "http://127.0.0.1:9");
            std::env::set_var(crate::index::INDEX_URL_ENV, "http://127.0.0.1:9/i.json");
        }
        assert!(!status(Source::GrayIndex).await);
        assert!(!status(Source::PiGallery).await);
        assert!(!status(Source::ClawHub).await);
        assert!(!status(Source::ClaudeRepo).await);
        unsafe {
            std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
            std::env::remove_var(CLAWHUB_BASE_ENV);
            std::env::remove_var(crate::ops::NPM_REGISTRY_ENV);
            std::env::remove_var(crate::index::INDEX_URL_ENV);
        }
    }
}
