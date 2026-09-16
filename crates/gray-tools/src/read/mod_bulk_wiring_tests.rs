use super::*;

#[test]
fn schema_has_single_scalar_types_paths_exclude_and_empty_required() {
    let def = ReadTool::default().def();
    let props = def
        .parameters
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("properties");
    for key in ["path", "paths", "exclude", "offset", "limit"] {
        assert!(props.contains_key(key), "schema missing {key}");
    }
    for (name, schema) in props {
        let t = schema
            .get("type")
            .unwrap_or_else(|| panic!("property {name} missing scalar type"));
        assert!(
            t.is_string(),
            "property {name} must have exactly one scalar type (no unions), got {t}"
        );
    }
    let req = def
        .parameters
        .get("required")
        .and_then(|r| r.as_array())
        .expect("required");
    assert!(
        req.is_empty(),
        "required must be [] (path-or-paths enforced at runtime)"
    );
}
