fn main() {
    println!(
        "cargo:rustc-env=GRAY_CHANNEL={}",
        std::env::var("GRAY_CHANNEL").unwrap_or_else(|_| "stable".into())
    );
    println!("cargo:rerun-if-env-changed=GRAY_CHANNEL");
}
