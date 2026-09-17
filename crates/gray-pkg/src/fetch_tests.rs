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

#[test]
fn url_policy_rejects_malformed_authorities() {
    for url in ["https://", "https://[::1", "https://localhost:bad/archive"] {
        assert!(check_url(url).is_err(), "accepted {url}");
    }
}

#[test]
fn url_policy_accepts_parsed_loopback_and_https() {
    for url in [
        "http://[::1]:9/archive",
        "http://LOCALHOST:9/archive",
        "https://example.com/archive",
    ] {
        assert!(check_url(url).is_ok(), "rejected {url}");
    }
}

#[test]
fn url_policy_uses_downloader_authority() {
    for url in [
        r"http://evil.example\@127.0.0.1/archive",
        r"http://evil.example\@localhost/archive",
        "http://localhost.evil.example/archive",
        "ftp://localhost/archive",
    ] {
        assert!(check_url(url).is_err(), "accepted {url}");
    }
}
