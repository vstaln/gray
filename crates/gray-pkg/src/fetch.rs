//! Download (https-only, size-capped, hash-verified) + tar.gz unpack.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256, Sha512};
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
pub(crate) fn check_url(url: &str) -> anyhow::Result<()> {
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

fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode standard base64 (npm `integrity` payloads). Hand-rolled: the
/// workspace has `base64` but `gray-pkg` takes no new deps by design.
fn b64_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let b = s.as_bytes();
    if b.is_empty() || !b.len().is_multiple_of(4) {
        anyhow::bail!("invalid base64 hash");
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (qi, quad) in b.chunks(4).enumerate() {
        let last = qi == b.len() / 4 - 1;
        let mut n: u32 = 0;
        let mut pad = 0;
        for &c in quad {
            if c == b'=' {
                pad += 1;
                n <<= 6;
            } else {
                if pad > 0 {
                    anyhow::bail!("invalid base64 hash");
                }
                n = (n << 6) | b64_val(c).ok_or_else(|| anyhow::anyhow!("invalid base64 hash"))?;
            }
        }
        if pad > 2 || (pad > 0 && !last) {
            anyhow::bail!("invalid base64 hash");
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad == 0 {
            out.push(n as u8);
        }
    }
    Ok(out)
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
    let tmp_dir = crate::plugins_dir().join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    log::debug!("downloading plugin archive from {}", redact(url));
    let mut resp = client.get(url).send().await?.error_for_status()?;

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
    fn b64_decode_standard_vectors() {
        assert_eq!(b64_decode("Zg==").unwrap(), b"f");
        assert_eq!(b64_decode("Zm9v").unwrap(), b"foo");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
        // 64-byte digest shape (sha512 length) round-trips.
        let raw: Vec<u8> = (0..64).collect();
        let mut enc = String::new();
        const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for ch in raw.chunks(3) {
            let mut n: u32 = 0;
            for &b in ch {
                n = (n << 8) | u32::from(b);
            }
            n <<= 8 * (3 - ch.len());
            enc.push(ALPH[((n >> 18) & 63) as usize] as char);
            enc.push(ALPH[((n >> 12) & 63) as usize] as char);
            enc.push(if ch.len() > 1 {
                ALPH[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            enc.push(if ch.len() > 2 {
                ALPH[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        assert_eq!(b64_decode(&enc).unwrap(), raw);
    }

    #[test]
    fn b64_decode_rejects_garbage() {
        for bad in ["", "!!!", "====", "Zm9v!", "Zm9vYmFy="] {
            assert!(b64_decode(bad).is_err(), "accepted {bad:?}");
        }
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
