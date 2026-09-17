//! Download (https-only, size-capped, hash-verified) + tar.gz unpack.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256, Sha512};
use tokio::io::AsyncWriteExt;

/// Max download size: 64 MiB.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// HTTP client for plugin downloads: redirects allowed only when every hop
/// passes [`check_url`] (no https→http downgrade), with connect/total
/// deadlines so slow bodies expire.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if check_url(attempt.url().as_str()).is_ok() {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
}

/// Strip userinfo, query, and fragment so URLs are safe to log.
/// Renders `https://u:p@h/x?token=1` as `https://h/x`.
pub fn redact(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, url),
    };
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(auth_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let path = after
        .split('#')
        .next()
        .unwrap_or(after)
        .split('?')
        .next()
        .unwrap_or(after);
    match scheme {
        Some(s) => format!("{s}://{host}{path}"),
        None => format!("{host}{path}"),
    }
}

/// https-only, except http loopback (127.0.0.1/::1/localhost) for tests.
pub(crate) fn check_url(url: &str) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| anyhow::anyhow!("invalid plugin URL: {}", redact(url)))?;
    if parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")))
    {
        return Ok(());
    }
    anyhow::bail!("refusing non-https plugin URL: {}", redact(url));
}

/// Decode standard base64 (npm `integrity` payloads) via the workspace
/// `base64` crate. Empty input is rejected explicitly: the crate decodes
/// `""` to empty, but an empty digest must never verify.
fn b64_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine as _;
    if s.is_empty() {
        anyhow::bail!("invalid base64 hash");
    }
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| anyhow::anyhow!("invalid base64 hash"))
}

/// Stream `url` to `$GRAY_HOME/plugins/tmp/`, enforcing the 64 MiB cap and
/// verifying against `expected` (`"sha256:<hex>"` or npm's `"sha512-<base64>"`).
/// Anything else bails `unsupported hash algorithm`. The temp file is
/// auto-deleted on failure; on success the kept path is returned.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    expected: Option<&crate::index::HashSpec>,
) -> anyhow::Result<PathBuf> {
    check_url(url)?;
    // An index entry that fails to pin a digest must not download: only an
    // explicit direct-URL install (`expected == None`, warned at the call
    // site) may skip verification.
    if let Some(spec) = expected
        && spec.primary().is_none_or(|s| s.is_empty())
    {
        anyhow::bail!("plugin download requires an expected digest");
    }
    let tmp_dir = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    log::debug!("downloading plugin archive from {}", redact(url));
    let mut resp = client.get(url).send().await?.error_for_status()?;
    // The redirect policy `stop()`s on a blocked hop and hands the 3xx back:
    // `error_for_status` does not treat 3xx as an error, so reject it here
    // instead of saving the redirect body as the archive.
    if resp.status().is_redirection() {
        anyhow::bail!("download redirected to a disallowed URL");
    }

    let tmp = tempfile::NamedTempFile::new_in(&tmp_dir)?;
    let temppath = tmp.into_temp_path();
    let path: PathBuf = temppath.to_path_buf();
    let mut file = tokio::fs::File::create(&path).await?;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut total: u64 = 0;
    loop {
        let chunk = resp.chunk().await?;
        let Some(bytes) = chunk else { break };
        total += bytes.len() as u64;
        if total > MAX_BYTES {
            drop(file);
            // `temppath` drops here and deletes the partial file.
            anyhow::bail!("plugin archive exceeds 64 MiB cap");
        }
        sha256.update(&bytes);
        sha512.update(&bytes);
        file.write_all(&bytes).await?;
    }
    file.flush().await?;
    drop(file);

    if let Some(spec) = expected
        && let Some(want) = spec.primary()
    {
        let (algo, hex) = match want.split_once(':') {
            Some((a, h)) => (a, h),
            None => ("", want),
        };
        if algo.eq_ignore_ascii_case("sha256") {
            let got = format!("{:x}", sha256.finalize());
            if !got.eq_ignore_ascii_case(hex) {
                anyhow::bail!("hash mismatch for {}", redact(url));
            }
        } else if let Some(payload) = want.strip_prefix("sha512-") {
            let want_raw = b64_decode(payload)
                .map_err(|_| anyhow::anyhow!("invalid sha512 integrity for {}", redact(url)))?;
            if sha512.finalize()[..] != want_raw[..] {
                anyhow::bail!("hash mismatch for {}", redact(url));
            }
        } else {
            anyhow::bail!("unsupported hash algorithm in index entry");
        }
    }

    temppath
        .keep()
        .map_err(|e| anyhow::anyhow!("keeping download: {e}"))?;
    Ok(path)
}

/// Unpack a ZIP archive (ClawHub skill bundles), rejecting absolute
/// paths and `..` entries. Stored (method 0) and deflate (method 8)
/// entries only; encrypted entries and data descriptors are refused.
/// CRC32 is NOT checked here — ClawHub callers verify per-file sha256
/// afterwards ([`crate::sources::verify_clawhub_files`]). Total
/// uncompressed output is capped at 256 MiB (zip-bomb guard).
/// Hand-rolled: `gray-pkg` takes no new deps by design (flate2 for
/// inflate is already aboard).
pub fn unpack_zip(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    const MAX_OUT: u64 = 256 * 1024 * 1024;
    let bytes = std::fs::read(archive)?;
    let u16le = |i: usize| -> anyhow::Result<u16> {
        bytes
            .get(i..i + 2)
            .and_then(|b| b.try_into().ok())
            .map(u16::from_le_bytes)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))
    };
    let u32le = |i: usize| -> anyhow::Result<u32> {
        bytes
            .get(i..i + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))
    };
    if bytes.len() < 22 {
        anyhow::bail!("invalid zip archive");
    }
    // EOCD hunt over the last 64 KiB + 22 (comment max + EOCD size).
    let mut eocd = None;
    let from = bytes.len().saturating_sub(65_557 + 22);
    let mut i = bytes.len() - 22;
    loop {
        if bytes[i..].starts_with(b"PK\x05\x06") {
            eocd = Some(i);
            break;
        }
        if i == from {
            break;
        }
        i -= 1;
    }
    let e = eocd.ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
    let total = u16le(e + 10)? as usize;
    let cd_size = u32le(e + 12)? as usize;
    let cd_off = u32le(e + 16)? as usize;
    if cd_off
        .checked_add(cd_size)
        .is_none_or(|end| end > bytes.len())
    {
        anyhow::bail!("invalid zip archive");
    }
    let mut total_out: u64 = 0;
    let mut p = cd_off;
    for _ in 0..total {
        if bytes.get(p..p + 4) != Some(b"PK\x01\x02".as_slice()) {
            anyhow::bail!("invalid zip archive");
        }
        let flags = u16le(p + 8)?;
        let method = u16le(p + 10)?;
        if flags & 0x1 != 0 {
            anyhow::bail!("refusing encrypted zip entry");
        }
        if flags & 0x8 != 0 {
            anyhow::bail!("unsupported zip data descriptor");
        }
        if method != 0 && method != 8 {
            anyhow::bail!("unsupported zip method {method}");
        }
        let comp = u32le(p + 20)? as usize;
        let name_len = u16le(p + 28)? as usize;
        let extra_len = u16le(p + 30)? as usize;
        let comment_len = u16le(p + 32)? as usize;
        let local_off = u32le(p + 42)? as usize;
        let name_start = p
            .checked_add(46)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
        if name_end > bytes.len() {
            anyhow::bail!("invalid zip archive");
        }
        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .map_err(|_| anyhow::anyhow!("invalid zip entry name"))?;
        p = name_end
            .checked_add(extra_len + comment_len)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
        if name.ends_with('/') {
            continue;
        }
        let rel = Path::new(name);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
            anyhow::bail!("refusing unsafe archive entry: {name}");
        }
        // Local header: skip name+extra to reach the data.
        if bytes.get(local_off..local_off + 4) != Some(b"PK\x03\x04".as_slice()) {
            anyhow::bail!("invalid zip archive");
        }
        let lh_name = u16le(local_off + 26)? as usize;
        let lh_extra = u16le(local_off + 28)? as usize;
        let data_start = local_off
            .checked_add(30 + lh_name + lh_extra)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
        let data_end = data_start
            .checked_add(comp)
            .ok_or_else(|| anyhow::anyhow!("invalid zip archive"))?;
        if data_end > bytes.len() {
            anyhow::bail!("invalid zip archive");
        }
        let data: Vec<u8> = if method == 0 {
            bytes[data_start..data_end].to_vec()
        } else {
            use std::io::Read;
            // Bound inflation *while* reading: a tiny compressed entry must
            // not expand past the remaining budget before the check below.
            let dec = flate2::read::DeflateDecoder::new(&bytes[data_start..data_end]);
            let mut v = Vec::new();
            dec.take(MAX_OUT.saturating_sub(total_out) + 1)
                .read_to_end(&mut v)?;
            v
        };
        total_out += data.len() as u64;
        if total_out > MAX_OUT {
            anyhow::bail!("zip archive exceeds 256 MiB cap");
        }
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &data)?;
    }
    Ok(())
}

/// Unpack a tar.gz, rejecting absolute paths and `..` entries.
/// Total expanded output is capped at 256 MiB and entry counts/types are
/// bounded (regular files and directories only).
pub fn unpack_tar_gz(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    use std::io::Read as _;
    const MAX_OUT: u64 = 256 * 1024 * 1024;
    const MAX_ENTRIES: usize = 10_000;
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(file).take(MAX_OUT + 1);
    let mut ar = tar::Archive::new(gz);
    let mut total = 0u64;
    for (index, entry) in ar.entries()?.enumerate() {
        anyhow::ensure!(index < MAX_ENTRIES, "too many archive entries");
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        anyhow::ensure!(kind.is_file() || kind.is_dir(), "unsupported archive entry");
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("archive size overflow"))?;
        anyhow::ensure!(total <= MAX_OUT, "tar archive exceeds output cap");
        let path = entry.path()?.into_owned();
        if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            anyhow::bail!("refusing unsafe archive entry: {}", path.display());
        }
        entry.unpack_in(dest)?;
    }
    // Compressed-input ceiling (download caps apply earlier; this is
    // belt-and-braces). Real output is bounded by the header-size
    // accounting above: tar only ever emits header.size() bytes per entry.
    anyhow::ensure!(
        ar.into_inner().limit() != 0,
        "tar archive exceeds input cap"
    );
    Ok(())
}

#[path = "fetch_tests.rs"]
#[cfg(test)]
mod tests;
