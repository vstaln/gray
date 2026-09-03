use gray_plugin::profile::load_profile;

#[test]
fn load_profile_returns_ordered_names() {
    let names = load_profile("testdata/gray.yml").unwrap();
    assert_eq!(names, vec!["tools-basic", "tools-search"]);
}
