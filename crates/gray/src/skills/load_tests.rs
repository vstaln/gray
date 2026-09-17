use super::*;
use std::io::Write as _;

#[test]
fn declared_args_split_commas_and_spaces() {
    assert_eq!(parse_declared_args("env, force"), vec!["env", "force"]);
    assert_eq!(parse_declared_args("env force"), vec!["env", "force"]);
    assert_eq!(parse_declared_args("[env, force]"), vec!["env", "force"]);
    assert!(parse_declared_args("").is_empty());
}

#[test]
fn frontmatter_args_land_on_skill() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        "---\nname: deploy\ndescription: test skill\nargs: env, force\n---\nBody"
    )
    .unwrap();
    let skill = load_skill_from_file(f.path(), "path");
    let skill = skill.expect("loads");
    assert_eq!(skill.args, vec!["env".to_string(), "force".to_string()]);
}

#[test]
fn frontmatter_without_args_means_no_args() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "---\ndescription: test skill\n---\nBody").unwrap();
    let skill = load_skill_from_file(f.path(), "path");
    let skill = skill.expect("loads");
    assert!(skill.args.is_empty());
}

#[test]
fn folded_description_joins_continuation_lines() {
    // ponytail-style `description: >` frontmatter: the indented lines
    // fold into one description (previously parsed as the bare `">"`).
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
            f,
            "---\nname: ponytail\ndescription: >\n  Forces the laziest solution\n  that actually works.\nargs: lite, full\n---\nBody"
        )
        .unwrap();
    let skill = load_skill_from_file(f.path(), "path");
    let skill = skill.expect("loads");
    assert_eq!(
        skill.description,
        "Forces the laziest solution that actually works."
    );
    assert_eq!(skill.args, vec!["lite".to_string(), "full".to_string()]);
}

#[test]
fn literal_and_chomped_markers_parse() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "---\ndescription: |-\n  line one\n  line two\n---\nBody").unwrap();
    let skill = load_skill_from_file(f.path(), "path");
    assert_eq!(skill.expect("loads").description, "line one\nline two");

    let mut g = tempfile::NamedTempFile::new().unwrap();
    writeln!(g, "---\ndescription: >-\n  folded here\n---\nBody").unwrap();
    let skill = load_skill_from_file(g.path(), "path");
    assert_eq!(skill.expect("loads").description, "folded here");
}

#[test]
fn frontmatter_preserves_crlf_unicode_and_body() {
    for nl in ["\n", "\r\n"] {
        let input = [
            "---",
            "name: deploy",
            "description: Ship the app…",
            "---",
            "Body…",
            "",
        ]
        .join(nl);
        let (fm, body) = parse_frontmatter(&input).unwrap();
        assert_eq!(fm.description.as_deref(), Some("Ship the app…"));
        assert_eq!(body, format!("Body…{nl}"));
    }
}
