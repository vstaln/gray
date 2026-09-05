//! Download (https-only, size-capped, hash-verified) + tar.gz unpack.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Max download size: 64 MiB.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Max redirects followed.
pub const MAX_REDIRECTS: usize = 5;

/// HTTP client for plugin downloads: max 5 redirects.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
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
fn check_url(url: &str) -> anyhow::Result<()> {
    let redacted = || redact(url);
    let Some((scheme, rest)) = url.split_once("://") else {
        anyhow::bail!("refusing non-https plugin URL: {}", redacted());
    };
    if scheme.eq_ignore_ascii_case("https") {
        return Ok(());
    }
    if scheme.eq_ignore_ascii_case("http") {
        let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..auth_end];
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.split(']').next())
            .unwrap_or_else(|| host.split(':').next().unwrap_or(host));
        if host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost") {
            return Ok(());
        }
    }
    anyhow::bail!("refusing non-https plugin URL: {}", redacted());
}

/// Stream `url` to `$GRAY_HOME/plugins/tmp/`, enforcing the 64 MiB cap and
/// verifying sha256 against `expected` (`"sha256:<hex>"`). The temp file is
/// auto-deleted on failure; on success the kept path is returned.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    expected: Option<&crate::index::HashSpec>,
) -> anyhow::Result<PathBuf> {
    check_url(url)?;
    let tmp_dir = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    log::debug!("downloading plugin archive from {}", redact(url));
    let mut resp = client.get(url).send().await?.error_for_status()?;

    let tmp = tempfile::NamedTempFile::new_in(&tmp_dir)?;
    let temppath = tmp.into_temp_path();
    let path: PathBuf = temppath.to_path_buf();
    let mut file = tokio::fs::File::create(&path).await?;
    let mut hasher = Sha256::new();
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
        hasher.update(&bytes);
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
        if !algo.eq_ignore_ascii_case("sha256") {
            anyhow::bail!("unsupported hash algorithm in index entry");
        }
        let got = format!("{:x}", hasher.finalize());
        if !got.eq_ignore_ascii_case(hex) {
            anyhow::bail!("hash mismatch for {}", redact(url));
        }
    }

    temppath
        .keep()
        .map_err(|e| anyhow::anyhow!("keeping download: {e}"))?;
    Ok(path)
}

/// Unpack a tar.gz, rejecting absolute paths and `..` entries.
pub fn unpack_tar_gz(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(gz);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            anyhow::bail!("refusing unsafe archive entry: {}", path.display());
        }
        entry.unpack_in(dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_strips_secrets() {
        assert_eq!(redact("https://u:p@h/x?token=1"), "https://h/x");
        assert_eq!(redact("https://h/x#frag"), "https://h/x");
        assert_eq!(redact("https://h/x"), "https://h/x");
    }

    #[test]
    fn refuses_plain_http_remote() {
        assert!(check_url("http://example.com/x.tar.gz").is_err());
        assert!(check_url("https://example.com/x.tar.gz").is_ok());
        assert!(check_url("http://127.0.0.1:9/x.tar.gz").is_ok());
        assert!(check_url("http://localhost:9/x.tar.gz").is_ok());
    }

    #[test]
    fn rejects_dotdot_entries() {
        // Hand-crafted ustar (the tar builder itself refuses `..` at write time).
        fn evil_tar_gz(name: &str) -> Vec<u8> {
            use std::io::Write;
            let mut hdr = [0u8; 512];
            hdr[..name.len()].copy_from_slice(name.as_bytes());
            hdr[100..108].copy_from_slice(b"0000777\0");
            hdr[124..136].copy_from_slice(b"00000000004\0");
            hdr[148..156].copy_from_slice(b"        ");
            hdr[156] = b'0';
            hdr[257..262].copy_from_slice(b"ustar");
            hdr[263..265].copy_from_slice(b"00");
            let sum: u32 = hdr.iter().map(|&b| b as u32).sum();
            hdr[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
            let mut raw = Vec::new();
            raw.extend_from_slice(&hdr);
            let mut data = [0u8; 512];
            data[..4].copy_from_slice(b"evil");
            raw.extend_from_slice(&data);
            raw.extend_from_slice(&[0u8; 1024]);
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap()
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.tar.gz");
        std::fs::write(&archive, evil_tar_gz("../evil")).unwrap();
        assert!(unpack_tar_gz(&archive, dir.path()).is_err());
        std::fs::write(&archive, evil_tar_gz("/abs")).unwrap();
        assert!(unpack_tar_gz(&archive, dir.path()).is_err());
    }
}
