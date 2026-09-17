use super::*;

#[test]
fn platform_catalog_and_checksum_validation() {
    assert_eq!(
        target("linux", "x86_64").unwrap(),
        "x86_64-unknown-linux-musl"
    );
    assert_eq!(target("macos", "aarch64").unwrap(), "aarch64-apple-darwin");
    assert!(target("windows", "x86_64").is_err());
    assert!(target("linux", "riscv64").is_err());
    let hash = "a".repeat(64);
    assert_eq!(
        checksum(&format!("{hash}  background-test\n"), "background-test").unwrap(),
        format!("sha256:{hash}")
    );
    for text in [
        String::new(),
        "z".repeat(64),
        format!("{hash}  other"),
        format!("{hash}  background-test\nextra"),
    ] {
        assert!(checksum(&text, "background-test").is_err(), "{text}");
    }
}

#[test]
fn publishing_rolls_back_files_if_registry_save_fails() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("plugins");
    std::fs::create_dir_all(root.join("background")).unwrap();
    std::fs::write(root.join("background/background"), b"old").unwrap();
    let stage = tempfile::tempdir_in(&root).unwrap();
    std::fs::create_dir(stage.path().join("next")).unwrap();
    std::fs::write(stage.path().join("next/background"), b"new").unwrap();
    let result = publish(stage, &root.join("background"), || {
        anyhow::bail!("disk full")
    });
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(root.join("background/background")).unwrap(),
        b"old"
    );
}
