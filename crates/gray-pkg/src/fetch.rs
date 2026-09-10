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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

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
        let enc = base64::engine::general_purpose::STANDARD.encode(&raw);
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

    /// Minimal stored-ZIP builder (method selectable per file).
    fn tiny_zip(files: &[(&str, &[u8], u16)]) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, method) in files {
            let payload: Vec<u8> = if *method == 8 {
                let mut enc =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(data).unwrap();
                enc.finish().unwrap()
            } else {
                data.to_vec()
            };
            let off = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&payload);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
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

    #[test]
    fn zip_roundtrips_stored_and_deflated() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(
            &archive,
            tiny_zip(&[("SKILL.md", b"# hi\n", 0), ("sub/notes.md", b"nested\n", 8)]),
        )
        .unwrap();
        let dest = dir.path().join("out");
        unpack_zip(&archive, &dest).unwrap();
        assert_eq!(std::fs::read(dest.join("SKILL.md")).unwrap(), b"# hi\n");
        assert_eq!(
            std::fs::read(dest.join("sub/notes.md")).unwrap(),
            b"nested\n"
        );
    }

    #[test]
    fn zip_rejects_traversal_garbage_and_method() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, tiny_zip(&[("../evil", b"x", 0)])).unwrap();
        assert!(unpack_zip(&archive, &dest).is_err());
        std::fs::write(&archive, tiny_zip(&[("/abs", b"x", 0)])).unwrap();
        assert!(unpack_zip(&archive, &dest).is_err());
        std::fs::write(&archive, tiny_zip(&[("a", b"x", 12)])).unwrap();
        assert!(unpack_zip(&archive, &dest).is_err());
        std::fs::write(&archive, b"not a zip at all................").unwrap();
        assert!(unpack_zip(&archive, &dest).is_err());
        std::fs::write(&archive, b"PK").unwrap();
        assert!(unpack_zip(&archive, &dest).is_err());
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

    #[test]
    fn tar_rejects_lying_size_and_special_entries() {
        use std::io::Write;
        // Header claims >256 MiB while carrying 4 bytes: must fail before
        // writing anything.
        let mut hdr = [0u8; 512];
        hdr[..8].copy_from_slice(b"big.bin\0");
        hdr[100..108].copy_from_slice(b"0000777\0");
        hdr[124..136].copy_from_slice(b"20000000001\0"); // 2*8^9+1 > 256 MiB
        hdr[148..156].copy_from_slice(b"        ");
        hdr[156] = b'0';
        hdr[257..262].copy_from_slice(b"ustar");
        hdr[263..265].copy_from_slice(b"00");
        let sum: u32 = hdr.iter().map(|&b| b as u32).sum();
        hdr[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut raw = Vec::new();
        raw.extend_from_slice(&hdr);
        raw.extend_from_slice(&[0u8; 512]);
        raw.extend_from_slice(&[0u8; 1024]);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("big.tar.gz");
        std::fs::write(&archive, enc.finish().unwrap()).unwrap();
        let dest = dir.path().join("out");
        assert!(unpack_tar_gz(&archive, &dest).is_err());
        assert!(!dest.join("big.bin").exists());
        // Symlink entries are unsupported, even in-workspace ones.
        let mut ar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut ar_bytes);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            ar.append_link(&mut h, "link", "/etc/passwd").unwrap();
            ar.finish().unwrap();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&ar_bytes).unwrap();
        std::fs::write(&archive, enc.finish().unwrap()).unwrap();
        assert!(unpack_tar_gz(&archive, &dest).is_err());
        assert!(!dest.join("link").exists());
    }
}
