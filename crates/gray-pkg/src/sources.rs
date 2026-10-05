//! ClawHub skill source (search + install adapter for `skills_ops`).
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
//!
//! No auth anywhere here (public read endpoints only). No new runtime
//! dependencies: reqwest/serde_json/git-CLI/std-fs only.

use std::path::{Path, PathBuf};

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

/// Split `owner/slug` (install reference form) from a bare slug.
pub fn split_clawhub_slug(slug: &str) -> (Option<String>, String) {
    match slug.trim().split_once('/') {
        Some((o, s)) if !o.trim().is_empty() && !s.trim().is_empty() => {
            (Some(o.trim().to_string()), s.trim().to_string())
        }
        _ => (None, slug.trim().to_string()),
    }
}

/// One 429 retry: honor `Retry-After` (default 2s, capped 30s), sleep,
/// then re-send via `send` (which must rebuild the request). Non-429
/// responses pass through untouched.
async fn clawhub_retry_once<F, Fut>(
    resp: reqwest::Response,
    send: F,
) -> anyhow::Result<reqwest::Response>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
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
    clawhub_retry_once(resp, send).await
}

/// Resolved ClawHub skill: pinned version plus the versions-endpoint file
/// list (empty when the endpoint won't answer — install then proceeds on
/// the unverified path).
#[derive(Debug, Clone, Default)]
pub struct ClawHubDetail {
    pub slug: String,
    pub owner: String,
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

/// A downloaded + unpacked ClawHub bundle awaiting the skills install.
/// `_staging` keeps the tempdir alive as long as `root`.
pub(crate) struct StagedClawHub {
    pub(crate) _staging: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) detail: ClawHubDetail,
    /// Per-file hashes matched (false = no file list or mismatch-free path).
    pub(crate) verified: bool,
}

/// Detail → download → unpack → per-file verify, shared by the skill
/// install arm (`skills_ops`).
pub(crate) async fn stage_clawhub_bundle(
    client: &reqwest::Client,
    slug: &str,
) -> anyhow::Result<StagedClawHub> {
    let detail = clawhub_detail(client, slug).await?;
    let archive = clawhub_download_bundle(client, &detail).await?;
    let tmp_root = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_root)?;
    let staging = tempfile::tempdir_in(&tmp_root)?;
    // Hosted skills are ZIPs; GitHub-handoff bundles are tarballs.
    let is_zip = std::fs::read(&archive)
        .map(|b| b.len() >= 2 && b[..2] == *b"PK")
        .unwrap_or(false);
    let unpacked = if is_zip {
        crate::fetch::unpack_zip(&archive, staging.path())
    } else {
        crate::fetch::unpack_tar_gz(&archive, staging.path())
    };
    if let Err(e) = unpacked {
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    let _ = std::fs::remove_file(&archive);
    // ClawHub zips carry no `package/` wrapper; `stage_root` covers both.
    let root = crate::ops::stage_root(staging.path());
    let verified = verify_clawhub_files(&root, &detail.files)?;
    Ok(StagedClawHub {
        _staging: staging,
        root,
        detail,
        verified,
    })
}

/// Shallow-clone `url` into `<tmp>/repo` (`extra` holds `--branch` /
/// `--filter` / `--sparse` flags) and return the keeper tempdir + path.
pub fn clone_into_tmp(
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
    // Installs are hash-verified content copies, not working trees: line
    // endings must reach the disk exactly as committed. Windows runners
    // default core.autocrlf=true, which rewrites every text file.
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-c", "core.autocrlf=false", "clone", "--depth", "1"])
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

#[path = "sources_tests.rs"]
#[cfg(test)]
mod tests;
