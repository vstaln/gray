//! Task 2.2: redact() unit-style integration tests (pure, no I/O).

#[test]
fn userinfo_query_stripped() {
    assert_eq!(
        gray_pkg::fetch::redact("https://u:p@h/x?token=1"),
        "https://h/x"
    );
}

#[test]
fn query_stripped() {
    assert_eq!(
        gray_pkg::fetch::redact("https://h/x?token=1"),
        "https://h/x"
    );
}

#[test]
fn fragment_stripped() {
    assert_eq!(gray_pkg::fetch::redact("https://h/x#frag"), "https://h/x");
}

#[test]
fn plain_url_unchanged() {
    assert_eq!(gray_pkg::fetch::redact("https://h/x"), "https://h/x");
}

#[test]
fn loopback_with_port_preserved() {
    assert_eq!(
        gray_pkg::fetch::redact("http://127.0.0.1:8080/x?k=1"),
        "http://127.0.0.1:8080/x"
    );
}

#[test]
fn bare_userinfo_stripped() {
    assert_eq!(gray_pkg::fetch::redact("https://u@h/x"), "https://h/x");
}
