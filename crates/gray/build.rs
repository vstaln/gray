fn main() {
    println!(
        "cargo:rustc-env=GRAY_CHANNEL={}",
        std::env::var("GRAY_CHANNEL").unwrap_or_else(|_| "stable".into())
    );
    println!("cargo:rerun-if-env-changed=GRAY_CHANNEL");
    println!("cargo:rerun-if-env-changed=GRAY_BUILD_ID");
    println!(
        "cargo:rustc-env=GRAY_BUILD_ID={}",
        std::env::var("GRAY_BUILD_ID").unwrap_or_default()
    );
}
