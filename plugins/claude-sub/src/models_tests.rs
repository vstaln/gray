use super::*;

#[test]
fn catalog_lists_pinned_routes_with_windows() {
    let got = catalog();
    assert!(!got.models.is_empty());
    let sonnet = got
        .models
        .iter()
        .find(|m| m.id == "sonnet")
        .expect("sonnet in catalog");
    assert_eq!(sonnet.context_window, Some(1_000_000));
    assert!(sonnet.reasoning_efforts.contains(&"high".to_string()));
    let haiku = got
        .models
        .iter()
        .find(|m| m.id == "haiku")
        .expect("haiku in catalog");
    assert_eq!(haiku.context_window, Some(200_000));
}
