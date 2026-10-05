use super::*;

#[test]
fn normalize_base_trims_slash_and_v1() {
    assert_eq!(normalize_base("http://x:1/"), "http://x:1");
    assert_eq!(normalize_base("http://x:1/v1"), "http://x:1");
    assert_eq!(normalize_base("http://x:1/v1/"), "http://x:1");
    assert_eq!(normalize_base("http://x:1/v10"), "http://x:1/v10");
}

#[test]
fn host_of_handles_schemes_ports_and_ipv6() {
    assert_eq!(host_of("http://localhost:11435"), "localhost");
    assert_eq!(host_of("https://API.TypeSafe.AI/v1"), "api.typesafe.ai");
    assert_eq!(host_of("http://[::1]:9"), "::1");
    assert_eq!(host_of("127.0.0.1:80/x"), "127.0.0.1");
}

#[test]
fn local_host_detection() {
    assert!(is_local_host("localhost"));
    assert!(is_local_host("127.0.0.1"));
    assert!(is_local_host("::1"));
    assert!(!is_local_host("api.typesafe.ai"));
}
