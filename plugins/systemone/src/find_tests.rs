use super::*;

#[test]
fn secret_names_are_filtered() {
    for name in [
        ".env",
        ".env.local",
        "key.pem",
        "id_rsa",
        "id_ed25519.pub",
        "auth.json",
        "gateway.yaml",
        "cert.p12",
        "API.KEY",
    ] {
        assert!(is_secret_name(name), "{name}");
    }
    for name in [
        "main.rs",
        "env.py",
        "keystore.txt",
        "identity_rsa_backup.md",
    ] {
        // `identity_rsa_backup.md` does not start with id_rsa; env.py is not .env*.
        assert!(!is_secret_name(name), "{name}");
    }
}

#[test]
fn chunking_uses_one_based_non_overlapping_windows() {
    let text = (1..=7).map(|i| format!("line{i}\n")).collect::<String>();
    let chunks = chunk_file("f.rs", &text, 3);
    assert_eq!(chunks.len(), 3);
    assert_eq!((chunks[0].start, chunks[0].end), (1, 3));
    assert_eq!((chunks[1].start, chunks[1].end), (4, 6));
    assert_eq!((chunks[2].start, chunks[2].end), (7, 7));
    assert_eq!(chunks[2].text, "line7");
}

#[test]
fn keywords_filter_is_case_insensitive() {
    let kw = vec!["marker".to_string()];
    assert!(matches_keywords("has a MARKER inside", &kw));
    assert!(!matches_keywords("nothing relevant", &kw));
}

#[test]
fn request_timeout_caps_at_twenty_and_refuses_thin_budgets() {
    assert_eq!(
        request_timeout(Duration::from_secs(25)),
        Some(Duration::from_secs(20))
    );
    assert_eq!(
        request_timeout(Duration::from_secs(10)),
        Some(Duration::from_secs(10))
    );
    assert_eq!(request_timeout(Duration::from_secs(2)), None);
}
